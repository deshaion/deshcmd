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
fn extraction_diagnostics_distinguish_password_errors_from_member_names() {
    let file = Path::new("folder : with spaces/archive.zip");
    for name in [
        "wrong password.txt",
        "incorrect password",
        "Cannot open encrypted archive. Wrong password?",
    ] {
        let original = format!("ERROR: CRC Failed : {name}");
        for password in ["", "supplied"] {
            let explained =
                explain_extract_error(io::Error::other(original.clone()), file, password);
            assert_eq!(explained.to_string(), original);
        }
    }
    let header = format!(
        "ERROR: {} : Cannot open encrypted archive. Wrong password?",
        file.display()
    );
    assert!(password_diagnostic(&header, file));
    assert!(password_diagnostic(
        "ERROR: Wrong password : docs/note.txt",
        file
    ));
    assert!(password_diagnostic(
        "Enter password (will not be echoed):",
        file
    ));
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
    wait(&mut engine, |engine| !engine.tab(side).busy());
    assert_eq!(
        engine.tab(side).visible_count(),
        2,
        "{:?}",
        engine.tab(side).error
    );
    engine.dispatch(navigate(archive.join("docs")));
    wait(&mut engine, |engine| !engine.tab(side).busy());
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
    wait(&mut engine, |engine| !engine.tab(side).busy());
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
    wait(&mut engine, |engine| !engine.tab(side).busy());
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
    let top = scan(&cache, &tarball, Path::new(""), false, &Default::default()).unwrap();
    let nested = scan(
        &cache,
        &tarball,
        Path::new("docs"),
        false,
        &Default::default(),
    )
    .unwrap();
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
    wait(&mut engine, |engine| !engine.tab(side).busy());
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
    wait(&mut engine, |engine| !engine.tab(side).busy());
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
    wait(&mut engine, |engine| !engine.tab(side).busy());
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
    wait(&mut engine, |engine| !engine.tab(side).busy());
    assert_eq!(engine.tab(side).current().unwrap().name, "shown.txt");
    engine.dispatch(Action::Parent(side));
    wait(&mut engine, |engine| !engine.tab(side).busy());
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
    wait(&mut engine, |engine| !engine.tab(side).busy());
    let names = engine
        .tab(side)
        .visible_entries()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["shown.txt"]);
}

