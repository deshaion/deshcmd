use super::*;
use std::time::{Duration, Instant};

#[test]
fn cancellation_during_rename_existing_keeps_both_original_paths() {
    let fixture = tempfile::tempdir().unwrap();
    let destination = fixture.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let source = fixture.path().join("example.txt");
    let target = destination.join("example.txt");
    fs::write(&source, b"incoming").unwrap();
    fs::write(&target, b"existing").unwrap();
    let control = Arc::new(Control::default());
    let report = |progress: Progress| {
        if progress.current == source.to_string_lossy() {
            control.cancel();
        }
    };
    let mut worker = Worker {
        control: control.clone(),
        progress: Progress::default(),
        report: &report,
        conflict: &|_| control.answer(Collision::RenameExisting, false),
        last_report: Instant::now() - Duration::from_secs(1),
        warnings: Vec::new(),
    };
    let error = worker
        .transfer(
            vec![source.clone()],
            &destination,
            true,
            TransferOptions::default(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("Cancelled · Moved 0 of 1"));
    assert!(control.is_cancelled());
    assert_eq!(fs::read(&source).unwrap(), b"incoming");
    assert_eq!(fs::read(&target).unwrap(), b"existing");
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
}

#[test]
fn rename_existing_restores_the_previous_item_if_publication_fails() {
    let fixture = tempfile::tempdir().unwrap();
    let target = fixture.path().join("existing");
    let backup = fixture.path().join("backup");
    fs::write(&target, b"existing").unwrap();
    let error =
        publish_renaming_existing(&fixture.path().join("missing"), &target, &backup).unwrap_err();
    assert!(error.to_string().contains("Restored the previous item"));
    assert_eq!(fs::read(&target).unwrap(), b"existing");
    assert!(!backup.exists());
}

fn execute(operation: Operation) -> (io::Result<String>, Vec<String>) {
    let mut worker = Worker {
        control: Arc::default(),
        progress: Progress::default(),
        report: &|_| {},
        conflict: &|conflict| panic!("Unexpected conflict: {conflict:?}"),
        last_report: Instant::now(),
        warnings: Vec::new(),
    };
    let result = worker.run(operation);
    (result, worker.warnings)
}

fn succeeded(operation: Operation) -> String {
    let (result, warnings) = execute(operation);
    assert!(warnings.is_empty(), "{warnings:?}");
    result.unwrap()
}

#[test]
fn file_operations_report_results_and_refuse_to_clobber_outputs() {
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("new/child");
    let source = folder.join("source");
    let renamed = folder.join("SOURCE");
    let hardlink = folder.join("hardlink");
    let report = folder.join("report");

    assert_eq!(succeeded(Operation::Mkdir(folder.clone())), "Created child");
    assert!(folder.is_dir());
    assert_eq!(
        succeeded(Operation::Create(source.clone())),
        "Created source"
    );
    fs::write(&source, b"abc").unwrap();
    assert!(execute(Operation::Create(source.clone())).0.is_err());
    assert_eq!(fs::read(&source).unwrap(), b"abc");
    assert_eq!(
        succeeded(Operation::Rename {
            source: source.clone(),
            target: renamed.clone()
        }),
        "Renamed item"
    );
    assert_eq!(fs::read(&renamed).unwrap(), b"abc");
    succeeded(Operation::Link {
        source: renamed.clone(),
        target: hardlink.clone(),
        symbolic: false,
        relative: false,
    });
    fs::write(&hardlink, b"updated").unwrap();
    assert_eq!(fs::read(&renamed).unwrap(), b"updated");
    assert_eq!(
        succeeded(Operation::Statistics(vec![folder.clone()])),
        "2 files, 1 folder · 0.014 KB"
    );
    let (result, warnings) = execute(Operation::Statistics(vec![folder.join("missing")]));
    assert!(result.is_ok());
    assert_eq!(warnings.len(), 1);

    let export = |contents: &str| Operation::Export {
        output: report.clone(),
        contents: contents.into(),
    };
    assert_eq!(succeeded(export("original")), "Saved report");
    assert!(execute(export("replacement")).0.is_err());
    assert_eq!(fs::read_to_string(report).unwrap(), "original");
    succeeded(Operation::Delete {
        paths: vec![folder.clone()],
        trash: false,
    });
    assert!(!folder.exists());
}

#[test]
fn comments_and_recursive_properties_persist_without_touching_symlink_targets() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let folder = root.join("folder");
    fs::create_dir(&folder).unwrap();
    let file = folder.join("with spaces.txt");
    fs::write(&file, "abc").unwrap();
    let comment = |path, text: &str| Operation::Comment {
        path,
        text: text.to_owned(),
    };
    succeeded(comment(file.clone(), "first\nsecond"));
    succeeded(comment(folder.join("other"), "plain"));
    assert_eq!(
        fs::read_to_string(folder.join("descript.ion")).unwrap(),
        "other plain\n\"with spaces.txt\" first second\n"
    );
    succeeded(comment(file.clone(), "   "));
    assert_eq!(
        read_comments(&folder),
        [("other".into(), "plain".into())].into()
    );

    let outside = root.join("outside");
    fs::write(&outside, "untouched").unwrap();
    let outside_modified = outside.metadata().unwrap().modified().unwrap();
    #[cfg(unix)]
    for (name, relative) in [("relative", true), ("absolute", false)] {
        let target = folder.join(name);
        succeeded(Operation::Link {
            source: outside.clone(),
            target: target.clone(),
            symbolic: true,
            relative,
        });
        if relative {
            assert_eq!(fs::read_link(&target).unwrap(), Path::new("../outside"));
        }
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
    }
    let modified = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    succeeded(Operation::Properties {
        paths: vec![folder.clone()],
        read_only: None,
        modified: Some(modified),
        recursive: true,
    });
    assert_eq!(file.metadata().unwrap().modified().unwrap(), modified);
    assert_eq!(folder.metadata().unwrap().modified().unwrap(), modified);
    assert_eq!(
        outside.metadata().unwrap().modified().unwrap(),
        outside_modified
    );

    for read_only in [true, false] {
        succeeded(Operation::Properties {
            paths: vec![file.clone()],
            read_only: Some(read_only),
            modified: None,
            recursive: false,
        });
        assert_eq!(file.metadata().unwrap().permissions().readonly(), read_only);
    }
    #[cfg(unix)]
    assert!(
        execute(comment(folder.join("bad\nname"), "text"))
            .0
            .unwrap_err()
            .to_string()
            .contains("descript.ion")
    );
}

