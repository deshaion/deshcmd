use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use tempfile::{NamedTempFile, TempDir, TempPath};

use crate::listing::{self, Entry, Kind, Preview, Scan};
use crate::operations::{Collision, Control, TransferOptions, Worker};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtractLayout {
    #[default]
    Direct,
    Separate,
    Smart,
}

#[derive(Clone, Debug, Default)]
pub struct ExtractOptions {
    pub mask: String,
    pub flatten: bool,
    pub layout: ExtractLayout,
    pub collision: Collision,
    pub password: String,
}

#[derive(Clone, Debug)]
pub struct PackOptions {
    pub level: u8,
    pub update: bool,
    pub password: String,
    pub volume: String,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            level: 5,
            update: false,
            password: String::new(),
            volume: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ArchiveSelection {
    pub file: PathBuf,
    pub folder: PathBuf,
    /// Full member paths relative to the archive root. Empty means the whole folder.
    pub members: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub enum ArchiveOperation {
    Pack {
        paths: Vec<PathBuf>,
        output: PathBuf,
        folder: PathBuf,
        options: PackOptions,
    },
    Extract {
        sources: Vec<ArchiveSelection>,
        destination: PathBuf,
        options: ExtractOptions,
    },
    Delete(ArchiveSelection),
    Test {
        paths: Vec<PathBuf>,
        password: String,
    },
}

impl ArchiveOperation {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pack { .. } => "Pack archive",
            Self::Extract { .. } => "Extract archive",
            Self::Delete(_) => "Delete archive members",
            Self::Test { .. } => "Test archive",
        }
    }
}

pub fn is_archive(path: &Path) -> bool {
    supports_extension(&extension(path))
}

pub(crate) fn supports_extension(extension: &str) -> bool {
    matches!(
        extension,
        "zip"
            | "zipx"
            | "7z"
            | "rar"
            | "tar"
            | "tgz"
            | "tbz"
            | "tbz2"
            | "txz"
            | "tzst"
            | "gz"
            | "gzip"
            | "bz2"
            | "bzip2"
            | "xz"
            | "lzma"
            | "zst"
            | "zstd"
            | "br"
            | "jar"
            | "war"
            | "ear"
            | "epub"
            | "apk"
            | "ipa"
            | "xpi"
            | "docx"
            | "xlsx"
            | "pptx"
            | "odt"
            | "ods"
            | "odp"
            | "arj"
            | "cab"
            | "iso"
            | "dmg"
            | "vhd"
            | "vhdx"
            | "wim"
            | "swm"
            | "squashfs"
            | "cpio"
            | "deb"
            | "udeb"
            | "rpm"
            | "b64"
            | "base64"
            | "001"
    ) || configured_archiver(extension).is_some()
}

fn extension(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}

fn filename(path: &Path) -> io::Result<&std::ffi::OsStr> {
    path.file_name()
        .ok_or_else(|| io::Error::other("Expected a file name"))
}

fn seven_zip() -> Command {
    const PROGRAMS: &[&str] = if cfg!(windows) {
        &["7z.exe", "7zz.exe", "7za.exe"]
    } else {
        &["7zz", "7z", "7za"]
    };
    if let Some(configured) = std::env::var_os("DESHCMD_7ZIP").filter(|value| !value.is_empty()) {
        return Command::new(configured);
    }
    if let Some(path) = find_program(PROGRAMS) {
        return Command::new(path);
    }
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .map(|path| path.join("7-Zip/7z.exe"))
        .filter(|path| path.is_file())
    {
        return Command::new(path);
    }
    Command::new(PROGRAMS[0])
}

fn find_program(programs: &[&str]) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|directory| programs.iter().map(move |name| directory.join(name)))
        .find(|path| path.is_file())
}

fn unrar(file: &Path, action: &str, password: &str) -> Option<Command> {
    if extension(file) != "rar" {
        return None;
    }
    let program = std::env::var_os("DESHCMD_UNRAR")
        .map(PathBuf::from)
        .or_else(|| {
            find_program(if cfg!(windows) {
                &["UnRAR.exe"]
            } else {
                &["unrar"]
            })
        })?;
    let mut command = Command::new(program);
    let password = if password.is_empty() {
        "-p-".to_owned()
    } else {
        format!("-p{password}")
    };
    command.args([action, "-cfg-", "-c-", &password]);
    Some(command)
}

fn seven(action: &str, password: &str) -> Command {
    let mut command = seven_zip();
    command.args([action, "-bd", "-sccUTF-8"]);
    if !password.is_empty() {
        command.arg(format!("-p{password}"));
    }
    command
}

fn run(
    command: &mut Command,
    output: &File,
    control: &Control,
    limit: Option<u64>,
) -> io::Result<()> {
    control.check()?;
    let mut errors = tempfile::tempfile()?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(errors.try_clone()?)
        .spawn()
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "Archive tool {:?} was not found. Install it or configure its path.",
                        command.get_program()
                    ),
                )
            } else {
                error
            }
        })?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let mut paused = false;
    let result = loop {
        if control.is_cancelled() {
            break Err(io::Error::other("Cancelled"));
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let pause = control.is_paused();
            if pause != paused {
                // The PID belongs to our still-running child.
                unsafe {
                    libc::kill(
                        child.id() as libc::pid_t,
                        if pause { libc::SIGSTOP } else { libc::SIGCONT },
                    )
                };
                paused = pause;
            }
        }
        if let Some(limit) = limit {
            match output.metadata() {
                Ok(metadata) if metadata.len() > limit => {
                    break Err(io::Error::other("Archive output exceeded its size limit"));
                }
                Err(error) => break Err(error),
                _ => {}
            }
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(status)) => {
                let mut details = Vec::new();
                errors.rewind()?;
                errors.take(8192).read_to_end(&mut details)?;
                let details = String::from_utf8_lossy(&details);
                break Err(io::Error::other(if details.trim().is_empty() {
                    format!("Archive tool exited with {status}")
                } else {
                    details.trim().to_owned()
                }));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(error) => break Err(error),
        }
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result?;
    if let Some(limit) = limit
        && output.metadata()?.len() > limit
    {
        return Err(io::Error::other("Archive output exceeded its size limit"));
    }
    control.check()
}

