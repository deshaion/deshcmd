use super::*;

#[test]
fn auto_refresh_updates_local_tabs_but_keeps_flat_views_stable() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let (wake, notifications) = mpsc::channel();
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root.clone()],
        Settings::default(),
        move || {
            let _ = wake.send(());
        },
    );
    let settle = |engine: &mut Engine| {
        while Side::ALL.into_iter().any(|side| engine.tab(side).loading) {
            notifications.recv_timeout(Duration::from_secs(5)).unwrap();
            engine.poll();
        }
    };
    settle(&mut engine);
    std::fs::write(root.join("first.txt"), "first").unwrap();
    engine.dispatch(Command::Flat(Side::Right, false));
    settle(&mut engine);
    assert_eq!(engine.tab(Side::Right).visible_count(), 1);

    std::fs::write(root.join("second.txt"), "second").unwrap();
    engine.tab_mut(Side::Left).last_scan = Some(Instant::now() - watch::WATCHED_INTERVAL);
    engine.poll();
    settle(&mut engine);
    assert_eq!(engine.tab(Side::Left).visible_count(), 2);
    assert_eq!(engine.tab(Side::Right).visible_count(), 1);

    engine.dispatch(Command::Refresh);
    settle(&mut engine);
    assert_eq!(engine.tab(Side::Right).visible_count(), 2);
}

#[test]
fn locked_history_navigation_preserves_the_original_tab() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    let side = Side::Left;
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root.clone()],
        Settings::default(),
        || {},
    );
    let wait = |engine: &mut Engine, done: &dyn Fn(&Engine) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            engine.poll();
            if done(engine) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    };
    engine.dispatch(Command::Navigate {
        side,
        path: folder.clone(),
        new_tab: false,
        background: false,
    });
    wait(&mut engine, &|engine| !engine.tab(side).busy());
    engine.dispatch(Command::LockTab(side, 0, TabLock::Locked));
    engine.dispatch(Command::Back(side));
    wait(&mut engine, &|engine| engine.pane(side).tabs.len() == 2);
    assert_eq!(engine.pane(side).tabs[0].base(), folder);
    assert_eq!(engine.tab(side).base(), root);
    assert!(!engine.tab(side).can_forward());

    let index = engine.pane(side).active;
    engine.dispatch(Command::Navigate {
        side,
        path: folder.clone(),
        new_tab: true,
        background: true,
    });
    wait(&mut engine, &|engine| engine.pane(side).tabs.len() == 3);
    assert_eq!(engine.pane(side).active, index);
    assert_eq!(engine.pane(side).tabs[2].base(), folder);
}

fn settle_views(engine: &mut Engine, done: impl Fn(&Engine) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(engine) {
        engine.poll();
        assert!(Instant::now() < deadline, "engine did not settle");
        std::thread::yield_now();
    }
}

fn gate_view_workers(engine: &Engine) -> Vec<mpsc::Sender<()>> {
    let mut releases = Vec::new();
    let (started, starts) = mpsc::channel();
    for id in 0..2 {
        let (release, gate) = mpsc::channel();
        releases.push(release);
        let started = started.clone();
        assert!(engine.views.submit(
            views::ViewKey {
                tab: u64::MAX - id,
                generation: 0,
                raw: 0,
                revision: 0,
            },
            move |cancel| {
                started.send(()).unwrap();
                let _ = gate.recv_timeout(Duration::from_secs(5));
                cancel.check()?;
                unreachable!()
            }
        ));
    }
    starts.recv_timeout(Duration::from_secs(5)).unwrap();
    starts.recv_timeout(Duration::from_secs(5)).unwrap();
    releases
}

#[test]
fn a_new_view_uses_the_latest_filter_and_marks_made_during_preparation() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    for name in ["alpha.txt", "beta.txt", "beta2.txt"] {
        std::fs::write(root.join(name), name).unwrap();
    }
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    let side = Side::Left;
    settle_views(&mut engine, |e| {
        Side::ALL.into_iter().all(|side| !e.tab(side).busy())
    });
    let raw = engine.tab(side).raw.clone().unwrap();
    let releases = gate_view_workers(&engine);
    engine.dispatch(Command::Filter(side, MatchOptions::filename("alpha")));
    engine.dispatch(Command::Filter(side, MatchOptions::filename("bet")));
    engine.dispatch(Command::Filter(side, MatchOptions::filename("beta")));
    engine.dispatch(Command::Sort(side, Column::Name, false));
    assert_eq!(engine.tab(side).visible_count(), 3);
    engine.dispatch(Command::Select(
        side,
        root.join("beta2.txt"),
        Select::Replace,
    ));
    for release in releases {
        release.send(()).unwrap();
    }
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert!(Arc::ptr_eq(&raw, engine.tab(side).raw.as_ref().unwrap()));
    assert!(
        engine
            .tab(side)
            .visible_entries()
            .map(|e| e.name.as_str())
            .eq(["beta2.txt", "beta.txt"])
    );
    assert_eq!(engine.tab(side).operands(), [root.join("beta2.txt")]);
    assert_eq!(engine.tab(side).selected_bytes(), 9);
    assert_eq!(engine.tab(side).current().unwrap().name, "beta2.txt");
}

