use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::{self, File};
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
        let link_meta = path.symlink_metadata()?;
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
        Ok(Self {
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
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
        let normalize = |text: &str| fold_case(text, self.case_sensitive, self.ignore_accents);
        let pattern = normalize(&self.text);
        let mask = if pattern.contains(['*', '?', '[', ';']) {
            compile_mask(&pattern, true)?
        } else {
            None
        };
        Ok(entries
            .filter(|entry| {
                if (entry.is_dir() && !self.directories)
                    || (!entry.is_dir() && !self.files)
                    || (self.read_only && !entry.read_only)
                {
                    return false;
                }
                let value = normalize(if self.extension_only {
                    &entry.extension
                } else {
                    &entry.name
                });
                mask.as_ref()
                    .map_or_else(|| value.contains(&pattern), |mask| mask.is_match(&value))
            })
            .collect())
    }
}

pub(crate) fn fold(text: &str, case_sensitive: bool) -> String {
    fold_case(text, case_sensitive, false)
}
fn fold_case(text: &str, case_sensitive: bool, ignore_accents: bool) -> String {
    let text = if ignore_accents {
        text.nfd().filter(|c| !is_combining_mark(*c)).collect()
    } else {
        text.to_owned()
    };
    if case_sensitive {
        text
    } else {
        text.to_lowercase()
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
pub(crate) fn scan(location: &Location, recursive: bool) -> io::Result<Scan> {
    let mut paths = Vec::new();
    let mut skipped = 0;
    let mut seen = std::collections::HashSet::new();
    match location {
        Location::Directory(path) if recursive => {
            for entry in walkdir::WalkDir::new(path).min_depth(1) {
                match entry {
                    Ok(entry) => paths.push(entry.into_path()),
                    Err(error) if error.depth() == 0 => return Err(error.into()),
                    Err(_) => skipped += 1,
                }
            }
        }
        Location::Directory(path) => {
            for entry in fs::read_dir(path)? {
                match entry {
                    Ok(entry) => paths.push(entry.path()),
                    Err(_) => skipped += 1,
                }
            }
        }
        Location::Flat { roots, .. } => {
            for root in roots {
                for entry in walkdir::WalkDir::new(root) {
                    match entry {
                        Ok(entry) if !entry.file_type().is_dir() => paths.push(entry.into_path()),
                        Ok(_) => {}
                        Err(_) => skipped += 1,
                    }
                }
            }
        }
        Location::List { paths: list, .. } => paths.extend(list.iter().cloned()),
    };
    let mut entries = Vec::with_capacity(paths.len());
    let mut cache = HashMap::new();
    for path in paths {
        if !seen.insert(path.clone()) {
            continue;
        }
        let folder = path.parent().unwrap_or(location.base());
        let comments = cache
            .entry(folder.to_path_buf())
            .or_insert_with(|| crate::operations::read_comments(folder));
        match Entry::from_path(path, comments) {
            Ok(entry) => entries.push(entry),
            Err(_) => skipped += 1,
        }
    }
    Ok(Scan { entries, skipped })
}

pub(crate) fn sort(entries: &mut [Entry], columns: &[Sort], dirs_first: bool, natural: bool) {
    entries.sort_by(|a, b| {
        if dirs_first && a.is_dir() != b.is_dir() {
            return b.is_dir().cmp(&a.is_dir());
        }
        for sort in columns {
            let order = match sort.column {
                Column::Name => compare_names(&a.name, &b.name, natural),
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
        compare_names(&a.name, &b.name, natural).then(a.path.cmp(&b.path))
    });
}

fn compare_names(a: &str, b: &str, natural: bool) -> Ordering {
    let left = a.to_lowercase();
    let right = b.to_lowercase();
    if !natural {
        return left.cmp(&right);
    }
    let mut a = left.as_bytes();
    let mut b = right.as_bytes();
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
