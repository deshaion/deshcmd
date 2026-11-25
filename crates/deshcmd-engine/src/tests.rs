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
    engine.last_refresh = Instant::now() - Duration::from_secs(4);
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
fn global_view_options_apply_to_inactive_tabs_without_rescanning() {
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
    let raw = engine.tab(side).raw.clone().unwrap();
    engine.dispatch(Command::NewTab(side));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    std::fs::remove_dir_all(&folder).unwrap();
    engine.set_sort_preferences(false, true);
    engine.dispatch(Command::ToggleHidden);
    engine.set_ignore("ignored".into()).unwrap();
    engine.dispatch(Command::Tab(side, 0));
    settle_views(&mut engine, |e| !e.tab(side).busy());
    assert_eq!(engine.tab(side).base(), folder);
    assert!(Arc::ptr_eq(&raw, engine.tab(side).raw.as_ref().unwrap()));
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