#[derive(Clone, Debug)]
struct Member {
    path: PathBuf,
    key: String,
    size: Option<u64>,
    folder: bool,
    mode: Option<u32>,
    crc: Option<u32>,
}

#[derive(Default)]
struct Inventory {
    members: Vec<Member>,
    skipped: usize,
}

fn access_mode(mode: &str) -> Option<u32> {
    let bits = mode.as_bytes().get(1..).filter(|bits| bits.len() == 9)?;
    let mut value = 0;
    for (index, &bit) in bits.iter().enumerate() {
        let set = match (index % 3, bit) {
            (0, b'r') | (1, b'w') | (2, b'x' | b's' | b't') => true,
            (_, b'-') | (2, b'S' | b'T') => false,
            _ => return None,
        };
        value = (value << 1) | u32::from(set);
    }
    Some(value)
}

fn member_path(key: &str) -> Option<PathBuf> {
    // 7-Zip uses native separators in technical listings on Windows.
    let key = key.trim_start_matches("./").trim_start_matches(".\\");
    let mut path = PathBuf::new();
    for name in key.trim_end_matches(['/', '\\']).split(['/', '\\']) {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || name.contains(':')
            || name.chars().any(char::is_control)
            || (cfg!(windows)
                && (name.ends_with(['.', ' '])
                    || name.contains(['<', '>', '"', '|', '?', '*'])
                    || windows_device(name)))
        {
            return None;
        }
        path.push(name);
    }
    Some(path)
}

fn windows_device(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

fn read_listing(text: &str) -> io::Result<Inventory> {
    let mut inventory = Inventory::default();
    let mut record = BTreeMap::new();
    for line in text.lines().chain(std::iter::once("")) {
        if !line.is_empty()
            && let Some((key, value)) = line.split_once(" = ")
        {
            if key == "Path" && record.contains_key("Path") {
                return Err(io::Error::other("Ambiguous archive listing"));
            }
            record.insert(key, value);
            continue;
        }
        if !line.is_empty() {
            return Err(io::Error::other("Malformed archive member listing"));
        }
        if record.is_empty() {
            continue;
        }
        let key = record
            .get("Path")
            .copied()
            .ok_or_else(|| io::Error::other("Archive member has no path"))?;
        let attributes = record.get("Attributes").copied().unwrap_or_default();
        let mode = record.get("Mode").copied().or_else(|| {
            attributes
                .split_whitespace()
                .find(|value| value.len() == 10)
        });
        let folder = key.ends_with(['/', '\\'])
            || record.get("Folder") == Some(&"+")
            || attributes.starts_with('D')
            || mode.is_some_and(|value| value.starts_with('d'));
        let special = ["Symbolic Link", "Hard Link", "Alternate Stream"]
            .iter()
            .any(|attribute| {
                record
                    .get(attribute)
                    .is_some_and(|value| !value.is_empty() && *value != "-")
            })
            || mode.is_some_and(|value| value.starts_with(['l', 'b', 'c', 'p', 's']));
        if let Some(path) = member_path(key).filter(|_| !special) {
            let size = record
                .get("Size")
                .filter(|value| !value.is_empty())
                .and_then(|value| value.parse().ok());
            inventory.members.push(Member {
                path,
                key: key.to_owned(),
                size,
                folder,
                mode: mode.and_then(access_mode),
                crc: record
                    .get("CRC")
                    .and_then(|value| u32::from_str_radix(value, 16).ok()),
            });
        } else if key.trim_end_matches(['/', '\\']) != "." {
            inventory.skipped += 1;
        }
        record.clear();
    }
    Ok(inventory)
}

fn read_limited(output: &mut NamedTempFile, max: u64) -> io::Result<String> {
    let mut text = String::new();
    output.rewind()?;
    output
        .as_file_mut()
        .take(max + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > max {
        return Err(io::Error::other("Archive listing is too large"));
    }
    Ok(text)
}

fn list(file: &Path, password: &str, control: &Control) -> io::Result<Inventory> {
    const MAX_LIST: u64 = 64 * 1024 * 1024;
    let mut output = NamedTempFile::new()?;
    let tool = external_archiver(file);
    if tool.is_none()
        && let Some(mut command) = unrar(file, "lt", password)
    {
        run_listing(command.arg("--").arg(file), &mut output, control, MAX_LIST)?;
        return read_rar_listing(&read_limited(&mut output, MAX_LIST)?);
    }
    let mut command = if let Some(tool) = tool {
        tool.command("list", file, None, None, password)?
    } else {
        let mut command = seven("l", password);
        command.args(["-slt", "-ba", "--"]).arg(file);
        command
    };
    run_listing(&mut command, &mut output, control, MAX_LIST)?;
    let text = read_limited(&mut output, MAX_LIST)?;
    match tool.map_or(ListingFormat::Technical, |tool| tool.listing) {
        ListingFormat::Technical => read_listing(&text),
        ListingFormat::Json => read_json_listing(&text),
    }
}

fn run_listing(
    command: &mut Command,
    output: &mut NamedTempFile,
    control: &Control,
    limit: u64,
) -> io::Result<()> {
    match run(command, output.as_file(), control, Some(limit)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let mut stdout = Vec::new();
            if output.rewind().is_ok() {
                let _ = output.as_file_mut().take(8192).read_to_end(&mut stdout);
            }
            let diagnostic =
                format!("{}\n{error}", String::from_utf8_lossy(&stdout)).to_ascii_lowercase();
            if [
                "enter password",
                "wrong password",
                "password is incorrect",
                "encrypted archive",
            ]
            .iter()
            .any(|message| diagnostic.contains(message))
            {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Enter the correct archive password to browse its contents",
                ))
            } else {
                Err(error)
            }
        }
    }
}