#[test]
fn wipe_rejects_zero_passes_and_preserves_other_hardlinks() {
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("folder");
    fs::create_dir(&folder).unwrap();
    let file = folder.join("file");
    let other = fixture.path().join("other");
    fs::write(&file, b"original contents").unwrap();
    let wipe = |path, passes| Operation::Wipe {
        paths: vec![path],
        passes,
    };
    assert!(
        execute(wipe(file.clone(), 0))
            .0
            .unwrap_err()
            .to_string()
            .contains("at least one")
    );
    assert_eq!(fs::read(&file).unwrap(), b"original contents");
    #[cfg(unix)]
    {
        fs::hard_link(&file, &other).unwrap();
        let (_, warnings) = execute(wipe(file.clone(), 1));
        assert!(warnings[0].contains("other hard links"));
        assert_eq!(fs::read(&other).unwrap(), b"original contents");
        fs::remove_file(&other).unwrap();
    }
    #[cfg(not(unix))]
    assert!(!other.exists());
    succeeded(wipe(folder.clone(), 3));
    assert!(!folder.exists());
}

#[test]
fn all_checksum_algorithms_generate_verifiable_manifests() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("with spaces.txt");
    fs::write(&source, b"abc").unwrap();
    let mut labels = std::collections::HashSet::new();
    for algorithm in Algorithm::ALL {
        assert!(labels.insert(algorithm.label()), "{algorithm:?}");
        let output = fixture
            .path()
            .join("manifest")
            .with_extension(algorithm.extension());
        assert_eq!(Algorithm::from_path(&output), Some(algorithm));
        let hash = Operation::Hash {
            paths: vec![source.clone(), source.clone()],
            output: output.clone(),
            algorithm,
            mode: ManifestMode::Single,
        };
        assert_eq!(succeeded(hash), "Wrote 1 checksum manifest(s) for 1 files");
        let manifest = fs::read_to_string(&output).unwrap();
        let digest = if algorithm == Algorithm::Sfv {
            let (name, digest) = manifest.trim_end().rsplit_once(' ').unwrap();
            assert_eq!(name, "with spaces.txt");
            digest
        } else {
            let (digest, name) = manifest.trim_end().split_once("  ").unwrap();
            assert_eq!(name, "with spaces.txt");
            digest
        };
        assert!(!digest.is_empty());
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        match algorithm {
            Algorithm::Sha256 => assert_eq!(
                digest,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            ),
            Algorithm::Md5 => assert_eq!(digest, "900150983cd24fb0d6963f7d28e17f72"),
            Algorithm::Crc32 | Algorithm::Sfv => {
                assert_eq!(digest.to_ascii_lowercase(), "352441c2")
            }
            _ => {}
        }
        assert_eq!(
            succeeded(Operation::Verify {
                source: output,
                expected: None,
                algorithm
            }),
            "Verified 1 of 1 files"
        );
        assert!(
            succeeded(Operation::Verify {
                source: source.clone(),
                expected: Some(format!("  {}  ", digest.to_ascii_uppercase())),
                algorithm
            })
            .starts_with("Verified with spaces.txt")
        );
    }
    assert_eq!(
        Algorithm::from_path(Path::new("manifest.SHA")),
        Some(Algorithm::Sha1)
    );
    assert_eq!(Algorithm::from_path(Path::new("manifest.txt")), None);
}

