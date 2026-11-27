use std::fs::File;
use std::io;
use std::path::Path;

use tempfile::NamedTempFile;

#[cfg(target_os = "linux")]
pub(super) fn preserve(source: &File, _path: &Path, staged: &NamedTempFile) -> io::Result<()> {
    use std::collections::BTreeSet;
    use std::os::unix::{fs::MetadataExt, io::AsRawFd};
    use xattr::FileExt;

    let destination = staged.as_file();
    let original = source.metadata()?;
    let temporary = destination.metadata()?;
    if (original.uid(), original.gid()) != (temporary.uid(), temporary.gid()) {
        // SAFETY: The staging descriptor is open and the IDs came from file metadata.
        if unsafe { libc::fchown(destination.as_raw_fd(), original.uid(), original.gid()) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // The temporary file may inherit an ACL from its parent. Check both files
    // so inherited attributes are removed if the original lacks them.
    let mut names = BTreeSet::new();
    for file in [source, destination] {
        names.extend(file.list_xattr()?.filter(|name| {
            let name = name.as_encoded_bytes();
            name.starts_with(b"security.") || name.starts_with(b"system.")
        }));
    }
    let listed = names.clone();
    names.extend(
        [
            "system.posix_acl_access",
            "system.nfs4_acl",
            "system.richacl",
        ]
        .map(Into::into),
    );
    for name in &names {
        let read = |file: &File| match file.get_xattr(name) {
            Err(error) if error.kind() == io::ErrorKind::Unsupported && !listed.contains(name) => {
                Ok(None)
            }
            result => result,
        };
        let value = read(source)?;
        let existing = read(destination)?;
        if value != existing {
            match value {
                Some(value) => destination.set_xattr(name, &value)?,
                None => destination.remove_xattr(name)?,
            }
        }
    }
    destination.set_permissions(original.permissions())
}

#[cfg(target_os = "macos")]
pub(super) fn preserve(source: &File, _path: &Path, staged: &NamedTempFile) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: Both descriptors are open and the default copy state is valid.
    let result = unsafe {
        libc::fcopyfile(
            source.as_raw_fd(),
            staged.as_file().as_raw_fd(),
            std::ptr::null_mut(),
            libc::COPYFILE_SECURITY,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
pub(super) fn preserve(source: &File, path: &Path, staged: &NamedTempFile) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Security::*;

    const IDENTITIES: u32 = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION;
    const SECURITY: u32 = IDENTITIES | DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION;

    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let path = wide(path);
    let destination = wide(staged.path());
    let descriptor = |path: &[u16]| -> io::Result<Vec<u32>> {
        let mut size = 0;
        // SAFETY: Each path is NUL terminated. A zero-sized buffer queries the required size.
        unsafe { GetFileSecurityW(path.as_ptr(), SECURITY, null_mut(), 0, &mut size) };
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0_u32; (size as usize).div_ceil(4)];
        // SAFETY: The aligned buffer contains at least the requested number of bytes.
        if unsafe {
            GetFileSecurityW(
                path.as_ptr(),
                SECURITY,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer)
    };
    staged
        .as_file()
        .set_permissions(source.metadata()?.permissions())?;
    if descriptor(&path)? != descriptor(&destination)? {
        return Err(io::Error::other(
            "The staged security descriptor differs from the original",
        ));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) fn preserve(_source: &File, _path: &Path, _staged: &NamedTempFile) -> io::Result<()> {
    Err(io::Error::other(
        "File access controls cannot be preserved on this platform",
    ))
}