fn read_rar_listing(text: &str) -> io::Result<Inventory> {
    let mut inventory = Inventory::default();
    let mut name = None;
    let mut kind = None;
    let mut size = None;
    let mut mode = None;
    let mut crc = None;
    for line in text.lines().chain(std::iter::once("")) {
        if let Some(value) = line.strip_prefix("        Name: ") {
            if name.replace(value).is_some() {
                return Err(io::Error::other("Ambiguous RAR member listing"));
            }
        } else if let Some(value) = line.strip_prefix("        Type: ") {
            kind = Some(value);
        } else if let Some(value) = line.strip_prefix("        Size: ") {
            size = value.parse().ok();
        } else if let Some(value) = line.trim_start().strip_prefix("Attributes: ") {
            mode = access_mode(value);
        } else if let Some(value) = line.trim_start().strip_prefix("CRC32: ") {
            crc = u32::from_str_radix(value, 16).ok();
        } else if line.is_empty()
            && let Some(key) = name.take()
        {
            let kind = kind.take();
            if let Some(path) =
                member_path(key).filter(|_| matches!(kind, Some("File" | "Directory")))
            {
                inventory.members.push(Member {
                    path,
                    key: key.to_owned(),
                    size,
                    folder: kind == Some("Directory"),
                    mode,
                    crc,
                });
            } else {
                inventory.skipped += 1;
            }
            size = None;
            mode = None;
            crc = None;
        }
    }
    Ok(inventory)
}

fn extract_stream(
    file: &Path,
    key: Option<&str>,
    password: &str,
    output: &File,
    control: &Control,
    limit: Option<u64>,
) -> io::Result<()> {
    let mut command = if let Some(tool) = external_archiver(file) {
        tool.command("extract", file, None, key, password)?
    } else if key.is_some_and(|name| !name.contains(['*', '?']))
        && let Some(mut command) = unrar(file, "p", password)
    {
        command
            .args(["-inul", "-@", "--"])
            .arg(file)
            .arg(key.unwrap());
        command
    } else {
        let mut command = seven("x", password);
        command.args(["-so", "-bso0", "-bsp0", "-spd", "-ssc"]);
        if let Some(key) = key {
            command.arg(format!("-i!{key}"));
        }
        command.arg("--").arg(file);
        command
    };
    run(&mut command, output, control, limit)?;
    Ok(())
}

enum Input {
    Archive(PathBuf),
    Stream { file: PathBuf, member: Member },
}

struct Reader {
    input: Input,
    _temp: Option<TempDir>,
}

impl Reader {
    fn open(file: &Path, password: &str, control: &Control) -> io::Result<Self> {
        let ext = extension(file);
        let stem = Path::new(file.file_stem().unwrap_or_default());
        let tarball = matches!(ext.as_str(), "tgz" | "tbz" | "tbz2" | "txz" | "tzst")
            || (matches!(
                ext.as_str(),
                "gz" | "gzip" | "bz2" | "bzip2" | "xz" | "lzma" | "zst" | "zstd" | "br"
            ) && extension(stem) == "tar");
        let package = matches!(ext.as_str(), "deb" | "udeb" | "rpm");
        let standalone = matches!(
            ext.as_str(),
            "gz" | "gzip"
                | "bz2"
                | "bzip2"
                | "xz"
                | "lzma"
                | "zst"
                | "zstd"
                | "br"
                | "b64"
                | "base64"
        );
        if external_archiver(file).is_some() || !(tarball || package || standalone) {
            return Ok(Self {
                input: Input::Archive(file.to_owned()),
                _temp: None,
            });
        }
        let temp = tempfile::tempdir()?;
        let inner = if package {
            let inventory = list(file, password, control)?;
            let payload = inventory
                .members
                .iter()
                .find(|member| {
                    let name = member.path.to_string_lossy();
                    if ext == "rpm" {
                        name.ends_with(".cpio")
                    } else {
                        name == "data.tar" || name.starts_with("data.tar.")
                    }
                })
                .ok_or_else(|| io::Error::other("Package payload is missing"))?;
            let payload_path = temp.path().join(filename(&payload.path)?);
            let output = File::create(&payload_path)?;
            extract_stream(
                file,
                Some(&payload.key),
                password,
                &output,
                control,
                payload.size,
            )?;
            if extension(&payload_path) == "tar" || extension(&payload_path) == "cpio" {
                payload_path
            } else {
                let tar = temp.path().join("contents.tar");
                decompress(&payload_path, &tar, password, control)?;
                tar
            }
        } else {
            let output = temp
                .path()
                .join(if tarball { "contents.tar" } else { "contents" });
            decompress(file, &output, password, control)?;
            output
        };
        let input = if tarball || package {
            Input::Archive(inner)
        } else {
            let name = stem
                .file_name()
                .filter(|name| !name.is_empty())
                .unwrap_or(std::ffi::OsStr::new("contents"));
            let key = name.to_string_lossy().into_owned();
            Input::Stream {
                member: Member {
                    path: PathBuf::from(name),
                    key,
                    size: Some(inner.metadata()?.len()),
                    folder: false,
                    mode: None,
                    crc: None,
                },
                file: inner,
            }
        };
        Ok(Self {
            input,
            _temp: Some(temp),
        })
    }

    fn list(&self, password: &str, control: &Control) -> io::Result<Inventory> {
        match &self.input {
            Input::Archive(file) => list(file, password, control),
            Input::Stream { member, .. } => Ok(Inventory {
                members: vec![member.clone()],
                skipped: 0,
            }),
        }
    }

    fn extract(
        &self,
        member: &Member,
        output: &File,
        password: &str,
        control: &Control,
        limit: Option<u64>,
    ) -> io::Result<()> {
        match &self.input {
            Input::Archive(file) => extract_stream(
                file,
                Some(&member.key),
                password,
                output,
                control,
                limit.or(member.size),
            )?,
            Input::Stream { file, .. } => {
                copy_stream(file, output, control, limit)?;
            }
        }
        if let Some(size) = member.size
            && output.metadata()?.len() != size
        {
            return Err(io::Error::other(format!(
                "Unexpected size for {}",
                member.path.display()
            )));
        }
        Ok(())
    }