#[test]
fn checksum_manifest_modes_keep_paths_relative_and_surface_bad_entries() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    let file = nested.join("file.txt");
    let second = nested.join("second.txt");
    fs::write(&file, b"abc").unwrap();
    fs::write(&second, b"def").unwrap();
    let output = root.join("unused");
    let algorithm = Algorithm::Md5;
    let hash = |mode, paths| Operation::Hash {
        paths,
        output: output.clone(),
        algorithm,
        mode,
    };
    let verify = |source, expected| Operation::Verify {
        source,
        expected,
        algorithm,
    };

    assert_eq!(
        succeeded(hash(ManifestMode::PerDirectory, vec![nested.clone()])),
        "Wrote 1 checksum manifest(s) for 2 files"
    );
    let directory_manifest = nested.join("checksums.md5");
    assert_eq!(
        succeeded(verify(directory_manifest.clone(), None)),
        "Verified 2 of 2 files"
    );
    assert!(
        execute(hash(ManifestMode::PerDirectory, vec![nested.clone()]))
            .0
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    assert_eq!(
        succeeded(hash(
            ManifestMode::PerFile,
            vec![file.clone(), second.clone()]
        )),
        "Wrote 2 checksum manifest(s) for 2 files"
    );
    assert_eq!(
        succeeded(verify(nested.join("file.txt.md5"), None)),
        "Verified 1 of 1 files"
    );
    assert!(
        execute(hash(ManifestMode::Single, Vec::new()))
            .0
            .unwrap_err()
            .to_string()
            .contains("No regular files")
    );

    fs::write(&directory_manifest, "\u{feff}# comment\n; another comment\n\n900150983CD24FB0D6963F7D28E17F72 *file.txt\n0000  second.txt\n0000  missing.txt\n").unwrap();
    let (message, warnings) = execute(verify(directory_manifest.clone(), None));
    assert_eq!(message.unwrap(), "Verified 1 of 3 files");
    assert!(warnings[0].contains("checksum mismatch"));
    assert!(warnings[1].contains("missing.txt"));
    assert!(
        execute(verify(file, Some("0000".into())))
            .0
            .unwrap_err()
            .to_string()
            .contains("Checksum mismatch")
    );

    for invalid in ["not a checksum", "zz  file.txt", "1234  "] {
        fs::write(&directory_manifest, invalid).unwrap();
        assert!(
            execute(verify(directory_manifest.clone(), None))
                .0
                .unwrap_err()
                .to_string()
                .contains("Invalid checksum manifest entry")
        );
    }
    fs::write(&directory_manifest, "; comment only\n").unwrap();
    assert!(
        execute(verify(directory_manifest, None))
            .0
            .unwrap_err()
            .to_string()
            .contains("manifest is empty")
    );
    #[cfg(unix)]
    {
        let multiline = root.join("line\nbreak.txt");
        fs::write(&multiline, b"abc").unwrap();
        assert!(
            execute(hash(ManifestMode::Single, vec![multiline]))
                .0
                .unwrap_err()
                .to_string()
                .contains("single-line")
        );
        assert!(!output.exists());
    }
}

