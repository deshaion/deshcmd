use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use deshcmd_engine::{
    Algorithm, Collision, Command, Engine, JobState, Location, ManifestMode, MatchOptions,
    Operation, Select, Settings, Side, TabLock, TransferOptions,
};

fn pump(engine: &mut Engine, done: impl Fn(&Engine) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        engine.poll();
        if done(engine) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the engine"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn open(left: &Path, right: &Path) -> Engine {
    let mut engine = Engine::new(
        left.to_owned(),
        [left.to_owned(), right.to_owned()],
        Settings::default(),
        || {},
    );
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).loading)
    });
    engine
}

fn run(engine: &mut Engine, operation: Operation) {
    engine.dispatch(Command::Queue(operation, false));
    pump(engine, |engine| {
        engine.jobs.last().unwrap().state.finished()
    });
}

#[test]
fn pane_navigation_filters_and_restores_marks() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir(root.join("sub")).unwrap();
    for name in ["report20.txt", "report3.txt", "café.txt", ".hidden"] {
        fs::write(root.join(name), name).unwrap();
    }
    let mut engine = open(root, root);
    let side = Side::Left;
    let names: Vec<_> = engine
        .tab(side)
        .visible_entries()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["sub", "café.txt", "report3.txt", "report20.txt"]);
    engine.dispatch(Command::Select(
        side,
        root.join("café.txt"),
        Select::Replace,
    ));
    engine.dispatch(Command::RememberMarks(side));
    engine.dispatch(Command::Filter(side, MatchOptions::filename("REPORT?.TXT")));
    assert_eq!(
        engine.tab(side).visible_entries().next().unwrap().name,
        "report3.txt"
    );
    assert!(engine.tab(side).marked_count() == 0);
    engine.dispatch(Command::Filter(side, MatchOptions::filename("")));
    engine.dispatch(Command::RestoreMarks(side));
    assert!(engine.tab(side).is_marked(&root.join("café.txt")));

    engine.dispatch(Command::Navigate {
        side,
        path: root.join("sub"),
        new_tab: false,
        background: false,
    });
    assert!(engine.tab(side).can_back());
    engine.dispatch(Command::Back(side));
    assert_eq!(engine.tab(side).base(), root);
    engine.dispatch(Command::LockTab(side, 0, TabLock::NewTab));
    engine.dispatch(Command::Navigate {
        side,
        path: root.join("sub"),
        new_tab: false,
        background: false,
    });
    assert_eq!(engine.pane(side).tabs.len(), 2);
    assert_eq!(engine.tab(side).base(), root.join("sub"));
    let snapshot = engine.snapshot();
    let reopened = Engine::new(
        root.to_owned(),
        [root.to_owned(), root.to_owned()],
        snapshot,
        || {},
    );
    assert_eq!(reopened.tab(side).base(), root.join("sub"));
}

#[test]
fn duplicate_cleanup_keeps_pinned_tabs_and_selects_the_surviving_location() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let folder = root.join("folder");
    fs::create_dir(&folder).unwrap();
    let mut engine = open(root, root);
    let side = Side::Left;

    engine.dispatch(Command::OpenFocusedDirectoryInTab {
        source: side,
        destination: side,
    });
    assert_eq!(engine.tab(side).base(), folder);
    engine.dispatch(Command::ToggleTabPin(side, 1));
    engine.dispatch(Command::NewTab(side));
    engine.dispatch(Command::ToggleTabPin(side, 2));
    engine.dispatch(Command::NewTab(side));
    engine.dispatch(Command::CloseDuplicates(side));

    let pane = engine.pane(side);
    assert_eq!(pane.tabs.len(), 3);
    assert!(pane.tabs[1..].iter().all(|tab| tab.saved.pinned));
    assert_eq!(pane.tab().base(), folder);
    assert!(pane.tab().saved.pinned);
}