    fn batch_stream(
        &self,
        files: &[(&Member, PathBuf)],
        stage: &Path,
        password: &str,
        control: &Control,
    ) -> io::Result<Option<NamedTempFile>> {
        let Input::Archive(file) = &self.input else {
            return Ok(None);
        };
        if files.len() < 2 || external_archiver(file).is_some() {
            return Ok(None);
        }
        let Some(total) = files
            .iter()
            .try_fold(0u64, |total, (member, _)| total.checked_add(member.size?))
        else {
            return Ok(None);
        };
        if fs2::available_space(stage)? < total.saturating_mul(2) {
            return Ok(None);
        }
        let mut names = NamedTempFile::new()?;
        for (member, _) in files {
            writeln!(names, "{}", member.key)?;
        }
        let mut command = if let Some(mut command) = unrar(file, "p", password)
            && files
                .iter()
                .all(|(member, _)| !member.key.contains(['*', '?']))
        {
            let mut list = OsString::from("@");
            list.push(names.path());
            command
                .arg("-inul")
                .arg("-scfl")
                .arg("--")
                .arg(file)
                .arg(list);
            command
        } else {
            let mut list = OsString::from("-i@");
            list.push(names.path());
            let mut command = seven("x", password);
            command
                .args(["-so", "-bso0", "-bsp0", "-spd", "-ssc", "-scsUTF-8"])
                .arg(list)
                .arg("--")
                .arg(file);
            command
        };
        // The subprocess only writes stdout. Files are split in archive order
        // into paths already validated by the inventory parser.
        let mut stream = NamedTempFile::new_in(stage)?;
        run(&mut command, stream.as_file(), control, Some(total))?;
        if stream.as_file().metadata()?.len() != total {
            return Err(io::Error::other(
                "Archive returned an unexpected amount of data",
            ));
        }
        stream.rewind()?;
        Ok(Some(stream))
    }
}

fn copy_batch_member(
    input: &mut NamedTempFile,
    output: &mut File,
    member: &Member,
    control: &Control,
) -> io::Result<()> {
    let mut remaining = member.size.unwrap();
    let mut buffer = [0; 64 * 1024];
    let mut checksum = crc32fast::Hasher::new();
    while remaining != 0 {
        control.check()?;
        let count = remaining.min(buffer.len() as u64) as usize;
        input.read_exact(&mut buffer[..count])?;
        output.write_all(&buffer[..count])?;
        if member.crc.is_some() {
            checksum.update(&buffer[..count]);
        }
        remaining -= count as u64;
    }
    if member
        .crc
        .is_some_and(|expected| checksum.finalize() != expected)
    {
        return Err(io::Error::other(format!(
            "Checksum mismatch for {}",
            member.path.display()
        )));
    }
    Ok(())
}

fn decompress(file: &Path, output: &Path, password: &str, control: &Control) -> io::Result<()> {
    let limit =
        fs2::available_space(crate::operations::parent(output)?)?.saturating_sub(64 * 1024 * 1024);
    let output = File::create(output)?;
    let program = match extension(file).as_str() {
        "zst" | "zstd" | "tzst" => "zstd",
        "br" => "brotli",
        "b64" | "base64" => "base64",
        _ => return extract_stream(file, None, password, &output, control, Some(limit)),
    };
    let mut command = Command::new(program);
    if program == "base64" {
        command.arg("-d").arg(file);
    } else {
        command.args(["-d", "-c", "--"]).arg(file);
    }
    match run(&mut command, &output, control, Some(limit)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound && program != "brotli" => {
            extract_stream(file, None, password, &output, control, Some(limit))
        }
        result => result,
    }
}

fn copy_stream(
    path: &Path,
    mut output: &File,
    control: &Control,
    limit: Option<u64>,
) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut buffer = vec![0; 256 * 1024];
    let mut total = 0;
    loop {
        control.check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        total += count as u64;
        if limit.is_some_and(|limit| total > limit) {
            return Err(io::Error::other("Archive output exceeded its size limit"));
        }
        output.write_all(&buffer[..count])?;
    }
}

#[derive(PartialEq, Eq)]
struct FileRevision {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl FileRevision {
    fn new(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        }
    }
}

pub struct ArchiveSource {
    pub file: PathBuf,
    pub revision: u64,
    stamp: FileRevision,
    password: Arc<str>,
    inventory: Mutex<Option<Arc<Inventory>>>,
}

impl std::fmt::Debug for ArchiveSource {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("ArchiveSource")
            .field("file", &self.file)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
pub(crate) struct ArchiveCache(Mutex<ArchiveCacheState>);

#[derive(Default)]
struct ArchiveCacheState {
    sources: VecDeque<Arc<ArchiveSource>>,
    passwords: HashMap<PathBuf, Arc<str>>,
    revision: u64,
}

impl ArchiveCache {
    fn source(&self, file: &Path) -> io::Result<Arc<ArchiveSource>> {
        let stamp = FileRevision::new(&file.metadata()?);
        let mut state = self.0.lock().unwrap();
        if let Some(index) = state.sources.iter().position(|source| source.file == file) {
            let source = state.sources.remove(index).unwrap();
            if source.stamp == stamp {
                state.sources.push_back(source.clone());
                return Ok(source);
            }
        }
        state.revision += 1;
        let source = Arc::new(ArchiveSource {
            file: file.to_owned(),
            revision: state.revision,
            stamp,
            password: state.passwords.get(file).cloned().unwrap_or_default(),
            inventory: Mutex::new(None),
        });
        if state.sources.len() == 8 {
            state.sources.pop_front();
        }
        state.sources.push_back(source.clone());
        Ok(source)
    }

    pub(crate) fn set_password(&self, file: &Path, password: String) {
        let mut state = self.0.lock().unwrap();
        if password.is_empty() {
            state.passwords.remove(file);
        } else {
            state.passwords.insert(file.to_owned(), password.into());
        }
        state.sources.retain(|source| source.file != file);
    }

    pub(crate) fn password(&self, file: &Path) -> Option<Arc<str>> {
        self.0.lock().unwrap().passwords.get(file).cloned()
    }