#[test]
fn split_and_combine_validate_inputs_and_leave_existing_files_alone() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let source = root.join("example");
    let output = root.join("out");
    let split = |part_bytes, part_count| Operation::Split {
        source: source.clone(),
        destination: root.into(),
        part_bytes,
        part_count,
    };
    let combine = |paths| Operation::Combine {
        paths,
        output: output.clone(),
    };

    fs::write(&source, b"").unwrap();
    assert!(
        execute(split(1, None))
            .0
            .unwrap_err()
            .to_string()
            .contains("nonempty")
    );
    fs::write(&source, b"abcde").unwrap();
    assert!(
        execute(split(0, None))
            .0
            .unwrap_err()
            .to_string()
            .contains("Part size")
    );
    assert!(
        execute(split(0, Some(0)))
            .0
            .unwrap_err()
            .to_string()
            .contains("Part count")
    );
    assert_eq!(succeeded(split(0, Some(2))), "Created 2 parts");
    let parts = crate::numbered_parts(&root.join("example.001"));
    assert_eq!(fs::read(&parts[0]).unwrap(), b"abc");
    assert_eq!(fs::read(&parts[1]).unwrap(), b"de");
    assert!(
        execute(split(0, Some(2)))
            .0
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    assert!(
        execute(combine(Vec::new()))
            .0
            .unwrap_err()
            .to_string()
            .contains("Select file parts")
    );
    assert!(
        execute(combine(vec![root.into()]))
            .0
            .unwrap_err()
            .to_string()
            .contains("regular file parts")
    );
    succeeded(combine(parts.clone()));
    assert!(execute(combine(parts)).0.is_err());
    assert_eq!(fs::read(output).unwrap(), b"abcde");
    fs::write(&source, vec![0; 10_001]).unwrap();
    assert!(
        execute(split(1, None))
            .0
            .unwrap_err()
            .to_string()
            .contains("10,000")
    );
}

#[test]
fn transfer_collision_choices_keep_the_expected_contents() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("example.txt");
    fs::write(&source, "new contents").unwrap();
    filetime::set_file_mtime(
        &source,
        filetime::FileTime::from_unix_time(1_700_000_100, 0),
    )
    .unwrap();
    for (choice, existing, wanted, renamed) in [
        (Collision::Replace, "old", "new contents", None),
        (Collision::Newer, "old", "new contents", None),
        (Collision::Larger, "old", "new contents", None),
        (Collision::Smaller, "old", "old", None),
        (
            Collision::Smaller,
            "much larger existing content",
            "new contents",
            None,
        ),
        (Collision::Append, "old", "oldnew contents", None),
        (
            Collision::RenameIncoming,
            "old",
            "old",
            Some("new contents"),
        ),
    ] {
        let destination = tempfile::tempdir_in(fixture.path()).unwrap();
        let target = destination.path().join("example.txt");
        fs::write(&target, existing).unwrap();
        filetime::set_file_mtime(
            &target,
            filetime::FileTime::from_unix_time(1_700_000_000, 0),
        )
        .unwrap();
        succeeded(Operation::Transfer {
            paths: vec![source.clone()],
            destination: destination.path().into(),
            moving: false,
            options: TransferOptions {
                collision: choice,
                ..Default::default()
            },
        });
        assert_eq!(fs::read_to_string(target).unwrap(), wanted, "{choice:?}");
        let renamed_target = destination.path().join("example (1).txt");
        if let Some(renamed) = renamed {
            assert_eq!(fs::read_to_string(renamed_target).unwrap(), renamed);
        } else {
            assert!(!renamed_target.exists());
        }
        assert_eq!(fs::read_to_string(&source).unwrap(), "new contents");
    }
}