#[test]
fn duplicate_cleanup_distinguishes_virtual_tabs_with_the_same_base() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let mut engine = open(root, root);
    let side = Side::Left;
    engine.dispatch(Command::ToggleTabPin(side, 0));

    for filename in ["first.txt", "second.txt", "first.txt"] {
        fs::write(root.join(filename), "").unwrap();
        engine.dispatch(Command::NewTab(side));
        engine.dispatch(Command::LoadList(side, filename.to_owned()));
    }
    let active_id = engine.tab(side).id;
    engine.dispatch(Command::CloseDuplicates(side));

    let pane = engine.pane(side);
    assert_eq!(pane.tabs.len(), 3);
    assert!(pane.tabs[0].saved.pinned);
    assert_eq!(pane.tabs[0].location(), &Location::Directory(root.into()));
    assert_eq!(pane.tab().id, active_id);
    for (tab, filename) in pane.tabs[1..].iter().zip(["second.txt", "first.txt"]) {
        assert_eq!(
            tab.location(),
            &Location::List {
                base: root.into(),
                paths: vec![root.join(filename)],
            }
        );
    }
}

#[test]
fn parent_navigation_restores_the_child_in_linked_panes_and_locked_tabs() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let left = root.join("left");
    let right = root.join("right");
    for parent in [&left, &right] {
        fs::create_dir_all(parent.join("first")).unwrap();
        fs::create_dir(parent.join("previous")).unwrap();
    }
    let left_child = left.join("previous");
    let right_child = right.join("previous");
    let mut engine = open(&left_child, &right_child);
    let side = Side::Left;
    engine.settings.linked = true;
    engine.dispatch(Command::Parent(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).loading)
    });
    assert_eq!(engine.settings.active, side);
    for (pane, child) in [(Side::Left, &left_child), (Side::Right, &right_child)] {
        assert_eq!(engine.tab(pane).current().unwrap().path, *child);
        assert!(engine.take_parent_focus_restored(pane));
        assert!(!engine.take_parent_focus_restored(pane));
    }

    engine.dispatch(Command::Back(side));
    engine.dispatch(Command::Forward(side));
    pump(&mut engine, |engine| !engine.tab(side).loading);
    assert_eq!(engine.tab(side).current().unwrap().path, left_child);

    engine.dispatch(Command::Back(side));
    engine.dispatch(Command::LockTab(side, 0, TabLock::NewTab));
    engine.dispatch(Command::Parent(side));
    pump(&mut engine, |engine| !engine.tab(side).loading);
    assert_eq!(engine.pane(side).tabs.len(), 2);
    assert_eq!(engine.tab(side).current().unwrap().path, left_child);
}

#[cfg(unix)]
#[test]
fn navigating_back_from_a_directory_symlink_focuses_the_link() {
    let fixture = tempfile::tempdir().unwrap();
    let parent = fixture.path().canonicalize().unwrap();
    fs::create_dir(parent.join("0-first")).unwrap();
    let target = parent.join("z-real");
    fs::create_dir(&target).unwrap();
    let link = parent.join("a-link");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let mut engine = open(&parent, &parent);
    let side = Side::Left;
    engine.dispatch(Command::Navigate {
        side,
        path: link.clone(),
        new_tab: false,
        background: false,
    });
    assert_eq!(engine.tab(side).base(), target);
    engine.dispatch(Command::Back(side));
    pump(&mut engine, |engine| !engine.tab(side).loading);
    assert_eq!(engine.tab(side).current().unwrap().path, link);

    engine.dispatch(Command::Forward(side));
    engine.dispatch(Command::Parent(side));
    pump(&mut engine, |engine| !engine.tab(side).loading);
    assert_eq!(engine.tab(side).current().unwrap().path, link);
}