#[test]
fn global_view_options_apply_when_an_inactive_tab_is_reopened() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    for name in ["file2", "file10", ".hidden", "ignored"] {
        std::fs::write(folder.join(name), []).unwrap();
    }
    let side = Side::Left;
    let mut engine = Engine::new(
        root.clone(),
        [folder.clone(), root.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::NewTab(side));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    let scans = engine.listing_stats().full_scans;
    engine.set_sort_preferences(false, true);
    engine.dispatch(Command::ToggleHidden);
    engine.set_ignore("ignored".into()).unwrap();
    assert_eq!(engine.listing_stats().full_scans, scans);
    engine.dispatch(Command::Tab(side, 0));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert_eq!(engine.tab(side).base(), folder);
    assert!(
        engine
            .tab(side)
            .visible_entries()
            .map(|e| e.name.as_str())
            .eq([".hidden", "file10", "file2"])
    );
}

struct GatedDirectory {
    _fixture: tempfile::TempDir,
    engine: Engine,
    root: PathBuf,
    directory: PathBuf,
    release: mpsc::Sender<()>,
}

fn gated_directory(failure: Option<io::ErrorKind>) -> GatedDirectory {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let directory = root.join("large");
    let empty = root.join("empty");
    std::fs::create_dir(&directory).unwrap();
    std::fs::create_dir(&empty).unwrap();
    for number in (0..256).rev() {
        std::fs::write(directory.join(format!("file{number:03}.txt")), b"data").unwrap();
    }
    let mut engine = Engine::new(
        root.clone(),
        [empty.clone(), empty],
        Settings {
            auto_refresh: false,
            ..Settings::default()
        },
        || {},
    );
    settle_views(&mut engine, |e| {
        Side::ALL.into_iter().all(|side| !e.tab(side).busy())
    });
    let id = engine.tab(Side::Left).id;
    let (release, gate) = mpsc::channel();
    let gate = std::sync::Mutex::new(gate);
    engine.first_page_hook = Some(Arc::new(move |key| {
        if key.0 == id {
            gate.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .map_err(io::Error::other)?;
            if let Some(kind) = failure {
                return Err(io::Error::new(kind, "injected error after the first page"));
            }
        }
        Ok(())
    }));
    engine.dispatch(Command::Navigate {
        side: Side::Left,
        path: directory.clone(),
        new_tab: false,
        background: false,
    });
    settle_views(&mut engine, |e| e.tab(Side::Left).provisional());
    GatedDirectory {
        _fixture: fixture,
        engine,
        root,
        directory,
        release,
    }
}

#[test]
fn page_accepts_known_operands_and_preserves_focus_and_marks_in_final_order() {
    let GatedDirectory {
        _fixture,
        mut engine,
        root,
        release,
        ..
    } = gated_directory(None);
    let side = Side::Left;
    let tab = engine.tab(side);
    assert!(tab.busy());
    assert!((1..=64).contains(&tab.visible_count()));
    let known = tab.visible_entries().last().unwrap().path.clone();
    engine.dispatch(Command::Select(side, known.clone(), Select::Replace));
    engine.dispatch(Command::MarkAll(side));
    assert_eq!(
        engine.notice.as_deref(),
        Some("Wait for the directory to finish loading")
    );
    engine.dispatch(Command::Select(side, known.clone(), Select::Extend));
    assert_eq!(engine.tab(side).operands(), std::slice::from_ref(&known));
    engine.dispatch(Command::Find(side, MatchOptions::filename("file")));
    assert_eq!(
        engine.notice.as_deref(),
        Some("Wait for the directory to finish loading")
    );

    engine.dispatch(Command::Queue(
        Operation::Transfer {
            paths: engine.tab(side).operands(),
            destination: root.join("empty"),
            moving: false,
            options: Default::default(),
        },
        false,
    ));
    settle_views(&mut engine, |e| e.jobs.last().unwrap().state.finished());
    assert_eq!(engine.jobs.last().unwrap().state, JobState::Done);
    assert_eq!(
        std::fs::read(root.join("empty").join(known.file_name().unwrap())).unwrap(),
        b"data"
    );
    engine.dispatch(Command::Sort(side, Column::Name, false));
    release.send(()).unwrap();
    settle_views(&mut engine, |e| !e.tab(side).busy());
    let tab = engine.tab(side);
    assert!(!tab.provisional());
    assert_eq!(tab.visible_count(), 256);
    assert!(
        tab.visible_entries()
            .map(|entry| entry.name.clone())
            .eq((0..256).rev().map(|number| format!("file{number:03}.txt")))
    );
    assert_eq!(tab.current().unwrap().path, known);
    assert_eq!(tab.selected_bytes(), 4);
    assert_eq!(tab.marked_count(), 1);
    assert!(engine.take_cursor_restored(side));
}

#[test]
fn changing_filter_while_the_scan_is_gated_never_installs_old_criteria() {
    let GatedDirectory {
        _fixture,
        mut engine,
        release,
        ..
    } = gated_directory(None);
    let side = Side::Left;
    engine.dispatch(Command::Filter(side, MatchOptions::filename("absent")));
    settle_views(&mut engine, |e| e.tab(side).visible_count() == 0);
    assert!(engine.tab(side).provisional());
    engine.dispatch(Command::Filter(side, MatchOptions::filename("file25?.txt")));
    engine.dispatch(Command::Sort(side, Column::Name, false));
    release.send(()).unwrap();
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert!(
        engine
            .tab(side)
            .visible_entries()
            .map(|entry| entry.name.clone())
            .eq((250..256)
                .rev()
                .map(|number| format!("file{number:03}.txt")))
    );
    assert_eq!(engine.tab(side).current().unwrap().name, "file255.txt");
}