    pub(crate) fn clear_inventories(&self) {
        self.0.lock().unwrap().sources.clear();
    }
}

impl ArchiveSource {
    fn inventory(&self, control: &Control) -> io::Result<Arc<Inventory>> {
        let mut cached = self.inventory.lock().unwrap();
        if let Some(inventory) = cached.as_ref() {
            return Ok(inventory.clone());
        }
        let reader = Reader::open(&self.file, &self.password, control)?;
        let inventory = Arc::new(reader.list(&self.password, control)?);
        if FileRevision::new(&self.file.metadata()?) != self.stamp {
            return Err(io::Error::other(
                "The archive changed while it was being listed",
            ));
        }
        *cached = Some(inventory.clone());
        Ok(inventory)
    }

    pub fn preview(&self, path: &Path, kind: Kind, pixels: u32) -> Preview {
        self.preview_result(path, kind, pixels)
            .unwrap_or(Preview::Unavailable)
    }

    fn preview_result(&self, path: &Path, kind: Kind, pixels: u32) -> io::Result<Preview> {
        const MAX_PREVIEW: u64 = 16 * 1024 * 1024;
        let control = Control::default();
        let inventory = self.inventory(&control)?;
        let mut matching = inventory
            .members
            .iter()
            .filter(|member| member.path == path);
        let member = matching
            .next()
            .filter(|member| !member.folder && member.size.is_none_or(|size| size <= MAX_PREVIEW))
            .ok_or_else(|| io::Error::other("No previewable member"))?;
        if matching.next().is_some() {
            return Err(io::Error::other("Duplicate archive member name"));
        }
        let reader = Reader::open(&self.file, &self.password, &control)?;
        let output = NamedTempFile::new()?;
        reader.extract(
            member,
            output.as_file(),
            &self.password,
            &control,
            Some(MAX_PREVIEW),
        )?;
        if FileRevision::new(&self.file.metadata()?) != self.stamp {
            return Err(io::Error::other(
                "The archive changed while it was being previewed",
            ));
        }
        Ok(listing::preview(output.path(), kind, pixels))
    }
}

pub(crate) fn scan(
    cache: &ArchiveCache,
    file: &Path,
    folder: &Path,
    recursive: bool,
) -> io::Result<Scan> {
    let source = cache.source(file)?;
    let inventory = source.inventory(&Control::default())?;
    let mut rows = BTreeMap::new();
    let mut folder_exists = folder.as_os_str().is_empty();
    for member in &inventory.members {
        let Ok(relative) = member.path.strip_prefix(folder) else {
            continue;
        };
        let mut components = relative.components();
        let Some(first) = components.next() else {
            folder_exists |= member.folder;
            continue;
        };
        folder_exists = true;
        if recursive || components.next().is_none() {
            if recursive {
                for ancestor in member
                    .path
                    .ancestors()
                    .skip(1)
                    .take_while(|path| *path != folder)
                {
                    rows.entry(ancestor.to_owned()).or_insert((true, 0));
                }
            }
            rows.insert(
                member.path.clone(),
                (member.folder, member.size.unwrap_or(0)),
            );
        } else {
            rows.entry(folder.join(first)).or_insert((true, 0));
        }
    }
    if !folder_exists {
        return Err(io::Error::other("This folder is no longer in the archive"));
    }
    Ok(Scan {
        entries: rows
            .into_iter()
            .map(|(path, (folder, size))| Entry::archive(&source, path, folder, size))
            .collect(),
        skipped: inventory.skipped,
    })
}

pub fn preview_member(file: &Path, path: &Path, kind: Kind, pixels: u32) -> Preview {
    ArchiveCache::default()
        .source(file)
        .map_or(Preview::Unavailable, |source| {
            source.preview(path, kind, pixels)
        })
}

fn select(
    inventory: Inventory,
    selection: &ArchiveSelection,
    mask: Option<&globset::GlobSet>,
    flatten: bool,
) -> io::Result<(Vec<Member>, usize)> {
    let mut files = HashSet::new();
    let mut selected = Vec::new();
    for member in inventory.members {
        if member.path == selection.folder
            || !member.path.starts_with(&selection.folder)
            || (!selection.members.is_empty()
                && !selection
                    .members
                    .iter()
                    .any(|path| member.path.starts_with(path)))
            || (member.folder && (flatten || mask.is_some()))
            || (!member.folder
                && !mask.is_none_or(|mask| {
                    member
                        .path
                        .file_name()
                        .is_some_and(|name| mask.is_match(name))
                }))
        {
            continue;
        }
        if !files.insert(member.path.clone()) {
            return Err(io::Error::other(format!(
                "Duplicate archive member: {}",
                member.path.display()
            )));
        }
        selected.push(member);
    }
    Ok((selected, inventory.skipped))
}

fn archive_stem(path: &Path) -> io::Result<OsString> {
    let stem = Path::new(
        path.file_stem()
            .ok_or_else(|| io::Error::other("Expected an archive file name"))?,
    );
    Ok(if extension(stem) == "tar" {
        stem.file_stem().unwrap_or(stem.as_os_str())
    } else {
        stem.as_os_str()
    }
    .to_owned())
}

impl Worker<'_> {
    pub(crate) fn archive(&mut self, operation: ArchiveOperation) -> io::Result<String> {
        match operation {
            ArchiveOperation::Pack {
                paths,
                output,
                folder,
                options,
            } => self.pack(paths, &output, &folder, &options),
            ArchiveOperation::Extract {
                sources,
                destination,
                options,
            } => self.unpack(sources, &destination, &options),
            ArchiveOperation::Delete(source) => self.delete_members(source),
            ArchiveOperation::Test { paths, password } => {
                for path in &paths {
                    self.update(path, 0);
                    let output = tempfile::tempfile()?;
                    let control = &self.control;
                    if let Some(tool) = external_archiver(path) {
                        run(
                            &mut tool.command("test", path, None, None, &password)?,
                            &output,
                            control,
                            None,
                        )?;
                    } else {
                        let reader = Reader::open(path, &password, control)?;
                        if let Input::Archive(file) = &reader.input {
                            let mut command = unrar(file, "t", &password)
                                .unwrap_or_else(|| seven("t", &password));
                            run(command.arg("--").arg(file), &output, control, None)?;
                        }
                    }
                    self.finish_item();
                }
                Ok(format!("Verified {} archive(s)", paths.len()))
            }
        }
    }