#[test]
fn recursive_filter_applies_to_nested_items_and_can_be_cleared() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("sub/Match folder")).unwrap();
    fs::create_dir(root.join(".private")).unwrap();
    for path in ["sub/Match.txt", "sub/lower match.txt", ".private/Match.txt"] {
        fs::write(root.join(path), "").unwrap();
    }
    let mut engine = open(root, root);
    let side = Side::Left;
    let mut criteria = MatchOptions::filename("Match");
    criteria.case_sensitive = true;
    criteria.recursive = true;
    criteria.directories = false;
    engine.dispatch(Command::Filter(side, criteria.clone()));
    pump(&mut engine, |engine| !engine.tab(side).loading);
    let visible_paths = |engine: &Engine| {
        engine
            .tab(side)
            .visible_entries()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(visible_paths(&engine), [root.join("sub/Match.txt")]);
    assert_eq!(engine.tab(side).base(), root);
    assert!(!engine.tab(side).can_back());

    criteria.files = false;
    criteria.directories = true;
    engine.dispatch(Command::Filter(side, criteria));
    assert_eq!(visible_paths(&engine), [root.join("sub/Match folder")]);

    engine.dispatch(Command::Filter(side, MatchOptions::filename("")));
    pump(&mut engine, |engine| !engine.tab(side).loading);
    assert_eq!(visible_paths(&engine), [root.join("sub")]);
}

#[test]
fn move_conflicts_keep_skipped_files_and_never_clobber_on_rename() {
    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("source");
    let right = fixture.path().join("target");
    fs::create_dir_all(left.join("folder")).unwrap();
    fs::create_dir_all(right.join("folder")).unwrap();
    let source = left.join("folder/original.txt");
    let target = right.join("folder/original.txt");
    fs::write(&source, b"incoming").unwrap();
    fs::write(&target, b"kept").unwrap();
    fs::write(left.join("folder/fresh.txt"), b"fresh").unwrap();

    let mut engine = open(&left, &right);
    engine.dispatch(Command::Queue(
        Operation::Transfer {
            paths: vec![left.join("folder")],
            destination: right.clone(),
            moving: true,
            options: TransferOptions {
                directory_collision: Collision::Merge,
                ..TransferOptions::default()
            },
        },
        false,
    ));
    pump(&mut engine, |engine| {
        engine.jobs[0].state == JobState::Conflict
    });
    engine.dispatch(Command::CompareConflict(engine.jobs[0].id));
    pump(&mut engine, |engine| {
        engine.jobs[0].message == "File contents differ"
    });
    engine.dispatch(Command::Resolve(engine.jobs[0].id, Collision::Skip, false));
    pump(&mut engine, |engine| engine.jobs[0].state.finished());
    assert_eq!(fs::read(&source).unwrap(), b"incoming");
    assert_eq!(fs::read(&target).unwrap(), b"kept");
    assert_eq!(fs::read(right.join("folder/fresh.txt")).unwrap(), b"fresh");
    assert!(!engine.jobs[0].warnings.is_empty());

    assert!(!left.join("folder/fresh.txt").exists());
    let protected = left.join("folder/guard.txt");
    fs::write(&protected, b"protected").unwrap();
    run(
        &mut engine,
        Operation::Rename {
            source: source.clone(),
            target: protected.clone(),
        },
    );
    assert_eq!(engine.jobs.last().unwrap().state, JobState::Failed);
    assert_eq!(fs::read(source).unwrap(), b"incoming");
    assert_eq!(fs::read(protected).unwrap(), b"protected");
}