#[test]
fn a_late_partial_scan_cannot_replace_a_new_navigation() {
    let GatedDirectory {
        _fixture,
        mut engine,
        root,
        release,
        ..
    } = gated_directory(None);
    let side = Side::Left;
    engine.dispatch(Command::Navigate {
        side,
        path: root.join("empty"),
        new_tab: false,
        background: false,
    });
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert_eq!(engine.tab(side).base(), root.join("empty"));
    release.send(()).unwrap();
    engine.poll();
    assert_eq!(engine.tab(side).visible_count(), 0);
    assert!(!engine.tab(side).provisional());
}

#[test]
fn a_terminal_error_discards_the_provisional_view() {
    let GatedDirectory {
        _fixture,
        mut engine,
        directory,
        release,
        ..
    } = gated_directory(Some(io::ErrorKind::PermissionDenied));
    let side = Side::Left;
    release.send(()).unwrap();
    settle_views(&mut engine, |e| !e.tab(side).busy());
    let tab = engine.tab(side);
    assert_eq!(tab.base(), directory);
    assert!(
        tab.error
            .as_deref()
            .is_some_and(|text| text.contains("injected error"))
    );
    assert!(!tab.provisional());
    assert_eq!(tab.visible_count(), 0);
}

#[test]
fn refresh_keeps_the_complete_snapshot_until_its_replacement_is_ready() {
    let GatedDirectory {
        _fixture,
        mut engine,
        directory,
        release,
        ..
    } = gated_directory(None);
    let side = Side::Left;
    release.send(()).unwrap();
    settle_views(&mut engine, |e| !e.tab(side).busy());
    let old = engine.tab(side).raw.clone().unwrap();
    std::fs::write(directory.join("added.txt"), "").unwrap();
    engine.dispatch(Command::Refresh);
    assert_eq!(engine.tab(side).visible_count(), 256);
    assert!(!engine.tab(side).provisional());
    assert!(Arc::ptr_eq(&old, &engine.tab(side).view.raw));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert_eq!(engine.tab(side).visible_count(), 257);
    assert!(!Arc::ptr_eq(&old, &engine.tab(side).view.raw));
    assert!(!engine.tab(side).provisional());
}

#[cfg(unix)]
#[test]
fn deleting_a_directory_after_its_first_page_recovers_to_the_parent() {
    let GatedDirectory {
        _fixture,
        mut engine,
        root,
        directory,
        release,
    } = gated_directory(None);
    let side = Side::Left;
    std::fs::remove_dir_all(directory).unwrap();
    release.send(()).unwrap();
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert_eq!(engine.tab(side).base(), root);
    assert!(!engine.tab(side).provisional());
}

#[test]
fn closing_the_tab_discards_a_gated_provisional_scan() {
    let GatedDirectory {
        _fixture,
        mut engine,
        root,
        release,
        ..
    } = gated_directory(None);
    let side = Side::Left;
    let closed = engine.tab(side).id;
    engine.dispatch(Command::NewTab(side));
    engine.dispatch(Command::Navigate {
        side,
        path: root.join("empty"),
        new_tab: false,
        background: false,
    });
    engine.dispatch(Command::CloseTab(side, 0));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    release.send(()).unwrap();
    engine.poll();
    assert_ne!(engine.tab(side).id, closed);
    assert_eq!(engine.pane(side).tabs.len(), 1);
    assert_eq!(engine.tab(side).base(), root.join("empty"));
    assert!(!engine.tab(side).provisional());
}

#[test]
fn closing_a_tab_prevents_its_prepared_sort_from_installing() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("one.txt"), []).unwrap();
    let side = Side::Left;
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    settle_views(&mut engine, |e| {
        Side::ALL.into_iter().all(|side| !e.tab(side).busy())
    });
    engine.dispatch(Command::Navigate {
        side,
        path: folder.clone(),
        new_tab: true,
        background: false,
    });
    settle_views(&mut engine, |e| !e.tab(side).busy());
    let closed = engine.tab(side).id;
    let releases = gate_view_workers(&engine);
    engine.dispatch(Command::Sort(side, Column::Name, false));
    assert!(engine.tab(side).preparing.is_some());
    engine.dispatch(Command::CloseTab(side, 1));
    for release in releases {
        release.send(()).unwrap();
    }
    engine.poll();
    assert_eq!(engine.pane(side).tabs.len(), 1);
    assert_ne!(engine.tab(side).id, closed);
    assert_ne!(engine.tab(side).base(), folder);
    assert_eq!(engine.tab(side).current().unwrap().name, "folder");
}

#[cfg(unix)]
#[test]
fn prepared_view_distinguishes_native_paths_with_the_same_display_name() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let [first, second] =
        [b"name\x80", b"name\x81"].map(|name| root.join(std::ffi::OsStr::from_bytes(name)));
    std::fs::write(&first, b"one").unwrap();
    std::fs::write(&second, b"two").unwrap();
    let side = Side::Left;
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert_eq!(
        engine.tab(side).visible_entry(0).unwrap().name,
        engine.tab(side).visible_entry(1).unwrap().name
    );
    engine.dispatch(Command::Select(side, second.clone(), Select::Replace));
    engine.dispatch(Command::Sort(side, Column::Path, false));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert_eq!(engine.tab(side).current().unwrap().path, second);
    assert!(engine.tab(side).is_marked(&second));
    assert!(!engine.tab(side).is_marked(&first));
}