#[test]
fn archive_listing_formats_reject_invalid_records_and_preserve_metadata() {
    let records = read_rar_listing(
        "        Name: folder\n        Type: Directory\n Attributes: drwx------\n\n        Name: folder/file.txt\n        Type: File\n        Size: 3\n Attributes: -rw-r-----\n CRC32: 352441c2\n\n        Name: ../outside\n        Type: File\n"
    ).unwrap();
    assert_eq!(records.skipped, 1);
    assert_eq!(records.members.len(), 2);
    assert!(records.members[0].folder);
    let file = &records.members[1];
    assert_eq!(file.path, Path::new("folder/file.txt"));
    assert_eq!(file.size, Some(3));
    assert_eq!(file.mode, Some(0o640));
    assert_eq!(file.crc, Some(0x352441c2));
    assert!(read_rar_listing("        Name: a\n        Name: b\n").is_err());

    let records = read_json_listing(
        r#"[
        {"path":"dir","folder":true},
        {"path":"dir/file","size":3,"mode":2496,"encrypted":true},
        {"path":"./dir/link","link":true},
        {"path":"C:\\absolute"}
    ]"#,
    )
    .unwrap();
    assert_eq!(records.skipped, 2);
    assert!(records.members[0].folder);
    assert_eq!(records.members[1].size, Some(3));
    assert_eq!(records.members[1].mode, Some(0o700));
    assert!(records.members[1].encrypted);
    assert!(read_json_listing(r#"[{"size":3}]"#).is_err());

    for (listing, message) in [
        ("Path = first\nPath = second", "Ambiguous"),
        ("wrong format", "Malformed"),
        ("Size = 3", "no path"),
    ] {
        assert!(matches!(read_listing(listing), Err(error) if error.to_string().contains(message)));
    }
    for name in ["CON", "conin$.txt", "COM1.foo", "LPT² ", "NUL.txt"] {
        assert!(windows_device(name), "{name}");
    }
    for name in ["COM0", "COM10", "COMPUTER", "ordinary.txt"] {
        assert!(!windows_device(name), "{name}");
    }
    assert!(member_path("dir/file\nname").is_none());
    assert_eq!(access_mode("-zzzzzzzzz"), None);
}

#[test]
fn duplicate_archive_members_are_rejected_before_extraction() {
    let inventory =
        read_listing("Path = folder/entry\nSize = 3\n\nPath = folder\\entry\nSize = 4\n").unwrap();
    let selection = ArchiveSelection {
        file: "archive.zip".into(),
        folder: "folder".into(),
        members: vec!["folder/entry".into()],
    };
    assert!(
        matches!(select(inventory, &selection, None, false), Err(error) if error.to_string().contains("Duplicate archive member"))
    );
}

#[cfg(unix)]
#[test]
fn external_archiver_configuration_round_trips_in_an_isolated_process() {
    const FIXTURE: &str = "DESHCMD_TEST_EXTERNAL_ARCHIVER_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE).map(PathBuf::from) {
        let archive = root.join("archive.fixture");
        let input = root.join("payload.txt");
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        fs::write(&input, "abc").unwrap();
        let mut engine = Engine::new(
            root.clone(),
            [root.clone(), destination.clone()],
            Settings::default(),
            || {},
        );
        assert!(is_archive(&archive));
        job(
            &mut engine,
            ArchiveOperation::Pack {
                paths: vec![input],
                output: archive.clone(),
                folder: PathBuf::new(),
                options: PackOptions {
                    password: "secret with spaces".into(),
                    ..Default::default()
                },
            },
        );
        assert!(
            matches!(preview_member(&archive, Path::new("payload.txt"), Kind::Text, 16), Preview::Text(text) if text == "abc")
        );
        job(
            &mut engine,
            unpack(&archive, &destination, ExtractOptions::default()),
        );
        assert_eq!(fs::read(destination.join("payload.txt")).unwrap(), b"abc");

        let cache = ArchiveCache::default();
        let changing = root.join("changing.fixture");
        fs::write(&changing, "abc").unwrap();
        let old = cache.source(&changing).unwrap();
        fs::write(&changing, "ab").unwrap();
        assert!(
            matches!(old.inventory(&Control::default()), Err(error) if error.to_string().contains("changed while it was being listed"))
        );
        let new = cache.source(&changing).unwrap();
        assert!(
            matches!(new.preview_result(Path::new("payload.txt"), Kind::Text, 16), Err(error) if error.to_string().contains("Unexpected size"))
        );
        fs::write(&changing, "abc").unwrap();
        assert!(
            matches!(new.preview_result(Path::new("payload.txt"), Kind::Text, 16), Err(error) if error.to_string().contains("changed while it was being previewed"))
        );

        job(
            &mut engine,
            ArchiveOperation::Test {
                paths: vec![archive.clone()],
                password: "secret with spaces".into(),
            },
        );
        job(
            &mut engine,
            ArchiveOperation::Delete(ArchiveSelection {
                file: archive.clone(),
                folder: PathBuf::new(),
                members: vec!["payload.txt".into()],
            }),
        );
        assert_eq!(fs::read(archive).unwrap(), b"");
        return;
    }

    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let script = root.join("adapter.sh");
    fs::write(
        &script,
        r#"set -eu
case "$1" in
pack)
    test "$4" = 'secret with spaces'
    cat "$3/payload.txt" > "$2"
    ;;
list)
    printf '[{"path":"payload.txt","size":3}]'
    ;;
extract)
    test "$3" = 'payload.txt'
    cat "$2"
    ;;
test)
    test "$3" = 'secret with spaces'
    test -s "$2"
    ;;
delete)
    test "$(cat "$3")" = 'payload.txt'
    : > "$2"
    ;;
esac
"#,
    )
    .unwrap();
    let config = root.join("handlers.json");
    fs::write(&config, serde_json::to_vec(&serde_json::json!([{
        "executable": "/bin/sh",
        "extensions": ["FIXTURE"],
        "listing": "json",
        "commands": {
            "pack": [&script.display().to_string(), "pack", "{archive}", "{input}", "{password}"],
            "list": [&script.display().to_string(), "list", "{archive}"],
            "extract": [&script.display().to_string(), "extract", "{archive}", "{member}"],
            "test": [&script.display().to_string(), "test", "{archive}", "{password}"],
            "delete": [&script.display().to_string(), "delete", "{archive}", "{input}"]
        }
    }])).unwrap()).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "archives::tests::external_archiver_configuration_round_trips_in_an_isolated_process",
        ])
        .env(FIXTURE, &root)
        .env("DESHCMD_ARCHIVERS", config)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
}

