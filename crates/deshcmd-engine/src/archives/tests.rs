use super::*;
use crate::{
    Command as Action, Engine, JobState, Location, MatchOptions, Operation, Settings, Side, Step,
};

fn archiver_available() -> bool {
    seven_zip()
        .arg("i")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn navigate(path: PathBuf) -> Action {
    Action::Navigate {
        side: Side::Left,
        path,
        new_tab: false,
        background: false,
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().unwrap().permissions().mode() & 0o7777
}

fn wait(engine: &mut Engine, ready: impl Fn(&Engine) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !ready(engine) {
        engine.poll();
        assert!(
            std::time::Instant::now() < deadline,
            "archive operation timed out"
        );
        std::thread::sleep(Duration::from_millis(8));
    }
}

fn job(engine: &mut Engine, operation: ArchiveOperation) {
    engine.dispatch(Action::Queue(Operation::Archive(operation), false));
    wait(engine, |engine| {
        engine.jobs.last().unwrap().state.finished()
    });
    let job = engine.jobs.last().unwrap();
    assert_eq!(
        job.state,
        JobState::Done,
        "{} {:?}",
        job.message,
        job.warnings
    );
}

fn unpack(path: &Path, destination: &Path, options: ExtractOptions) -> ArchiveOperation {
    ArchiveOperation::Extract {
        sources: vec![ArchiveSelection {
            file: path.into(),
            folder: PathBuf::new(),
            members: Vec::new(),
        }],
        destination: destination.into(),
        options,
    }
}

#[test]
fn listing_skips_traversal_links_and_special_files() {
    let text = "Path = ./docs/notes.txt\nSize = 4\nMode = -rw-r--r--\n\nPath = ../outside\nSize = 4\n\nPath = /absolute\nSize = 4\n\nPath = docs/link\nSize = 0\nSymbolic Link = /outside\n\nPath = docs/device\nSize = 0\nMode = crw-r--r--\n";
    let inventory = read_listing(text).unwrap();
    assert_eq!(inventory.skipped, 4);
    assert_eq!(inventory.members.len(), 1);
    assert_eq!(inventory.members[0].path, Path::new("docs/notes.txt"));
    assert_eq!(inventory.members[0].key, "./docs/notes.txt");
    assert_eq!(access_mode("-rws------"), Some(0o700));
    assert_eq!(access_mode("-rw-r-Sr-T"), Some(0o644));
}

#[test]
fn archive_queue_round_trip() {
    if !archiver_available() {
        eprintln!("Archive integration test requires 7-Zip");
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let source = root.join("source");
    let destination = root.join("extracted");
    fs::create_dir_all(source.join("docs/empty")).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(source.join("docs/notes.txt"), b"hello").unwrap();
    fs::write(source.join("other.md"), b"world").unwrap();
    fs::write(source.join("extra.dat"), b"added").unwrap();
    #[cfg(unix)]
    {
        set_mode(&source.join("docs/notes.txt"), 0o700);
        set_mode(&source.join("other.md"), 0o600);
    }
    let archive = root.join("sample.zip");
    let mut engine = Engine::new(
        root.clone(),
        [source.clone(), destination.clone()],
        Settings::default(),
        || {},
    );
    job(
        &mut engine,
        ArchiveOperation::Pack {
            paths: vec![source.join("docs"), source.join("other.md")],
            output: archive.clone(),
            folder: PathBuf::new(),
            options: PackOptions::default(),
        },
    );
    job(
        &mut engine,
        ArchiveOperation::Test {
            paths: vec![archive.clone()],
            password: String::new(),
        },
    );
    #[cfg(unix)]
    set_mode(&archive, 0o600);
    let batch = destination.join("batch");
    fs::create_dir(&batch).unwrap();
    job(
        &mut engine,
        unpack(&archive, &batch, ExtractOptions::default()),
    );
    assert_eq!(fs::read(batch.join("docs/notes.txt")).unwrap(), b"hello");
    assert_eq!(fs::read(batch.join("other.md")).unwrap(), b"world");
    #[cfg(unix)]
    {
        assert_eq!(mode(&batch.join("docs/notes.txt")), 0o700);
        assert_eq!(mode(&batch.join("other.md")), 0o600);
    }
    let side = Side::Left;
    engine.dispatch(navigate(archive.clone()));
    wait(&mut engine, |engine| !engine.tab(side).loading);
    assert_eq!(
        engine.tab(side).visible_count(),
        2,
        "{:?}",
        engine.tab(side).error
    );
    engine.dispatch(navigate(archive.join("docs")));
    wait(&mut engine, |engine| !engine.tab(side).loading);
    assert_eq!(
        engine.tab(side).visible_count(),
        2,
        "{:?}",
        engine.tab(side).error
    );
    assert!(
        matches!(preview_member(&archive, Path::new("docs/notes.txt"), Kind::Text, 64), Preview::Text(text) if text == "hello")
    );
    engine.dispatch(Action::Parent(side));
    wait(&mut engine, |engine| !engine.tab(side).loading);
    assert_eq!(
        engine.tab(side).location(),
        &Location::Archive {
            file: archive.clone(),
            folder: PathBuf::new()
        }
    );

    job(
        &mut engine,
        ArchiveOperation::Pack {
            paths: vec![source.join("extra.dat")],
            output: archive.clone(),
            folder: PathBuf::from("docs"),
            options: PackOptions {
                update: true,
                ..Default::default()
            },
        },
    );
    #[cfg(unix)]
    assert_eq!(mode(&archive), 0o600);
    let inventory = list(&archive, "", &Control::default()).unwrap();
    assert!(
        inventory
            .members
            .iter()
            .any(|member| member.path == Path::new("docs/extra.dat"))
    );
    job(
        &mut engine,
        unpack(
            &archive,
            &destination,
            ExtractOptions {
                mask: "*.txt".into(),
                flatten: true,
                layout: ExtractLayout::Separate,
                ..Default::default()
            },
        ),
    );
    assert_eq!(
        fs::read(destination.join("sample/notes.txt")).unwrap(),
        b"hello"
    );
    assert!(!destination.join("sample/docs").exists());
    #[cfg(unix)]
    assert_eq!(mode(&destination.join("sample/notes.txt")), 0o700);

    job(
        &mut engine,
        ArchiveOperation::Delete(ArchiveSelection {
            file: archive.clone(),
            folder: PathBuf::new(),
            members: vec![PathBuf::from("other.md")],
        }),
    );
    #[cfg(unix)]
    assert_eq!(mode(&archive), 0o600);
    wait(&mut engine, |engine| !engine.tab(side).loading);
    engine.dispatch(Action::Step(side, Step::First, false));
    assert_eq!(
        engine.tab(side).visible_count(),
        1,
        "{:?}",
        engine.tab(side).error
    );
    assert_eq!(engine.tab(side).current().unwrap().name, "docs");

    let tarball = root.join("sample.tar.gz");
    job(
        &mut engine,
        ArchiveOperation::Pack {
            paths: vec![source.join("docs"), source.join("extra.dat")],
            output: tarball.clone(),
            folder: PathBuf::new(),
            options: PackOptions::default(),
        },
    );
    job(
        &mut engine,
        unpack(&tarball, &destination, ExtractOptions::default()),
    );
    assert_eq!(
        fs::read(destination.join("docs/notes.txt")).unwrap(),
        b"hello"
    );
    assert!(destination.join("docs/empty").is_dir());
    assert_eq!(fs::read(destination.join("extra.dat")).unwrap(), b"added");
    let cache = ArchiveCache::default();
    let top = scan(&cache, &tarball, Path::new(""), false).unwrap();
    let nested = scan(&cache, &tarball, Path::new("docs"), false).unwrap();
    let top_source = top.entries[0].archive.as_ref().unwrap();
    let nested_source = nested.entries[0].archive.as_ref().unwrap();
    assert!(Arc::ptr_eq(top_source, nested_source));
    assert!(Arc::ptr_eq(
        &top_source.inventory(&Control::default()).unwrap(),
        &nested_source.inventory(&Control::default()).unwrap(),
    ));

    let stream = root.join("notes.txt.xz");
    job(
        &mut engine,
        ArchiveOperation::Pack {
            paths: vec![source.join("docs/notes.txt")],
            output: stream.clone(),
            folder: PathBuf::new(),
            options: PackOptions::default(),
        },
    );
    job(
        &mut engine,
        unpack(&stream, &destination, ExtractOptions::default()),
    );
    assert_eq!(fs::read(destination.join("notes.txt")).unwrap(), b"hello");

    let encrypted = root.join("secret.7z");
    let password = "pass with spaces";
    job(
        &mut engine,
        ArchiveOperation::Pack {
            paths: vec![source.join("other.md")],
            output: encrypted,
            folder: PathBuf::new(),
            options: PackOptions {
                password: password.into(),
                volume: "1m".into(),
                ..Default::default()
            },
        },
    );
    let first_volume = root.join("secret.7z.001");
    job(
        &mut engine,
        ArchiveOperation::Test {
            paths: vec![first_volume.clone()],
            password: password.into(),
        },
    );
    job(
        &mut engine,
        unpack(
            &first_volume,
            &destination,
            ExtractOptions {
                password: password.into(),
                ..Default::default()
            },
        ),
    );
    assert_eq!(fs::read(destination.join("other.md")).unwrap(), b"world");

    if Command::new("ar")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        fs::copy(&tarball, root.join("data.tar.gz")).unwrap();
        fs::write(root.join("debian-binary"), b"2.0\n").unwrap();
        let package = root.join("example.deb");
        assert!(
            Command::new("ar")
                .current_dir(&root)
                .args(["rc", "example.deb", "debian-binary", "data.tar.gz"])
                .status()
                .unwrap()
                .success()
        );
        job(
            &mut engine,
            unpack(
                &package,
                &destination,
                ExtractOptions {
                    layout: ExtractLayout::Separate,
                    ..Default::default()
                },
            ),
        );
        assert_eq!(
            fs::read(destination.join("example/docs/notes.txt")).unwrap(),
            b"hello"
        );
    }
}

#[test]
fn unlock_rescans_encrypted_headers_and_previews_members() {
    if !archiver_available() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let file = root.join("encrypted.7z");
    let password = "archive fixture password";
    fs::write(root.join("member.txt"), "encrypted text").unwrap();
    let result = seven("a", password)
        .current_dir(&root)
        .arg("-mhe=on")
        .arg(&file)
        .arg("member.txt")
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root],
        Settings::default(),
        || {},
    );
    let side = Side::Left;
    engine.dispatch(navigate(file.clone()));
    wait(&mut engine, |engine| !engine.tab(side).loading);
    assert!(
        engine
            .tab(side)
            .error
            .as_ref()
            .unwrap()
            .contains("password")
    );
    assert_eq!(engine.tab(side).visible_count(), 0);

    engine.dispatch(Action::UnlockArchive {
        side,
        file: file.clone(),
        password: password.into(),
    });
    wait(&mut engine, |engine| !engine.tab(side).loading);
    let entry = engine.tab(side).current().unwrap();
    let source = entry.archive.as_ref().unwrap();
    assert!(
        matches!(source.preview(Path::new("member.txt"), Kind::Text, 64), Preview::Text(text) if text == "encrypted text")
    );
    assert!(
        !serde_json::to_string(&engine.snapshot())
            .unwrap()
            .contains(password)
    );

    engine.dispatch(Action::UnlockArchive {
        side,
        file,
        password: "wrong password".into(),
    });
    wait(&mut engine, |engine| !engine.tab(side).loading);
    assert!(
        engine
            .tab(side)
            .error
            .as_ref()
            .unwrap()
            .contains("password")
    );
}