#[test]
fn review_scope_change_preserves_marks_outside_the_first_page() {
    let GatedDirectory {
        _fixture,
        mut engine,
        release,
        ..
    } = gated_directory(None);
    let side = Side::Left;
    release.send(()).unwrap();
    settle_views(&mut engine, |e| !e.tab(side).busy());
    let mut recursive = MatchOptions::filename("");
    recursive.recursive = true;
    engine.dispatch(Command::Filter(side, recursive));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::MarkAll(side));
    assert_eq!(engine.tab(side).marked_count(), 256);

    let (release, gate) = mpsc::channel();
    let gate = std::sync::Mutex::new(gate);
    engine.first_page_hook = Some(Arc::new(move |_| {
        gate.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .map_err(io::Error::other)
    }));
    engine.dispatch(Command::Filter(side, MatchOptions::filename("")));
    settle_views(&mut engine, |e| {
        e.tab(side).provisional() || !e.tab(side).busy()
    });
    if engine.tab(side).provisional() {
        release.send(()).unwrap();
        settle_views(&mut engine, |e| !e.tab(side).busy());
    }
    assert_eq!(engine.tab(side).visible_count(), 256);
    assert_eq!(engine.tab(side).marked_count(), 256);
    assert_eq!(engine.tab(side).selected_bytes(), 1024);
}

#[test]
fn review_parent_focus_survives_the_first_page_and_yields_to_explicit_selection() {
    for select_another in [false, true] {
        let GatedDirectory {
            _fixture,
            mut engine,
            directory,
            release,
            ..
        } = gated_directory(None);
        let side = Side::Left;
        release.send(()).unwrap();
        settle_views(&mut engine, |e| !e.tab(side).busy());
        let child = directory.join("z-child");
        std::fs::create_dir(&child).unwrap();
        engine.set_sort_preferences(true, false);
        engine.dispatch(Command::Navigate {
            side,
            path: child.clone(),
            new_tab: false,
            background: false,
        });
        settle_views(&mut engine, |e| !e.tab(side).busy());

        let (release, gate) = mpsc::channel();
        let gate = std::sync::Mutex::new(gate);
        engine.first_page_hook = Some(Arc::new(move |_| {
            gate.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .map_err(io::Error::other)
        }));
        engine.snapshots.remove(&directory);
        engine.dispatch(Command::Parent(side));
        settle_views(&mut engine, |e| e.tab(side).provisional());
        let expected = if select_another {
            let other = engine
                .tab(side)
                .visible_entries()
                .find(|entry| entry.path != child)
                .unwrap()
                .path
                .clone();
            engine.dispatch(Command::Select(side, other.clone(), Select::Focus));
            other
        } else {
            child
        };
        release.send(()).unwrap();
        settle_views(&mut engine, |e| !e.tab(side).busy());
        assert_eq!(engine.tab(side).current().unwrap().path, expected);
        assert!(engine.take_cursor_restored(side));
    }
}

#[test]
fn review_bulk_mark_waits_for_the_requested_filter_membership() {
    let GatedDirectory {
        _fixture,
        mut engine,
        release,
        ..
    } = gated_directory(None);
    let side = Side::Left;
    release.send(()).unwrap();
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::Filter(side, MatchOptions::filename("file000.txt")));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert_eq!(engine.tab(side).visible_count(), 1);
    engine.dispatch(Command::Filter(side, MatchOptions::filename("")));
    engine.dispatch(Command::MarkAll(side));
    assert_eq!(
        engine.notice.as_deref(),
        Some("Wait for the directory to finish loading")
    );
    assert_eq!(engine.tab(side).marked_count(), 0);
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::MarkAll(side));
    assert_eq!(engine.tab(side).marked_count(), 256);
}

