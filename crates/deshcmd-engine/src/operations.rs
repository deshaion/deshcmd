use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use sha2::digest::DynDigest;
use tempfile::NamedTempFile;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Collision {
    #[default]
    Ask,
    Skip,
    Replace,
    Newer,
    Larger,
    Smaller,
    RenameIncoming,
    RenameExisting,
    Merge,
    Append,
    Resume,
    Cancel,
}

#[derive(Clone, Debug)]
pub struct Conflict {
    pub source: PathBuf,
    pub target: PathBuf,
    pub directory: bool,
    pub can_append: bool,
}

#[derive(Clone, Debug)]
pub struct TransferOptions {
    pub collision: Collision,
    pub directory_collision: Collision,
    pub verify: bool,
    pub check_space: bool,
    pub preallocate: bool,
    pub preserve_metadata: bool,
    pub follow_links: bool,
    pub adjust_links: bool,
    pub reflink: bool,
    pub omit_empty: bool,
    pub mask: String,
    pub rename_mask: String,
}
impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            collision: Collision::Ask,
            directory_collision: Collision::Ask,
            verify: false,
            check_space: true,
            preallocate: false,
            preserve_metadata: true,
            follow_links: false,
            adjust_links: false,
            reflink: false,
            omit_empty: false,
            mask: String::new(),
            rename_mask: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Algorithm {
    #[default]
    Blake3,
    Blake2b,
    Blake2bp,
    Blake2s,
    Blake2sp,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Sha3_224,
    Sha3_256,
    Sha3_384,
    Sha3_512,
    Md4,
    Md5,
    Ripemd128,
    Ripemd160,
    Tiger,
    Xxh128,
    Crc32,
    Sfv,
}
impl Algorithm {
    pub const ALL: [Self; 22] = [
        Self::Blake3,
        Self::Blake2b,
        Self::Blake2bp,
        Self::Blake2s,
        Self::Blake2sp,
        Self::Sha1,
        Self::Sha224,
        Self::Sha256,
        Self::Sha384,
        Self::Sha512,
        Self::Sha3_224,
        Self::Sha3_256,
        Self::Sha3_384,
        Self::Sha3_512,
        Self::Md4,
        Self::Md5,
        Self::Ripemd128,
        Self::Ripemd160,
        Self::Tiger,
        Self::Xxh128,
        Self::Crc32,
        Self::Sfv,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Blake3 => "BLAKE3",
            Self::Blake2b => "BLAKE2b",
            Self::Blake2bp => "BLAKE2bp",
            Self::Blake2s => "BLAKE2s",
            Self::Blake2sp => "BLAKE2sp",
            Self::Sha1 => "SHA-1",
            Self::Sha224 => "SHA-224",
            Self::Sha256 => "SHA-256",
            Self::Sha384 => "SHA-384",
            Self::Sha512 => "SHA-512",
            Self::Sha3_224 => "SHA3-224",
            Self::Sha3_256 => "SHA3-256",
            Self::Sha3_384 => "SHA3-384",
            Self::Sha3_512 => "SHA3-512",
            Self::Md4 => "MD4",
            Self::Md5 => "MD5",
            Self::Ripemd128 => "RIPEMD-128",
            Self::Ripemd160 => "RIPEMD-160",
            Self::Tiger => "Tiger/192",
            Self::Xxh128 => "XXH128",
            Self::Crc32 => "CRC32",
            Self::Sfv => "SFV (CRC32)",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::Blake3 => "blake3",
            Self::Blake2b => "blake2b",
            Self::Blake2bp => "blake2bp",
            Self::Blake2s => "blake2s",
            Self::Blake2sp => "blake2sp",
            Self::Sha1 => "sha1",
            Self::Sha224 => "sha224",
            Self::Sha256 => "sha256",
            Self::Sha384 => "sha384",
            Self::Sha512 => "sha512",
            Self::Sha3_224 => "sha3-224",
            Self::Sha3_256 => "sha3-256",
            Self::Sha3_384 => "sha3-384",
            Self::Sha3_512 => "sha3-512",
            Self::Md4 => "md4",
            Self::Md5 => "md5",
            Self::Ripemd128 => "ripemd128",
            Self::Ripemd160 => "ripemd160",
            Self::Tiger => "tiger",
            Self::Xxh128 => "xxh128",
            Self::Crc32 => "crc32",
            Self::Sfv => "sfv",
        }
    }
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        if extension == "sha" {
            return Some(Self::Sha1);
        }
        Self::ALL
            .into_iter()
            .find(|algo| extension == algo.extension())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ManifestMode {
    #[default]
    Single,
    PerFile,
    PerDirectory,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Transfer {
        paths: Vec<PathBuf>,
        destination: PathBuf,
        moving: bool,
        options: TransferOptions,
    },
    Rename {
        source: PathBuf,
        target: PathBuf,
    },
    Mkdir(PathBuf),
    Create(PathBuf),
    Delete {
        paths: Vec<PathBuf>,
        trash: bool,
    },
    Wipe {
        paths: Vec<PathBuf>,
        passes: u8,
    },
    Link {
        source: PathBuf,
        target: PathBuf,
        symbolic: bool,
        relative: bool,
    },
    Properties {
        paths: Vec<PathBuf>,
        read_only: Option<bool>,
        modified: Option<std::time::SystemTime>,
        recursive: bool,
    },
    Comment {
        path: PathBuf,
        text: String,
    },
    Statistics(Vec<PathBuf>),
    Split {
        source: PathBuf,
        destination: PathBuf,
        part_bytes: u64,
        part_count: Option<u64>,
    },
    Combine {
        paths: Vec<PathBuf>,
        output: PathBuf,
    },
    Hash {
        paths: Vec<PathBuf>,
        output: PathBuf,
        algorithm: Algorithm,
        mode: ManifestMode,
    },
    Verify {
        source: PathBuf,
        expected: Option<String>,
        algorithm: Algorithm,
    },
    Benchmark,
    Export {
        output: PathBuf,
        contents: String,
    },
}

impl Operation {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Transfer { moving: true, .. } => "Move",
            Self::Transfer { .. } => "Copy",
            Self::Rename { .. } => "Rename",
            Self::Mkdir(_) => "Create folder",
            Self::Create(_) => "Create file",
            Self::Delete { trash: true, .. } => "Move to Trash",
            Self::Delete { .. } => "Delete",
            Self::Wipe { .. } => "Wipe",
            Self::Link { .. } => "Create link",
            Self::Properties { .. } => "Change attributes",
            Self::Comment { .. } => "Edit comment",
            Self::Statistics(_) => "Calculate size",
            Self::Split { .. } => "Split file",
            Self::Combine { .. } => "Combine files",
            Self::Hash { .. } => "Calculate checksums",
            Self::Verify { .. } => "Verify checksums",
            Self::Benchmark => "Hash benchmark",
            Self::Export { .. } => "Export list",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub current: String,
    pub bytes: u64,
    pub total: Option<u64>,
    pub file_bytes: u64,
    pub file_total: u64,
    pub items: usize,
}