    fn unpack(
        &mut self,
        sources: Vec<ArchiveSelection>,
        destination: &Path,
        options: &ExtractOptions,
    ) -> io::Result<String> {
        if !destination.is_dir() {
            return Err(io::Error::other("Choose an existing destination folder"));
        }
        let mask = listing::compile_mask(&options.mask, false)?;
        let mut count = 0;
        let mut skipped_members = 0;
        for source in sources {
            self.update(&source.file, 0);
            let revision = FileRevision::new(&source.file.metadata()?);
            let reader = Reader::open(&source.file, &options.password, &self.control)?;
            let inventory = reader.list(&options.password, &self.control)?;
            let (members, skipped) = select(inventory, &source, mask.as_ref(), options.flatten)?;
            skipped_members += skipped;
            if members.is_empty() {
                continue;
            }
            let top_level = members
                .iter()
                .filter_map(|member| {
                    member
                        .path
                        .strip_prefix(&source.folder)
                        .ok()?
                        .components()
                        .next()
                })
                .collect::<HashSet<_>>();
            let separate = options.layout == ExtractLayout::Separate
                || options.layout == ExtractLayout::Smart && top_level.len() > 1;
            let staging = TempDir::new_in(destination)?;
            let stage = staging.path().join("files");
            fs::create_dir(&stage)?;
            let mut flattened = Vec::new();
            let mut files = Vec::new();
            for (index, member) in members.iter().enumerate() {
                self.control.check()?;
                let path = if options.flatten {
                    let path = stage.join(index.to_string()).join(filename(&member.path)?);
                    flattened.push(path.clone());
                    path
                } else {
                    stage.join(
                        member
                            .path
                            .strip_prefix(&source.folder)
                            .map_err(io::Error::other)?,
                    )
                };
                if member.folder {
                    fs::create_dir_all(&path)?;
                } else {
                    files.push((member, path));
                }
            }
            let mut batch =
                reader.batch_stream(&files, staging.path(), &options.password, &self.control)?;
            for (member, path) in files {
                self.control.check()?;
                fs::create_dir_all(path.parent().unwrap())?;
                let mut output = File::options().write(true).create_new(true).open(&path)?;
                self.update(&member.path, member.size.unwrap_or(0));
                if let Some(stream) = &mut batch {
                    copy_batch_member(stream, &mut output, member, &self.control)?;
                } else {
                    reader.extract(member, &output, &options.password, &self.control, None)?;
                }
                #[cfg(unix)]
                if let Some(mode) = member.mode {
                    use std::os::unix::fs::PermissionsExt;
                    output.set_permissions(fs::Permissions::from_mode(mode))?;
                }
                count += 1;
            }
            drop(batch);
            if FileRevision::new(&source.file.metadata()?) != revision {
                return Err(io::Error::other(
                    "The archive changed while it was being extracted",
                ));
            }
            let target = if separate {
                let target = destination.join(archive_stem(&source.file)?);
                match fs::symlink_metadata(&target) {
                    Ok(metadata) if !metadata.is_dir() => {
                        return Err(io::Error::other(format!(
                            "{} is not a regular folder",
                            target.display()
                        )));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        fs::create_dir(&target)?
                    }
                    Err(error) => return Err(error),
                }
                target
            } else {
                destination.to_owned()
            };
            let paths = if options.flatten {
                flattened
            } else {
                fs::read_dir(&stage)?
                    .map(|result| result.map(|entry| entry.path()))
                    .collect::<io::Result<_>>()?
            };
            self.transfer(
                paths,
                &target,
                true,
                TransferOptions {
                    collision: options.collision,
                    directory_collision: Collision::Merge,
                    ..Default::default()
                },
            )?;
        }
        let mut message = format!("Processed {count} matching file(s)");
        if skipped_members != 0 {
            message.push_str(&format!(
                " · skipped {skipped_members} unsafe or unsupported member(s)"
            ));
        }
        Ok(message)
    }