#[test]
fn transfer_rename_masks_and_fast_moves_respect_source_and_destination() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let source = root.join("alpha.txt");
    let target = root.join("destination");
    fs::create_dir(&target).unwrap();
    fs::write(&source, "contents").unwrap();
    let transfer = |paths, destination, moving, options| Operation::Transfer {
        paths,
        destination,
        moving,
        options,
    };
    let sources = vec![source.clone()];
    succeeded(transfer(
        sources.clone(),
        root.into(),
        false,
        TransferOptions::default(),
    ));
    assert_eq!(
        fs::read_to_string(root.join("alpha (1).txt")).unwrap(),
        "contents"
    );
    for (mask, name) in [
        ("backup-*", "backup-alpha.txt"),
        ("a??-copy.*", "alp-copy.txt"),
        ("*.", "alpha"),
    ] {
        succeeded(transfer(
            sources.clone(),
            target.clone(),
            false,
            TransferOptions {
                rename_mask: mask.into(),
                preallocate: true,
                ..Default::default()
            },
        ));
        assert_eq!(fs::read_to_string(target.join(name)).unwrap(), "contents");
    }
    for mask in ["../elsewhere", "."] {
        let result = execute(transfer(
            sources.clone(),
            target.clone(),
            false,
            TransferOptions {
                rename_mask: mask.into(),
                ..Default::default()
            },
        ))
        .0;
        assert!(result.unwrap_err().to_string().contains("valid filename"));
    }
    let (_, warnings) = execute(transfer(
        vec![target.clone()],
        target.clone(),
        true,
        TransferOptions::default(),
    ));
    assert!(warnings[0].contains("into itself"));
    assert!(
        execute(transfer(
            sources.clone(),
            source.clone(),
            false,
            TransferOptions::default()
        ))
        .0
        .unwrap_err()
        .to_string()
        .contains("directory")
    );
    assert!(
        succeeded(transfer(
            sources.clone(),
            root.into(),
            true,
            TransferOptions::default()
        ))
        .contains("1 skipped")
    );
    assert_eq!(
        succeeded(transfer(
            sources,
            target.clone(),
            true,
            TransferOptions::default()
        )),
        "Moved 1 of 1 items, 0 skipped, 0 incomplete"
    );
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(target.join("alpha.txt")).unwrap(),
        "contents"
    );
}

#[test]
fn invalid_transfer_collisions_preserve_existing_items() {
    let fixture = tempfile::tempdir().unwrap();
    let folder = fixture.path().join("folder");
    let target = fixture.path().join("destination");
    fs::create_dir(&folder).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(folder.join("entry"), "incoming").unwrap();
    fs::write(target.join("folder"), "existing file").unwrap();
    for (choice, warning) in [
        (Collision::Replace, "Use merge"),
        (Collision::Merge, "Only two folders"),
    ] {
        let (_, warnings) = execute(Operation::Transfer {
            paths: vec![folder.clone()],
            destination: target.clone(),
            moving: true,
            options: TransferOptions {
                directory_collision: choice,
                ..Default::default()
            },
        });
        assert!(warnings[0].contains(warning));
        assert!(folder.join("entry").exists());
        assert_eq!(
            fs::read_to_string(target.join("folder")).unwrap(),
            "existing file"
        );
    }
    let file = fixture.path().join("file");
    fs::write(&file, "incoming").unwrap();
    fs::write(target.join("file"), "old").unwrap();
    let (_, warnings) = execute(Operation::Transfer {
        paths: vec![file],
        destination: target.clone(),
        moving: false,
        options: TransferOptions {
            verify: true,
            collision: Collision::Append,
            ..Default::default()
        },
    });
    assert!(warnings[0].contains("Append cannot be combined"));
    assert_eq!(fs::read_to_string(target.join("file")).unwrap(), "old");
}