#[derive(Default)]
struct ControlState {
    paused: bool,
    cancelled: bool,
    answer: Option<(Collision, bool)>,
}

#[derive(Default)]
pub(crate) struct Control(Mutex<ControlState>, Condvar);

impl Control {
    pub fn pause(&self, pause: bool) {
        self.0.lock().unwrap().paused = pause;
        self.1.notify_all();
    }
    pub fn cancel(&self) {
        self.0.lock().unwrap().cancelled = true;
        self.1.notify_all();
    }
    pub fn answer(&self, choice: Collision, all: bool) {
        self.0.lock().unwrap().answer = Some((choice, all));
        self.1.notify_all();
    }
    fn check(&self) -> io::Result<()> {
        let state = self
            .1
            .wait_while(self.0.lock().unwrap(), |s| s.paused && !s.cancelled)
            .unwrap();
        if state.cancelled {
            Err(io::Error::other("Cancelled"))
        } else {
            Ok(())
        }
    }
    fn cancelled(&self) -> bool {
        self.0.lock().unwrap().cancelled
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled()
    }
    fn wait_answer(&self) -> io::Result<(Collision, bool)> {
        let mut state = self
            .1
            .wait_while(self.0.lock().unwrap(), |s| {
                s.answer.is_none() && !s.cancelled
            })
            .unwrap();
        if state.cancelled {
            return Err(io::Error::other("Cancelled"));
        }
        let answer = state.answer.take().unwrap();
        if answer.0 == Collision::Cancel {
            return Err(io::Error::other("Cancelled"));
        }
        Ok(answer)
    }
}

pub(crate) struct Worker<'a> {
    pub control: Arc<Control>,
    pub progress: Progress,
    pub report: &'a dyn Fn(Progress),
    pub conflict: &'a dyn Fn(Conflict),
    pub last_report: std::time::Instant,
    pub warnings: Vec<String>,
}

impl Worker<'_> {
    fn update(&mut self, path: &Path, total: u64) {
        self.progress.current = path.display().to_string();
        self.progress.file_bytes = 0;
        self.progress.file_total = total;
        self.emit();
    }

    fn emit(&mut self) {
        if self.last_report.elapsed().as_millis() >= 60 {
            (self.report)(self.progress.clone());
            self.last_report = std::time::Instant::now();
        }
    }

    fn add_bytes(&mut self, count: usize) {
        self.progress.bytes += count as u64;
        self.progress.file_bytes += count as u64;
        self.emit();
    }

    fn stream(&mut self, mut source: impl Read, mut target: impl Write) -> io::Result<()> {
        let mut buffer = vec![0; 256 * 1024];
        loop {
            self.control.check()?;
            let count = source.read(&mut buffer)?;
            if count == 0 {
                return Ok(());
            }
            target.write_all(&buffer[..count])?;
            self.add_bytes(count);
        }
    }

    fn finish_item(&mut self) {
        self.progress.items += 1;
        self.emit();
    }

    fn attempt(&mut self, path: &Path, result: io::Result<()>) -> io::Result<()> {
        if let Err(error) = result {
            if self.control.cancelled() || error.to_string() == "Cancelled" {
                return Err(error);
            }
            self.warnings.push(format!("{}: {error}", path.display()));
        }
        self.finish_item();
        Ok(())
    }

    pub(crate) fn run(&mut self, operation: Operation) -> io::Result<String> {
        self.control.check()?;
        let message = match operation {
            Operation::Transfer {
                paths,
                destination,
                moving,
                options,
            } => self.transfer(paths, &destination, moving, options)?,
            Operation::Rename { source, target } => {
                if source != target {
                    let case_alias =
                        source
                            .file_name()
                            .zip(target.file_name())
                            .is_some_and(|(a, b)| {
                                a.to_string_lossy()
                                    .eq_ignore_ascii_case(&b.to_string_lossy())
                            })
                            && !source.symlink_metadata()?.is_symlink()
                            && source
                                .canonicalize()
                                .ok()
                                .zip(target.canonicalize().ok())
                                .is_some_and(|(a, b)| a == b);
                    if exists(&target) && !case_alias {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "The name is already in use",
                        ));
                    }
                    if case_alias || !try_rename_noreplace(&source, &target)? {
                        fs::rename(source, target)?;
                    }
                }
                "Renamed item".into()
            }
            Operation::Mkdir(path) => {
                fs::create_dir_all(parent(&path)?)?;
                fs::create_dir(&path)?;
                format!("Created {}", name(&path))
            }
            Operation::Create(path) => {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                format!("Created {}", name(&path))
            }
            Operation::Delete { paths, trash } => {
                for path in paths {
                    self.control.check()?;
                    self.update(&path, 0);
                    let result = nonroot(&path).and_then(|()| {
                        if trash {
                            trash::delete(&path).map_err(io::Error::other)
                        } else {
                            remove(&path)
                        }
                    });
                    self.attempt(&path, result)?;
                }
                format!("Processed {} items", self.progress.items)
            }
            Operation::Wipe { paths, passes } => {
                if passes == 0 {
                    return Err(io::Error::other("Choose at least one overwrite pass"));
                }
                for path in paths {
                    self.control.check()?;
                    let result = nonroot(&path).and_then(|()| self.wipe(&path, passes));
                    self.attempt(&path, result)?;
                }
                format!("Processed {} items", self.progress.items)
            }
            Operation::Link {
                source,
                target,
                symbolic,
                relative,
            } => {
                if symbolic {
                    let points_to = if relative {
                        relative_to(parent(&target)?, &source)?
                    } else {
                        source.clone()
                    };
                    symlink(&points_to, &target, source.is_dir())?;
                } else {
                    fs::hard_link(source, &target)?;
                }
                format!("Created {}", name(&target))
            }
            Operation::Properties {
                paths,
                read_only,
                modified,
                recursive,
            } => {
                for path in paths {
                    if recursive && path.symlink_metadata()?.is_dir() {
                        for entry in walkdir::WalkDir::new(&path).contents_first(true) {
                            self.control.check()?;
                            match entry {
                                Ok(entry) => {
                                    let result = set_properties(entry.path(), read_only, modified);
                                    self.attempt(entry.path(), result)?;
                                }
                                Err(error) => self.warnings.push(error.to_string()),
                            }
                        }
                    } else {
                        self.control.check()?;
                        self.attempt(&path, set_properties(&path, read_only, modified))?;
                    }
                }
                format!("Processed {} items", self.progress.items)
            }
            Operation::Comment { path, text } => {
                save_comment(&path, &text)?;
                "Saved file comment".into()
            }
            Operation::Statistics(paths) => {
                let (mut files, mut dirs, mut bytes) = (0_u64, 0_u64, 0_u64);
                for path in paths {
                    for entry in walkdir::WalkDir::new(&path) {
                        self.control.check()?;
                        match entry {
                            Ok(entry) => match entry.metadata() {
                                Ok(meta) if meta.is_dir() => dirs += 1,
                                Ok(meta) => {
                                    files += 1;
                                    bytes = bytes.saturating_add(meta.len());
                                }
                                Err(error) => self.warnings.push(error.to_string()),
                            },
                            Err(error) => self.warnings.push(error.to_string()),
                        }
                    }
                }
                format!("{files} files, {dirs} folders · {bytes} bytes")
            }
            Operation::Split {
                source,
                destination,
                part_bytes,
                part_count,
            } => self.split(&source, &destination, part_bytes, part_count)?,
            Operation::Combine { paths, output } => self.combine(paths, &output)?,
            Operation::Hash {
                paths,
                output,
                algorithm,
                mode,
            } => self.manifest(paths, &output, algorithm, mode)?,
            Operation::Verify {
                source,
                expected,
                algorithm,
            } => self.verify(&source, expected.as_deref(), algorithm)?,
            Operation::Benchmark => self.benchmark()?,
            Operation::Export { output, contents } => {
                let mut temporary = NamedTempFile::new_in(parent(&output)?)?;
                temporary.write_all(contents.as_bytes())?;
                commit(temporary, &output, false)?;
                format!("Saved {}", name(&output))
            }
        };
        (self.report)(self.progress.clone());
        Ok(message)
    }
}

fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
fn parent(path: &Path) -> io::Result<&Path> {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("The path needs a parent directory"))
}
fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}
pub(crate) fn compare_files(source: &Path, target: &Path, control: &Control) -> io::Result<bool> {
    let mut source = File::open(source)?;
    let mut target = File::open(target)?;
    let source_meta = source.metadata()?;
    let target_meta = target.metadata()?;
    if !source_meta.is_file() || !target_meta.is_file() {
        return Err(io::Error::other("Comparison requires two regular files"));
    }
    if source_meta.len() != target_meta.len() {
        return Ok(false);
    }
    Ok(
        compare_prefix(&mut source, &mut target, source_meta.len(), control)?
            && same_metadata(&source_meta, &source.metadata()?)
            && same_metadata(&target_meta, &target.metadata()?),
    )
}

fn compare_prefix(
    source: &mut File,
    target: &mut File,
    mut remaining: u64,
    control: &Control,
) -> io::Result<bool> {
    let mut a = vec![0; 256 * 1024];
    let mut b = vec![0; 256 * 1024];
    while remaining > 0 {
        control.check()?;
        let count = remaining.min(a.len() as u64) as usize;
        source.read_exact(&mut a[..count])?;
        target.read_exact(&mut b[..count])?;
        if a[..count] != b[..count] {
            return Ok(false);
        }
        remaining -= count as u64;
    }
    Ok(true)
}

