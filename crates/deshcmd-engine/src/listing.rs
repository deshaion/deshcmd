use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Image,
    Audio,
    Video,
    Archive,
    Code,
    Text,
    Other,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "Folder",
            Self::Image => "Image",
            Self::Audio => "Audio",
            Self::Video => "Video",
            Self::Archive => "Archive",
            Self::Code => "Code",
            Self::Text => "Document",
            Self::Other => "File",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub extension: String,
    pub kind: Kind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub read_only: bool,
    pub link: bool,
    pub hidden: bool,
    pub comment: String,
    pub attributes: String,
}
impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Folder
    }

    fn from_path(path: PathBuf, comments: &HashMap<String, String>) -> io::Result<Self> {
        let metadata = path.symlink_metadata()?;
        Ok(Self::from_metadata(path, metadata, comments))
    }

    fn from_metadata(
        path: PathBuf,
        link_meta: Metadata,
        comments: &HashMap<String, String>,
    ) -> Self {
        let link = link_meta.is_symlink();
        let meta = if link {
            path.metadata().unwrap_or(link_meta)
        } else {
            link_meta
        };
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let extension = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let kind = if meta.is_dir() {
            Kind::Folder
        } else {
            match extension.as_str() {
                "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "tiff" | "tif"
                | "svg" | "heic" | "avif" => Kind::Image,
                "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "opus" => Kind::Audio,
                "mp4" | "mkv" | "avi" | "mov" | "webm" | "m4v" => Kind::Video,
                "zip" | "tar" | "gz" | "xz" | "rar" | "7z" | "bz2" | "zst" | "deb" | "rpm" => {
                    Kind::Archive
                }
                "rs" | "js" | "ts" | "jsx" | "tsx" | "py" | "html" | "css" | "cpp" | "c" | "h"
                | "sh" | "toml" | "json" | "yaml" | "yml" | "go" | "java" => Kind::Code,
                "txt" | "md" | "log" | "csv" | "tsv" | "pdf" | "rtf" | "docx" | "odt" | "xlsx" => {
                    Kind::Text
                }
                _ => Kind::Other,
            }
        };
        #[cfg(unix)]
        let attributes = {
            use std::os::unix::fs::PermissionsExt;
            format!("{:04o}", meta.permissions().mode() & 0o7777)
        };
        #[cfg(not(unix))]
        let attributes = if meta.permissions().readonly() {
            "Read-only".into()
        } else {
            "Writable".into()
        };
        #[cfg(windows)]
        let system_hidden = {
            use std::os::windows::fs::MetadataExt;
            meta.file_attributes() & 6 != 0
        };
        #[cfg(not(windows))]
        let system_hidden = false;
        Self {
            path,
            extension,
            kind,
            size: meta.len(),
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            accessed: meta.accessed().ok(),
            read_only: meta.permissions().readonly(),
            link,
            hidden: name.starts_with('.') || system_hidden,
            comment: comments.get(&name).cloned().unwrap_or_default(),
            name,
            attributes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Location {
    Directory(PathBuf),
    Flat { base: PathBuf, roots: Vec<PathBuf> },
    List { base: PathBuf, paths: Vec<PathBuf> },
}
impl Location {
    pub fn base(&self) -> &Path {
        match self {
            Self::Directory(path)
            | Self::Flat { base: path, .. }
            | Self::List { base: path, .. } => path,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum View {
    #[default]
    Details,
    Compact,
    Gallery,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Column {
    #[default]
    Name,
    Size,
    Modified,
    Kind,
    Extension,
    Path,
    Attributes,
    Comment,
}
impl Column {
    pub const ALL: [Self; 8] = [
        Self::Name,
        Self::Size,
        Self::Modified,
        Self::Kind,
        Self::Extension,
        Self::Path,
        Self::Attributes,
        Self::Comment,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Modified => "Modified",
            Self::Kind => "Kind",
            Self::Extension => "Extension",
            Self::Path => "Path",
            Self::Attributes => "Permissions",
            Self::Comment => "Comment",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sort {
    pub column: Column,
    pub descending: bool,
}

#[derive(Clone, Debug, Default)]
pub struct MatchOptions {
    pub text: String,
    pub case_sensitive: bool,
    pub recursive: bool,
    pub ignore_accents: bool,
    pub extension_only: bool,
    pub directories: bool,
    pub files: bool,
    pub read_only: bool,
}

impl MatchOptions {
    pub fn filename(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ignore_accents: true,
            directories: true,
            files: true,
            ..Self::default()
        }
    }

    pub fn is_active(&self) -> bool {
        !self.text.is_empty()
            || self.recursive
            || !self.files
            || !self.directories
            || self.read_only
    }

    pub fn matching<'a>(
        &self,
        entries: impl Iterator<Item = &'a Entry>,
    ) -> io::Result<Vec<&'a Entry>> {
        let query = self.compile()?;
        Ok(entries.filter(|entry| query.matches(self, entry)).collect())
    }

    pub(crate) fn compile(&self) -> io::Result<MatchQuery> {
        let pattern = fold_case(&self.text, self.case_sensitive, self.ignore_accents).into_owned();
        let mask = if pattern.contains(['*', '?', '[', ';']) {
            compile_mask(&pattern, true)?
        } else {
            None
        };
        Ok(MatchQuery { pattern, mask })
    }
}

#[derive(Default)]
pub(crate) struct MatchQuery {
    pattern: String,
    mask: Option<globset::GlobSet>,
}

impl MatchQuery {
    pub(crate) fn matches(&self, options: &MatchOptions, entry: &Entry) -> bool {
        if (entry.is_dir() && !options.directories)
            || (!entry.is_dir() && !options.files)
            || (options.read_only && !entry.read_only)
        {
            return false;
        }
        if self.pattern.is_empty() {
            return true;
        }
        let value = fold_case(
            if options.extension_only {
                &entry.extension
            } else {
                &entry.name
            },
            options.case_sensitive,
            options.ignore_accents,
        );
        self.mask.as_ref().map_or_else(
            || value.contains(&self.pattern),
            |mask| mask.is_match(value.as_ref()),
        )
    }
}

pub(crate) fn fold(text: &str, case_sensitive: bool) -> Cow<'_, str> {
    if case_sensitive || (text.is_ascii() && !text.bytes().any(|byte| byte.is_ascii_uppercase())) {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.to_lowercase())
    }
}

fn fold_case(text: &str, case_sensitive: bool, ignore_accents: bool) -> Cow<'_, str> {
    if ignore_accents && !text.is_ascii() {
        let normalized: String = text.nfd().filter(|&c| !is_combining_mark(c)).collect();
        match fold(&normalized, case_sensitive) {
            Cow::Borrowed(_) => Cow::Owned(normalized),
            Cow::Owned(lowercase) => Cow::Owned(lowercase),
        }
    } else {
        fold(text, case_sensitive)
    }
}

pub(crate) fn compile_mask(
    text: &str,
    case_sensitive: bool,
) -> io::Result<Option<globset::GlobSet>> {
    let mut masks = text
        .split(';')
        .map(str::trim)
        .filter(|glob| !glob.is_empty())
        .peekable();
    if masks.peek().is_none() {
        return Ok(None);
    }
    let mut builder = globset::GlobSetBuilder::new();
    for mask in masks {
        builder.add(
            globset::GlobBuilder::new(if mask == "*.*" { "*" } else { mask })
                .case_insensitive(!case_sensitive)
                .backslash_escape(false)
                .build()
                .map_err(io::Error::other)?,
        );
    }
    builder.build().map(Some).map_err(io::Error::other)
}

pub(crate) struct Scan {
    pub entries: Vec<Entry>,
    pub skipped: usize,
}
impl Scan {
    fn push(&mut self, result: io::Result<Entry>) {
        match result {
            Ok(entry) => self.entries.push(entry),
            Err(_) => self.skipped += 1,
        }
    }
}

pub(crate) fn scan(location: &Location, recursive: bool) -> io::Result<Scan> {
    #[cfg(feature = "perf")]
    let mut timing = crate::perf::ScanTiming::new();
    let mut scan = Scan {
        entries: Vec::new(),
        skipped: 0,
    };

    if let Location::Directory(path) = location
        && !recursive
    {
        let directory = fs::read_dir(path)?;
        let mut comments = None;
        for item in directory {
            let item = match item {
                Ok(item) => item,
                Err(_) => {
                    scan.skipped += 1;
                    continue;
                }
            };
            let comments = comments.get_or_insert_with(|| {
                #[cfg(feature = "perf")]
                timing.comment_read();
                crate::operations::read_comments(path)
            });
            let load = || {
                // DirEntry metadata describes the link itself, as symlink_metadata does.
                let metadata = item.metadata()?;
                Ok(Entry::from_metadata(item.path(), metadata, comments))
            };
            #[cfg(feature = "perf")]
            let result = timing.entry(load);
            #[cfg(not(feature = "perf"))]
            let result = load();
            scan.push(result);
        }
        #[cfg(feature = "perf")]
        timing.finish(scan.entries.capacity());
        return Ok(scan);
    }

    let mut paths = Vec::new();
    match location {
        Location::Directory(path) => {
            for entry in walkdir::WalkDir::new(path).min_depth(1) {
                match entry {
                    Ok(entry) => paths.push(entry.into_path()),
                    Err(error) if error.depth() == 0 => return Err(error.into()),
                    Err(_) => scan.skipped += 1,
                }
            }
        }
        Location::Flat { roots, .. } => {
            let mut seen = HashSet::new();
            for root in roots {
                for entry in walkdir::WalkDir::new(root) {
                    match entry {
                        Ok(entry) if !entry.file_type().is_dir() => {
                            let path = entry.into_path();
                            if !seen.contains(path.as_path()) {
                                #[cfg(feature = "perf")]
                                timing.dedup_path();
                                seen.insert(path.clone());
                                paths.push(path);
                            }
                        }
                        Ok(_) => {}
                        Err(_) => scan.skipped += 1,
                    }
                }
            }
        }
        Location::List { paths: list, .. } => {
            let mut seen = HashSet::new();
            paths.extend(
                list.iter()
                    .filter(|path| seen.insert(path.as_path()))
                    .cloned(),
            );
        }
    }
    scan.entries.reserve_exact(paths.len());
    let mut cache = HashMap::new();
    for path in paths {
        let folder = path.parent().unwrap_or(location.base());
        let comments = if let Some(comments) = cache.get(folder) {
            comments
        } else {
            #[cfg(feature = "perf")]
            timing.comment_read();
            let key = folder.to_path_buf();
            let comments = crate::operations::read_comments(folder);
            cache.entry(key).or_insert(comments)
        };
        let load = || Entry::from_path(path, comments);
        #[cfg(feature = "perf")]
        let result = timing.entry(load);
        #[cfg(not(feature = "perf"))]
        let result = load();
        scan.push(result);
    }
    #[cfg(feature = "perf")]
    timing.finish(scan.entries.capacity());
    Ok(scan)
}

pub(crate) fn sort(entries: &mut [Entry], columns: &[Sort], dirs_first: bool, natural: bool) {
    #[cfg(feature = "perf")]
    let started = crate::perf::timestamp();
    let names: Vec<_> = entries
        .iter()
        .map(|entry| fold(&entry.name, false))
        .collect();
    let mut order: Vec<_> = (0..entries.len()).collect();
    order.sort_by(|&left, &right| {
        let (a, b) = (&entries[left], &entries[right]);
        let by_name = || compare_names(&names[left], &names[right], natural);
        if dirs_first && a.is_dir() != b.is_dir() {
            return b.is_dir().cmp(&a.is_dir());
        }
        for sort in columns {
            let order = match sort.column {
                Column::Name => by_name(),
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
        by_name().then_with(|| a.path.cmp(&b.path))
    });
    drop(names);
    // Each cycle moves entries into their final positions without a second row buffer.
    for start in 0..order.len() {
        let mut position = start;
        while order[position] != start {
            let next = order[position];
            entries.swap(position, next);
            order[position] = position;
            position = next;
        }
        order[position] = position;
    }
    #[cfg(feature = "perf")]
    crate::perf::elapsed("sort", started);
}

fn compare_names(a: &str, b: &str, natural: bool) -> Ordering {
    if !natural {
        return a.cmp(b);
    }
    let mut a = a.as_bytes();
    let mut b = b.as_bytes();
    while let (Some(x), Some(y)) = (a.first(), b.first()) {
        if x.is_ascii_digit() && y.is_ascii_digit() {
            let a_len = a.iter().take_while(|c| c.is_ascii_digit()).count();
            let b_len = b.iter().take_while(|c| c.is_ascii_digit()).count();
            let a_start = a[..a_len].iter().position(|&v| v != b'0').unwrap_or(a_len);
            let b_start = b[..b_len].iter().position(|&v| v != b'0').unwrap_or(b_len);
            let aa = &a[a_start..a_len];
            let bb = &b[b_start..b_len];
            let order = aa
                .len()
                .cmp(&bb.len())
                .then(aa.cmp(bb))
                .then(a_len.cmp(&b_len));
            if !order.is_eq() {
                return order;
            }
            a = &a[a_len..];
            b = &b[b_len..];
        } else {
            let order = x.cmp(y);
            if !order.is_eq() {
                return order;
            }
            a = &a[1..];
            b = &b[1..];
        }
    }
    a.len().cmp(&b.len())
}

pub enum Preview {
    Image { size: [usize; 2], rgba: Vec<u8> },
    Text(String),
    Unavailable,
}
pub fn preview(path: &Path, kind: Kind, pixels: u32) -> Preview {
    if kind == Kind::Image {
        let result = (|| {
            let mut reader = image::ImageReader::open(path)
                .ok()?
                .with_guessed_format()
                .ok()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(12_000);
            limits.max_image_height = Some(12_000);
            limits.max_alloc = Some(128 * 1024 * 1024);
            reader.limits(limits);
            let image = reader.decode().ok()?.thumbnail(pixels, pixels).into_rgba8();
            Some(Preview::Image {
                size: [image.width() as usize, image.height() as usize],
                rgba: image.into_raw(),
            })
        })();
        return result.unwrap_or(Preview::Unavailable);
    }
    let Ok(meta) = path.metadata() else {
        return Preview::Unavailable;
    };
    if !meta.is_file() {
        return Preview::Unavailable;
    }
    let mut bytes = Vec::new();
    if File::open(path)
        .and_then(|file| file.take(24 * 1024).read_to_end(&mut bytes))
        .is_err()
    {
        return Preview::Unavailable;
    }
    if bytes.contains(&0) {
        return Preview::Unavailable;
    }
    match String::from_utf8(bytes) {
        Ok(text) => Preview::Text(text),
        Err(error) if error.utf8_error().error_len().is_none() => {
            let end = error.utf8_error().valid_up_to();
            Preview::Text(String::from_utf8_lossy(&error.as_bytes()[..end]).to_string())
        }
        Err(_) => Preview::Unavailable,
    }
}

#[cfg(test)]
mod tests;