#[test]
fn review_resident_edits_install_while_refresh_is_blocked() {
    let GatedDirectory {
        _fixture,
        mut engine,
        directory,
        release,
        ..
    } = gated_directory(None);
    let side = Side::Left;
    release.send(()).unwrap();
    settle_views(&mut engine, |e| {
        Side::ALL.into_iter().all(|side| !e.tab(side).busy())
    });
    let old = engine.tab(side).raw.clone().unwrap();
    let mut releases = Vec::new();
    let (started, starts) = mpsc::channel();
    for id in 0..2 {
        let (release, gate) = mpsc::channel();
        releases.push(release);
        let started = started.clone();
        assert!(engine.scans.submit((u64::MAX - id, 0, 0), move |cancel| {
            started.send(()).unwrap();
            let _ = gate.recv_timeout(Duration::from_secs(5));
            (cancel.check().map(|()| unreachable!()), None)
        }));
    }
    for _ in 0..2 {
        starts.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    std::fs::write(directory.join("file259.txt"), "new").unwrap();
    engine.dispatch(Command::Refresh);
    assert!(engine.tab(side).scanning);
    engine.dispatch(Command::Sort(side, Column::Name, false));
    engine.dispatch(Command::Filter(side, MatchOptions::filename("file25?.txt")));
    assert!(engine.tab(side).preparing.is_some());
    settle_views(&mut engine, |e| e.tab(side).preparing.is_none());
    assert!(engine.tab(side).busy());
    assert!(engine.tab(side).scanning);
    assert!(Arc::ptr_eq(&old, &engine.tab(side).view.raw));
    assert_eq!(engine.tab(side).visible_count(), 6);
    assert_eq!(
        engine.tab(side).visible_entry(0).unwrap().name,
        "file255.txt"
    );
    for release in releases {
        release.send(()).unwrap();
    }
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert!(!Arc::ptr_eq(&old, &engine.tab(side).view.raw));
    assert_eq!(engine.tab(side).visible_count(), 7);
    assert_eq!(
        engine.tab(side).visible_entry(0).unwrap().name,
        "file259.txt"
    );
}

#[test]
fn ordinary_panes_share_raw_rows_and_revisit_with_a_ready_view() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let folder = root.join("child");
    std::fs::create_dir(&folder).unwrap();
    let first = root.join("a.txt");
    std::fs::write(&first, "small").unwrap();
    std::fs::write(root.join("z.txt"), "larger").unwrap();
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    settle_views(&mut engine, |e| {
        Side::ALL.into_iter().all(|side| !e.tab(side).busy())
    });
    assert_eq!(engine.listing_stats().full_scans, 1);
    assert!(Arc::ptr_eq(
        &engine.tab(Side::Left).view.raw,
        &engine.tab(Side::Right).view.raw
    ));
    engine.dispatch(Command::Select(Side::Right, first.clone(), Select::Toggle));
    engine.dispatch(Command::Sort(Side::Right, Column::Name, false));
    settle_views(&mut engine, |e| !e.tab(Side::Right).busy());
    assert_eq!(engine.tab(Side::Right).marked_count(), 1);
    assert_eq!(engine.tab(Side::Left).marked_count(), 0);
    assert_ne!(
        engine.tab(Side::Right).visible_entry(1).unwrap().path,
        engine.tab(Side::Left).visible_entry(1).unwrap().path
    );
    // Keep the last retained view in the options the left tab will request on Back.
    engine.dispatch(Command::Sort(Side::Right, Column::Name, true));
    settle_views(&mut engine, |e| !e.tab(Side::Right).busy());
    let order = engine.tab(Side::Right).view.order.clone();
    engine.dispatch(Command::Navigate {
        side: Side::Left,
        path: folder.clone(),
        new_tab: false,
        background: false,
    });
    settle_views(&mut engine, |e| !e.tab(Side::Left).busy());
    assert_eq!(engine.listing_stats().full_scans, 2);

    engine.snapshots.invalidate(|path, _| path == root);
    engine.dispatch(Command::Back(Side::Left));
    settle_views(&mut engine, |e| e.tab(Side::Left).base() == root);
    assert_eq!(engine.tab(Side::Left).visible_count(), 3);
    assert!(Arc::ptr_eq(&order, &engine.tab(Side::Left).view.order));
    assert!(engine.tab(Side::Left).scanning);
    assert_eq!(engine.listing_stats().cache_hits, 1);
    settle_views(&mut engine, |e| !e.tab(Side::Left).busy());
    assert_eq!(engine.listing_stats().full_scans, 3);
}