    fn pack(
        &mut self,
        paths: Vec<PathBuf>,
        output: &Path,
        folder: &Path,
        options: &PackOptions,
    ) -> io::Result<String> {
        if paths.is_empty() {
            return Err(io::Error::other("Choose files to pack"));
        }
        let destination = crate::operations::parent(output)?;
        let format = PackFormat::detect(output)?;
        let existing = existing_archive(output, options.update)?;
        if existing.is_some() && (!format.supports_update() || !options.volume.is_empty()) {
            return Err(io::Error::other(
                "This archive format or volume configuration does not support updates",
            ));
        }
        if options.update && !options.volume.is_empty() {
            return Err(io::Error::other(
                "Volumes can only be created for a new archive",
            ));
        }
        if !folder.as_os_str().is_empty() && existing.is_none() {
            return Err(io::Error::other("The destination archive no longer exists"));
        }
        let contents = TempDir::new()?;
        let content = contents.path().join("content");
        let work = content.join(folder);
        if folder
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(io::Error::other("Invalid archive folder"));
        }
        fs::create_dir_all(&work)?;
        let mut names = HashSet::new();
        let output_path = if existing.is_some() {
            output.canonicalize()?
        } else {
            destination.canonicalize()?.join(filename(output)?)
        };
        for path in &paths {
            let name = filename(path)?;
            if !names.insert(name) {
                return Err(io::Error::other("Selected items have duplicate file names"));
            }
            if output_path.starts_with(path.canonicalize()?) {
                return Err(io::Error::other("An archive cannot include itself"));
            }
            for entry in walkdir::WalkDir::new(path) {
                self.control.check()?;
                let entry = entry?;
                if !(entry.file_type().is_file() || entry.file_type().is_dir()) {
                    return Err(io::Error::other(format!(
                        "Archive packing requires regular files and directories: {}",
                        entry.path().display()
                    )));
                }
            }
        }
        self.transfer(paths, &work, false, TransferOptions::default())?;
        if !self.warnings.is_empty() {
            return Err(io::Error::other("Could not stage every selected item"));
        }
        let staging = TempDir::new_in(destination)?;
        let staged = staging.path().join(filename(output)?);
        if existing.is_some() {
            copy_stream(output, &File::create(&staged)?, &self.control, None)?;
        }
        self.update(output, 0);
        format.write(&content, &staged, &work, options, &self.control)?;
        if options.volume.is_empty() {
            publish(&staged, output, existing, &self.control)?;
        } else {
            let name = filename(output)?.to_string_lossy();
            let prefix = format!("{name}.");
            let mut parts = fs::read_dir(staging.path())?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.strip_prefix(&prefix).is_some_and(|suffix| {
                                !suffix.is_empty()
                                    && suffix.bytes().all(|digit| digit.is_ascii_digit())
                            })
                        })
                })
                .collect::<Vec<_>>();
            parts.sort();
            if parts.is_empty() {
                return Err(io::Error::other("Archive tool produced no volumes"));
            }
            let mut published = Vec::new();
            for part in parts {
                let target = destination.join(filename(&part)?);
                if let Err(error) = publish(&part, &target, None, &self.control) {
                    for path in published {
                        let _ = fs::remove_file(path);
                    }
                    return Err(error);
                }
                published.push(target);
            }
        }
        self.finish_item();
        Ok(format!("Packed {}", output.display()))
    }

    fn delete_members(&mut self, source: ArchiveSelection) -> io::Result<String> {
        if source.members.is_empty() {
            return Err(io::Error::other("Choose members to delete"));
        }
        let format = PackFormat::detect(&source.file)?;
        if !format.supports_update() || matches!(format, PackFormat::Rar) {
            return Err(io::Error::other(
                "This archive format cannot delete members",
            ));
        }
        let existing = existing_archive(&source.file, true)?
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let inventory = list(&source.file, "", &self.control)?;
        let (members, _) = select(inventory, &source, None, false)?;
        if members.is_empty() {
            return Err(io::Error::other(
                "No selected members are still in the archive",
            ));
        }
        let parent = crate::operations::parent(&source.file)?;
        let staging = TempDir::new_in(parent)?;
        let staged = staging.path().join(filename(&source.file)?);
        copy_stream(&source.file, &File::create(&staged)?, &self.control, None)?;
        let mut names = NamedTempFile::new_in(staging.path())?;
        for member in &members {
            writeln!(names, "{}", member.key)?;
        }
        let sink = tempfile::tempfile()?;
        let mut command = if let Some(tool) = external_archiver(&source.file) {
            tool.command("delete", &staged, Some(names.path()), None, "")?
        } else {
            let mut command = seven("d", "");
            let mut list = OsString::from("-i@");
            list.push(names.path());
            command
                .args(["-spd", "-ssc", "-scsUTF-8"])
                .arg(list)
                .arg("--")
                .arg(&staged);
            command
        };
        run(&mut command, &sink, &self.control, None)?;
        publish(&staged, &source.file, Some(existing), &self.control)?;
        Ok(format!("Deleted {} archive member(s)", members.len()))
    }
}

