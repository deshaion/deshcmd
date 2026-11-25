use std::io;
use std::path::Path;

#[cfg(unix)]
use std::{
    ffi::CString,
    fs,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt, process::CommandExt},
    process::{Command, Stdio},
};

pub(super) fn open_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    if try_launch_program(path)? {
        return Ok(());
    }
    open::that_detached(path)
}

#[cfg(unix)]
fn try_launch_program(path: &Path) -> io::Result<bool> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Ok(false);
    }

    let path = std::path::absolute(path)?;
    let program = CString::new(path.as_os_str().as_bytes())?;
    let mut command = Command::new(&path);
    if let Some(directory) = path.parent() {
        command.current_dir(directory);
    }
    command.stdin(Stdio::null());

    // Ask the kernel to load the file. execvp can interpret an unrecognized
    // executable file as a shell script, which would bypass file associations.
    // SAFETY: The child uses a prebuilt C string and a null-terminated argv.
    // execv and fetching errno on failure are safe between fork and exec.
    unsafe {
        command.pre_exec(move || {
            let arguments = [program.as_ptr(), std::ptr::null()];
            libc::execv(program.as_ptr(), arguments.as_ptr());
            Err(io::Error::last_os_error())
        });
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if use_associated_app(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(true)
}

#[cfg(unix)]
fn use_associated_app(error: &io::Error) -> bool {
    match error.raw_os_error() {
        // Execute bits can be present on ordinary files, including on noexec mounts.
        Some(libc::ENOEXEC | libc::EACCES) => true,
        #[cfg(target_os = "macos")]
        Some(libc::EBADEXEC | libc::EBADARCH | libc::EBADMACHO | libc::EFTYPE) => true,
        _ => false,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    // A fork can inherit a fixture's write handle from a parallel test until exec.
    // Keep fixture writes and launches from overlapping to avoid ETXTBSY.
    static FIXTURE_IO: Mutex<()> = Mutex::new(());

    #[test]
    fn executable_script_runs_from_its_own_directory() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("example program");
        {
            let _fixture_io = FIXTURE_IO.lock().unwrap();
            fs::write(&script, "#!/bin/sh\nprintf launched > result.txt\n").unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
            open_file(&script).unwrap();
        }

        let result = directory.path().join("result.txt");
        let deadline = Instant::now() + Duration::from_secs(5);
        while fs::read(&result).as_deref().ok() != Some(b"launched") {
            assert!(
                Instant::now() < deadline,
                "program did not create its result"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn documents_and_nonexecutable_scripts_use_the_system_opener() {
        let directory = tempfile::tempdir().unwrap();
        let _fixture_io = FIXTURE_IO.lock().unwrap();
        let image = directory.path().join("image.png");
        fs::write(&image, b"\x89PNG\r\n\x1a\n").unwrap();
        fs::set_permissions(&image, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!try_launch_program(&image).unwrap());

        let script = directory.path().join("example.sh");
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!try_launch_program(&script).unwrap());
        assert!(try_launch_program(Path::new("/bin/true")).unwrap());
    }

    #[test]
    fn executable_bits_do_not_turn_objects_or_plain_text_into_programs() {
        let directory = tempfile::tempdir().unwrap();
        let mut elf_object = [0; 64];
        elf_object[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        elf_object[16] = 1; // ET_REL
        elf_object[18] = 0x3e; // EM_X86_64
        elf_object[20] = 1; // ELF version
        elf_object[52] = 64; // ELF header size
        let _fixture_io = FIXTURE_IO.lock().unwrap();
        for (name, bytes) in [
            ("object.o", elf_object.as_slice()),
            ("Example.class", b"\xca\xfe\xba\xbe\x00\x00\x00\x3d\x00\x01"),
            ("note.txt", b"printf launched > unexpected.txt\n"),
        ] {
            let path = directory.path().join(name);
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(!try_launch_program(&path).unwrap(), "{name} was run");
        }
    }
}