#[test]
fn jobs_refresh_only_affected_scopes_even_after_partial_failure() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let source = root.join("source");
    let target = root.join("target");
    for folder in [&source, &target] {
        std::fs::create_dir(folder).unwrap();
    }
    let file = source.join("first");
    std::fs::write(&file, "contents").unwrap();
    let mut engine = Engine::new(
        root,
        [source.clone(), target.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    let idle = |e: &Engine| Side::ALL.into_iter().all(|side| !e.tab(side).busy());
    settle_views(&mut engine, idle);
    let initial = engine.listing_stats().full_scans;
    let run = |engine: &mut Engine, operation| {
        engine.dispatch(Command::Queue(operation, false));
        settle_views(engine, |e| {
            e.jobs.last().unwrap().state.finished() && idle(e)
        });
    };
    run(&mut engine, Operation::Statistics(vec![source.clone()]));
    assert_eq!(engine.listing_stats().full_scans, initial);
    run(
        &mut engine,
        Operation::Transfer {
            paths: vec![file.clone(), source.join("missing")],
            destination: target.clone(),
            moving: false,
            options: Default::default(),
        },
    );
    assert_eq!(engine.jobs.last().unwrap().state, JobState::Failed);
    assert_eq!(engine.listing_stats().full_scans, initial + 1);
    assert!(
        engine
            .tab(Side::Right)
            .visible_position(&target.join("first"))
            .is_some()
    );

    let second = source.join("second");
    run(
        &mut engine,
        Operation::Rename {
            source: file,
            target: second.clone(),
        },
    );
    assert_eq!(engine.listing_stats().full_scans, initial + 2);
    run(
        &mut engine,
        Operation::Transfer {
            paths: vec![second.clone()],
            destination: target.clone(),
            moving: true,
            options: Default::default(),
        },
    );
    assert_eq!(engine.listing_stats().full_scans, initial + 4);
    assert!(engine.tab(Side::Left).visible_position(&second).is_none());
    assert!(
        engine
            .tab(Side::Right)
            .visible_position(&target.join("second"))
            .is_some()
    );
}

#[test]
fn watch_hint_during_baseline_enumeration_triggers_one_reconciliation() {
    let GatedDirectory {
        _fixture,
        mut engine,
        directory,
        release,
        ..
    } = gated_directory(None);
    let side = Side::Left;
    engine.settings.auto_refresh = true;
    engine.poll();
    settle_views(&mut engine, |e| e.tab(side).provisional());
    let scans = engine.listing_stats().full_scans;
    let path = directory.join("new.txt");
    let event = || {
        notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
            .add_path(path.clone())
    };
    engine.watches.inject(&directory, Ok(event()));
    engine.watches.inject(&directory, Ok(event()));
    std::fs::write(&path, "late").unwrap();
    engine.poll();
    assert!(engine.tab(side).scanning);
    assert!(engine.tab(side).dirty);
    release.send(()).unwrap();
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert_eq!(engine.listing_stats().full_scans, scans + 1);
    assert!(engine.tab(side).visible_position(&path).is_some());
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
#[test]
fn native_watch_updates_an_active_directory() {
    let fixture = tempfile::tempdir().unwrap();
    let dir = fixture.path().canonicalize().unwrap();
    let mut engine = Engine::new(
        dir.clone(),
        [dir.clone(), dir.clone()],
        Settings::default(),
        || {},
    );
    settle_views(&mut engine, |e| {
        !e.tab(Side::Left).busy() && e.watches.reliable(&dir)
    });
    let count = engine.listing_stats().full_scans;
    engine.tab_mut(Side::Left).last_scan = Some(Instant::now() - Duration::from_secs(5));
    engine.poll();
    assert_eq!(engine.listing_stats().full_scans, count);
    let path = dir.join("external");
    std::fs::write(&path, b"external").unwrap();
    settle_views(&mut engine, |e| {
        e.tab(Side::Left).visible_position(&path).is_some()
    });
    #[cfg(target_os = "linux")]
    {
        assert_eq!(engine.listing_stats().full_scans, count);
        assert_eq!(engine.listing_stats().patches, 1);
    }
}

#[test]
fn cached_history_rows_do_not_replace_committed_operands_when_validation_fails() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let a = root.join("a");
    let b = root.join("b");
    for path in [&a, &b] {
        std::fs::create_dir(path).unwrap();
        std::fs::write(path.join("file"), b"data").unwrap();
    }
    let side = Side::Left;
    let mut engine = Engine::new(
        root,
        [a.clone(), b.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::Navigate {
        side,
        path: b.clone(),
        new_tab: false,
        background: false,
    });
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::MarkAll(side));
    std::fs::remove_dir_all(&a).unwrap();
    engine.dispatch(Command::History(side, 0));
    let preview = engine.history_preview(side).unwrap();
    assert_eq!(preview.path, a);
    assert_eq!(preview.entries().next().unwrap().path, a.join("file"));
    assert_eq!(engine.tab(side).operands(), [b.join("file")]);
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert!(engine.history_preview(side).is_none());
    assert_eq!(engine.tab(side).base(), b);
    assert_eq!(engine.tab(side).operands(), [b.join("file")]);
}

#[cfg(unix)]
#[test]
fn incremental_changes_preserve_native_focus_marks_and_sorting() {
    let fixture = tempfile::tempdir().unwrap();
    let dir = fixture.path().canonicalize().unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"bbb").unwrap();
    let mut engine = Engine::new(
        dir.clone(),
        [dir.clone(), dir.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    let side = Side::Left;
    settle_views(&mut engine, |e| {
        Side::ALL.into_iter().all(|side| !e.tab(side).busy())
    });
    engine.dispatch(Command::Sort(side, Column::Size, false));
    engine.dispatch(Command::Sort(side, Column::Size, false));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::Select(side, a.clone(), Select::Toggle));

    std::fs::write(&a, b"aaaaa").unwrap();
    engine.dirty_directory(&dir, watch::Hint::Paths(vec![a.clone()]));
    engine.poll();
    engine.dispatch(Command::Select(side, b.clone(), Select::Toggle));
    settle_views(&mut engine, |e| {
        let tab = e.tab(side);
        !tab.busy()
            && tab
                .visible_entry(0)
                .is_some_and(|entry| entry.path == a && entry.size == 5)
    });
    let tab = engine.tab(side);
    assert_eq!(tab.visible_entry(0).unwrap().path, a);
    assert_eq!(tab.current().unwrap().path, b);
    assert!(tab.is_marked(&a) && tab.is_marked(&b));
    assert_eq!(tab.selected_bytes(), 8);
    assert_eq!(engine.listing_stats().full_scans, 1);
    assert_eq!(engine.listing_stats().patches, 1);
    let reference = listing::scan_cancellable(
        &Location::Directory(dir),
        false,
        &Default::default(),
        |_| {},
    )
    .unwrap();
    for expected in reference.entries {
        let actual = tab
            .visible_entry(tab.visible_position(&expected.path).unwrap())
            .unwrap();
        assert_eq!(actual.size, expected.size);
        assert_eq!(actual.comment, expected.comment);
    }
}

#[test]
fn sidecar_and_replaced_directory_trigger_complete_reconciliation() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let dir = root.join("active");
    let old = root.join("old");
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("file");
    std::fs::write(&file, b"data").unwrap();
    let side = Side::Left;
    let mut engine = Engine::new(
        root.clone(),
        [dir.clone(), root],
        Settings::default(),
        || {},
    );
    settle_views(&mut engine, |e| !e.tab(side).busy());
    let scans = engine.listing_stats().full_scans;
    let sidecar = dir.join("descript.ion");
    let modify = notify::EventKind::Modify(notify::event::ModifyKind::Data(
        notify::event::DataChange::Content,
    ));
    engine.watches.inject(
        &dir,
        Ok(notify::Event::new(modify).add_path(sidecar.clone())),
    );
    std::fs::write(&sidecar, b"file updated comment\n").unwrap();
    settle_views(&mut engine, |e| {
        e.tab(side)
            .visible_entries()
            .any(|row| row.comment == "updated comment")
    });
    assert!(engine.listing_stats().full_scans > scans);
    engine
        .watches
        .inject(&dir, Err(notify::Error::generic("watch invalidation")));
    std::fs::rename(&dir, old).unwrap();
    std::fs::create_dir(&dir).unwrap();
    let replacement = dir.join("replacement");
    std::fs::write(&replacement, b"new").unwrap();
    settle_views(&mut engine, |e| {
        e.tab(side).visible_position(&replacement).is_some()
            && e.tab(side).visible_position(&file).is_none()
    });
    assert_eq!(engine.tab(side).base(), dir);
}

