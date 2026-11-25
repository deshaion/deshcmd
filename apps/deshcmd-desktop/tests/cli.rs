use std::process::Command;

fn deshcmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_deshcmd"))
}

#[test]
fn help_prints_usage_without_opening_a_window() {
    for flag in ["--help", "-h"] {
        let output = deshcmd().arg(flag).output().unwrap();
        assert!(output.status.success());
        let usage = String::from_utf8(output.stdout).unwrap();
        assert!(usage.contains("deshcmd [LEFT_DIRECTORY [RIGHT_DIRECTORY]]"));
        #[cfg(any(target_os = "linux", windows))]
        assert!(usage.contains("deshcmd --install-menu-entry"));
    }
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn menu_install_rejects_unexpected_arguments() {
    let output = deshcmd()
        .args(["--install-menu-entry", "unexpected"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("takes no arguments"));
}

#[cfg(target_os = "linux")]
#[test]
fn menu_install_writes_and_replaces_the_launcher_in_an_isolated_data_dir() {
    let fixture = tempfile::tempdir().unwrap();
    let app_dir = fixture.path().join("appdir");
    let app_image = fixture.path().join("Desh Cmd.AppImage");
    let data = fixture.path().join("data");
    std::fs::create_dir(&app_dir).unwrap();
    std::fs::write(&app_image, b"AppImage fixture").unwrap();

    let binary = std::fs::canonicalize(env!("CARGO_BIN_EXE_deshcmd")).unwrap();
    let output = deshcmd()
        .arg("--install-menu-entry")
        .env("XDG_DATA_HOME", &data)
        .env("APPIMAGE", &app_image)
        .env("APPDIR", &app_dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    let launcher = data.join("applications/deshcmd.desktop");
    let contents = std::fs::read_to_string(&launcher).unwrap();
    assert!(contents.contains(&format!("Exec=\"{}\"\n", binary.display())));
    assert!(contents.contains("Icon=deshcmd"));
    assert!(String::from_utf8_lossy(&output.stdout).contains(launcher.to_str().unwrap()));

    let installed_icon = data.join("icons/hicolor/scalable/apps/deshcmd.svg");
    assert_eq!(
        std::fs::read(&installed_icon).unwrap(),
        include_bytes!("../../../packaging/linux/deshcmd.svg")
    );
    std::fs::write(&launcher, "current launcher must be replaced").unwrap();

    let output = deshcmd()
        .arg("--install-menu-entry")
        .env("XDG_DATA_HOME", &data)
        .env("APPIMAGE", &app_image)
        .env("APPDIR", binary.parent().unwrap())
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(
        std::fs::read_to_string(launcher)
            .unwrap()
            .contains(&format!("Exec=\"{}\"\n", app_image.display()))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn startup_without_a_display_returns_an_error() {
    let fixture = tempfile::tempdir().unwrap();
    let output = deshcmd()
        .current_dir(fixture.path())
        .args([".", "missing"])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("WINIT_X11_SCALE_FACTOR")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("DISPLAY") || error.contains("display"),
        "{error}"
    );
}