#[cfg(unix)]
#[test]
fn archive_child_errors_and_output_limits_are_reported() {
    let fixture = tempfile::tempdir().unwrap();
    let output = tempfile::tempfile().unwrap();
    let control = Control::default();
    let error = run(
        &mut Command::new(fixture.path().join("missing-program")),
        &output,
        &control,
        None,
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(error.to_string().contains("Install it or configure"));
    let shell = |script: &str| {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    };
    assert!(
        run(&mut shell("exit 7"), &output, &control, None)
            .unwrap_err()
            .to_string()
            .contains("Archive tool exited")
    );
    assert_eq!(
        run(
            &mut shell("printf 'broken archive\\n' >&2; exit 1"),
            &output,
            &control,
            None
        )
        .unwrap_err()
        .to_string(),
        "broken archive"
    );
    assert!(
        run(&mut shell("printf 'too much'"), &output, &control, Some(2))
            .unwrap_err()
            .to_string()
            .contains("size limit")
    );
    let mut listing = NamedTempFile::new().unwrap();
    let error = run_listing(
        &mut shell("printf 'Enter password (will not be echoed):'; exit 1"),
        Path::new("archive.zip"),
        &mut listing,
        &control,
        1024,
    )
    .unwrap_err();
    assert!(is_password_error(&error));
}

#[test]
fn archive_stream_and_tarball_formats_round_trip() {
    if !archiver_available() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let file = root.join("sample.txt");
    fs::write(&file, "codec fixture").unwrap();
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root.clone()],
        Settings::default(),
        || {},
    );

    for (suffix, tool, standalone) in [
        ("txt.gz", None, true),
        ("tgz", None, false),
        ("tar.bz2", None, false),
        ("tar.xz", None, false),
        ("txt.lzma", Some("xz"), true),
        ("tar.lzma", Some("xz"), false),
        ("txt.zst", Some("zstd"), true),
        ("tzst", Some("zstd"), false),
        ("txt.b64", Some("base64"), true),
    ] {
        if let Some(tool) = tool
            && !Command::new(tool)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        {
            eprintln!("Skipping {suffix} codec test because {tool} is unavailable");
            continue;
        }
        let archive = root.join(format!("sample.{suffix}"));
        job(
            &mut engine,
            ArchiveOperation::Pack {
                paths: vec![file.clone()],
                output: archive.clone(),
                folder: PathBuf::new(),
                options: PackOptions::default(),
            },
        );
        let output = root.join(format!("extract-{suffix}"));
        fs::create_dir(&output).unwrap();
        let options = ExtractOptions {
            layout: ExtractLayout::Separate,
            ..Default::default()
        };
        job(&mut engine, unpack(&archive, &output, options));
        let folder = if standalone { "sample.txt" } else { "sample" };
        assert_eq!(
            fs::read(output.join(folder).join("sample.txt")).unwrap(),
            b"codec fixture",
            "{suffix}"
        );
        assert!(
            matches!(preview_member(&archive, Path::new("sample.txt"), Kind::Text, 16), Preview::Text(text) if text == "codec fixture"),
            "{suffix}"
        );
        job(
            &mut engine,
            ArchiveOperation::Test {
                paths: vec![archive],
                password: String::new(),
            },
        );
    }
}

#[test]
fn debian_package_reads_raw_and_compressed_payloads() {
    if !archiver_available()
        || !Command::new("ar")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let input = root.join("payload.txt");
    fs::write(&input, b"package member").unwrap();
    fs::write(root.join("debian-binary"), b"2.0\n").unwrap();
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root.clone()],
        Settings::default(),
        || {},
    );
    for suffix in ["data.tar", "data.tar.gz"] {
        job(
            &mut engine,
            ArchiveOperation::Pack {
                paths: vec![input.clone()],
                output: root.join(suffix),
                folder: PathBuf::new(),
                options: PackOptions::default(),
            },
        );
        let package = root.join(format!("{suffix}.deb"));
        let result = Command::new("ar")
            .current_dir(&root)
            .args(["rcs"])
            .arg(&package)
            .args(["debian-binary", suffix])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            matches!(preview_member(&package, Path::new("payload.txt"), Kind::Text, 16), Preview::Text(text) if text == "package member")
        );
    }
    let missing_payload = root.join("incomplete.deb");
    assert!(
        Command::new("ar")
            .current_dir(&root)
            .args(["rcs"])
            .arg(&missing_payload)
            .arg("debian-binary")
            .status()
            .unwrap()
            .success()
    );
    assert!(
        matches!(Reader::open(&missing_payload, "", &Control::default()), Err(error) if error.to_string().contains("payload is missing"))
    );
}