fn nonroot(path: &Path) -> io::Result<()> {
    let metadata = path.symlink_metadata()?;
    if metadata.is_dir() && path.canonicalize()?.parent().is_none() {
        return Err(io::Error::other("A filesystem root cannot be removed"));
    }
    Ok(())
}
fn remove(path: &Path) -> io::Result<()> {
    if path.symlink_metadata()?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        remove_file_or_link(path)
    }
}
#[cfg(windows)]
fn remove_file_or_link(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;
    if path.symlink_metadata()?.file_type().is_symlink_dir() {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}
#[cfg(not(windows))]
fn remove_file_or_link(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

fn commit(temporary: NamedTempFile, target: &Path, replace: bool) -> io::Result<()> {
    temporary.as_file().sync_all()?;
    if replace {
        temporary.persist(target).map_err(|e| e.error)?;
    } else {
        temporary.persist_noclobber(target).map_err(|e| e.error)?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(source: &Path, target: &Path, _directory: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}
#[cfg(windows)]
fn symlink(source: &Path, target: &Path, directory: bool) -> io::Result<()> {
    if directory {
        std::os::windows::fs::symlink_dir(source, target)
    } else {
        std::os::windows::fs::symlink_file(source, target)
    }
}
#[cfg(not(any(unix, windows)))]
fn symlink(_source: &Path, _target: &Path, _directory: bool) -> io::Result<()> {
    Err(io::Error::other("Symbolic links are unavailable"))
}

fn relative_to(from: &Path, to: &Path) -> io::Result<PathBuf> {
    let from = fs::canonicalize(from)?;
    let to = if let Some(file) = to.file_name() {
        fs::canonicalize(parent(to)?)?.join(file)
    } else {
        fs::canonicalize(to)?
    };
    let left: Vec<_> = from.components().collect();
    let right: Vec<_> = to.components().collect();
    let shared = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    if shared == 0 {
        return Ok(to);
    }
    let mut path = PathBuf::new();
    for _ in shared..left.len() {
        path.push("..");
    }
    path.extend(&right[shared..]);
    if path.as_os_str().is_empty() {
        path.push(".");
    }
    Ok(path)
}

fn set_properties(
    path: &Path,
    read_only: Option<bool>,
    modified: Option<std::time::SystemTime>,
) -> io::Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.is_symlink() {
        return Ok(());
    }
    if let Some(modified) = modified {
        filetime::set_file_mtime(path, filetime::FileTime::from_system_time(modified))?;
    }
    if let Some(read_only) = read_only {
        let mut permissions = meta.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = permissions.mode();
            permissions.set_mode(if read_only {
                mode & !0o222
            } else {
                mode | 0o200
            });
        }
        #[cfg(not(unix))]
        permissions.set_readonly(read_only);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

pub(crate) fn read_comments(folder: &Path) -> std::collections::HashMap<String, String> {
    fs::read_to_string(folder.join("descript.ion"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (name, text) = if let Some(rest) = line.strip_prefix('"') {
                rest.split_once("\" ")?
            } else {
                line.split_once(' ')?
            };
            Some((name.to_owned(), text.to_owned()))
        })
        .collect()
}

fn save_comment(path: &Path, text: &str) -> io::Result<()> {
    let folder = parent(path)?;
    let listing = folder.join("descript.ion");
    let mut comments: std::collections::BTreeMap<_, _> = match fs::read_to_string(&listing) {
        Ok(_) => read_comments(folder).into_iter().collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Default::default(),
        Err(error) => return Err(error),
    };
    let filename = name(path);
    if filename.contains(['"', '\n', '\r']) {
        return Err(io::Error::other(
            "This filename cannot be stored in descript.ion",
        ));
    }
    if text.trim().is_empty() {
        comments.remove(&filename);
    } else {
        comments.insert(filename, text.replace(['\r', '\n'], " "));
    }
    let mut temporary = NamedTempFile::new_in(folder)?;
    for (key, value) in comments {
        if key.contains(' ') {
            writeln!(temporary, "\"{key}\" {value}")?;
        } else {
            writeln!(temporary, "{key} {value}")?;
        }
    }
    commit(temporary, &listing, true)
}

struct Transfer<'a, 'worker> {
    options: TransferOptions,
    mask: Option<globset::GlobSet>,
    moving: bool,
    ancestors: Vec<PathBuf>,
    transferred: std::collections::HashSet<PathBuf>,
    worker: &'a mut Worker<'worker>,
}

impl Worker<'_> {
    fn transfer(
        &mut self,
        paths: Vec<PathBuf>,
        destination: &Path,
        moving: bool,
        options: TransferOptions,
    ) -> io::Result<String> {
        let destination = destination.canonicalize()?;
        if !destination.is_dir() {
            return Err(io::Error::other("Destination must be a directory"));
        }
        let mask = crate::listing::compile_mask(&options.mask, false)?;
        let bytes = paths.iter().try_fold(0_u64, |sum, path| {
            let mut total = sum;
            for entry in walkdir::WalkDir::new(path)
                .follow_links(options.follow_links)
                .into_iter()
                .flatten()
            {
                self.control.check()?;
                if let Ok(meta) = entry.metadata()
                    && meta.is_file()
                {
                    total = total.saturating_add(meta.len());
                }
            }
            Ok::<_, io::Error>(total)
        })?;
        self.progress.total = Some(if options.verify {
            bytes.saturating_mul(3)
        } else {
            bytes
        });
        let mut transfer = Transfer {
            options,
            mask,
            moving,
            ancestors: Vec::new(),
            transferred: Default::default(),
            worker: self,
        };
        let mut complete = 0;
        for path in paths {
            transfer.worker.control.check()?;
            let target =
                destination.join(rename_for_transfer(&path, &transfer.options.rename_mask)?);
            let result = (|| {
                let meta = path.symlink_metadata()?;
                if path == target && moving {
                    return Ok(false);
                }
                let same_parent = fs::canonicalize(parent(&path)?)? == destination;
                let same_entry = same_parent
                    && (path.file_name() == target.file_name()
                        || (!target
                            .symlink_metadata()
                            .is_ok_and(|meta| meta.is_symlink())
                            && path
                                .canonicalize()
                                .ok()
                                .zip(target.canonicalize().ok())
                                .is_some_and(|(a, b)| a == b)));
                if same_entry {
                    if moving {
                        return Ok(false);
                    }
                    return transfer.copy_tree(&path, &unique_path(&target), true);
                }
                if (meta.is_dir() || transfer.options.follow_links && path.is_dir())
                    && destination.starts_with(path.canonicalize()?)
                {
                    return Err(io::Error::other("Cannot transfer a folder into itself"));
                }
                if transfer.mask.is_none()
                    && !transfer.options.follow_links
                    && !transfer.options.adjust_links
                    && !transfer.options.verify
                    && !transfer.options.omit_empty
                    && transfer.moving
                    && !exists(&target)
                    && try_rename_noreplace(&path, &target)?
                {
                    return Ok(true);
                }
                let snapshot = if moving {
                    Some(move_snapshot(&path, &transfer.worker.control)?)
                } else {
                    None
                };
                transfer.transferred.clear();
                let copied = transfer.copy_tree(&path, &target, true)?;
                if let Some(snapshot) = snapshot
                    && !transfer.transferred.is_empty()
                {
                    remove_transferred(
                        &path,
                        snapshot,
                        &transfer.transferred,
                        &transfer.worker.control,
                    )?;
                }
                Ok(copied)
            })();
            match result {
                Ok(true) => {
                    complete += 1;
                    transfer.worker.finish_item();
                }
                Ok(false) => transfer.worker.finish_item(),
                Err(error) => transfer.worker.attempt(&path, Err(error))?,
            }
        }
        Ok(format!(
            "{} {complete} of {} items",
            if moving { "Moved" } else { "Copied" },
            transfer.worker.progress.items
        ))
    }

    fn wipe(&mut self, path: &Path, passes: u8) -> io::Result<()> {
        let meta = path.symlink_metadata()?;
        if meta.is_symlink() {
            return remove_file_or_link(path);
        }
        if meta.is_dir() {
            for entry in fs::read_dir(path)? {
                self.control.check()?;
                self.wipe(&entry?.path(), passes)?;
            }
            return fs::remove_dir(path);
        }
        if !meta.is_file() {
            return Err(io::Error::other("Wipe requires a regular file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() > 1 {
                return Err(io::Error::other("File has other hard links"));
            }
        }
        let mut file = OpenOptions::new().write(true).open(path)?;
        let mut buffer = vec![0; 256 * 1024];
        self.update(path, meta.len().saturating_mul(u64::from(passes)));
        for pass in 0..passes {
            file.rewind()?;
            let mut remaining = meta.len();
            while remaining > 0 {
                self.control.check()?;
                let size = remaining.min(buffer.len() as u64) as usize;
                match pass % 3 {
                    0 => buffer[..size].fill(0),
                    1 => buffer[..size].fill(255),
                    _ => getrandom::fill(&mut buffer[..size])
                        .map_err(|e| io::Error::other(e.to_string()))?,
                }
                file.write_all(&buffer[..size])?;
                self.add_bytes(size);
                remaining -= size as u64;
            }
            file.sync_all()?;
        }
        drop(file);
        fs::remove_file(path)
    }
}

impl Transfer<'_, '_> {
    fn resolve(
        &mut self,
        source: &Path,
        target: &Path,
        directory: bool,
        regular: bool,
    ) -> io::Result<(PathBuf, Collision)> {
        if !exists(target) {
            return Ok((target.to_path_buf(), Collision::Skip));
        }
        let target_meta = target.symlink_metadata()?;
        let can_append = regular && target_meta.is_file();
        let mut choice = if directory {
            self.options.directory_collision
        } else {
            self.options.collision
        };
        if choice == Collision::Ask {
            (self.worker.conflict)(Conflict {
                source: source.to_path_buf(),
                target: target.to_path_buf(),
                directory,
                can_append,
            });
            let (selected, all) = self.worker.control.wait_answer()?;
            choice = selected;
            if all {
                if directory {
                    self.options.directory_collision = choice;
                } else {
                    self.options.collision = choice;
                }
            }
        }
        let meta = source.metadata().or_else(|_| source.symlink_metadata())?;
        if choice == Collision::Newer {
            choice = if meta.modified()? > target_meta.modified()? {
                Collision::Replace
            } else {
                Collision::Skip
            };
        }
        if choice == Collision::Larger {
            choice = if meta.len() > target_meta.len() {
                Collision::Replace
            } else {
                Collision::Skip
            };
        }
        if choice == Collision::Smaller {
            choice = if meta.len() < target_meta.len() {
                Collision::Replace
            } else {
                Collision::Skip
            };
        }
        match choice {
            Collision::RenameIncoming => Ok((unique_path(target), Collision::Skip)),
            Collision::RenameExisting => {
                if !try_rename_noreplace(target, &unique_path(target))? {
                    return Err(io::Error::other(
                        "Renaming the existing file safely is unsupported on this filesystem",
                    ));
                }
                Ok((target.to_path_buf(), Collision::Skip))
            }
            Collision::Replace if directory || target_meta.is_dir() => Err(io::Error::other(
                "Use merge for two folders or choose a different destination name",
            )),
            Collision::Merge if !directory || !target_meta.is_dir() => {
                Err(io::Error::other("Only two folders can be merged"))
            }
            Collision::Append | Collision::Resume if !can_append => {
                Err(io::Error::other("Both items must be regular files"))
            }
            Collision::Append if self.options.verify => Err(io::Error::other(
                "Append cannot be combined with copy verification",
            )),
            Collision::Cancel => Err(io::Error::other("Cancelled")),
            other => Ok((target.to_path_buf(), other)),
        }
    }

    fn copy_tree(&mut self, source: &Path, target: &Path, top: bool) -> io::Result<bool> {
        self.worker.control.check()?;
        let link_meta = source.symlink_metadata()?;
        let source_link = link_meta.is_symlink();
        let meta = if self.options.follow_links && source_link {
            source.metadata()?
        } else {
            link_meta
        };
        if !meta.is_dir()
            && self.mask.as_ref().is_some_and(|mask| {
                !mask.is_match(crate::listing::fold(&name(source), false).as_ref())
            })
        {
            return Ok(false);
        }
        let (target, choice) = self.resolve(source, target, meta.is_dir(), meta.is_file())?;
        if choice == Collision::Skip && exists(&target) {
            return Ok(false);
        }
        if meta.is_dir() {
            let real_source = source.canonicalize()?;
            if self.ancestors.contains(&real_source) {
                return Err(io::Error::other("Symbolic link cycle"));
            }
            self.ancestors.push(real_source);
            let result = self.copy_dir(source, &target, &meta, top, choice == Collision::Merge);
            self.ancestors.pop();
            if self.moving && matches!(result, Ok(true)) {
                self.transferred.insert(source.to_path_buf());
            }
            return result;
        }
        if meta.is_symlink() {
            let mut link = fs::read_link(source)?;
            if self.options.adjust_links && link.is_relative() {
                link = relative_to(parent(&target)?, &parent(source)?.join(&link))?;
            }
            if choice == Collision::Replace {
                let staging = tempfile::tempdir_in(parent(&target)?)?;
                let staged = staging.path().join("link");
                symlink(&link, &staged, source.is_dir())?;
                fs::rename(staged, target)?;
            } else {
                symlink(&link, &target, source.is_dir())?;
            }
            if self.moving {
                self.transferred.insert(source.to_path_buf());
            }
            return Ok(true);
        }
        if !meta.is_file() {
            return Err(io::Error::other("Unsupported file type"));
        }
        self.copy_file(source, &target, &meta, choice)?;
        if self.moving {
            self.transferred.insert(source.to_path_buf());
        }
        Ok(true)
    }

    fn copy_dir(
        &mut self,
        source: &Path,
        target: &Path,
        meta: &fs::Metadata,
        top: bool,
        merging: bool,
    ) -> io::Result<bool> {
        let created = !merging;
        if created {
            fs::create_dir(target)?;
        } else if !target.symlink_metadata()?.is_dir() {
            return Err(io::Error::other(
                "Destination folder changed while preparing the transfer",
            ));
        }
        let mut complete = true;
        let mut copied = false;
        let result: io::Result<()> = (|| {
            for entry in fs::read_dir(source)? {
                self.worker.control.check()?;
                let entry = entry?;
                match self.copy_tree(&entry.path(), &target.join(entry.file_name()), false) {
                    Ok(true) => copied = true,
                    Ok(false) => complete = false,
                    Err(error) => {
                        if self.worker.control.cancelled() || error.to_string() == "Cancelled" {
                            return Err(error);
                        }
                        self.worker
                            .warnings
                            .push(format!("{}: {error}", entry.path().display()));
                        complete = false;
                    }
                }
            }
            Ok(())
        })();
        let empty = created && fs::read_dir(target)?.next().is_none();
        if empty && (self.options.omit_empty || self.mask.is_some() || result.is_err()) {
            fs::remove_dir(target)?;
            return result.map(|()| false);
        }
        result?;
        if self.options.preserve_metadata && created {
            preserve(target, meta)?;
        }
        if top && !complete && self.moving {
            self.worker.warnings.push(format!(
                "{} retained because some contents were skipped or could not be copied",
                source.display()
            ));
        }
        Ok(complete || (!self.moving && copied))
    }

    fn copy_file(
        &mut self,
        source: &Path,
        target: &Path,
        meta: &fs::Metadata,
        choice: Collision,
    ) -> io::Result<()> {
        self.worker.update(
            source,
            meta.len()
                .saturating_mul(if self.options.verify { 2 } else { 1 }),
        );
        let mut input = File::open(source)?;
        let mut staging = NamedTempFile::new_in(parent(target)?)?;
        let mut bytes = meta.len();
        if matches!(choice, Collision::Append | Collision::Resume) {
            let mut old = File::open(target)?;
            let old_size = old.metadata()?.len();
            if choice == Collision::Resume && old_size >= bytes {
                return Err(io::Error::other("The target must be smaller to resume"));
            }
            if choice == Collision::Append {
                bytes = bytes.saturating_add(old_size);
            }
            if choice == Collision::Resume {
                if !compare_prefix(&mut input, &mut old, old_size, &self.worker.control)? {
                    return Err(io::Error::other(
                        "The existing bytes differ from the start of the source file",
                    ));
                }
                old.rewind()?;
            }
            self.worker.stream(&mut old, &mut staging)?;
        }
        let cloned = self.options.reflink
            && !matches!(choice, Collision::Append | Collision::Resume)
            && try_clone(&input, staging.as_file());
        if cloned {
            self.worker.progress.bytes = self.worker.progress.bytes.saturating_add(bytes);
        } else {
            if self.options.check_space && fs2::available_space(parent(target)?)? < bytes {
                return Err(io::Error::other("Insufficient free space"));
            }
            if self.options.preallocate {
                fs2::FileExt::allocate(staging.as_file(), bytes)?;
            }
            self.worker.stream(&mut input, &mut staging)?;
            let length = staging.as_file_mut().stream_position()?;
            staging.as_file().set_len(length)?;
        }
        if self.options.verify {
            let actual = self.worker.digest(staging.path(), Algorithm::Blake3)?;
            let expected = self.worker.digest(source, Algorithm::Blake3)?;
            if actual != expected {
                return Err(io::Error::other("Copy verification failed"));
            }
        }
        if !same_metadata(meta, &source.metadata()?) {
            return Err(io::Error::other("Source changed while copying"));
        }
        if self.options.preserve_metadata {
            preserve(staging.path(), meta)?;
        }
        self.worker.control.check()?;
        commit(
            staging,
            target,
            matches!(
                choice,
                Collision::Replace | Collision::Append | Collision::Resume
            ),
        )
    }
}

struct MoveItem {
    path: PathBuf,
    metadata: fs::Metadata,
    link_target: Option<PathBuf>,
}

fn move_snapshot(root: &Path, control: &Control) -> io::Result<Vec<MoveItem>> {
    nonroot(root)?;
    let mut snapshot = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .contents_first(true)
        .follow_root_links(false)
    {
        control.check()?;
        let path = entry?.into_path();
        let metadata = path.symlink_metadata()?;
        let link_target = if metadata.is_symlink() {
            Some(fs::read_link(&path)?)
        } else {
            None
        };
        snapshot.push(MoveItem {
            path,
            metadata,
            link_target,
        });
    }
    Ok(snapshot)
}

fn remove_transferred(
    root: &Path,
    snapshot: Vec<MoveItem>,
    transferred: &std::collections::HashSet<PathBuf>,
    control: &Control,
) -> io::Result<()> {
    // Check the complete source tree before removing anything. Directory removal stays
    // non-recursive so a newly created child cannot become part of a move.
    let after = move_snapshot(root, control)?;
    let by_path: std::collections::HashMap<_, _> =
        after.iter().map(|item| (&item.path, item)).collect();
    if snapshot.len() != after.len()
        || snapshot.iter().any(|before| {
            by_path
                .get(&before.path)
                .is_none_or(|after| !same_move_item(before, after))
        })
    {
        return Err(io::Error::other(
            "Source changed during transfer. The original was retained.",
        ));
    }
    for item in snapshot
        .into_iter()
        .filter(|item| transferred.contains(&item.path))
    {
        control.check()?;
        if item.metadata.is_dir() {
            fs::remove_dir(&item.path)?;
        } else {
            let latest = item.path.symlink_metadata()?;
            let target = if latest.is_symlink() {
                Some(fs::read_link(&item.path)?)
            } else {
                None
            };
            if !same_metadata(&item.metadata, &latest) || item.link_target != target {
                return Err(io::Error::other("Source changed during cleanup"));
            }
            remove_file_or_link(&item.path)?;
        }
    }
    Ok(())
}

fn same_move_item(first: &MoveItem, second: &MoveItem) -> bool {
    same_metadata(&first.metadata, &second.metadata) && first.link_target == second.link_target
}

fn same_metadata(first: &fs::Metadata, second: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if first.ino() != second.ino() || first.dev() != second.dev() {
            return false;
        }
    }
    first.file_type() == second.file_type()
        && first.len() == second.len()
        && first.modified().ok() == second.modified().ok()
}

fn rename_for_transfer(path: &Path, mask: &str) -> io::Result<std::ffi::OsString> {
    let original = path
        .file_name()
        .ok_or_else(|| io::Error::other("Cannot copy a filesystem root"))?;
    if mask.is_empty() || mask == "*.*" {
        return Ok(original.to_owned());
    }
    let stem = path.file_stem().unwrap_or(original).to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    let mut result = match mask.rsplit_once('.') {
        Some((base, ext)) => {
            let extension = ext.replace('*', &extension);
            if extension.is_empty() {
                base.replace('*', &stem)
            } else {
                format!("{}.{}", base.replace('*', &stem), extension)
            }
        }
        None => mask.replace('*', &original.to_string_lossy()),
    };
    if result.contains('?') {
        result = result
            .chars()
            .enumerate()
            .filter_map(|(index, ch)| {
                if ch == '?' {
                    original.to_string_lossy().chars().nth(index)
                } else {
                    Some(ch)
                }
            })
            .collect();
    }
    if Path::new(&result).components().count() != 1
        || !matches!(
            Path::new(&result).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(io::Error::other(
            "Rename mask must produce a single valid filename",
        ));
    }
    Ok(result.into())
}

fn preserve(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    filetime::set_file_times(
        path,
        filetime::FileTime::from_last_access_time(meta),
        filetime::FileTime::from_last_modification_time(meta),
    )?;
    fs::set_permissions(path, meta.permissions())
}

fn unique_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|index| path.with_file_name(format!("{stem} ({index}){extension}")))
        .find(|candidate| !exists(candidate))
        .unwrap()
}

#[cfg(target_os = "linux")]
fn try_rename_noreplace(source: &Path, target: &Path) -> io::Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let target = std::ffi::CString::new(target.as_os_str().as_bytes())?;
    // The pointers refer to live, nul-terminated paths for the duration of the syscall.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EXDEV | libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP) => Ok(false),
        _ => Err(error),
    }
}
#[cfg(target_os = "macos")]
fn try_rename_noreplace(source: &Path, target: &Path) -> io::Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let target = std::ffi::CString::new(target.as_os_str().as_bytes())?;
    // Both C strings remain valid until the OS call returns.
    if unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EXDEV | libc::ENOTSUP | libc::EINVAL) => Ok(false),
        _ => Err(error),
    }
}
#[cfg(windows)]
fn try_rename_noreplace(source: &Path, target: &Path) -> io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain([0]).collect();
    if source[..source.len() - 1].contains(&0) || target[..target.len() - 1].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid file path",
        ));
    }
    // Flags of zero prevent both replacement and cross-volume copying.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } != 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_NOT_SAME_DEVICE as i32) {
        Ok(false)
    } else {
        Err(error)
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn try_rename_noreplace(_source: &Path, _target: &Path) -> io::Result<bool> {
    Ok(false)
}

#[cfg(target_os = "linux")]
fn try_clone(source: &File, target: &File) -> bool {
    use std::os::fd::AsRawFd;
    const FICLONE: libc::c_ulong = 0x4004_9409;
    // Both descriptors remain owned by their File values throughout the call.
    unsafe { libc::ioctl(target.as_raw_fd(), FICLONE as _, source.as_raw_fd()) == 0 }
}
#[cfg(not(target_os = "linux"))]
fn try_clone(_source: &File, _target: &File) -> bool {
    false
}

enum HashState {
    Blake3(Box<blake3::Hasher>),
    Blake2b(Box<blake2b_simd::State>),
    Blake2bp(Box<blake2b_simd::blake2bp::State>),
    Blake2s(Box<blake2s_simd::State>),
    Blake2sp(Box<blake2s_simd::blake2sp::State>),
    Digest(Box<dyn DynDigest>),
    Crc32(crc32fast::Hasher),
    Xxh128(Box<xxhash_rust::xxh3::Xxh3>),
}

impl HashState {
    fn new(algorithm: Algorithm) -> Self {
        match algorithm {
            Algorithm::Blake3 => Self::Blake3(Box::default()),
            Algorithm::Blake2b => Self::Blake2b(Box::new(blake2b_simd::State::new())),
            Algorithm::Blake2bp => Self::Blake2bp(Box::new(blake2b_simd::blake2bp::State::new())),
            Algorithm::Blake2s => Self::Blake2s(Box::new(blake2s_simd::State::new())),
            Algorithm::Blake2sp => Self::Blake2sp(Box::new(blake2s_simd::blake2sp::State::new())),
            Algorithm::Sha1 => Self::Digest(Box::<sha1::Sha1>::default()),
            Algorithm::Sha224 => Self::Digest(Box::<sha2::Sha224>::default()),
            Algorithm::Sha256 => Self::Digest(Box::<sha2::Sha256>::default()),
            Algorithm::Sha384 => Self::Digest(Box::<sha2::Sha384>::default()),
            Algorithm::Sha512 => Self::Digest(Box::<sha2::Sha512>::default()),
            Algorithm::Sha3_224 => Self::Digest(Box::<sha3::Sha3_224>::default()),
            Algorithm::Sha3_256 => Self::Digest(Box::<sha3::Sha3_256>::default()),
            Algorithm::Sha3_384 => Self::Digest(Box::<sha3::Sha3_384>::default()),
            Algorithm::Sha3_512 => Self::Digest(Box::<sha3::Sha3_512>::default()),
            Algorithm::Md4 => Self::Digest(Box::<md4::Md4>::default()),
            Algorithm::Md5 => Self::Digest(Box::<md5::Md5>::default()),
            Algorithm::Ripemd128 => Self::Digest(Box::<ripemd::Ripemd128>::default()),
            Algorithm::Ripemd160 => Self::Digest(Box::<ripemd::Ripemd160>::default()),
            Algorithm::Tiger => Self::Digest(Box::<tiger::Tiger>::default()),
            Algorithm::Xxh128 => Self::Xxh128(Box::default()),
            Algorithm::Crc32 | Algorithm::Sfv => Self::Crc32(Default::default()),
        }
    }
    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Blake3(state) => {
                state.update(bytes);
            }
            Self::Blake2b(state) => {
                state.update(bytes);
            }
            Self::Blake2bp(state) => {
                state.update(bytes);
            }
            Self::Blake2s(state) => {
                state.update(bytes);
            }
            Self::Blake2sp(state) => {
                state.update(bytes);
            }
            Self::Digest(state) => state.update(bytes),
            Self::Crc32(state) => state.update(bytes),
            Self::Xxh128(state) => state.update(bytes),
        }
    }
    fn finish(self) -> String {
        use std::fmt::Write as _;
        match self {
            Self::Blake3(state) => state.finalize().to_hex().to_string(),
            Self::Blake2b(state) => state.finalize().to_hex().to_string(),
            Self::Blake2bp(state) => state.finalize().to_hex().to_string(),
            Self::Blake2s(state) => state.finalize().to_hex().to_string(),
            Self::Blake2sp(state) => state.finalize().to_hex().to_string(),
            Self::Digest(state) => {
                let bytes = state.finalize();
                let mut result = String::with_capacity(bytes.len() * 2);
                for byte in bytes {
                    let _ = write!(result, "{byte:02x}");
                }
                result
            }
            Self::Crc32(state) => format!("{:08x}", state.finalize()),
            Self::Xxh128(state) => format!("{:032x}", state.digest128()),
        }
    }
}