fn existing_archive(path: &Path, update: bool) -> io::Result<Option<fs::Metadata>> {
    match path.symlink_metadata() {
        Ok(metadata) if update && metadata.is_file() => Ok(Some(metadata)),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} already exists. Choose a new name or enable updating an existing archive.",
                path.display()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn publish(
    source: &Path,
    target: &Path,
    original: Option<fs::Metadata>,
    control: &Control,
) -> io::Result<()> {
    control.check()?;
    let temporary = TempPath::try_from_path(source)?;
    if let Some(original) = original {
        if existing_archive(target, true)?.is_none_or(|current| {
            FileRevision::new(&current) != FileRevision::new(&original)
                || current.permissions() != original.permissions()
        }) {
            return Err(io::Error::other(
                "The archive changed while this task was running",
            ));
        }
        fs::set_permissions(source, original.permissions())?;
        temporary.persist(target).map_err(|error| error.error)
    } else {
        temporary
            .persist_noclobber(target)
            .map_err(|error| error.error)
    }
}

enum PackFormat {
    Seven(&'static str),
    Tarball(&'static str),
    Zstandard(bool),
    Lzma(bool),
    Base64,
    External,
    Rar,
}

impl PackFormat {
    fn detect(path: &Path) -> io::Result<Self> {
        if external_archiver(path).is_some() {
            return Ok(Self::External);
        }
        let ext = extension(path);
        let tar = extension(Path::new(path.file_stem().unwrap_or_default())) == "tar";
        Ok(match ext.as_str() {
            "7z" => Self::Seven("7z"),
            "zip" | "zipx" | "jar" | "war" | "ear" | "epub" | "xpi" | "apk" | "docx" | "xlsx"
            | "pptx" | "odt" | "ods" | "odp" => Self::Seven("zip"),
            "tar" => Self::Seven("tar"),
            "tgz" => Self::Tarball("gzip"),
            "tbz" | "tbz2" => Self::Tarball("bzip2"),
            "txz" => Self::Tarball("xz"),
            "tzst" => Self::Zstandard(true),
            "gz" | "gzip" => {
                if tar {
                    Self::Tarball("gzip")
                } else {
                    Self::Seven("gzip")
                }
            }
            "bz2" | "bzip2" => {
                if tar {
                    Self::Tarball("bzip2")
                } else {
                    Self::Seven("bzip2")
                }
            }
            "xz" => {
                if tar {
                    Self::Tarball("xz")
                } else {
                    Self::Seven("xz")
                }
            }
            "zst" | "zstd" => Self::Zstandard(tar),
            "lzma" => Self::Lzma(tar),
            "b64" | "base64" => Self::Base64,
            "rar" => Self::Rar,
            _ => {
                return Err(io::Error::other(
                    "This archive format does not support packing. Choose ZIP, 7z, TAR, a supported compressed stream, or a configured archiver.",
                ));
            }
        })
    }

    fn supports_update(&self) -> bool {
        matches!(
            self,
            Self::Seven("zip" | "7z" | "tar") | Self::External | Self::Rar
        )
    }

    fn write(
        &self,
        contents: &Path,
        output: &Path,
        work: &Path,
        options: &PackOptions,
        control: &Control,
    ) -> io::Result<()> {
        let sink = tempfile::tempfile()?;
        let encryptable = matches!(self, Self::Seven("zip" | "7z") | Self::Rar | Self::External);
        if !encryptable && !options.password.is_empty() {
            return Err(io::Error::other("This format does not support encryption"));
        }
        if !options.volume.is_empty() && !matches!(self, Self::Seven("zip" | "7z")) {
            return Err(io::Error::other(
                "Multi-volume creation is available for ZIP and 7z",
            ));
        }
        let one_file = || -> io::Result<PathBuf> {
            let paths = fs::read_dir(work)?.collect::<io::Result<Vec<_>>>()?;
            if paths.len() != 1 || !paths[0].file_type()?.is_file() {
                return Err(io::Error::other("This format takes one regular file"));
            }
            Ok(paths[0].path())
        };
        let tar = || -> io::Result<PathBuf> {
            let archive = output.with_file_name("payload.tar");
            run(
                seven("a", "")
                    .current_dir(contents)
                    .arg("-ttar")
                    .arg("--")
                    .arg(&archive)
                    .arg("."),
                &sink,
                control,
                None,
            )?;
            Ok(archive)
        };
        let mut command = match self {
            Self::Base64 => {
                let target = File::create(output)?;
                return run(
                    Command::new("base64").arg(one_file()?),
                    &target,
                    control,
                    None,
                );
            }
            Self::Zstandard(tarball) => {
                let source = if *tarball { tar()? } else { one_file()? };
                return run(
                    Command::new("zstd")
                        .arg(format!("-{}", options.level.max(1)))
                        .args(["-c", "--"])
                        .arg(source),
                    &File::create(output)?,
                    control,
                    None,
                );
            }
            Self::Lzma(tarball) => {
                let source = if *tarball { tar()? } else { one_file()? };
                return run(
                    Command::new("xz")
                        .arg("--format=lzma")
                        .arg(format!("-{}", options.level.min(9)))
                        .args(["-c", "--"])
                        .arg(source),
                    &File::create(output)?,
                    control,
                    None,
                );
            }
            Self::External => external_archiver(output)
                .ok_or_else(|| io::Error::other("External archiver configuration is unavailable"))?
                .command("pack", output, Some(contents), None, &options.password)?,
            Self::Rar => {
                let mut command =
                    Command::new(std::env::var_os("DESHCMD_RAR").unwrap_or_else(|| "rar".into()));
                command
                    .arg("a")
                    .arg(format!("-m{}", options.level.min(9).div_ceil(2)))
                    .arg("-r");
                if !options.password.is_empty() {
                    command.arg(format!("-p{}", options.password));
                }
                command.current_dir(contents).arg("--").arg(output).arg(".");
                command
            }
            Self::Seven(format) | Self::Tarball(format) => {
                let mut command = seven("a", &options.password);
                command
                    .current_dir(contents)
                    .arg(format!("-t{format}"))
                    .arg(format!("-mx={}", options.level.min(9)));
                if !options.volume.is_empty() {
                    command.arg(format!("-v{}", options.volume));
                }
                command.arg("--").arg(output);
                match self {
                    Self::Tarball(_) => {
                        command.arg(tar()?);
                    }
                    Self::Seven("gzip" | "bzip2" | "xz") => {
                        command.arg(one_file()?);
                    }
                    _ => {
                        command.arg(".");
                    }
                }
                command
            }
        };
        run(&mut command, &sink, control, None)
    }
}

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ListingFormat {
    #[default]
    Technical,
    Json,
}

#[derive(serde::Deserialize)]
struct ExternalArchiver {
    extensions: Vec<String>,
    executable: PathBuf,
    #[serde(default)]
    listing: ListingFormat,
    commands: BTreeMap<String, Vec<String>>,
}

fn read_json_listing(text: &str) -> io::Result<Inventory> {
    #[derive(serde::Deserialize)]
    struct ListedMember {
        path: String,
        size: Option<u64>,
        #[serde(default)]
        folder: bool,
        #[serde(default)]
        link: bool,
        mode: Option<u32>,
    }
    let mut inventory = Inventory::default();
    for entry in serde_json::from_str::<Vec<ListedMember>>(text).map_err(io::Error::other)? {
        if let Some(path) = member_path(&entry.path).filter(|_| !entry.link) {
            inventory.members.push(Member {
                path,
                key: entry.path,
                size: entry.size,
                folder: entry.folder,
                mode: entry.mode.map(|mode| mode & 0o777),
                crc: None,
            });
        } else {
            inventory.skipped += 1;
        }
    }
    Ok(inventory)
}

fn external_archiver(path: &Path) -> Option<&'static ExternalArchiver> {
    configured_archiver(&extension(path))
}

fn configured_archiver(extension: &str) -> Option<&'static ExternalArchiver> {
    use std::sync::OnceLock;
    static TOOLS: OnceLock<Vec<ExternalArchiver>> = OnceLock::new();
    let tools = TOOLS.get_or_init(|| {
        std::env::var_os("DESHCMD_ARCHIVERS")
            .and_then(|path| fs::File::open(path).ok())
            .and_then(|file| serde_json::from_reader(file).ok())
            .unwrap_or_default()
    });
    tools.iter().find(|tool| {
        tool.extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    })
}

impl ExternalArchiver {
    fn command(
        &self,
        action: &str,
        file: &Path,
        input: Option<&Path>,
        member: Option<&str>,
        password: &str,
    ) -> io::Result<Command> {
        let args = self.commands.get(action).ok_or_else(|| {
            io::Error::other(format!("External archiver does not configure {action}"))
        })?;
        let mut command = Command::new(&self.executable);
        for arg in args {
            match arg.as_str() {
                "{archive}" => {
                    command.arg(file);
                }
                "{input}" => {
                    command.arg(
                        input.ok_or_else(|| io::Error::other("This action has no input path"))?,
                    );
                }
                "{member}" => {
                    command.arg(
                        member.ok_or_else(|| io::Error::other("This action has no member name"))?,
                    );
                }
                "{password}" => {
                    command.arg(password);
                }
                _ => {
                    command.arg(arg);
                }
            }
        }
        Ok(command)
    }
}

#[cfg(test)]
mod tests;
