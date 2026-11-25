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
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
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
fn removing_an_open_folder_recovers_to_its_nearest_existing_parent() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let parent = root.join("parent");
    let removed = parent.join("removed");
    let child = removed.join("nested");
    fs::create_dir_all(&child).unwrap();
    let mut engine = open(&root, &root);
    engine.settings.auto_refresh = false;
    for path in [&parent, &removed, &child] {
        engine.dispatch(Command::Navigate {
            side: Side::Left,
            path: path.clone(),
            new_tab: false,
            background: false,
        });
        pump(&mut engine, |engine| {
            Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
        });
    }
    engine.dispatch(Command::Focus(Side::Right));
    run(
        &mut engine,
        Operation::Delete {
            paths: vec![removed],
            trash: false,
        },
    );
    assert_eq!(engine.jobs.last().unwrap().state, JobState::Done);
    pump(&mut engine, |engine| {
        !engine.tab(Side::Left).busy() && engine.tab(Side::Left).base() == parent
    });
    assert!(engine.tab(Side::Left).error.is_none());
    assert_eq!(engine.settings.active, Side::Right);
    engine.dispatch(Command::Back(Side::Left));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(Side::Left).base(), root);
    engine.dispatch(Command::Forward(Side::Left));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(Side::Left).base(), parent);
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
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert!(engine.tab(side).can_back());
    engine.dispatch(Command::Back(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(side).base(), root);
    engine.dispatch(Command::LockTab(side, 0, TabLock::NewTab));
    engine.dispatch(Command::Navigate {
        side,
        path: root.join("sub"),
        new_tab: false,
        background: false,
    });
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
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
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
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
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.settings.active, side);
    for (pane, child) in [(Side::Left, &left_child), (Side::Right, &right_child)] {
        assert_eq!(engine.tab(pane).current().unwrap().path, *child);
        assert!(engine.take_parent_focus_restored(pane));
        assert!(!engine.take_parent_focus_restored(pane));
    }

    engine.dispatch(Command::Back(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    engine.dispatch(Command::Forward(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    pump(&mut engine, |engine| !engine.tab(side).busy());
    assert_eq!(engine.tab(side).current().unwrap().path, left_child);

    engine.dispatch(Command::Back(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    engine.dispatch(Command::LockTab(side, 0, TabLock::NewTab));
    engine.dispatch(Command::Parent(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    pump(&mut engine, |engine| !engine.tab(side).busy());
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
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(side).base(), target);
    engine.dispatch(Command::Back(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    pump(&mut engine, |engine| !engine.tab(side).busy());
    assert_eq!(engine.tab(side).current().unwrap().path, link);

    engine.dispatch(Command::Forward(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    engine.dispatch(Command::Parent(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    pump(&mut engine, |engine| !engine.tab(side).busy());
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
    pump(&mut engine, |engine| !engine.tab(side).busy());
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
    pump(&mut engine, |engine| !engine.tab(side).busy());
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
    assert!(engine.jobs[0].message.contains("Skipped "));

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

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[test]
fn rename_existing_moves_the_incoming_file_and_reports_the_original() {
    let fixture = tempfile::tempdir().unwrap();
    let cross_volume = std::env::var_os("DESHCMD_TEST_DESTINATION");
    let destination = tempfile::tempdir_in(
        cross_volume
            .as_deref()
            .map(Path::new)
            .unwrap_or(fixture.path()),
    )
    .unwrap();
    #[cfg(unix)]
    if cross_volume.is_some() {
        use std::os::unix::fs::MetadataExt;
        assert_ne!(
            fixture.path().metadata().unwrap().dev(),
            destination.path().metadata().unwrap().dev()
        );
    }
    let source = fixture.path().join("example.txt");
    let target = destination.path().join("example.txt");
    let taken = destination.path().join("example (1).txt");
    let backup = destination.path().join("example (2).txt");
    fs::write(&source, b"incoming").unwrap();
    fs::write(&target, b"existing").unwrap();
    fs::write(&taken, b"already taken").unwrap();

    let mut engine = open(fixture.path(), destination.path());
    run(
        &mut engine,
        Operation::Transfer {
            paths: vec![source.clone()],
            destination: destination.path().to_path_buf(),
            moving: true,
            options: TransferOptions {
                collision: Collision::RenameExisting,
                verify: true,
                ..TransferOptions::default()
            },
        },
    );
    let job = engine.jobs.last().unwrap();
    assert_eq!(job.state, JobState::Done, "{:?}", job.warnings);
    assert!(job.message.contains("Moved 1 of 1"));
    assert!(job.message.contains("example (2).txt"));
    assert!(!source.exists());
    assert_eq!(fs::read(target).unwrap(), b"incoming");
    assert_eq!(fs::read(taken).unwrap(), b"already taken");
    assert_eq!(fs::read(backup).unwrap(), b"existing");
}

#[cfg(target_os = "linux")]
#[test]
fn rename_existing_leaves_the_destination_in_place_when_space_check_fails() {
    let fixture = tempfile::tempdir().unwrap();
    let destination = fixture.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let source = fixture.path().join("example.txt");
    let target = destination.join("example.txt");
    let length = fs2::total_space(&destination).unwrap() + 1;
    fs::File::create(&source).unwrap().set_len(length).unwrap();
    fs::write(&target, b"existing").unwrap();

    let mut engine = open(fixture.path(), &destination);
    run(
        &mut engine,
        Operation::Transfer {
            paths: vec![source.clone()],
            destination: destination.clone(),
            moving: false,
            options: TransferOptions {
                collision: Collision::RenameExisting,
                ..TransferOptions::default()
            },
        },
    );
    let job = engine.jobs.last().unwrap();
    assert_eq!(job.state, JobState::Failed);
    assert!(
        job.warnings
            .iter()
            .any(|warning| warning.contains("Insufficient free space"))
    );
    assert_eq!(source.metadata().unwrap().len(), length);
    assert_eq!(fs::read(target).unwrap(), b"existing");
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_failed_staged_folder_move_keeps_the_destination_and_source_tree() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("left");
    let right = fixture.path().join("right");
    let source = left.join("outer/folder");
    let target = right.join("outer/folder");
    fs::create_dir_all(source.join("readonly")).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(source.join("readonly/incoming.txt"), b"incoming").unwrap();
    fs::set_permissions(source.join("readonly"), fs::Permissions::from_mode(0o500)).unwrap();
    fs::write(target.join("existing.txt"), b"existing").unwrap();
    let blocked = source.join("blocked");
    fs::write(&blocked, b"blocked").unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    let source_is_readable = fs::File::open(&blocked).is_ok();
    if source_is_readable {
        fs::remove_file(&blocked).unwrap();
        symlink("missing", &blocked).unwrap();
    }
    let mut engine = open(&left, &right);
    engine.dispatch(Command::Queue(
        Operation::Transfer {
            paths: vec![left.join("outer")],
            destination: right.clone(),
            moving: true,
            options: TransferOptions {
                follow_links: true,
                ..TransferOptions::default()
            },
        },
        false,
    ));
    for (conflict_target, choice) in [
        (right.join("outer"), Collision::Merge),
        (target.clone(), Collision::RenameExisting),
    ] {
        pump(&mut engine, |engine| {
            engine.jobs[0]
                .conflict
                .as_ref()
                .is_some_and(|conflict| conflict.target == conflict_target.canonicalize().unwrap())
        });
        engine.dispatch(Command::Resolve(engine.jobs[0].id, choice, false));
    }
    pump(&mut engine, |engine| engine.jobs[0].state.finished());
    fs::set_permissions(source.join("readonly"), fs::Permissions::from_mode(0o700)).unwrap();
    let job = &engine.jobs[0];
    assert_eq!(job.state, JobState::Failed);
    assert!(
        job.warnings
            .iter()
            .any(|warning| warning.contains("blocked")),
        "{:?}",
        job.warnings
    );
    assert_eq!(
        fs::read(source.join("readonly/incoming.txt")).unwrap(),
        b"incoming"
    );
    assert_eq!(fs::read(target.join("existing.txt")).unwrap(), b"existing");
    assert!(blocked.symlink_metadata().is_ok());
    assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn rename_existing_preserves_adjusted_relative_links_in_a_filtered_folder_move() {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("left");
    let right = fixture.path().join("right");
    let source = left.join("folder");
    let target = right.join("folder");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(left.join("original"), b"referenced").unwrap();
    fs::write(source.join("keep.bin"), b"excluded").unwrap();
    fs::write(target.join("existing"), b"existing").unwrap();
    symlink("../../original", source.join("nested/link.txt")).unwrap();

    let mut engine = open(&left, &right);
    run(
        &mut engine,
        Operation::Transfer {
            paths: vec![source.clone()],
            destination: right.clone(),
            moving: true,
            options: TransferOptions {
                directory_collision: Collision::RenameExisting,
                adjust_links: true,
                mask: "*.txt".into(),
                ..TransferOptions::default()
            },
        },
    );
    assert!(engine.jobs.last().unwrap().message.contains("folder (1)"));
    assert_eq!(fs::read(source.join("keep.bin")).unwrap(), b"excluded");
    assert!(!source.join("nested").exists());
    assert_eq!(
        fs::read(target.join("nested/link.txt")).unwrap(),
        b"referenced"
    );
    assert_eq!(
        fs::read(right.join("folder (1)/existing")).unwrap(),
        b"existing"
    );
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
    pump(&mut engine, |engine| !engine.tab(side).busy());
    check(engine.tab(side), Some("sub"), 11, 1);
    engine.dispatch(Command::Step(side, Step::Last, false));
    check(engine.tab(side), Some("file2.txt"), 11, 1);
    engine.dispatch(Command::UnmarkAll(side));
    check(engine.tab(side), Some("file2.txt"), 0, 0);
    fs::remove_dir_all(&folder).unwrap();
    engine.dispatch(Command::Refresh);
    pump(&mut engine, |engine| !engine.tab(side).busy());
    assert!(engine.tab(side).error.is_none());
    assert_eq!(engine.tab(side).base(), fixture.path());
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
    pump(&mut engine, |engine| !engine.tab(side).busy());
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
    fs::write(empty.join("fast.txt"), "existing").unwrap();
    let fifo = blocked.join("descript.ion");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);

    let (send, _wake) = mpsc::channel();
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
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    let side = Side::Left;
    let navigate = |path| Command::Navigate {
        side,
        path,
        new_tab: false,
        background: false,
    };
    engine.dispatch(navigate(blocked.clone()));
    pump(&mut engine, |engine| {
        engine.tab(side).pending_navigation().is_none()
    });

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
    pump(&mut engine, |engine| !engine.tab(side).busy());
    assert_eq!(engine.tab(side).current().unwrap().name, "fast.txt");

    engine.dispatch(Command::Queue(
        Operation::Transfer {
            paths: vec![fast.join("fast.txt")],
            destination: root.path().join("empty"),
            moving: false,
            options: Default::default(),
        },
        false,
    ));
    pump(&mut engine, |engine| {
        engine.jobs.last().unwrap().state == JobState::Conflict
    });
    let id = engine.jobs.last().unwrap().id;
    engine.dispatch(Command::Resolve(id, Collision::Skip, false));
    pump(&mut engine, |engine| {
        engine.jobs.last().unwrap().state.finished()
    });
    assert_eq!(
        fs::read(root.path().join("empty/fast.txt")).unwrap(),
        b"existing"
    );

    fs::remove_file(fifo).unwrap();
    fs::write(blocked.join("latest.txt"), "newer visit").unwrap();
    drop(writer);
    engine.dispatch(navigate(blocked.clone()));
    pump(&mut engine, |engine| !engine.tab(side).busy());
    assert_eq!(engine.tab(side).base(), blocked);
    assert_eq!(engine.tab(side).current().unwrap().name, "latest.txt");
}

#[test]
fn workspace_tabs_and_bookmarks_round_trip_through_settings() {
    use deshcmd_engine::{Bookmark, Column, SavedPane, View};

    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let left = root.join("left");
    let right = root.join("right");
    let child = left.join("child");
    fs::create_dir_all(&child).unwrap();
    fs::create_dir(&right).unwrap();
    let mut engine = open(&left, &right);

    let bookmark = Bookmark {
        name: "Pair".into(),
        path: left.clone(),
        opposite: Some(right.clone()),
        group: "Work".into(),
    };
    engine.dispatch(Command::AddBookmark(bookmark.clone()));
    engine.dispatch(Command::ImportBookmarks(vec![bookmark.clone()]));
    let mut edited = bookmark.clone();
    edited.name = "Edited pair".into();
    engine.dispatch(Command::EditBookmark(0, edited.clone()));
    assert_eq!(engine.settings.bookmarks, [edited.clone()]);

    engine.dispatch(Command::Select(Side::Left, child.clone(), Select::Focus));
    engine.dispatch(Command::OpenFocusedDirectoryInTab {
        source: Side::Left,
        destination: Side::Right,
    });
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(Side::Right).base(), child);
    engine.dispatch(Command::RenameTab(Side::Right, 1, "  Work tab  ".into()));
    engine.dispatch(Command::View(Side::Right, View::Gallery));
    engine.dispatch(Command::Sort(Side::Right, Column::Size, false));
    engine.dispatch(Command::Sort(Side::Right, Column::Extension, true));
    engine.dispatch(Command::ReorderTab(Side::Right, 1, 0));
    assert_eq!(engine.tab(Side::Right).title(), "Work tab");
    assert_eq!(engine.pane(Side::Right).active, 0);
    engine.dispatch(Command::SendTab(Side::Right, 0, true));
    assert_eq!(engine.pane(Side::Right).tabs.len(), 1);
    assert_eq!(engine.tab(Side::Left).base(), child);
    engine.dispatch(Command::ToggleTabPin(Side::Left, 1));
    engine.dispatch(Command::CloseTab(Side::Left, 1));
    assert_eq!(engine.pane(Side::Left).tabs.len(), 2);
    engine.dispatch(Command::Tab(Side::Left, 0));
    engine.dispatch(Command::SaveFavorite("Desk".into()));
    engine.dispatch(Command::SaveFavorite("Desk".into()));
    assert_eq!(engine.settings.favorites.len(), 1);

    engine.dispatch(Command::Swap);
    engine.dispatch(Command::MatchPanes);
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(
        engine.tab(Side::Left).base(),
        engine.tab(Side::Right).base()
    );
    engine.dispatch(Command::Focus(Side::Left));
    engine.dispatch(Command::OpenBookmark(0));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(Side::Left).base(), left);
    assert_eq!(engine.tab(Side::Right).base(), right);

    let saved: Settings =
        serde_json::from_str(&serde_json::to_string(&engine.snapshot()).unwrap()).unwrap();
    let mut restored = Engine::new(root, [left.clone(), right.clone()], saved, || {});
    assert_eq!(restored.settings.bookmarks, [edited]);
    restored.dispatch(Command::LoadFavorite(0, false));
    assert_eq!(restored.tab(Side::Left).base(), left);
    restored.dispatch(Command::LoadFavorite(0, true));
    assert_eq!(restored.pane(Side::Left).tabs.len(), 4);
    assert_eq!(restored.tab(Side::Left).base(), left);
    restored.dispatch(Command::Tab(Side::Left, 1));
    let snapshot = restored.snapshot();
    let tab = &snapshot.panes[0].tabs[1];
    assert!(tab.pinned);
    assert_eq!(tab.view, View::Gallery);
    assert_eq!(
        tab.sort.iter().map(|sort| sort.column).collect::<Vec<_>>(),
        [Column::Size, Column::Extension]
    );
    restored.dispatch(Command::RestoreTabs([
        SavedPane::default(),
        snapshot.panes[1].clone(),
    ]));
    assert_eq!(restored.pane(Side::Left).tabs.len(), 1);
    assert_eq!(restored.tab(Side::Left).base(), child);
    restored.dispatch(Command::RemoveBookmark(0));
    restored.dispatch(Command::RemoveFavorite(0));
    assert!(restored.settings.bookmarks.is_empty());
    assert!(restored.settings.favorites.is_empty());
}

#[test]
fn find_navigation_and_mark_commands_follow_the_visible_list() {
    use deshcmd_engine::{Column, Step};

    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    for (name, contents) in [
        ("alpha.txt", "abc"),
        ("alpha.md", "ab"),
        ("beta.txt", "12345"),
        (".hidden", "hidden"),
    ] {
        fs::write(root.join(name), contents).unwrap();
    }
    let mut engine = open(&root, &root);
    let side = Side::Left;
    let alpha = root.join("alpha.txt");
    let beta = root.join("beta.txt");
    let marked = |engine: &Engine| {
        engine
            .tab(side)
            .marked_entries()
            .map(|entry| entry.name.clone())
            .collect::<Vec<_>>()
    };

    engine.dispatch(Command::Find(side, MatchOptions::filename("*.txt")));
    assert_eq!(engine.tab(side).current().unwrap().path, alpha);
    engine.dispatch(Command::Find(side, MatchOptions::filename("*.txt")));
    assert_eq!(engine.tab(side).current().unwrap().path, beta);
    engine.dispatch(Command::Find(side, MatchOptions::filename("*.txt")));
    assert_eq!(engine.tab(side).current().unwrap().path, alpha);
    engine.dispatch(Command::Find(side, MatchOptions::filename("no match")));
    assert_eq!(engine.notice.as_deref(), Some("No names match"));

    engine.dispatch(Command::MarkLike(side, Column::Extension, true));
    assert_eq!(marked(&engine), ["alpha.txt", "beta.txt"]);
    assert!(engine.tab(side).single_operand().is_none());
    engine.dispatch(Command::RememberMarks(side));
    engine.dispatch(Command::Invert(side));
    assert_eq!(engine.tab(side).single_operand().unwrap().name, "alpha.md");
    engine.dispatch(Command::RestoreMarks(side));
    assert_eq!(engine.tab(side).selected_bytes(), 8);
    engine.dispatch(Command::UnmarkAll(side));
    engine.dispatch(Command::MarkLike(side, Column::Name, true));
    assert_eq!(marked(&engine), ["alpha.md", "alpha.txt"]);
    engine.dispatch(Command::MarkLike(side, Column::Path, false));
    assert_eq!(engine.tab(side).marked_count(), 0);
    engine.dispatch(Command::LoadMarks(
        side,
        format!("\u{feff}{}\nalpha.md\nabsent\n", beta.display()),
    ));
    assert_eq!(marked(&engine), ["alpha.md", "beta.txt"]);

    for (step, expected) in [
        (Step::First, "alpha.md"),
        (Step::Next, "alpha.txt"),
        (Step::Last, "beta.txt"),
        (Step::Previous, "alpha.txt"),
        (Step::PageUp, "alpha.md"),
        (Step::PageDown, "beta.txt"),
    ] {
        engine.dispatch(Command::Step(side, step, false));
        assert_eq!(engine.tab(side).current().unwrap().name, expected);
    }
    engine.dispatch(Command::Step(side, Step::First, true));
    assert_eq!(engine.tab(side).marked_count(), 3);
    engine.dispatch(Command::ToggleHidden);
    assert_eq!(engine.tab(side).visible_count(), 4);
    engine.dispatch(Command::Ignore("*.md".into()));
    assert_eq!(engine.tab(side).visible_count(), 3);
    engine.dispatch(Command::ToggleHidden);
    engine.dispatch(Command::MarkAll(side));
    assert_eq!(marked(&engine), ["alpha.txt", "beta.txt"]);
}

#[test]
fn history_skips_missing_locations_and_reset_tabs_return_home() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let past = root.join("past");
    let current = root.join("current");
    fs::create_dir(&past).unwrap();
    fs::create_dir(&current).unwrap();
    let mut engine = open(&root, &root);
    let side = Side::Left;
    for path in [&past, &current] {
        engine.dispatch(Command::Navigate {
            side,
            path: path.clone(),
            new_tab: false,
            background: false,
        });
        pump(&mut engine, |engine| {
            Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
        });
        assert_eq!(engine.tab(side).base(), path);
    }
    fs::remove_dir(&past).unwrap();
    engine.dispatch(Command::History(side, 1));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(
        engine.notice.as_deref(),
        Some("This history location is unavailable")
    );
    engine.dispatch(Command::Back(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(side).base(), root);
    assert!(engine.tab(side).can_forward());
    engine.dispatch(Command::Forward(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(side).base(), current);
    assert_eq!(engine.tab(side).history().count(), 3);

    engine.dispatch(Command::LockTab(side, 0, TabLock::Reset));
    engine.dispatch(Command::Parent(side));
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(side).base(), root);
    engine.dispatch(Command::NewTab(side));
    engine.dispatch(Command::Tab(side, 0));
    assert_eq!(engine.tab(side).base(), current);
    engine.dispatch(Command::CloseTab(side, 0));
    assert_eq!(engine.pane(side).tabs.len(), 1);
}

#[test]
fn queued_jobs_can_be_held_reordered_cancelled_and_cleared() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let mut engine = open(root, root);
    for name in ["cancelled", "second", "first"] {
        engine.dispatch(Command::Queue(Operation::Create(root.join(name)), true));
    }
    let [cancelled, second, first] = std::array::from_fn(|index| engine.jobs[index].id);
    assert_eq!(engine.jobs[1].state, JobState::Held);
    assert_eq!(engine.jobs[1].state.label(), "On hold");
    engine.dispatch(Command::CancelJob(cancelled));
    assert_eq!(engine.jobs[0].state, JobState::Cancelled);
    engine.dispatch(Command::PrioritizeJob(first));
    assert_eq!(engine.jobs[0].id, first);
    for id in [first, second] {
        engine.dispatch(Command::ResumeJob(id));
    }
    pump(&mut engine, |engine| {
        engine.jobs.iter().all(|job| job.state.finished())
    });
    assert!(!root.join("cancelled").exists());
    assert!(root.join("first").is_file());
    assert!(root.join("second").is_file());
    assert!(engine.jobs[0].started <= engine.jobs[2].started);
    engine.dispatch(Command::ClearJobs);
    assert!(engine.jobs.is_empty());
}

#[test]
fn filesystem_helpers_sort_folders_and_validate_path_lists() {
    use deshcmd_engine::{lines, local_properties, numbered_parts, read_list, space, tree_folders};

    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let first = root.join("report.001");
    let second = root.join("report.002");
    let list = root.join("list.txt");
    for path in [
        &second,
        &first,
        &root.join("report.0002"),
        &root.join("other.003"),
    ] {
        fs::write(path, "part").unwrap();
    }
    fs::create_dir(root.join("Beta")).unwrap();
    fs::create_dir(root.join("alpha")).unwrap();
    assert_eq!(
        tree_folders(root).unwrap(),
        [root.join("alpha"), root.join("Beta")]
    );
    assert_eq!(numbered_parts(&second), [first.clone(), second]);
    assert_eq!(numbered_parts(&list), std::slice::from_ref(&list));
    assert_eq!(
        numbered_parts(&root.join("absent.001")),
        [root.join("absent.001")]
    );
    fs::write(&list, "\u{feff}report.001\r\n\n  \nreport.002\n").unwrap();
    assert_eq!(
        lines(&read_list(&list).unwrap()).collect::<Vec<_>>(),
        ["report.001", "report.002"]
    );
    fs::File::options()
        .write(true)
        .open(&list)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    assert!(read_list(&list).unwrap_err().to_string().contains("16 MiB"));
    let (available, total) = space(root).unwrap();
    assert!(total >= available && total > 0);
    assert!(local_properties(&root.join("missing")).is_none());
    #[cfg(unix)]
    {
        let link = root.join("link");
        std::os::unix::fs::symlink("report.001", &link).unwrap();
        assert_eq!(
            local_properties(&link).as_deref(),
            Some("Link target  report.001")
        );
        assert!(local_properties(&first).unwrap().contains("Links  1"));
    }
}

#[test]
fn large_text_preview_rejects_invalid_offsets_and_non_files() {
    use deshcmd_engine::TextPreviewReader;

    let fixture = tempfile::tempdir().unwrap();
    let file = fixture.path().join("empty.log");
    fs::write(&file, "").unwrap();
    let mut reader = TextPreviewReader::open(&file, 0).unwrap();
    assert!(reader.read_window(0).unwrap().rows.is_empty());
    assert!(
        matches!(reader.read_window(1), Err(error) if error.kind() == std::io::ErrorKind::InvalidInput)
    );
    assert!(
        matches!(TextPreviewReader::open(fixture.path(), 0), Err(error) if error.to_string().contains("regular file"))
    );
}

#[test]
fn transfer_conflict_can_be_compared_then_cancelled_after_resuming() {
    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("left");
    let right = fixture.path().join("right");
    fs::create_dir(&left).unwrap();
    fs::create_dir(&right).unwrap();
    for folder in [&left, &right] {
        fs::write(folder.join("file"), "identical").unwrap();
    }
    let mut engine = open(&left, &right);
    engine.dispatch(Command::Queue(
        Operation::Transfer {
            paths: vec![left.join("file")],
            destination: right.clone(),
            moving: true,
            options: TransferOptions::default(),
        },
        false,
    ));
    let id = engine.jobs[0].id;
    engine.dispatch(Command::PauseJob(id));
    assert_eq!(engine.jobs[0].state, JobState::Paused);
    engine.dispatch(Command::Queue(
        Operation::Create(right.join("later")),
        false,
    ));
    let later = engine.jobs[1].id;
    engine.dispatch(Command::PauseJob(later));
    assert_eq!(engine.jobs[1].state, JobState::Held);
    engine.dispatch(Command::ResumeJob(id));
    pump(&mut engine, |engine| {
        engine.jobs[0].state == JobState::Conflict
    });
    engine.dispatch(Command::CompareConflict(id));
    pump(&mut engine, |engine| {
        engine.jobs[0].message == "Files are identical"
    });
    engine.dispatch(Command::Resolve(id, Collision::Cancel, false));
    pump(&mut engine, |engine| engine.jobs[0].state.finished());
    assert_eq!(engine.jobs[0].state, JobState::Cancelled);
    assert_eq!(fs::read(left.join("file")).unwrap(), b"identical");
    assert_eq!(fs::read(right.join("file")).unwrap(), b"identical");
    assert!(!right.join("later").exists());
}

#[test]
fn flat_view_can_be_limited_to_marked_folders_and_restored() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let child = root.join("child");
    fs::create_dir(&child).unwrap();
    fs::write(child.join("nested.txt"), "nested").unwrap();
    fs::write(root.join("top.txt"), "top").unwrap();
    let mut engine = open(&root, &root);
    let side = Side::Left;
    engine.dispatch(Command::Select(side, child.clone(), Select::Toggle));
    engine.dispatch(Command::Flat(side, true));
    pump(&mut engine, |engine| !engine.tab(side).busy());
    assert!(
        matches!(engine.tab(side).location(), Location::Flat { roots, .. } if roots.as_slice() == std::slice::from_ref(&child))
    );
    assert_eq!(engine.tab(side).title(), "Flat view");
    assert_eq!(
        engine
            .tab(side)
            .visible_entries()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["nested.txt"]
    );
    engine.dispatch(Command::Flat(side, false));
    pump(&mut engine, |engine| !engine.tab(side).busy());
    assert!(matches!(engine.tab(side).location(), Location::Directory(base) if base == &root));
    assert_eq!(engine.tab(side).visible_count(), 2);

    engine.dispatch(Command::Navigate {
        side,
        path: "~/child".into(),
        new_tab: false,
        background: false,
    });
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(side).base(), child);
    engine.dispatch(Command::Navigate {
        side,
        path: "~".into(),
        new_tab: false,
        background: false,
    });
    pump(&mut engine, |engine| {
        Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
    });
    assert_eq!(engine.tab(side).base(), root);
}
