use super::*;

#[test]
fn zip_based_documents_keep_the_document_open_action() {
    for extension in ["docx", "xlsx", "pptx", "odt", "ods", "odp", "epub"] {
        assert_eq!(file_kind(extension, false), Kind::Text);
        assert!(crate::archives::supports_extension(extension));
    }
}

fn reference_name(left: &str, right: &str, natural: bool) -> Ordering {
    let (left, right) = (left.to_lowercase(), right.to_lowercase());
    if !natural {
        return left.cmp(&right);
    }
    let (mut a, mut b) = (left.as_bytes(), right.as_bytes());
    while let (Some(&x), Some(&y)) = (a.first(), b.first()) {
        let number = |digits: &[u8]| {
            let end = digits.iter().take_while(|c| c.is_ascii_digit()).count();
            let first = digits[..end].iter().position(|&c| c != b'0').unwrap_or(end);
            (first, end)
        };
        if x.is_ascii_digit() && y.is_ascii_digit() {
            let (start_a, end_a) = number(a);
            let (start_b, end_b) = number(b);
            let order = (end_a - start_a)
                .cmp(&(end_b - start_b))
                .then(a[start_a..end_a].cmp(&b[start_b..end_b]))
                .then(end_a.cmp(&end_b));
            if !order.is_eq() {
                return order;
            }
            (a, b) = (&a[end_a..], &b[end_b..]);
        } else if x != y {
            return x.cmp(&y);
        } else {
            (a, b) = (&a[1..], &b[1..]);
        }
    }
    a.len().cmp(&b.len())
}

fn reference_sort(entries: &mut [Entry], columns: &[Sort], dirs_first: bool, natural: bool) {
    entries.sort_by(|a, b| {
        if dirs_first && a.is_dir() != b.is_dir() {
            return b.is_dir().cmp(&a.is_dir());
        }
        for sort in columns {
            let order = match sort.column {
                Column::Name => reference_name(&a.name, &b.name, natural),
                Column::Size => a.size.cmp(&b.size),
                Column::Modified => a.modified.cmp(&b.modified),
                Column::Kind => a.kind.label().cmp(b.kind.label()),
                Column::Extension => a.extension.cmp(&b.extension),
                Column::Path => a.path.cmp(&b.path),
                Column::Attributes => a.attributes.cmp(&b.attributes),
                Column::Comment => a.comment.cmp(&b.comment),
            };
            if !order.is_eq() {
                return if sort.descending {
                    order.reverse()
                } else {
                    order
                };
            }
        }
        reference_name(&a.name, &b.name, natural).then(a.path.cmp(&b.path))
    });
}

fn entry(name: &str, i: usize) -> Entry {
    Entry {
        archive: None,
        path: PathBuf::from(format!("root/{i}/{name}")),
        name: name.into(),
        extension: Path::new(name)
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase(),
        kind: [Kind::Folder, Kind::Text, Kind::Other][i % 3],
        size: (i % 4) as u64,
        modified: (!i.is_multiple_of(3))
            .then(|| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs((i % 2) as u64)),
        created: None,
        accessed: None,
        read_only: i.is_multiple_of(2),
        link: false,
        hidden: name.starts_with('.'),
        comment: ["a", "B", "é", ""][i % 4].into(),
        attributes: ["0444", "0644", "0755"][i % 3].into(),
    }
}

#[test]
fn sorting_matches_frozen_order_across_columns_and_directions() {
    let names = [
        "part10.txt",
        "part2.txt",
        "part02.txt",
        "part0",
        "part00",
        "PART2.txt",
        "part2.txt",
        "part0002.txt",
        "part9999999999999999999999999999",
        "Σ",
        "ΟΣ",
        "Οσ",
        "İ",
        "é.txt",
        "e\u{301}.txt",
        "Café",
        "CAFE",
        "Ⅳ2",
        "Ⅳ02",
        ".hidden",
    ];
    let entries: Vec<_> = (0..80)
        .map(|i| entry(names[(i * 17) % names.len()], i))
        .collect();
    for column in Column::ALL {
        for descending in [false, true] {
            for dirs_first in [false, true] {
                for natural in [false, true] {
                    for columns in [
                        vec![Sort { column, descending }],
                        vec![
                            Sort { column, descending },
                            Sort {
                                column: Column::Comment,
                                descending: !descending,
                            },
                        ],
                    ] {
                        let mut expected = entries.clone();
                        reference_sort(&mut expected, &columns, dirs_first, natural);
                        let mut actual = entries.clone();
                        sort(&mut actual, &columns, dirs_first, natural);
                        assert!(
                            actual
                                .iter()
                                .map(|entry| &entry.path)
                                .eq(expected.iter().map(|entry| &entry.path)),
                            "{columns:?}, dirs_first={dirs_first}, natural={natural}"
                        );
                    }
                }
            }
        }
    }

    let mut entries = vec![
        entry("part10", 0),
        entry("part002", 1),
        entry("part02", 2),
        entry("part2", 3),
        entry("part00", 4),
        entry("part0", 5),
    ];
    sort(&mut entries, &[], false, true);
    assert!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .eq(["part0", "part00", "part2", "part02", "part002", "part10"])
    );
}

