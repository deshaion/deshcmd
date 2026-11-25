use std::io::{self, Write};
use std::path::{Path, PathBuf};

const DESKTOP_ENTRY: &str = include_str!("../../../packaging/linux/deshcmd.desktop");
const ICON: &[u8] = include_bytes!("../../../packaging/linux/deshcmd.svg");

pub fn install() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    // An AppImage mounts or extracts the executable into a temporary directory.
    // Also check APPDIR in case the environment was inherited from another app.
    let launcher = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|path| {
            path.is_absolute()
                && path.is_file()
                && std::env::var_os("APPDIR")
                    .is_some_and(|directory| executable.starts_with(directory))
        })
        .unwrap_or(executable);
    let exec = desktop_exec(&launcher)?;
    let data =
        dirs::data_dir().ok_or_else(|| io::Error::other("User data directory unavailable"))?;
    let entry = data.join("applications/deshcmd.desktop");
    let icon = data.join("icons/hicolor/scalable/apps/deshcmd.svg");
    let contents = DESKTOP_ENTRY.replace("Exec=deshcmd\n", &format!("Exec={exec}\n"));
    install_file(&icon, ICON)?;
    install_file(&entry, contents.as_bytes())?;
    Ok(entry)
}

fn desktop_exec(path: &Path) -> io::Result<String> {
    let path = path.to_str().ok_or_else(|| {
        io::Error::other("The executable path cannot be written to a desktop entry")
    })?;
    let argument = quote_exec_argument(path);
    // Exec forbids '=' in the executable token, and GLib checks that token
    // before expanding '%%'. Pass the path as an argument to a fixed command.
    Ok(if path.contains(['=', '%']) {
        let command = quote_exec_argument(r#"exec "$0""#);
        format!("sh -c {command} {argument}")
    } else {
        argument
    })
}

fn quote_exec_argument(argument: &str) -> String {
    // Desktop-entry string escaping applies before Exec argument unquoting.
    let mut exec = String::from("\"");
    for ch in argument.chars() {
        exec.push_str(match ch {
            '\\' => r"\\\\",
            '"' => r#"\\""#,
            '`' => r"\\`",
            '$' => r"\\$",
            '%' => "%%",
            '\n' => r"\n",
            '\r' => r"\r",
            '\t' => r"\t",
            _ => {
                exec.push(ch);
                continue;
            }
        });
    }
    exec.push('"');
    exec
}

fn install_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(contents)?;
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn desktop_exec_quotes_spaces_shell_characters_and_desktop_field_codes() {
        assert_eq!(
            desktop_exec(Path::new("/tmp/Desh Cmd")).unwrap(),
            r#""/tmp/Desh Cmd""#
        );
        assert_eq!(
            quote_exec_argument("a\\\"\u{0060}$%\r\n\t"),
            r#""a\\\\\\"\\`\\$%%\r\n\t""#
        );
        for path in ["/tmp/a=b", "/tmp/100%"] {
            let command = desktop_exec(Path::new(path)).unwrap();
            assert!(command.starts_with(r#"sh -c "exec \\"\\$0\\"""#));
            assert!(command.ends_with(&quote_exec_argument(path)));
        }
        let invalid = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/non-utf8-\xff"));
        assert!(desktop_exec(invalid).is_err());
    }
}
