use super::*;

#[test]
fn startup_paths_are_resolved_lexically_without_probing_storage() {
    let cwd = PathBuf::from("cwd");
    let absolute = std::env::temp_dir().join("missing");
    assert_eq!(
        requested_directories(&cwd, std::iter::empty()),
        [None, None]
    );
    assert_eq!(
        requested_directories(
            &cwd,
            ["left".into(), absolute.clone().into_os_string()].into_iter()
        ),
        [Some(cwd.join("left")), Some(absolute)]
    );
}

#[test]
fn launcher_icon_keeps_transparent_corners_and_paints_the_panes() {
    let icon = icon();
    assert_eq!((icon.width, icon.height), (64, 64));
    assert_eq!(icon.rgba.len(), 64 * 64 * 4);
    let pixel = |x: usize, y: usize| &icon.rgba[(y * 64 + x) * 4..][..4];
    assert_eq!(pixel(0, 0), [0, 0, 0, 0]);
    assert_eq!(pixel(20, 24), [12, 15, 19, 255]);
    assert_eq!(pixel(40, 24), [12, 15, 19, 255]);
    assert_eq!(pixel(32, 24), [170, 235, 129, 255]);
}

#[cfg(target_os = "linux")]
#[test]
fn mount_list_decodes_paths_filters_system_mounts_and_keeps_root_first() {
    let mounts = parse_mounts(
        r"invalid
1 0 8:1 / / rw - ext4 /dev/sda1 rw
2 1 8:1 / / rw - ext4 /dev/sda1 rw
3 1 0:7 / /proc rw - proc proc rw
4 1 8:2 / /run/media/Test\040Drive rw - ext4 /dev/sdb1 rw
5 1 0:9 / /net/Alpha rw - nfs server:/export rw
6 1 0:9 / /net/Alpha rw - nfs server:/export rw
7 1 0:7 / /mnt/cache rw - tmpfs tmpfs rw
8 1 8:3 / /media/Z\134backup rw - ext4 /dev/sdc1 rw
9 1 0:9 / /net/Unix\377 rw - cifs //server/export rw
",
    );
    let paths = mounts
        .iter()
        .map(|place| place.path.clone())
        .collect::<Vec<_>>();
    use std::os::unix::ffi::OsStrExt;
    assert_eq!(
        paths
            .iter()
            .map(|path| path.as_os_str().as_bytes())
            .collect::<Vec<_>>(),
        [
            b"/".as_slice(),
            b"/net/Alpha",
            b"/run/media/Test Drive",
            b"/net/Unix\xff",
            b"/media/Z\\backup"
        ]
    );
    assert!(mounts.iter().all(|place| place.drive));
    assert_eq!(
        mounts
            .iter()
            .map(|place| place.ejectable)
            .collect::<Vec<_>>(),
        [false, false, true, false, true]
    );
    let fallback = parse_mounts("");
    assert_eq!(fallback.len(), 1);
    assert_eq!(fallback[0].path, Path::new("/"));
}

#[cfg(target_os = "linux")]
#[test]
fn native_places_always_include_the_filesystem() {
    let places = read_places();
    assert!(
        places
            .iter()
            .any(|place| place.drive && place.path == Path::new("/"))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn desktop_commands_use_the_requested_directory_and_surface_tool_errors() {
    const FIXTURE: &str = "DESHCMD_TEST_DESKTOP_COMMANDS";
    if let Some(root) = std::env::var_os(FIXTURE).map(PathBuf::from) {
        let directory = root.join("work with spaces");
        let mut desktop = NativeDesktop { clipboard: None };
        assert_eq!(
            desktop.open_terminal(&directory).unwrap_err(),
            "No supported terminal emulator was found."
        );
        assert!(
            desktop
                .open_terminal(&root.join("missing"))
                .unwrap_err()
                .starts_with("xdg-terminal-exec:")
        );
        std::fs::rename(root.join("terminal-fixture"), root.join("ptyxis")).unwrap();
        desktop.open_terminal(&directory).unwrap();
        let output = root.join("terminal-args");
        let expected = format!(
            "{}\n--working-directory={}\n",
            directory.display(),
            directory.display()
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if std::fs::read_to_string(&output).as_deref().ok() == Some(&expected) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "terminal did not receive its arguments"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        desktop.eject(&directory).unwrap();
        assert_eq!(
            desktop.eject(&root.join("busy")).unwrap_err(),
            "volume busy"
        );
        assert!(
            desktop
                .eject(&root.join("silent"))
                .unwrap_err()
                .starts_with("Unmount returned")
        );
        assert!(
            desktop
                .open(&root.join("missing"))
                .unwrap_err()
                .contains("missing")
        );
        assert!(desktop.shortcut(&root.join("link"), &directory).is_err());
        assert!(desktop.clipboard_files().is_empty());
        assert!(desktop.clipboard_text().is_none());
        assert!(desktop.copy_text("clipboard is unavailable").is_err());
        assert!(desktop.copy_files(&[directory]).is_err());
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("work with spaces")).unwrap();
    let terminal = root.join("terminal-fixture");
    std::fs::write(
        &terminal,
        r#"#!/bin/sh
printf '%s\n' "$PWD" "$@" > "$DESHCMD_TEST_TERMINAL_ARGS"
"#,
    )
    .unwrap();
    let gio = root.join("gio");
    std::fs::write(
        &gio,
        r#"#!/bin/sh
[ "$1" = mount ] && [ "$2" = --unmount ] || exit 99
case "$3" in
    */busy) printf '  volume busy\n' >&2; exit 4 ;;
    */silent) exit 5 ;;
    */'work with spaces') exit 0 ;;
    *) exit 98 ;;
esac
"#,
    )
    .unwrap();
    for script in [terminal, gio] {
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::desktop_commands_use_the_requested_directory_and_surface_tool_errors",
        ])
        .env(FIXTURE, &root)
        .env("PATH", &root)
        .env("DESHCMD_TEST_TERMINAL_ARGS", root.join("terminal-args"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
}