#[cfg(unix)]
#[test]
fn links_and_manifests_use_the_real_parent_when_written_through_a_symlink() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let deep = root.join("nested/deep");
    fs::create_dir_all(&deep).unwrap();
    let alias = root.join("alias");
    symlink(&deep, &alias, true).unwrap();
    let source = root.join("source");
    fs::write(&source, "abc").unwrap();
    let link = alias.join("link");
    succeeded(Operation::Link {
        source: source.clone(),
        target: link.clone(),
        symbolic: true,
        relative: true,
    });
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("../../source"));
    assert_eq!(fs::read(&link).unwrap(), b"abc");

    let output = alias.join("manifest.md5");
    succeeded(Operation::Hash {
        paths: vec![source],
        output: output.clone(),
        algorithm: Algorithm::Md5,
        mode: ManifestMode::Single,
    });
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        "900150983cd24fb0d6963f7d28e17f72  ../../source\n"
    );
    assert_eq!(
        succeeded(Operation::Verify {
            source: output,
            expected: None,
            algorithm: Algorithm::Md5
        }),
        "Verified 1 of 1 files"
    );
}

#[test]
fn changed_source_during_copy_never_replaces_the_destination() {
    let fixture = tempfile::tempdir().unwrap();
    let destination = fixture.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let source = fixture.path().join("file");
    let target = destination.join("file");
    fs::write(&source, "incoming").unwrap();
    fs::write(&target, "existing").unwrap();
    let updated = std::cell::Cell::new(false);
    let mut worker = Worker {
        control: Arc::default(),
        progress: Progress::default(),
        report: &|progress| {
            if progress.current == source.to_string_lossy() && !updated.replace(true) {
                fs::write(&source, "updated incoming contents").unwrap();
            }
        },
        conflict: &|conflict| panic!("Unexpected conflict: {conflict:?}"),
        last_report: Instant::now() - Duration::from_secs(1),
        warnings: Vec::new(),
    };
    worker
        .run(Operation::Transfer {
            paths: vec![source.clone()],
            destination: destination.clone(),
            moving: false,
            options: TransferOptions {
                collision: Collision::Replace,
                ..Default::default()
            },
        })
        .unwrap();
    assert!(worker.warnings[0].contains("Source changed while copying"));
    assert_eq!(fs::read(source).unwrap(), b"updated incoming contents");
    assert_eq!(fs::read(target).unwrap(), b"existing");
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn transfer_keeps_relative_symlinks_and_omits_empty_directories_when_requested() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(source.join("empty")).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(source.join("referenced"), b"payload").unwrap();
    symlink(Path::new("referenced"), &source.join("link"), false).unwrap();
    let existing = destination.join("link");
    fs::write(&existing, "existing").unwrap();
    succeeded(Operation::Transfer {
        paths: vec![source.join("link"), source.join("empty")],
        destination: destination.clone(),
        moving: false,
        options: TransferOptions {
            adjust_links: true,
            omit_empty: true,
            collision: Collision::Replace,
            ..Default::default()
        },
    });
    assert_eq!(
        fs::read_link(&existing).unwrap(),
        Path::new("../source/referenced")
    );
    assert_eq!(fs::read(&existing).unwrap(), b"payload");
    assert!(!destination.join("empty").exists());

    symlink(Path::new("."), &source.join("cycle"), true).unwrap();
    let (result, warnings) = execute(Operation::Transfer {
        paths: vec![source],
        destination,
        moving: false,
        options: TransferOptions {
            follow_links: true,
            ..Default::default()
        },
    });
    assert!(result.unwrap().contains("1 incomplete"));
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("Symbolic link cycle"))
    );
}