#[cfg(unix)]
#[test]
fn manual_refresh_supersedes_an_incremental_read_before_it_can_publish() {
    let fixture = tempfile::tempdir().unwrap();
    let dir = fixture.path().canonicalize().unwrap();
    let path = dir.join("update");
    std::fs::write(&path, b"before").unwrap();
    let mut engine = Engine::new(
        dir.clone(),
        [dir.clone(), dir.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    let idle = |e: &Engine| Side::ALL.into_iter().all(|side| !e.tab(side).busy());
    settle_views(&mut engine, idle);
    let initial = engine.listing_stats().full_scans;
    let (started, starts) = mpsc::channel();
    let releases: Vec<_> = (0..2)
        .map(|id| {
            let (release, gate) = mpsc::channel();
            let started = started.clone();
            assert!(engine.scans.submit((u64::MAX - id, 0, 0), move |cancel| {
                started.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(5)).unwrap();
                (cancel.check().map(|()| unreachable!()), None)
            }));
            release
        })
        .collect();
    for _ in 0..2 {
        starts.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    std::fs::write(&path, b"incremental").unwrap();
    engine.dirty_directory(&dir, watch::Hint::Paths(vec![path.clone()]));
    engine.poll();
    assert!(engine.tab(Side::Left).scan_base.is_some());
    std::fs::remove_file(&path).unwrap();
    engine.dispatch(Command::Refresh);
    for release in releases {
        release.send(()).unwrap();
    }
    settle_views(&mut engine, idle);
    assert_eq!(engine.listing_stats().patches, 0);
    assert_eq!(engine.listing_stats().full_scans, initial + 1);
    assert_eq!(engine.tab(Side::Left).visible_count(), 0);
    assert_eq!(engine.tab(Side::Right).visible_count(), 0);
}

#[test]
fn ordinary_cache_cannot_discard_marks_learned_by_a_newer_recursive_scan() {
    let fixture = tempfile::tempdir().unwrap();
    let dir = fixture.path().canonicalize().unwrap();
    let mut engine = Engine::new(
        dir.clone(),
        [dir.clone(), dir.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    let side = Side::Left;
    settle_views(&mut engine, |e| !e.tab(side).busy());
    let late = dir.join("late");
    std::fs::write(&late, b"data").unwrap();
    let mut recursive = MatchOptions::filename("");
    recursive.recursive = true;
    engine.dispatch(Command::Filter(side, recursive));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::MarkAll(side));
    assert!(engine.tab(side).is_marked(&late));
    engine.dispatch(Command::Filter(side, MatchOptions::filename("")));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert!(engine.tab(side).is_marked(&late));
    assert_eq!(engine.tab(side).selected_bytes(), 4);
}

#[test]
fn unsupported_watch_and_virtual_scope_reconcile_on_the_fallback_interval() {
    let fixture = tempfile::tempdir().unwrap();
    let dir = fixture.path().canonicalize().unwrap();
    let path = dir.join("entry");
    std::fs::write(&path, "before").unwrap();
    let left = Side::Left;
    let right = Side::Right;
    let mut engine = Engine::new(
        dir.clone(),
        [dir.clone(), dir.clone()],
        Settings::default(),
        || {},
    );
    engine.tab_mut(right).set_location(
        Location::List {
            base: dir.clone(),
            paths: vec![path.clone()],
        },
        true,
    );
    let idle = |e: &Engine| Side::ALL.into_iter().all(|side| !e.tab(side).busy());
    settle_views(&mut engine, idle);
    let before = engine.listing_stats().full_scans;
    engine
        .watches
        .inject(&dir, Err(notify::Error::generic("unsupported filesystem")));
    engine.watches.drain(Instant::now());
    assert!(!engine.watches.reliable(&dir));
    let completed = Instant::now() - snapshots::UNWATCHED_INTERVAL;
    for side in Side::ALL {
        engine.tab_mut(side).last_scan = Some(completed);
    }
    engine.poll();
    assert!(engine.tab(left).scanning && engine.tab(right).scanning);
    settle_views(&mut engine, idle);
    assert_eq!(engine.listing_stats().full_scans, before + 1);
    assert_eq!(engine.tab(right).visible_entry(0).unwrap().path, path);
}

#[test]
fn review_revisit_revalidates_after_watch_coverage_ends() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let dir = root.join("watched");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("cached"), "old").unwrap();
    let side = Side::Left;
    let mut engine = Engine::new(
        root.clone(),
        [dir.clone(), root.clone()],
        Settings::default(),
        || {},
    );
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::Navigate {
        side,
        path: root.clone(),
        new_tab: false,
        background: false,
    });
    settle_views(&mut engine, |e| {
        e.tab(side).base() == root && !e.tab(side).busy()
    });
    let new = dir.join("new");
    std::fs::write(&new, "changed while unwatched").unwrap();

    engine.dispatch(Command::Back(side));
    settle_views(&mut engine, |e| e.tab(side).base() == dir);
    assert!(
        engine
            .tab(side)
            .visible_position(&dir.join("cached"))
            .is_some()
    );
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert!(engine.tab(side).visible_position(&new).is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn review_inactive_cached_directory_does_not_hold_a_descriptor() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let dir = root.join("removable");
    std::fs::create_dir(&dir).unwrap();
    let side = Side::Left;
    let mut engine = Engine::new(
        root.clone(),
        [dir.clone(), root.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::Navigate {
        side,
        path: root.clone(),
        new_tab: false,
        background: false,
    });
    settle_views(&mut engine, |e| {
        e.tab(side).base() == root && !e.tab(side).busy()
    });
    assert!(engine.listing_stats().retained_directories >= 2);
    assert!(
        !std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|path| path == dir))
    );
}