#[test]
fn verified_copy_split_join_and_manifest_round_trip() {
    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("source");
    let right = fixture.path().join("target");
    fs::create_dir(&left).unwrap();
    fs::create_dir(&right).unwrap();
    let source = left.join("data.bin");
    let payload = b"original bytes for the integrity workflow";
    fs::write(&source, payload).unwrap();
    let mut engine = open(&left, &right);
    run(
        &mut engine,
        Operation::Transfer {
            paths: vec![source.clone()],
            destination: right.clone(),
            moving: false,
            options: TransferOptions {
                verify: true,
                preallocate: true,
                ..TransferOptions::default()
            },
        },
    );
    assert_eq!(engine.jobs.last().unwrap().state, JobState::Done);
    let copy = right.join("data.bin");
    assert_eq!(fs::read(&copy).unwrap(), payload);

    run(
        &mut engine,
        Operation::Split {
            source: copy.clone(),
            destination: right.clone(),
            part_bytes: 12,
            part_count: None,
        },
    );
    assert_eq!(engine.jobs.last().unwrap().state, JobState::Done);
    let parts = deshcmd_engine::numbered_parts(&right.join("data.bin.001"));
    assert_eq!(parts.len(), 4);
    let output = right.join("joined.bin");
    run(
        &mut engine,
        Operation::Combine {
            paths: parts,
            output: output.clone(),
        },
    );
    assert_eq!(fs::read(&output).unwrap(), payload);

    for algorithm in [Algorithm::Sha256, Algorithm::Sfv, Algorithm::Xxh128] {
        let manifest = right.join(format!("data.{}", algorithm.extension()));
        run(
            &mut engine,
            Operation::Hash {
                paths: vec![output.clone()],
                output: manifest.clone(),
                algorithm,
                mode: ManifestMode::Single,
            },
        );
        assert_eq!(engine.jobs.last().unwrap().state, JobState::Done);
        run(
            &mut engine,
            Operation::Verify {
                source: manifest,
                expected: None,
                algorithm,
            },
        );
        assert_eq!(engine.jobs.last().unwrap().state, JobState::Done);
    }
    fs::write(&output, b"changed").unwrap();
    run(
        &mut engine,
        Operation::Verify {
            source: right.join("data.sha256"),
            expected: None,
            algorithm: Algorithm::Sha256,
        },
    );
    assert_eq!(engine.jobs.last().unwrap().state, JobState::Failed);
    assert!(engine.jobs.last().unwrap().warnings[0].contains("checksum mismatch"));
}

#[cfg(unix)]
#[test]
fn a_filtered_move_and_a_symlink_wipe_preserve_other_files() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(source.join("match.txt"), b"keep contents").unwrap();
    fs::write(source.join("ignored.bin"), b"unselected").unwrap();
    let link = root.join("alias");
    std::os::unix::fs::symlink(source.join("ignored.bin"), &link).unwrap();
    let mut engine = open(root, &destination);
    run(
        &mut engine,
        Operation::Transfer {
            paths: vec![source.clone()],
            destination: destination.clone(),
            moving: true,
            options: TransferOptions {
                mask: "*.txt".into(),
                ..TransferOptions::default()
            },
        },
    );
    assert!(source.join("ignored.bin").exists());
    assert!(destination.join("source/match.txt").exists());
    assert!(!source.join("match.txt").exists());
    run(
        &mut engine,
        Operation::Wipe {
            paths: vec![link.clone()],
            passes: 1,
        },
    );
    assert!(link.symlink_metadata().is_err());
    assert_eq!(fs::read(source.join("ignored.bin")).unwrap(), b"unselected");
}

#[test]
fn resume_requires_the_destination_to_match_the_source_prefix() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let left = root.join("left");
    let right = root.join("right");
    fs::create_dir(&left).unwrap();
    fs::create_dir(&right).unwrap();
    let source = left.join("data.txt");
    let target = right.join("data.txt");
    fs::write(&source, b"complete content").unwrap();
    fs::write(&target, b"compared").unwrap();
    let mut engine = open(&left, &right);
    let transfer = || Operation::Transfer {
        paths: vec![source.clone()],
        destination: right.clone(),
        moving: false,
        options: TransferOptions {
            collision: Collision::Resume,
            ..TransferOptions::default()
        },
    };

    run(&mut engine, transfer());
    assert_eq!(engine.jobs.last().unwrap().state, JobState::Failed);
    assert_eq!(fs::read(&target).unwrap(), b"compared");

    fs::write(&target, b"complete").unwrap();
    run(&mut engine, transfer());
    assert_eq!(engine.jobs.last().unwrap().state, JobState::Done);
    assert_eq!(fs::read(&target).unwrap(), b"complete content");
}