#[test]
fn verified_resume_rejects_a_destination_at_least_as_large_as_the_source() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("file");
    let destination = fixture.path().join("destination");
    fs::create_dir(&destination).unwrap();
    let target = destination.join("file");
    fs::write(&source, "incoming").unwrap();
    fs::write(&target, "incoming too long").unwrap();
    let (_, warnings) = execute(Operation::Transfer {
        paths: vec![source.clone()],
        destination,
        moving: true,
        options: TransferOptions {
            collision: Collision::Resume,
            verify: true,
            ..Default::default()
        },
    });
    assert!(warnings[0].contains("target must be smaller"));
    assert_eq!(fs::read(source).unwrap(), b"incoming");
    assert_eq!(fs::read(target).unwrap(), b"incoming too long");
}

#[test]
fn splitting_rolls_back_published_parts_when_a_later_name_is_taken() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let source = root.join("file");
    let first = root.join("file.001");
    let second = root.join("file.002");
    fs::write(&source, b"abcdef").unwrap();
    let mut worker = Worker {
        control: Arc::default(),
        progress: Progress::default(),
        report: &|progress| {
            if progress.current == source.to_string_lossy() && progress.bytes == 0 {
                fs::write(&second, b"concurrent file").unwrap();
            }
        },
        conflict: &|conflict| panic!("Unexpected conflict: {conflict:?}"),
        last_report: Instant::now() - Duration::from_secs(1),
        warnings: Vec::new(),
    };
    let error = worker
        .run(Operation::Split {
            source: source.clone(),
            destination: root.into(),
            part_bytes: 3,
            part_count: None,
        })
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert!(!first.exists());
    assert_eq!(fs::read(second).unwrap(), b"concurrent file");
    assert_eq!(fs::read(source).unwrap(), b"abcdef");
    assert_eq!(fs::read_dir(root).unwrap().count(), 2);
}

#[test]
fn move_cleanup_rejects_changed_sources_before_removing_any_entries() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("source");
    fs::create_dir(&root).unwrap();
    let first = root.join("first");
    let second = root.join("second");
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();
    let control = Control::default();
    let snapshot = move_snapshot(&root, &control).unwrap();
    fs::write(&second, b"concurrently modified").unwrap();
    let transferred = [first.clone(), second.clone(), root.clone()].into();
    let error = remove_transferred(&root, snapshot, &transferred, &control).unwrap_err();
    assert!(error.to_string().contains("original was retained"));
    assert_eq!(fs::read(first).unwrap(), b"first");
    assert_eq!(fs::read(second).unwrap(), b"concurrently modified");
}