impl Worker<'_> {
    fn split(
        &mut self,
        source: &Path,
        destination: &Path,
        part_bytes: u64,
        part_count: Option<u64>,
    ) -> io::Result<String> {
        let mut file = File::open(source)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(io::Error::other("Select a nonempty regular file"));
        }
        let size = metadata.len();
        let width = match part_count {
            Some(count) if count > 0 => size.div_ceil(count),
            Some(_) => return Err(io::Error::other("Part count must be positive")),
            None if part_bytes > 0 => part_bytes,
            None => return Err(io::Error::other("Part size must be positive")),
        };
        let count = size.div_ceil(width);
        if count > 10_000 {
            return Err(io::Error::other(
                "Choose a size that creates at most 10,000 parts",
            ));
        }
        let digits = count.to_string().len().max(3);
        let mut outputs = Vec::new();
        for part in 1..=count {
            let mut filename = source.file_name().unwrap_or_default().to_owned();
            filename.push(format!(".{part:0digits$}"));
            let target = destination.join(filename);
            if exists(&target) {
                return Err(io::Error::other(format!(
                    "{} already exists",
                    target.display()
                )));
            }
            outputs.push(target);
        }
        let staging = tempfile::tempdir_in(destination)?;
        self.progress.total = Some(size);
        self.update(source, size);
        for (index, _) in outputs.iter().enumerate() {
            let staged = staging.path().join(index.to_string());
            let mut output = File::create_new(staged)?;
            let before = self.progress.bytes;
            let expected = width.min(size - before);
            self.stream((&mut file).take(expected), &mut output)?;
            if self.progress.bytes - before != expected {
                return Err(io::Error::other("Source changed while splitting"));
            }
            output.sync_all()?;
        }
        let mut committed = Vec::new();
        for (index, target) in outputs.iter().enumerate() {
            let result = self
                .control
                .check()
                .and_then(|()| fs::hard_link(staging.path().join(index.to_string()), target));
            if let Err(error) = result {
                for path in committed {
                    let _ = fs::remove_file(path);
                }
                return Err(error);
            }
            committed.push(target);
        }
        Ok(format!("Created {count} parts"))
    }

    fn combine(&mut self, paths: Vec<PathBuf>, output: &Path) -> io::Result<String> {
        if paths.is_empty() {
            return Err(io::Error::other("Select file parts to combine"));
        }
        let total = paths.iter().try_fold(0_u64, |size, path| {
            let meta = path.metadata()?;
            if !meta.is_file() {
                return Err(io::Error::other("Select only regular file parts"));
            }
            Ok(size.saturating_add(meta.len()))
        })?;
        self.progress.total = Some(total);
        let mut staging = NamedTempFile::new_in(parent(output)?)?;
        for path in &paths {
            let file = File::open(path)?;
            self.update(path, file.metadata()?.len());
            self.stream(file, &mut staging)?;
            self.finish_item();
        }
        self.control.check()?;
        commit(staging, output, false)?;
        Ok(format!("Combined {} parts", paths.len()))
    }

    fn digest(&mut self, source: &Path, algorithm: Algorithm) -> io::Result<String> {
        let mut file = File::open(source)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::other("Checksums require regular files"));
        }
        self.update(source, meta.len());
        let mut state = HashState::new(algorithm);
        let mut data = vec![0; 256 * 1024];
        loop {
            self.control.check()?;
            let count = file.read(&mut data)?;
            if count == 0 {
                break;
            }
            state.update(&data[..count]);
            self.add_bytes(count);
        }
        Ok(state.finish())
    }

    fn manifest(
        &mut self,
        paths: Vec<PathBuf>,
        output: &Path,
        algorithm: Algorithm,
        mode: ManifestMode,
    ) -> io::Result<String> {
        let mut by_output: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
        let mut seen = std::collections::HashSet::new();
        let mut total = 0_u64;
        for path in paths {
            for entry in walkdir::WalkDir::new(&path).sort_by_file_name() {
                self.control.check()?;
                let entry = entry?;
                if !entry.file_type().is_file() || !seen.insert(entry.path().to_path_buf()) {
                    continue;
                }
                let path = entry.into_path();
                let manifest = match mode {
                    ManifestMode::Single => output.to_path_buf(),
                    ManifestMode::PerFile => {
                        let mut filename = path.file_name().unwrap_or_default().to_owned();
                        filename.push(format!(".{}", algorithm.extension()));
                        path.with_file_name(filename)
                    }
                    ManifestMode::PerDirectory => {
                        parent(&path)?.join(format!("checksums.{}", algorithm.extension()))
                    }
                };
                total = total.saturating_add(path.metadata()?.len());
                by_output.entry(manifest).or_default().push(path);
            }
        }
        if by_output.is_empty() {
            return Err(io::Error::other("No regular files found"));
        }
        if let Some(existing) = by_output.keys().find(|path| exists(path)) {
            return Err(io::Error::other(format!(
                "{} already exists",
                existing.display()
            )));
        }
        self.progress.total = Some(total);
        let count = by_output.len();
        for (manifest, paths) in by_output {
            let folder = parent(&manifest)?;
            let mut temporary = NamedTempFile::new_in(folder)?;
            for path in paths {
                let relative = relative_to(folder, &path)?;
                let path_text = relative
                    .to_str()
                    .ok_or_else(|| io::Error::other("Checksum manifests require UTF-8 paths"))?;
                if path_text.contains(['\r', '\n']) {
                    return Err(io::Error::other(
                        "Checksum manifests require single-line paths",
                    ));
                }
                let digest = self.digest(&path, algorithm)?;
                if algorithm == Algorithm::Sfv {
                    writeln!(temporary, "{path_text} {digest}")?;
                } else {
                    writeln!(temporary, "{digest}  {path_text}")?;
                }
                self.finish_item();
            }
            self.control.check()?;
            commit(temporary, &manifest, false)?;
        }
        Ok(format!(
            "Wrote {count} checksum manifest(s) for {} files",
            self.progress.items
        ))
    }

    fn verify(
        &mut self,
        source: &Path,
        expected: Option<&str>,
        algorithm: Algorithm,
    ) -> io::Result<String> {
        if let Some(expected) = expected {
            let digest = self.digest(source, algorithm)?;
            if !digest.eq_ignore_ascii_case(expected.trim()) {
                return Err(io::Error::other(format!(
                    "Checksum mismatch · actual {digest}"
                )));
            }
            return Ok(format!("Verified {} · {digest}", name(source)));
        }
        let folder = parent(source)?;
        let reader = BufReader::new(File::open(source)?);
        let mut matched = 0;
        let mut checked = 0;
        for line in reader.lines() {
            self.control.check()?;
            let line = line?;
            let line = line.trim_start_matches('\u{feff}');
            if line.trim().is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let parsed = if algorithm == Algorithm::Sfv
                || source
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("sfv"))
            {
                line.rsplit_once(char::is_whitespace)
                    .map(|(filename, digest)| (digest, filename.trim_end()))
            } else {
                line.split_once("  ").or_else(|| line.split_once(" *"))
            };
            let Some((digest, filename)) = parsed else {
                return Err(io::Error::other("Invalid checksum manifest entry"));
            };
            if digest.is_empty()
                || filename.is_empty()
                || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(io::Error::other("Invalid checksum manifest entry"));
            }
            checked += 1;
            let path = folder.join(filename);
            match self.digest(&path, algorithm) {
                Ok(actual) if actual.eq_ignore_ascii_case(digest) => matched += 1,
                Ok(_) => self
                    .warnings
                    .push(format!("{}: checksum mismatch", path.display())),
                Err(error) if self.control.cancelled() => return Err(error),
                Err(error) => self.warnings.push(format!("{}: {error}", path.display())),
            }
            self.finish_item();
        }
        if checked == 0 {
            return Err(io::Error::other("The checksum manifest is empty"));
        }
        Ok(format!("Verified {matched} of {checked} files"))
    }

    fn benchmark(&mut self) -> io::Result<String> {
        let data = vec![42; 1024 * 1024];
        let mut results = Vec::new();
        for algorithm in Algorithm::ALL
            .into_iter()
            .filter(|algorithm| *algorithm != Algorithm::Sfv)
        {
            let start = std::time::Instant::now();
            let mut hash = HashState::new(algorithm);
            for _ in 0..32 {
                self.control.check()?;
                hash.update(&data);
                self.add_bytes(data.len());
            }
            std::hint::black_box(hash.finish());
            results.push(format!(
                "{}  {:.0} MiB/s",
                algorithm.label(),
                32.0 / start.elapsed().as_secs_f64()
            ));
            self.finish_item();
        }
        Ok(results.join("\n"))
    }
}
