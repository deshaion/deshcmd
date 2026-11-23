use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use deshcmd_engine::{
    Algorithm, Collision, Command, Engine, JobState, ManifestMode, MatchOptions, Operation, Select,
    Settings, Side, TabLock, TransferOptions,
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
    assert!(engine.tab(side).selected.is_empty());
    engine.dispatch(Command::Filter(side, MatchOptions::filename("")));
    engine.dispatch(Command::RestoreMarks(side));
    assert!(engine.tab(side).selected.contains(&root.join("café.txt")));

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