#[test]
fn filters_keep_empty_flags_and_complex_glob_semantics() {
    let entries = [
        entry("sub", 0),
        entry("notes", 1),
        entry("Café.txt", 2),
        entry("Literal;name", 3),
        entry("CAFE10.MD", 4),
        entry("cafe1.TXT", 5),
    ];
    let matching = |criteria: MatchOptions| {
        criteria
            .matching(entries.iter())
            .unwrap()
            .into_iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        matching(MatchOptions {
            files: false,
            ..MatchOptions::filename("")
        }),
        ["sub", "Literal;name"]
    );
    assert_eq!(
        matching(MatchOptions {
            directories: false,
            read_only: true,
            ..MatchOptions::filename("")
        }),
        ["Café.txt", "CAFE10.MD"]
    );
    assert_eq!(matching(MatchOptions::filename("*.*")).len(), entries.len());
    assert_eq!(
        matching(MatchOptions::filename("CAFÉ[0-9].txt; café*.md")),
        ["CAFE10.MD", "cafe1.TXT"]
    );
    assert_eq!(
        matching(MatchOptions {
            extension_only: true,
            ..MatchOptions::filename("t?t")
        }),
        ["Café.txt", "cafe1.TXT"]
    );
    assert_eq!(
        matching(MatchOptions {
            case_sensitive: true,
            ignore_accents: false,
            ..MatchOptions::filename("Café")
        }),
        ["Café.txt"]
    );
    assert_eq!(matching(MatchOptions::filename(";")), ["Literal;name"]);
    assert!(
        MatchOptions::filename("[")
            .matching(entries.iter())
            .is_err()
    );
}

#[test]
fn duplicate_paths_are_skipped_before_metadata_and_comments_are_per_parent() {
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    let top_file = root.path().join("same.txt");
    let sub_file = sub.join("same.txt");
    let missing = sub.join("missing");
    fs::write(&top_file, b"top").unwrap();
    fs::write(&sub_file, b"sub").unwrap();
    fs::write(root.path().join("descript.ion"), "same.txt top-level\n").unwrap();
    fs::write(sub.join("descript.ion"), "same.txt nested\n").unwrap();
    let list = scan(
        &Location::List {
            base: root.path().into(),
            paths: vec![
                sub_file.clone(),
                missing.clone(),
                top_file.clone(),
                missing,
                sub_file.clone(),
            ],
        },
        false,
    )
    .unwrap();
    assert_eq!(list.skipped, 1);
    assert!(
        list.entries
            .iter()
            .map(|entry| (&entry.path, entry.comment.as_str()))
            .eq([(&sub_file, "nested"), (&top_file, "top-level")])
    );
    let flat = scan(
        &Location::Flat {
            base: root.path().into(),
            roots: vec![root.path().into(), sub],
        },
        false,
    )
    .unwrap();
    assert_eq!(flat.entries.len(), 4);
    assert_eq!(
        flat.entries
            .iter()
            .filter(|entry| entry.path == sub_file)
            .count(),
        1
    );
    assert!(scan(&Location::Directory(root.path().join("missing")), false).is_err());
}

#[cfg(unix)]
#[test]
fn enumeration_metadata_preserves_links_permissions_and_native_path_identity() {
    use std::ffi::OsStr;
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{PermissionsExt, symlink},
    };

    let root = tempfile::tempdir().unwrap();
    let path = root.path();
    fs::create_dir(path.join("dir")).unwrap();
    fs::write(path.join("target.txt"), b"hello").unwrap();
    fs::set_permissions(path.join("target.txt"), fs::Permissions::from_mode(0o444)).unwrap();
    symlink("target.txt", path.join("file-link")).unwrap();
    symlink("dir", path.join("dir-link")).unwrap();
    symlink("missing", path.join("broken")).unwrap();
    for name in [b"a\xff", b"a\xfe"] {
        fs::write(path.join(OsStr::from_bytes(name)), name).unwrap();
    }

    let listing = scan(&Location::Directory(path.into()), false).unwrap();
    let row = |name| {
        listing
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .unwrap()
    };
    assert!(row("file-link").link);
    assert_eq!(row("file-link").size, 5);
    assert!(row("file-link").read_only);
    assert_eq!(row("file-link").attributes, "0444");
    assert!(row("dir-link").is_dir() && row("dir-link").link);
    assert!(row("broken").link && !row("broken").is_dir());
    assert_eq!(
        row("broken").size,
        path.join("broken").symlink_metadata().unwrap().len()
    );

    let paths: Vec<_> = listing
        .entries
        .iter()
        .filter(|entry| entry.name == "a�")
        .map(|entry| entry.path.clone())
        .collect();
    assert_eq!(paths.len(), 2);
    assert_ne!(paths[0], paths[1]);
    let virtual_list = scan(
        &Location::List {
            base: path.into(),
            paths,
        },
        false,
    )
    .unwrap();
    assert_eq!(virtual_list.entries.len(), 2);
}

#[cfg(windows)]
#[test]
fn enumeration_metadata_keeps_windows_hidden_and_system_attributes() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM, SetFileAttributesW,
    };

    let root = tempfile::tempdir().unwrap();
    for (name, attribute) in [
        ("hidden.txt", FILE_ATTRIBUTE_HIDDEN),
        ("system.txt", FILE_ATTRIBUTE_SYSTEM),
    ] {
        let path = root.path().join(name);
        fs::write(&path, b"").unwrap();
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), attribute) }, 0);
    }
    let listing = scan(&Location::Directory(root.path().into()), false).unwrap();
    assert_eq!(listing.entries.len(), 2);
    assert!(listing.entries.iter().all(|entry| entry.hidden));
}