#[test]
fn archive_preflight_rejects_invalid_selections_and_preserves_existing_files() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let input = root.join("input");
    let archive = root.join("archive.zip");
    let stream = root.join("existing.gz");
    fs::write(&input, "abc").unwrap();
    fs::write(&archive, "existing archive").unwrap();
    fs::write(&stream, "existing stream").unwrap();
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root.clone()],
        Settings::default(),
        || {},
    );
    let pack = |paths, output, folder, options| ArchiveOperation::Pack {
        paths,
        output,
        folder,
        options,
    };
    let fail = |engine: &mut Engine, operation, expected: &str| {
        engine.dispatch(Action::Queue(Operation::Archive(operation), false));
        wait(engine, |engine| {
            engine.jobs.last().unwrap().state.finished()
        });
        let job = engine.jobs.last().unwrap();
        assert_eq!(job.state, JobState::Failed, "{}", job.message);
        assert!(job.message.contains(expected), "{}", job.message);
    };
    let output = root.join("new.zip");
    let update = || PackOptions {
        update: true,
        ..Default::default()
    };
    fail(
        &mut engine,
        pack(vec![], output.clone(), "".into(), PackOptions::default()),
        "Choose files",
    );
    fail(
        &mut engine,
        pack(
            vec![input.clone()],
            archive.clone(),
            "".into(),
            PackOptions::default(),
        ),
        "already exists",
    );
    fail(
        &mut engine,
        pack(vec![input.clone()], stream.clone(), "".into(), update()),
        "does not support updates",
    );
    fail(
        &mut engine,
        pack(
            vec![input.clone()],
            output.clone(),
            "".into(),
            PackOptions {
                volume: "1m".into(),
                ..update()
            },
        ),
        "new archive",
    );
    fail(
        &mut engine,
        pack(
            vec![input.clone()],
            output.clone(),
            "folder".into(),
            update(),
        ),
        "no longer exists",
    );
    fail(
        &mut engine,
        pack(
            vec![input.clone()],
            archive.clone(),
            "../folder".into(),
            update(),
        ),
        "Invalid archive folder",
    );
    fail(
        &mut engine,
        pack(
            vec![input.clone(), input.clone()],
            output.clone(),
            "".into(),
            PackOptions::default(),
        ),
        "duplicate file names",
    );
    fail(
        &mut engine,
        pack(
            vec![root.clone()],
            output.clone(),
            "".into(),
            PackOptions::default(),
        ),
        "cannot include itself",
    );
    fail(
        &mut engine,
        pack(
            vec![input.clone()],
            root.join("unknown.foo"),
            "".into(),
            PackOptions::default(),
        ),
        "does not support packing",
    );

    let select_from = |file: PathBuf, members| {
        ArchiveOperation::Delete(ArchiveSelection {
            file,
            folder: PathBuf::new(),
            members,
        })
    };
    fail(
        &mut engine,
        select_from(archive.clone(), vec![]),
        "Choose members",
    );
    fail(
        &mut engine,
        select_from(stream.clone(), vec!["input".into()]),
        "cannot delete members",
    );
    fail(
        &mut engine,
        unpack(&archive, &root.join("absent"), ExtractOptions::default()),
        "existing destination folder",
    );
    assert_eq!(fs::read(archive).unwrap(), b"existing archive");
    assert_eq!(fs::read(stream).unwrap(), b"existing stream");
    assert!(!output.exists());
}

#[test]
fn archive_commit_detects_a_changed_destination_and_batch_checks_member_crc() {
    let fixture = tempfile::tempdir().unwrap();
    let target = fixture.path().join("archive.zip");
    let staged = fixture.path().join("staged");
    fs::write(&target, b"original").unwrap();
    fs::write(&staged, b"incoming").unwrap();
    let original = existing_archive(&target, true).unwrap();
    fs::write(&target, b"changed destination").unwrap();
    let error = publish(&staged, &target, original, &Control::default()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("changed while this task was running")
    );
    assert_eq!(fs::read(&target).unwrap(), b"changed destination");

    let mut batch = NamedTempFile::new().unwrap();
    batch.write_all(b"bad").unwrap();
    batch.rewind().unwrap();
    let mut output = tempfile::tempfile().unwrap();
    let member = read_listing("Path = expected.txt\nSize = 3\nCRC = 352441C2")
        .unwrap()
        .members
        .remove(0);
    let error =
        copy_batch_member(&mut batch, &mut output, &member, &Control::default()).unwrap_err();
    assert_eq!(error.to_string(), "Checksum mismatch for expected.txt");
}

