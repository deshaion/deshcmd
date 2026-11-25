use std::path::Path;

#[cfg(target_os = "linux")]
pub fn open(directory: &Path) -> Result<(), String> {
    use std::ffi::OsString;
    use std::io::ErrorKind;
    use std::process::{Command, Stdio};

    for (program, directory_flag) in [
        ("xdg-terminal-exec", Some("--dir=")),
        ("x-terminal-emulator", None),
        ("ptyxis", Some("--working-directory=")),
        ("gnome-terminal", Some("--working-directory=")),
        ("kgx", Some("--working-directory=")),
        ("konsole", Some("--workdir=")),
        ("xfce4-terminal", Some("--working-directory=")),
        ("xterm", None),
    ] {
        let mut command = Command::new(program);
        command
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(flag) = directory_flag {
            let mut argument = OsString::from(flag);
            argument.push(directory);
            command.arg(argument);
        }
        match command.spawn() {
            Ok(mut child) => {
                // Reap the launcher when it exits without waiting on the UI thread.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            Err(error) if error.kind() == ErrorKind::NotFound && directory.is_dir() => continue,
            Err(error) => return Err(format!("{program}: {error}")),
        }
    }
    Err("No supported terminal emulator was found.".into())
}

#[cfg(target_os = "macos")]
pub fn open(directory: &Path) -> Result<(), String> {
    open::with_detached(directory, "Terminal").map_err(|error| error.to_string())
}

#[cfg(windows)]
pub fn open(directory: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    Command::new("powershell.exe")
        .current_dir(directory)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn open(_directory: &Path) -> Result<(), String> {
    Err("Opening a terminal is unavailable in this build.".into())
}