#[test]
fn cursor_and_mark_totals_follow_the_visible_view() {
    use deshcmd_engine::{Column, Step, Tab};

    fn check(tab: &Tab, current: Option<&str>, bytes: u64, marks: usize) {
        assert_eq!(tab.current().map(|entry| entry.name.as_str()), current);
        assert_eq!(
            tab.cursor_position(),
            tab.visible_entries()
                .position(|entry| Some(entry.name.as_str()) == current)
        );
        assert_eq!(tab.marked_count(), marks);
        let marked: Vec<_> = tab
            .visible_entries()
            .filter(|entry| tab.is_marked(&entry.path))
            .collect();
        assert_eq!(marked.len(), marks);
        assert_eq!(tab.selected_bytes(), bytes);
        assert_eq!(
            marked
                .iter()
                .filter(|entry| !entry.is_dir())
                .map(|entry| entry.size)
                .sum::<u64>(),
            bytes
        );
        assert_eq!(tab.has_operands(), current.is_some() || marks != 0);
        let expected = if marked.is_empty() {
            tab.current()
                .into_iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>()
        } else {
            marked.into_iter().map(|entry| entry.path.clone()).collect()
        };
        assert_eq!(tab.operands(), expected);
    }

    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("files");
    fs::create_dir_all(folder.join("sub")).unwrap();
    for (name, size) in [("file2.txt", 2), ("file10.txt", 5), ("report.csv", 7)] {
        fs::write(folder.join(name), vec![0; size]).unwrap();
    }
    let mut engine = open(&folder, fixture.path());
    engine.settings.auto_refresh = false;
    let side = Side::Left;
    let file = |name: &str| folder.join(name);
    engine.dispatch(Command::Select(side, file("file2.txt"), Select::Replace));
    check(engine.tab(side), Some("file2.txt"), 2, 1);
    engine.dispatch(Command::Step(side, Step::Next, true));
    check(engine.tab(side), Some("file10.txt"), 7, 2);
    engine.dispatch(Command::Sort(side, Column::Size, false));
    check(engine.tab(side), Some("file10.txt"), 7, 2);
    engine.dispatch(Command::Step(side, Step::Next, true));
    check(engine.tab(side), Some("report.csv"), 14, 3);
    engine.dispatch(Command::Select(side, file("file2.txt"), Select::Toggle));
    check(engine.tab(side), Some("file2.txt"), 12, 2);
    engine.dispatch(Command::Invert(side));
    check(engine.tab(side), Some("file2.txt"), 2, 2);
    engine.dispatch(Command::MarkMask(
        side,
        MatchOptions::filename("file*.txt"),
        true,
    ));
    check(engine.tab(side), Some("file2.txt"), 7, 3);
    engine.dispatch(Command::MarkLike(side, Column::Extension, false));
    check(engine.tab(side), Some("file2.txt"), 0, 1);
    engine.dispatch(Command::MarkAll(side));
    check(engine.tab(side), Some("file2.txt"), 14, 4);
    engine.dispatch(Command::RememberMarks(side));
    engine.dispatch(Command::Filter(side, MatchOptions::filename("file10")));
    check(engine.tab(side), Some("file10.txt"), 5, 1);
    engine.dispatch(Command::Filter(side, MatchOptions::filename("")));
    engine.dispatch(Command::RestoreMarks(side));
    check(engine.tab(side), Some("file10.txt"), 14, 4);
    engine.dispatch(Command::LoadMarks(side, "file2.txt\nfile10.txt".into()));
    check(engine.tab(side), Some("file10.txt"), 7, 2);
    fs::write(file("file2.txt"), [0; 11]).unwrap();
    fs::remove_file(file("file10.txt")).unwrap();
    engine.dispatch(Command::Refresh);
    pump(&mut engine, |engine| !engine.tab(side).loading);
    check(engine.tab(side), Some("sub"), 11, 1);
    engine.dispatch(Command::Step(side, Step::Last, false));
    check(engine.tab(side), Some("file2.txt"), 11, 1);
    engine.dispatch(Command::UnmarkAll(side));
    check(engine.tab(side), Some("file2.txt"), 0, 0);
    fs::remove_dir_all(&folder).unwrap();
    engine.dispatch(Command::Refresh);
    pump(&mut engine, |engine| !engine.tab(side).loading);
    assert!(engine.tab(side).error.is_some());
    check(engine.tab(side), None, 0, 0);
}