#[cfg(unix)]
#[test]
fn review_file_changes_update_hard_link_aliases_and_marked_bytes() {
    let fixture = tempfile::tempdir().unwrap();
    let dir = fixture.path().canonicalize().unwrap();
    let a = dir.join("a");
    let b = dir.join("b");
    std::fs::write(&a, b"a").unwrap();
    std::fs::hard_link(&a, &b).unwrap();
    let side = Side::Left;
    let mut engine = Engine::new(
        dir.clone(),
        [dir.clone(), dir.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    settle_views(&mut engine, |e| !e.tab(side).busy());
    engine.dispatch(Command::Select(side, b.clone(), Select::Toggle));
    std::fs::write(&a, b"changed").unwrap();
    engine.dirty_directory(&dir, watch::Hint::Paths(vec![a]));
    engine.poll();
    settle_views(&mut engine, |e| !e.tab(side).busy());
    let tab = engine.tab(side);
    assert_eq!(tab.current().unwrap().path, b);
    assert_eq!(tab.current().unwrap().size, 7);
    assert_eq!(tab.selected_bytes(), 7);
}

#[cfg(unix)]
#[test]
fn review_job_through_real_path_updates_the_directory_symlink_pane() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let dir = root.join("real");
    let alias = root.join("alias");
    std::fs::create_dir(&dir).unwrap();
    std::os::unix::fs::symlink(&dir, &alias).unwrap();
    let mut engine = Engine::new(
        root,
        [dir.clone(), alias.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    let idle = |e: &Engine| Side::ALL.into_iter().all(|side| !e.tab(side).busy());
    settle_views(&mut engine, idle);
    engine.dispatch(Command::Queue(Operation::Create(dir.join("new")), false));
    settle_views(&mut engine, |e| {
        e.jobs.last().unwrap().state.finished() && idle(e)
    });
    assert_eq!(engine.tab(Side::Left).visible_count(), 1);
    assert!(
        engine
            .tab(Side::Right)
            .visible_position(&alias.join("new"))
            .is_some()
    );
}

#[test]
fn review_multivolume_archive_effects_include_virtual_members() {
    let fixture = tempfile::tempdir().unwrap();
    let dir = fixture.path().canonicalize().unwrap();
    let operation = Operation::Archive(ArchiveOperation::Pack {
        paths: vec![dir.join("input")],
        output: dir.join("archive.7z"),
        folder: PathBuf::new(),
        options: PackOptions {
            volume: "1m".into(),
            ..Default::default()
        },
    });
    let effects = changes::Effects::for_operation(&operation);
    assert!(effects.listing(
        &Location::List {
            base: dir.clone(),
            paths: vec![dir.join("archive.7z.001"), dir.join("archive.7z.002")],
        },
        false
    ));
}

#[test]
fn deleting_focused_rows_prefers_the_next_survivor_then_the_previous_one() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let [a, b, c, d, e] = ["a", "b", "c", "d", "e"].map(|name| root.join(name));
    for path in [&a, &b, &c, &d, &e] {
        std::fs::write(path, "").unwrap();
    }
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    let side = Side::Left;
    settle_views(&mut engine, |engine| !engine.tab(side).busy());

    for (focused, removed, expected) in [
        (&b, vec![b.clone(), c], Some(&d)),
        (&e, vec![e.clone()], Some(&d)),
        (&d, vec![a.clone(), d.clone()], None),
    ] {
        engine.dispatch(Command::Select(side, focused.clone(), Select::Focus));
        engine.dispatch(Command::Queue(
            Operation::Delete {
                paths: removed,
                trash: false,
            },
            false,
        ));
        settle_views(&mut engine, |engine| {
            engine.jobs.last().unwrap().state.finished() && !engine.tab(side).busy()
        });
        assert_eq!(engine.jobs.last().unwrap().state, JobState::Done);
        assert_eq!(
            engine.tab(side).current().map(|entry| &entry.path),
            expected
        );
        assert_eq!(engine.take_cursor_restored(side), expected.is_some());
    }
}