#[test]
fn archive_cache_invalidates_old_revisions_without_exposing_passwords() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let cache = ArchiveCache::default();
    let first_path = root.join("first.zip");
    fs::write(&first_path, "first revision").unwrap();
    let first = cache.source(&first_path).unwrap();
    assert!(Arc::ptr_eq(&first, &cache.source(&first_path).unwrap()));
    fs::write(&first_path, "different revision and size").unwrap();
    let second = cache.source(&first_path).unwrap();
    assert!(second.revision > first.revision);
    cache.set_password(&first_path, "fixture-password".into());
    let unlocked = cache.source(&first_path).unwrap();
    assert!(unlocked.revision > second.revision);
    assert_eq!(
        cache.password(&first_path).as_deref(),
        Some("fixture-password")
    );
    assert!(!format!("{unlocked:?}").contains("fixture-password"));
    for index in 0..8 {
        let path = root.join(format!("entry-{index}.zip"));
        fs::write(&path, "fixture").unwrap();
        cache.source(&path).unwrap();
    }
    let reopened = cache.source(&first_path).unwrap();
    assert!(!Arc::ptr_eq(&reopened, &unlocked));
    assert_eq!(reopened.password.as_ref(), unlocked.password.as_ref());
    cache.set_password(&first_path, String::new());
    assert!(cache.password(&first_path).is_none());
    assert!(cache.source(&first_path).unwrap().password.is_empty());
    assert!(matches!(
        preview_member(
            &root.join("missing.zip"),
            Path::new("entry"),
            Kind::Text,
            16
        ),
        Preview::Unavailable
    ));
}

#[test]
fn archives_preserve_destination_when_separate_folder_name_is_taken() {
    if !archiver_available() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let input = root.join("input.txt");
    let archive = root.join("one.zip");
    let destination = root.join("dest");
    fs::write(&input, "archive payload").unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("one"), "do not overwrite").unwrap();
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), destination.clone()],
        Settings::default(),
        || {},
    );
    job(
        &mut engine,
        ArchiveOperation::Pack {
            paths: vec![input],
            output: archive.clone(),
            folder: PathBuf::new(),
            options: PackOptions::default(),
        },
    );
    engine.dispatch(Action::Queue(
        Operation::Archive(unpack(
            &archive,
            &destination,
            ExtractOptions {
                layout: ExtractLayout::Separate,
                ..Default::default()
            },
        )),
        false,
    ));
    wait(&mut engine, |engine| {
        engine.jobs.last().unwrap().state.finished()
    });
    let result = engine.jobs.last().unwrap();
    assert_eq!(result.state, JobState::Failed);
    assert!(result.message.contains("not a regular folder"));
    assert_eq!(
        fs::read(destination.join("one")).unwrap(),
        b"do not overwrite"
    );
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);

    engine.dispatch(navigate(archive.clone()));
    wait(&mut engine, |engine| !engine.tab(Side::Left).busy());
    assert_eq!(engine.tab(Side::Left).title(), "one.zip");
    assert!(
        matches!(engine.tab(Side::Left).location(), Location::Archive { folder, .. } if folder.as_os_str().is_empty())
    );
    engine.dispatch(Action::NewTab(Side::Left));
    assert_eq!(engine.pane(Side::Left).tabs.len(), 2);
    assert_eq!(engine.tab(Side::Left).title(), "one.zip");
    engine.dispatch(Action::Flat(Side::Left, false));
    assert_eq!(
        engine.notice.as_deref(),
        Some("Flat view needs a local folder")
    );
    engine.dispatch(navigate(archive.join("absent")));
    wait(&mut engine, |engine| !engine.tab(Side::Left).busy());
    assert_eq!(
        engine.tab(Side::Left).error.as_deref(),
        Some("This folder is no longer in the archive")
    );
}