#[test]
fn collision_choice_can_be_applied_to_remaining_files_without_reprompting() {
    let fixture = tempfile::tempdir().unwrap();
    let destination = fixture.path().join("out");
    fs::create_dir(&destination).unwrap();
    let paths = ["first.txt", "second.txt"].map(|name| {
        fs::write(destination.join(name), b"old").unwrap();
        let path = fixture.path().join(name);
        fs::write(&path, b"incoming").unwrap();
        path
    });
    let control = Arc::new(Control::default());
    let prompts = std::cell::Cell::new(0);
    let mut worker = Worker {
        control: control.clone(),
        progress: Progress::default(),
        report: &|_| {},
        conflict: &|_| {
            prompts.set(prompts.get() + 1);
            control.answer(Collision::RenameIncoming, true);
        },
        last_report: Instant::now(),
        warnings: Vec::new(),
    };
    worker
        .run(Operation::Transfer {
            paths: paths.into(),
            destination: destination.clone(),
            moving: false,
            options: TransferOptions::default(),
        })
        .unwrap();
    assert_eq!(prompts.get(), 1);
    assert!(worker.warnings.is_empty());
    for name in ["first", "second"] {
        assert_eq!(
            fs::read(destination.join(format!("{name}.txt"))).unwrap(),
            b"old"
        );
        assert_eq!(
            fs::read(destination.join(format!("{name} (1).txt"))).unwrap(),
            b"incoming"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn trash_moves_the_file_into_an_isolated_desktop_trash() {
    const FIXTURE: &str = "DESHCMD_TEST_TRASH_FIXTURE";
    if let Some(root) = std::env::var_os(FIXTURE).map(PathBuf::from) {
        let source = root.join("file.txt");
        let trash = root.join("Trash");
        fs::write(&source, b"original").unwrap();
        succeeded(Operation::Delete {
            paths: vec![source.clone()],
            trash: true,
        });
        assert!(!source.exists());
        assert_eq!(fs::read(trash.join("files/file.txt")).unwrap(), b"original");
        let record = fs::read_to_string(trash.join("info/file.txt.trashinfo")).unwrap();
        assert!(record.contains("file.txt"));
        assert!(record.contains("DeletionDate="));
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "operations::tests::trash_moves_the_file_into_an_isolated_desktop_trash",
        ])
        .env(FIXTURE, &root)
        .env("XDG_DATA_HOME", &root)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
}

#[test]
#[ignore = "runs the full checksum throughput workload"]
fn checksum_benchmark_reports_each_algorithm_that_can_be_benchmarked() {
    let message = succeeded(Operation::Benchmark);
    let mut lines = message.lines();
    for algorithm in Algorithm::ALL
        .into_iter()
        .filter(|algorithm| *algorithm != Algorithm::Sfv)
    {
        let (name, rate) = lines.next().unwrap().split_once("  ").unwrap();
        assert_eq!(name, algorithm.label());
        assert!(
            rate.strip_suffix(" MiB/s").unwrap().parse::<u64>().unwrap() > 0,
            "{rate}"
        );
    }
    assert!(lines.next().is_none());
}

#[test]
fn rename_existing_preserves_a_destination_changed_during_staging() {
    let fixture = tempfile::tempdir().unwrap();
    let destination = fixture.path().join("out");
    fs::create_dir(&destination).unwrap();
    let source = fixture.path().join("file");
    let target = destination.join("file");
    fs::write(&source, b"incoming").unwrap();
    fs::write(&target, b"existing").unwrap();
    let changed = std::cell::Cell::new(false);
    let mut worker = Worker {
        control: Arc::default(),
        progress: Progress::default(),
        report: &|progress| {
            if progress.current == source.to_string_lossy() && !changed.replace(true) {
                fs::write(&target, b"concurrent destination").unwrap();
            }
        },
        conflict: &|conflict| panic!("Unexpected conflict: {conflict:?}"),
        last_report: Instant::now() - Duration::from_secs(1),
        warnings: Vec::new(),
    };
    worker
        .run(Operation::Transfer {
            paths: vec![source.clone()],
            destination: destination.clone(),
            moving: true,
            options: TransferOptions {
                collision: Collision::RenameExisting,
                ..Default::default()
            },
        })
        .unwrap();
    assert!(worker.warnings[0].contains("destination changed while copying"));
    assert_eq!(fs::read(source).unwrap(), b"incoming");
    assert_eq!(fs::read(target).unwrap(), b"concurrent destination");
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
}

#[test]
fn occupied_size_uses_scaled_units_and_counts_nested_files() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("payload"), [0; 1536]).unwrap();
    let small = root.path().join("small");
    fs::write(&small, [0; 8]).unwrap();
    let (result, warnings) = execute(Operation::Statistics(vec![root.path().to_owned()]));
    assert_eq!(result.unwrap(), "2 files, 2 folders · 1.5 KB");
    assert!(warnings.is_empty());
    let (result, _) = execute(Operation::Statistics(vec![small]));
    assert_eq!(result.unwrap(), "1 file, 0 folders · 0.008 KB");
    assert_eq!(
        crate::folder_size(&folder, &Default::default()).unwrap(),
        1536
    );
}