#[cfg(unix)]
#[test]
fn archive_alias_and_recursive_ancestor_filtering() {
    if !archiver_available() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let hidden = ".hidden/hidden.txt";
    let ignored = "ignored/ignored.txt";
    let shown = "visible/shown.txt";
    for entry in [hidden, ignored, shown] {
        let path = root.join(entry);
        fs::create_dir(path.parent().unwrap()).unwrap();
        fs::write(path, entry).unwrap();
    }
    let archive = root.join("extensionless");
    let result = seven("a", "")
        .arg("-tzip")
        .arg("-sae")
        .current_dir(&root)
        .arg(&archive)
        .args([hidden, ignored, shown])
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let alias = root.join("alias.zip");
    std::os::unix::fs::symlink(&archive, &alias).unwrap();
    assert!(
        list(&alias, "", &Control::default())
            .unwrap()
            .members
            .iter()
            .all(|member| !member.folder)
    );

    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root],
        Settings::default(),
        || {},
    );
    let side = Side::Left;
    engine.dispatch(navigate(alias.join("visible")));
    wait(&mut engine, |engine| !engine.tab(side).loading);
    assert_eq!(engine.tab(side).current().unwrap().name, "shown.txt");
    engine.dispatch(Action::Parent(side));
    wait(&mut engine, |engine| !engine.tab(side).loading);
    assert!(
        matches!(engine.tab(side).location(), Location::Archive { file, folder } if file == &alias && folder.as_os_str().is_empty())
    );
    engine.set_ignore("ignored".into()).unwrap();
    engine.dispatch(Action::Filter(
        side,
        MatchOptions {
            recursive: true,
            ..MatchOptions::filename("*.txt")
        },
    ));
    wait(&mut engine, |engine| !engine.tab(side).loading);
    let names = engine
        .tab(side)
        .visible_entries()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["shown.txt"]);
}