#[cfg(unix)]
#[test]
fn configured_unrar_receives_single_and_batch_member_arguments() {
    const FIXTURE: &str = "DESHCMD_TEST_UNRAR_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE).map(PathBuf::from) {
        let archive = root.join("sample.rar");
        let destination = root.join("out");
        fs::write(&archive, "fixture for the UnRAR command adapter").unwrap();
        fs::create_dir(&destination).unwrap();
        let mut engine = Engine::new(
            root.clone(),
            [root, destination.clone()],
            Settings::default(),
            || {},
        );
        assert!(
            matches!(preview_member(&archive, Path::new("first.txt"), Kind::Text, 16), Preview::Text(text) if text == "abc")
        );
        job(
            &mut engine,
            unpack(&archive, &destination, ExtractOptions::default()),
        );
        assert_eq!(fs::read(destination.join("first.txt")).unwrap(), b"abc");
        assert_eq!(fs::read(destination.join("second.txt")).unwrap(), b"def");
        job(
            &mut engine,
            ArchiveOperation::Test {
                paths: vec![archive],
                password: "fixture password".into(),
            },
        );
        return;
    }

    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    fs::write(root.join("lt"), r#"set -eu
test "$3" = "-p-"
printf '        Name: first.txt\n        Type: File\n        Size: 3\n CRC32: 352441c2\n\n        Name: second.txt\n        Type: File\n        Size: 3\n CRC32: 0cc4e161\n'
"#).unwrap();
    fs::write(
        root.join("p"),
        r#"set -eu
test "$3" = "-p-"
last=
for arg in "$@"; do last=$arg; done
case "$last" in
    first.txt) printf abc ;;
    @*) test "$(cat "${last#@}")" = "$(printf 'first.txt\nsecond.txt')"; printf abcdef ;;
    *) exit 1 ;;
esac
"#,
    )
    .unwrap();
    fs::write(root.join("t"), r#"test "$3" = "-pfixture password""#).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "archives::tests::configured_unrar_receives_single_and_batch_member_arguments",
        ])
        .current_dir(&root)
        .env(FIXTURE, &root)
        .env("DESHCMD_UNRAR", "/bin/sh")
        .env_remove("DESHCMD_ARCHIVERS")
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn archive_child_can_pause_resume_and_cancel() {
    let fixture = tempfile::tempdir().unwrap();
    let marker = fixture.path().join("pid");
    let control = Control::default();
    let output = tempfile::tempfile().unwrap();
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", r#"printf '%s\n' "$$" > "$1"; exec sleep 10"#, "test"])
        .arg(&marker);

    fn poll_until<T>(mut check: impl FnMut() -> Option<T>) -> Option<T> {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = check() {
                return Some(result);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    std::thread::scope(|scope| {
        let child = scope.spawn(|| run(&mut command, &output, &control, None));
        let pid = poll_until(|| fs::read_to_string(&marker).ok()?.trim().parse::<u32>().ok());
        let states = pid.map(|pid| {
            let status = format!("/proc/{pid}/status");
            let is_stopped = || {
                let content = fs::read_to_string(&status).ok()?;
                Some(
                    content
                        .lines()
                        .find(|line| line.starts_with("State:"))?
                        .split_whitespace()
                        .nth(1)?
                        == "T",
                )
            };
            control.pause(true);
            let stopped = poll_until(|| is_stopped().filter(|stopped| *stopped)).is_some();
            control.pause(false);
            let resumed = poll_until(|| is_stopped().filter(|stopped| !*stopped)).is_some();
            (stopped, resumed)
        });
        control.cancel();
        let error = child.join().unwrap().unwrap_err();
        assert_eq!(states, Some((true, true)));
        assert_eq!(error.to_string(), "Cancelled");
    });
}

#[test]
fn multi_volume_pack_rolls_back_earlier_parts_after_a_conflict() {
    if !archiver_available() {
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let input = root.join("input.txt");
    let output = root.join("split.7z");
    let first = root.join("split.7z.001");
    let occupied = root.join("split.7z.002");
    fs::write(&input, vec![b'a'; 30_000]).unwrap();
    fs::write(&occupied, "existing volume").unwrap();
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root],
        Settings::default(),
        || {},
    );
    engine.dispatch(Action::Queue(
        Operation::Archive(ArchiveOperation::Pack {
            paths: vec![input.clone()],
            output,
            folder: PathBuf::new(),
            options: PackOptions {
                level: 0,
                volume: "10k".into(),
                ..Default::default()
            },
        }),
        false,
    ));
    wait(&mut engine, |engine| {
        engine.jobs.last().unwrap().state.finished()
    });
    let result = engine.jobs.last().unwrap();
    assert_eq!(result.state, JobState::Failed);
    assert!(!first.exists());
    assert_eq!(fs::read(occupied).unwrap(), b"existing volume");
    assert_eq!(fs::read(input).unwrap(), vec![b'a'; 30_000]);
}