#[test]
fn recursive_projection_excludes_hidden_and_ignored_ancestors() {
    let root = tempfile::tempdir().unwrap();
    for name in ["keep", ".private", "keep/IgnoreME"] {
        fs::create_dir(root.path().join(name)).unwrap();
        fs::write(root.path().join(name).join("item.txt"), name).unwrap();
    }
    let side = Side::Left;
    let mut engine = open(root.path(), root.path());
    engine.dispatch(Command::Ignore("ignore*".to_owned()));
    engine.dispatch(Command::Filter(
        side,
        MatchOptions {
            recursive: true,
            directories: false,
            ..MatchOptions::filename("*.*")
        },
    ));
    pump(&mut engine, |engine| !engine.tab(side).loading);
    let file_paths = |engine: &Engine| {
        engine
            .tab(side)
            .visible_entries()
            .map(|entry| entry.path.strip_prefix(root.path()).unwrap().to_path_buf())
            .collect::<Vec<_>>()
    };
    assert_eq!(file_paths(&engine), [Path::new("keep/item.txt")]);
    engine.dispatch(Command::ToggleHidden);
    assert_eq!(
        file_paths(&engine),
        [Path::new(".private/item.txt"), Path::new("keep/item.txt")]
    );
    engine.dispatch(Command::Filter(side, MatchOptions::filename("[")));
    assert!(engine.notice.is_some());
    assert_eq!(file_paths(&engine).len(), 2);
}

#[cfg(target_os = "linux")]
#[test]
fn delayed_directory_result_cannot_replace_newer_navigation() {
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
    use std::sync::mpsc;

    let root = tempfile::tempdir().unwrap();
    let empty = root.path().join("empty");
    let blocked = root.path().join("blocked");
    let fast = root.path().join("fast");
    for path in [&empty, &blocked, &fast] {
        fs::create_dir(path).unwrap();
    }
    fs::write(fast.join("fast.txt"), "ready").unwrap();
    let fifo = blocked.join("descript.ion");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);

    let (send, wake) = mpsc::channel();
    let mut engine = Engine::new(
        root.path().into(),
        [empty.clone(), empty],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        move || {
            let _ = send.send(());
        },
    );
    let timeout = Duration::from_secs(5);
    for _ in Side::ALL {
        wake.recv_timeout(timeout).unwrap();
    }
    engine.poll();
    let side = Side::Left;
    let navigate = |path| Command::Navigate {
        side,
        path,
        new_tab: false,
        background: false,
    };
    engine.dispatch(navigate(blocked));

    // The writer can open only when the scanner reaches the FIFO. Keeping it open
    // with no data blocks the comment read until the fast navigation has installed.
    let deadline = Instant::now() + timeout;
    let writer = loop {
        match fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
        {
            Ok(writer) => break writer,
            Err(error)
                if error.raw_os_error() == Some(libc::ENXIO) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("Failed to synchronize with the blocked scanner: {error}"),
        }
    };
    engine.dispatch(navigate(fast.clone()));
    wake.recv_timeout(timeout).unwrap();
    engine.poll();
    assert_eq!(engine.tab(side).current().unwrap().name, "fast.txt");

    drop(writer);
    wake.recv_timeout(timeout).unwrap();
    engine.poll();
    assert_eq!(engine.tab(side).base(), fast);
    assert_eq!(engine.tab(side).current().unwrap().name, "fast.txt");
    assert!(!engine.tab(side).loading);
}
