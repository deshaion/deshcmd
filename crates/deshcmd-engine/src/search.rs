mod replacement_metadata;

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::{Local, NaiveDateTime, NaiveTime};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::archives::{ArchiveCache, SearchArchive};
use crate::listing::{compile_mask, open_regular_file};
use crate::read_pool::Cancellation;

const CONTENT_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchMode {
    #[default]
    Contains,
    Exact,
    Mask,
    Regex,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub name: String,
    pub name_mode: MatchMode,
    pub name_case: bool,
    pub exclude_files: String,
    pub exclude_folders: String,
    pub depth: Option<usize>,
    pub follow_links: bool,
    pub archives: bool,
    pub text: String,
    pub text_case: bool,
    pub text_regex: bool,
    pub text_hex: bool,
    pub text_absent: bool,
    pub office_xml: bool,
    pub encoding: String,
    pub replacement: Option<String>,
    pub size_from: String,
    pub size_to: String,
    pub size_unit: u64,
    pub modified_from: String,
    pub modified_to: String,
    pub time_from: String,
    pub time_to: String,
    pub age_days: String,
    pub attributes: Attributes,
    pub duplicates: Duplicates,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Attributes {
    #[default]
    Any,
    ReadOnly,
    Writable,
    Hidden,
    Executable,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Duplicates {
    pub name: bool,
    pub size: bool,
    pub hash: bool,
    pub content: bool,
}

impl Duplicates {
    fn enabled(self) -> bool {
        self.name || self.size || self.hash || self.content
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Hit {
    pub path: PathBuf,
    pub member: Option<PathBuf>,
}

impl Hit {
    pub fn local(path: PathBuf) -> Self {
        Self { path, member: None }
    }

    pub fn display_path(&self) -> PathBuf {
        self.member
            .as_ref()
            .map_or_else(|| self.path.clone(), |member| self.path.join(member))
    }
}

#[derive(Default)]
pub struct Results {
    pub hits: Vec<(Hit, Option<usize>)>,
    pub skipped: usize,
    pub replacements: BTreeMap<PathBuf, Replacement>,
}

#[derive(Clone, Debug)]
pub struct Replacement {
    pub path: PathBuf,
    digest: blake3::Hash,
}

enum Name {
    Any,
    Regex(Regex),
    Mask(globset::GlobSet),
}

impl Name {
    fn compile(options: &Options) -> io::Result<Self> {
        if options.name.is_empty() {
            return Ok(Self::Any);
        }
        if options.name_mode == MatchMode::Mask {
            return Ok(
                compile_mask(&options.name, options.name_case)?.map_or(Self::Any, Self::Mask)
            );
        }
        let pattern = match options.name_mode {
            MatchMode::Contains => regex::escape(&options.name),
            MatchMode::Exact => format!(r"\A{}\z", regex::escape(&options.name)),
            MatchMode::Regex => options.name.clone(),
            MatchMode::Mask => unreachable!(),
        };
        Ok(Self::Regex(compile_regex(&pattern, options.name_case)?))
    }

    fn matches(&self, name: &Path) -> bool {
        match self {
            Self::Any => true,
            Self::Regex(regex) => regex.is_match(&name.to_string_lossy()),
            Self::Mask(mask) => mask.is_match(name),
        }
    }
}

fn compile_regex(pattern: &str, case: bool) -> io::Result<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(!case)
        .build()
        .map_err(io::Error::other)
}

fn hex(text: &str) -> io::Result<Vec<u8>> {
    let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if !digits.len().is_multiple_of(2) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::other("Enter pairs of hexadecimal digits"));
    }
    (0..digits.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&digits[index..index + 2], 16).map_err(io::Error::other))
        .collect()
}

enum Content {
    Text(Regex, Option<&'static encoding_rs::Encoding>),
    Bytes(Vec<u8>),
}

impl Content {
    fn compile(options: &Options) -> io::Result<Option<Self>> {
        if let Some(value) = &options.replacement {
            if options.text.is_empty() || options.text_absent {
                return Err(io::Error::other(
                    "Replacement requires a positive content search",
                ));
            }
            if options.text_hex {
                hex(value)?;
            }
        }
        if options.text.is_empty() {
            return Ok(None);
        }
        if options.text_hex {
            let pattern = hex(&options.text)?;
            if pattern.is_empty() {
                return Err(io::Error::other("Enter a hexadecimal search pattern"));
            }
            return Ok(Some(Self::Bytes(pattern)));
        }
        let pattern = if options.text_regex {
            options.text.clone()
        } else {
            regex::escape(&options.text)
        };
        let encoding = if options.encoding.is_empty() {
            None
        } else {
            Some(
                encoding_rs::Encoding::for_label(options.encoding.as_bytes())
                    .ok_or_else(|| io::Error::other("Unknown text encoding"))?,
            )
        };
        Ok(Some(Self::Text(
            compile_regex(&pattern, options.text_case)?,
            encoding,
        )))
    }

    fn matches(&self, bytes: &[u8]) -> io::Result<bool> {
        match self {
            Self::Bytes(pattern) => Ok(bytes.windows(pattern.len()).any(|chunk| chunk == pattern)),
            Self::Text(regex, encoding) => {
                let (encoding, bom) = text_encoding(bytes, *encoding);
                let (text, errors) = encoding.decode_without_bom_handling(&bytes[bom..]);
                if errors {
                    return Err(io::Error::other(
                        "File cannot be decoded using the selected encoding",
                    ));
                }
                Ok(regex.is_match(&text))
            }
        }
    }
}

fn text_encoding(
    bytes: &[u8],
    requested: Option<&'static encoding_rs::Encoding>,
) -> (&'static encoding_rs::Encoding, usize) {
    let bom = encoding_rs::Encoding::for_bom(bytes);
    let encoding = requested
        .or_else(|| bom.map(|(encoding, _)| encoding))
        .unwrap_or(encoding_rs::UTF_8);
    let bom_length = bom
        .filter(|(detected, _)| *detected == encoding)
        .map_or(0, |(_, length)| length);
    (encoding, bom_length)
}

fn optional<T>(
    input: &str,
    label: &str,
    parse: impl FnOnce(&str) -> Option<T>,
) -> io::Result<Option<T>> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    parse(input)
        .map(Some)
        .ok_or_else(|| io::Error::other(format!("Invalid {label}")))
}

fn date(value: &str, end: bool) -> Option<NaiveDateTime> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| {
            if end {
                date.and_hms_nano_opt(23, 59, 59, 999_999_999)
            } else {
                date.and_hms_opt(0, 0, 0)
            }
        })
}

fn time(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M"))
        .ok()
}

struct Criteria {
    name: Name,
    exclude_files: Option<globset::GlobSet>,
    exclude_folders: Option<globset::GlobSet>,
    content: Option<Content>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    from: Option<NaiveDateTime>,
    to: Option<NaiveDateTime>,
    from_time: Option<NaiveTime>,
    to_time: Option<NaiveTime>,
    oldest: Option<SystemTime>,
}

impl Criteria {
    fn compile(options: &Options) -> io::Result<Self> {
        let size = |value: &str| {
            value
                .parse::<u64>()
                .ok()?
                .checked_mul(options.size_unit.max(1))
        };
        let min_size = optional(&options.size_from, "minimum size", size)?;
        let max_size = optional(&options.size_to, "maximum size", size)?;
        let from = optional(&options.modified_from, "start date (YYYY-MM-DD)", |value| {
            date(value, false)
        })?;
        let to = optional(&options.modified_to, "end date (YYYY-MM-DD)", |value| {
            date(value, true)
        })?;
        if min_size.zip(max_size).is_some_and(|(a, b)| a > b)
            || from.zip(to).is_some_and(|(a, b)| a > b)
        {
            return Err(io::Error::other("The lower bound exceeds the upper bound"));
        }
        Ok(Self {
            name: Name::compile(options)?,
            exclude_files: compile_mask(&options.exclude_files, options.name_case)?,
            exclude_folders: compile_mask(&options.exclude_folders, options.name_case)?,
            content: Content::compile(options)?,
            min_size,
            max_size,
            from,
            to,
            from_time: optional(&options.time_from, "start time (HH:MM[:SS])", time)?,
            to_time: optional(&options.time_to, "end time (HH:MM[:SS])", time)?,
            oldest: optional(&options.age_days, "maximum age in days", |value| {
                let seconds = value.parse::<u64>().ok()?.checked_mul(86_400)?;
                SystemTime::now().checked_sub(Duration::from_secs(seconds))
            })?,
        })
    }

    fn name_matches(&self, path: &Path) -> bool {
        path.file_name().is_some_and(|name| {
            let name = Path::new(name);
            self.name.matches(name)
                && !self
                    .exclude_files
                    .as_ref()
                    .is_some_and(|mask| mask.is_match(name))
        })
    }

    fn folder_excluded(&self, path: &Path) -> bool {
        path.file_name().is_some_and(|name| {
            self.exclude_folders
                .as_ref()
                .is_some_and(|mask| mask.is_match(name))
        })
    }

    fn metadata_matches(&self, size: Option<u64>, modified: Option<SystemTime>) -> bool {
        if self
            .min_size
            .is_some_and(|min| size.is_none_or(|size| size < min))
            || self
                .max_size
                .is_some_and(|max| size.is_none_or(|size| size > max))
            || self
                .oldest
                .is_some_and(|oldest| modified.is_none_or(|value| value < oldest))
        {
            return false;
        }
        if self.from.is_none()
            && self.to.is_none()
            && self.from_time.is_none()
            && self.to_time.is_none()
        {
            return true;
        }
        let Some(modified) = modified else {
            return false;
        };
        let local = chrono::DateTime::<Local>::from(modified).naive_local();
        if self.from.is_some_and(|from| local < from) || self.to.is_some_and(|to| local > to) {
            return false;
        }
        let time = local.time();
        match (self.from_time, self.to_time) {
            (Some(from), Some(to)) if from > to => time >= from || time <= to,
            (from, to) => from.is_none_or(|from| time >= from) && to.is_none_or(|to| time <= to),
        }
    }
}

fn attributes_match(
    filter: Attributes,
    path: &Path,
    metadata: Option<&Metadata>,
    mode: Option<u32>,
) -> bool {
    let hidden = path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'));
    #[cfg(windows)]
    let hidden = {
        use std::os::windows::fs::MetadataExt;
        hidden || metadata.is_some_and(|metadata| metadata.file_attributes() & 2 != 0)
    };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        mode.or_else(|| metadata.map(|metadata| metadata.permissions().mode()))
    };
    let read_only = metadata
        .map(|value| value.permissions().readonly())
        .or_else(|| mode.map(|mode| mode & 0o222 == 0));
    let executable = mode.is_some_and(|mode| mode & 0o111 != 0);
    #[cfg(windows)]
    let executable = executable
        || (metadata.is_some_and(Metadata::is_file)
            && path.extension().is_some_and(|extension| {
                ["exe", "com", "bat", "cmd"]
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            }));
    match filter {
        Attributes::Any => true,
        Attributes::ReadOnly => read_only == Some(true),
        Attributes::Writable => read_only == Some(false),
        Attributes::Hidden => hidden,
        Attributes::Executable => executable,
    }
}

#[derive(Default)]
pub struct Archives(pub(crate) Arc<ArchiveCache>);

impl Archives {
    fn open(&self, path: &Path, cancel: &Cancellation) -> io::Result<SearchArchive> {
        SearchArchive::open(path, self.0.password(path).unwrap_or_default(), cancel)
    }
}

fn read_bytes(path: &Path, cancel: &Cancellation) -> io::Result<Vec<u8>> {
    cancel.check()?;
    read_content(&mut open_regular_file(path)?, || cancel.check())
}

fn read_content(file: &mut File, check: impl Fn() -> io::Result<()>) -> io::Result<Vec<u8>> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > CONTENT_LIMIT {
        return Err(io::Error::other(
            "File is not regular or exceeds the content search limit",
        ));
    }
    let mut bytes = Vec::new();
    let mut chunk = [0; 64 * 1024];
    loop {
        check()?;
        let count = file.read(&mut chunk)?;
        if count == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() as u64 > CONTENT_LIMIT {
            return Err(io::Error::other("File exceeds the content search limit"));
        }
    }
}

fn is_office(path: &Path) -> bool {
    matches!(
        path.extension()
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .as_deref(),
        Some("docx" | "docm" | "xlsx" | "xlsm" | "pptx" | "pptm" | "odt" | "ods" | "odp")
    )
}

fn office_text(path: &Path, archives: &Archives, cancel: &Cancellation) -> io::Result<Vec<u8>> {
    use quick_xml::{events::Event, name::ResolveResult};
    const ODF_TEXT: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:text:1.0";
    fn is_odf_text(namespace: ResolveResult<'_>) -> bool {
        matches!(namespace, ResolveResult::Bound(uri) if uri.as_ref() == ODF_TEXT)
    }

    let archive = archives.open(path, cancel)?;
    let mut text = String::new();
    for member in archive.files() {
        cancel.check()?;
        let member = member.path;
        let name = member.to_string_lossy().replace('\\', "/");
        if !(name == "content.xml"
            || name.ends_with(".xml")
                && (name.starts_with("word/")
                    || name.starts_with("xl/worksheets/")
                    || name == "xl/sharedStrings.xml"
                    || name.starts_with("ppt/slides/")))
        {
            continue;
        }
        let bytes = archive.read(member, CONTENT_LIMIT)?;
        let mut reader = quick_xml::NsReader::from_reader(bytes.as_slice());
        loop {
            cancel.check()?;
            match reader.read_event().map_err(io::Error::other)? {
                Event::Text(part) => {
                    text.push_str(&part.xml10_content().map_err(io::Error::other)?)
                }
                Event::GeneralRef(part) => {
                    if let Some(reference) = part.resolve_char_ref().map_err(io::Error::other)? {
                        text.push(reference);
                    } else if let Some(value) = quick_xml::escape::resolve_xml_entity(
                        &part.decode().map_err(io::Error::other)?,
                    ) {
                        text.push_str(value);
                    }
                }
                Event::Start(start) | Event::Empty(start) => {
                    let resolver = reader.resolver();
                    let (namespace, name) = resolver.resolve_element(start.name());
                    if is_odf_text(namespace) {
                        match name.as_ref() {
                            b"tab" => text.push('\t'),
                            b"line-break" => text.push('\n'),
                            b"s" => {
                                let mut count = 1;
                                for attribute in start.attributes() {
                                    let attribute = attribute.map_err(io::Error::other)?;
                                    let (namespace, name) =
                                        resolver.resolve_attribute(attribute.key);
                                    if is_odf_text(namespace) && name.as_ref() == b"c" {
                                        count = attribute
                                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                            .map_err(io::Error::other)?
                                            .parse::<usize>()
                                            .map_err(io::Error::other)?;
                                    }
                                }
                                if count > (CONTENT_LIMIT as usize).saturating_sub(text.len()) {
                                    return Err(io::Error::other(
                                        "Office document exceeds the content search limit",
                                    ));
                                }
                                text.extend(std::iter::repeat_n(' ', count));
                            }
                            _ => {}
                        }
                    }
                }
                Event::End(end) if matches!(end.local_name().as_ref(), b"p" | b"si" | b"row") => {
                    text.push('\n')
                }
                Event::Eof => break,
                _ => {}
            }
            if text.len() as u64 > CONTENT_LIMIT {
                return Err(io::Error::other(
                    "Office document exceeds the content search limit",
                ));
            }
        }
        text.push('\n');
    }
    Ok(text.into_bytes())
}

struct Candidate {
    hit: Hit,
    size: Option<u64>,
}

struct Scan<'a> {
    options: &'a Options,
    archives: &'a Archives,
    criteria: Criteria,
    cancel: &'a Cancellation,
    seen: HashSet<Hit>,
    candidates: Vec<Candidate>,
    replacements: BTreeMap<PathBuf, Replacement>,
    skipped: usize,
}

impl Scan<'_> {
    fn add(&mut self, hit: Hit, size: Option<u64>) {
        if self.seen.insert(hit.clone()) {
            self.candidates.push(Candidate { hit, size });
        }
    }

    fn content_matches(
        &mut self,
        read: impl FnOnce() -> io::Result<Vec<u8>>,
        office: bool,
    ) -> bool {
        let Some(content) = self.criteria.content.as_ref() else {
            return true;
        };
        let result = read().and_then(|bytes| {
            if office && let Content::Text(regex, _) = content {
                return Ok(regex.is_match(&String::from_utf8_lossy(&bytes)));
            }
            content.matches(&bytes)
        });
        match result {
            Ok(matches) => matches != self.options.text_absent,
            Err(_) => {
                self.skipped += 1;
                false
            }
        }
    }

    fn archive(&mut self, file: &Path, folder: &Path) -> io::Result<()> {
        let archive = self.archives.open(file, self.cancel)?;
        for entry in archive.files() {
            self.cancel.check()?;
            let member = entry.path;
            let Ok(relative) = member.strip_prefix(folder) else {
                continue;
            };
            let direct = relative.as_os_str().is_empty();
            let depth = relative.components().count().saturating_sub(1);
            if (!direct && self.options.depth.is_some_and(|max| depth > max))
                || relative
                    .ancestors()
                    .skip(1)
                    .any(|path| self.criteria.folder_excluded(path))
                || !self.criteria.name_matches(member)
                || !self.criteria.metadata_matches(entry.size, entry.modified)
                || !attributes_match(self.options.attributes, member, None, entry.mode)
            {
                continue;
            }
            if self.content_matches(|| archive.read(member, CONTENT_LIMIT), false) {
                self.add(
                    Hit {
                        path: file.to_owned(),
                        member: Some(member.to_owned()),
                    },
                    entry.size,
                );
            }
        }
        Ok(())
    }

    fn local(&mut self, root: &Path) -> io::Result<()> {
        let max_depth = self
            .options
            .depth
            .map_or(usize::MAX, |depth| depth.saturating_add(1));
        let mut walker = walkdir::WalkDir::new(root)
            .max_depth(max_depth)
            .follow_root_links(self.options.follow_links)
            .follow_links(self.options.follow_links)
            .into_iter();
        while let Some(item) = walker.next() {
            self.cancel.check()?;
            let Ok(entry) = item else {
                self.skipped += 1;
                continue;
            };
            let path = entry.path();
            if entry.file_type().is_dir() {
                if entry.depth() > 0 && self.criteria.folder_excluded(path) {
                    walker.skip_current_dir();
                }
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                self.skipped += 1;
                continue;
            };
            let office = self.options.office_xml
                && self.criteria.content.is_some()
                && !self.options.text_hex
                && is_office(path);
            let mut digest = None;
            let replaceable = self.options.replacement.is_some()
                && !is_office(path)
                && !crate::is_archive(path)
                && !metadata.permissions().readonly()
                && !entry.path_is_symlink();
            if self.criteria.name_matches(path)
                && self
                    .criteria
                    .metadata_matches(Some(metadata.len()), metadata.modified().ok())
                && attributes_match(self.options.attributes, path, Some(&metadata), None)
                && self.content_matches(
                    || {
                        if office {
                            office_text(path, self.archives, self.cancel)
                        } else {
                            let bytes = read_bytes(path, self.cancel)?;
                            if replaceable {
                                digest = Some(blake3::hash(&bytes));
                            }
                            Ok(bytes)
                        }
                    },
                    office,
                )
            {
                self.add(Hit::local(path.to_owned()), Some(metadata.len()));
                if let Some(digest) = digest {
                    self.replacements
                        .entry(path.to_owned())
                        .or_insert_with(|| Replacement {
                            path: path.to_owned(),
                            digest,
                        });
                }
            }
            if self.options.archives
                && crate::is_archive(path)
                && !office
                && self.archive(path, Path::new("")).is_err()
            {
                self.skipped += 1;
            }
        }
        self.cancel.check()
    }
}

fn fingerprint(hit: &Hit, archives: &Archives, cancel: &Cancellation) -> io::Result<blake3::Hash> {
    if let Some(member) = &hit.member {
        let bytes = archives
            .open(&hit.path, cancel)?
            .read(member, CONTENT_LIMIT)?;
        return Ok(blake3::hash(&bytes));
    }
    cancel.check()?;
    let mut source = open_regular_file(&hit.path)?;
    let mut hasher = blake3::Hasher::new();
    let mut data = [0; 64 * 1024];
    loop {
        cancel.check()?;
        let count = source.read(&mut data)?;
        if count == 0 {
            return Ok(hasher.finalize());
        }
        hasher.update(&data[..count]);
    }
}

fn same_content(a: &Hit, b: &Hit, archives: &Archives, cancel: &Cancellation) -> io::Result<bool> {
    fn open(hit: &Hit, archives: &Archives, cancel: &Cancellation) -> io::Result<Box<dyn Read>> {
        if let Some(member) = &hit.member {
            let bytes = archives
                .open(&hit.path, cancel)?
                .read(member, CONTENT_LIMIT)?;
            Ok(Box::new(io::Cursor::new(bytes)))
        } else {
            cancel.check()?;
            Ok(Box::new(open_regular_file(&hit.path)?))
        }
    }
    let (mut a, mut b) = (open(a, archives, cancel)?, open(b, archives, cancel)?);
    let (mut left, mut right) = ([0; 64 * 1024], [0; 64 * 1024]);
    loop {
        cancel.check()?;
        let n = io::copy(
            &mut a.by_ref().take(left.len() as u64),
            &mut left.as_mut_slice(),
        )? as usize;
        let m = io::copy(
            &mut b.by_ref().take(right.len() as u64),
            &mut right.as_mut_slice(),
        )? as usize;
        if n != m || left[..n] != right[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

pub fn validate(options: &Options) -> io::Result<()> {
    Criteria::compile(options).map(|_| ())
}

pub fn run(roots: &[Hit], options: &Options, cancel: &Cancellation) -> io::Result<Results> {
    run_with_archives(roots, options, &Archives::default(), cancel)
}

pub fn run_with_archives(
    roots: &[Hit],
    options: &Options,
    archives: &Archives,
    cancel: &Cancellation,
) -> io::Result<Results> {
    if roots.is_empty() {
        return Err(io::Error::other("Choose a search location"));
    }
    let mut scan = Scan {
        options,
        archives,
        criteria: Criteria::compile(options)?,
        cancel,
        seen: HashSet::new(),
        candidates: Vec::new(),
        replacements: BTreeMap::new(),
        skipped: 0,
    };
    for root in roots {
        cancel.check()?;
        if let Some(member) = &root.member {
            if scan.archive(&root.path, member).is_err() {
                scan.skipped += 1;
            }
        } else {
            scan.local(&root.path)?;
        }
    }
    let duplicates = options.duplicates;
    let mut results = Results {
        skipped: scan.skipped,
        replacements: scan.replacements,
        ..Default::default()
    };
    if !duplicates.enabled() {
        results.hits = scan
            .candidates
            .into_iter()
            .map(|candidate| (candidate.hit, None))
            .collect();
        results
            .hits
            .sort_by(|a, b| (&a.0.path, &a.0.member).cmp(&(&b.0.path, &b.0.member)));
        return Ok(results);
    }
    let mut groups = BTreeMap::<_, Vec<Hit>>::new();
    for candidate in scan.candidates {
        cancel.check()?;
        let hit = candidate.hit;
        let name = duplicates.name.then(|| {
            let filename = hit
                .member
                .as_deref()
                .unwrap_or(&hit.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            crate::listing::fold(&filename, options.name_case).into_owned()
        });
        let size = if duplicates.size {
            candidate.size
        } else {
            None
        };
        if duplicates.size && size.is_none() {
            results.skipped += 1;
            continue;
        }
        groups.entry((name, size)).or_default().push(hit);
    }
    let mut number = 0;
    for group in groups.into_values().filter(|group| group.len() > 1) {
        let mut hashes = BTreeMap::<_, Vec<Hit>>::new();
        for hit in group {
            cancel.check()?;
            let hash = if duplicates.hash || duplicates.content {
                match fingerprint(&hit, archives, cancel) {
                    Ok(hash) => Some(*hash.as_bytes()),
                    Err(_) => {
                        results.skipped += 1;
                        continue;
                    }
                }
            } else {
                None
            };
            hashes.entry(hash).or_default().push(hit);
        }
        for group in hashes.into_values().filter(|group| group.len() > 1) {
            let mut verified: Vec<Vec<Hit>> = Vec::new();
            if duplicates.content {
                'hits: for hit in group {
                    for partition in &mut verified {
                        match same_content(&partition[0], &hit, archives, cancel) {
                            Ok(true) => {
                                partition.push(hit);
                                continue 'hits;
                            }
                            Ok(false) => {}
                            Err(_) => {
                                cancel.check()?;
                                results.skipped += 1;
                                continue 'hits;
                            }
                        }
                    }
                    verified.push(vec![hit]);
                }
            } else {
                verified.push(group);
            }
            for partition in verified.into_iter().filter(|partition| partition.len() > 1) {
                number += 1;
                results
                    .hits
                    .extend(partition.into_iter().map(|hit| (hit, Some(number))));
            }
        }
    }
    cancel.check()?;
    Ok(results)
}

pub(crate) struct ReplaceContent {
    pattern: Content,
    value: String,
}

impl ReplaceContent {
    pub(crate) fn compile(options: &Options) -> io::Result<Self> {
        let pattern = Content::compile(options)?
            .ok_or_else(|| io::Error::other("Enter content to replace"))?;
        let value = options
            .replacement
            .clone()
            .ok_or_else(|| io::Error::other("Enter a replacement"))?;
        Ok(Self { pattern, value })
    }

    pub(crate) fn apply(
        &self,
        replacement: &Replacement,
        control: &crate::operations::Control,
    ) -> io::Result<()> {
        use crate::archives::FileRevision;

        let path = &replacement.path;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.permissions().readonly() {
            return Err(io::Error::other(
                "Replacement requires a writable regular file",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() > 1 {
                return Err(io::Error::other("Replacement would break hard links"));
            }
        }
        let revision = FileRevision::new(&metadata);
        control.check()?;
        let mut file = open_regular_file(path)?;
        if FileRevision::new(&file.metadata()?) != revision {
            return Err(io::Error::other("File changed while opening"));
        }
        let bytes = read_content(&mut file, || control.check())?;
        if blake3::hash(&bytes) != replacement.digest {
            return Err(io::Error::other("File changed since the search"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("No containing directory"))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        let mut written = 0;
        let mut write = |part: &[u8]| {
            control.check()?;
            written += part.len() as u64;
            if written > CONTENT_LIMIT {
                return Err(io::Error::other(
                    "Replacement output exceeds the 64 MiB limit",
                ));
            }
            temporary.write_all(part)
        };
        match &self.pattern {
            Content::Bytes(pattern) => {
                let replacement_bytes = hex(&self.value)?;
                let mut rest = bytes.as_slice();
                while let Some(index) = rest.windows(pattern.len()).position(|part| part == pattern)
                {
                    write(&rest[..index])?;
                    write(&replacement_bytes)?;
                    rest = &rest[index + pattern.len()..];
                }
                write(rest)?;
            }
            Content::Text(pattern, requested) => {
                let (encoding, bom) = text_encoding(&bytes, *requested);
                let (text, errors) = encoding.decode_without_bom_handling(&bytes[bom..]);
                if errors {
                    return Err(io::Error::other(
                        "File cannot be decoded using the selected encoding",
                    ));
                }
                write(&bytes[..bom])?;
                let mut encode = |part: &str| -> io::Result<()> {
                    if encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE {
                        let little = encoding == encoding_rs::UTF_16LE;
                        let mut buffer = Vec::with_capacity(8192);
                        for unit in part.encode_utf16() {
                            buffer.extend(if little {
                                unit.to_le_bytes()
                            } else {
                                unit.to_be_bytes()
                            });
                            if buffer.len() >= 8192 {
                                write(&buffer)?;
                                buffer.clear();
                            }
                        }
                        return write(&buffer);
                    }
                    let (encoded, _, errors) = encoding.encode(part);
                    if errors {
                        return Err(io::Error::other(
                            "Replacement cannot be represented in the selected encoding",
                        ));
                    }
                    write(&encoded)
                };
                let mut end = 0;
                for found in pattern.find_iter(&text) {
                    encode(&text[end..found.start()])?;
                    encode(&self.value)?;
                    end = found.end();
                }
                encode(&text[end..])?;
            }
        }
        replacement_metadata::preserve(&file, path, &temporary).map_err(|error| {
            io::Error::other(format!("Unable to preserve file access controls: {error}"))
        })?;
        drop(file);
        publish_replacement(temporary, path, &revision, control, File::sync_all)
    }
}

fn publish_replacement(
    temporary: tempfile::NamedTempFile,
    path: &Path,
    revision: &crate::archives::FileRevision,
    control: &crate::operations::Control,
    sync: impl FnOnce(&File) -> io::Result<()>,
) -> io::Result<()> {
    sync(temporary.as_file())?;
    control.check()?;
    let current = fs::symlink_metadata(path)?;
    if !current.is_file() || &crate::archives::FileRevision::new(&current) != revision {
        return Err(io::Error::other(
            "File changed while preparing the replacement",
        ));
    }
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_roots_with_recursive_masks_content_and_metadata() {
        let root = tempfile::tempdir().unwrap();
        let at = |path| root.path().join(path);
        fs::create_dir(at("include")).unwrap();
        fs::create_dir(at("ignored")).unwrap();
        fs::write(at("report.txt"), b"Report 123").unwrap();
        fs::write(at("include/deep.txt"), b"REPORT 456").unwrap();
        fs::write(at("ignored/hidden.txt"), b"Report 789").unwrap();
        fs::write(at("other.txt"), b"No digits").unwrap();
        fs::write(at("wrong.bak"), b"Report 123").unwrap();
        fs::write(at("broken.txt"), [0xff]).unwrap();
        let locations = [
            Hit::local(root.path().to_owned()),
            Hit::local(at("include")),
        ];
        let mut options = Options {
            name: "*.txt;*.bak".into(),
            name_mode: MatchMode::Mask,
            exclude_folders: "ignored".into(),
            exclude_files: "*.bak".into(),
            text: r"report \d+".into(),
            text_regex: true,
            size_from: "9".into(),
            ..Default::default()
        };
        let search =
            |options: &Options| run(&locations, options, &Cancellation::default()).unwrap();
        let paths = |results: Results| {
            results
                .hits
                .into_iter()
                .map(|(hit, _)| hit.path)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            paths(search(&options)),
            [at("include/deep.txt"), at("report.txt")]
        );
        options.depth = Some(0);
        assert_eq!(
            paths(run(&locations[..1], &options, &Cancellation::default()).unwrap()),
            [at("report.txt")]
        );
        options.size_from.clear();
        options.text_absent = true;
        let absent = search(&options);
        assert_eq!(absent.skipped, 1);
        assert_eq!(paths(absent), [at("other.txt")]);
        options.modified_from = "2099-01-01".into();
        assert!(search(&options).hits.is_empty());
    }

    #[test]
    fn groups_byte_identical_files_and_preserves_modified_files_on_replace() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.txt");
        let b = root.path().join("b.txt");
        let other = root.path().join("other.txt");
        let contents = [0xff, 0xfe, b'A', 0, b'B', 0];
        fs::write(&a, contents).unwrap();
        fs::write(&b, contents).unwrap();
        fs::write(&other, [0xff, 0xfe, b'A', 0, b'C', 0]).unwrap();
        let options = Options {
            text: "A.".into(),
            text_regex: true,
            replacement: Some("λ".into()),
            duplicates: Duplicates {
                content: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let results = run(
            &[Hit::local(root.path().to_owned())],
            &options,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(results.hits.len(), 2);
        assert_eq!(results.hits[0].1, results.hits[1].1);
        let replace = ReplaceContent::compile(&options).unwrap();
        let control = crate::operations::Control::default();
        replace.apply(&results.replacements[&a], &control).unwrap();
        assert_eq!(fs::read(a).unwrap(), [0xff, 0xfe, 0xbb, 0x03]);
        fs::write(&b, b"changed").unwrap();
        assert!(replace.apply(&results.replacements[&b], &control).is_err());
        assert_eq!(fs::read(b).unwrap(), b"changed");
    }

    #[test]
    fn searches_archive_members_and_office_xml() {
        let mut command = std::process::Command::new("7z");
        if command.output().is_err() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let at = |name| root.path().join(name);
        fs::create_dir(at("word")).unwrap();
        fs::write(at("word/document.xml"), r#"<w:document xmlns:w="urn:w"><w:p><w:t>Ready &amp; </w:t><w:t>waiting</w:t></w:p></w:document>"#).unwrap();
        assert!(
            command
                .current_dir(root.path())
                .args(["a", "-tzip", "fixture.docx", "word"])
                .output()
                .unwrap()
                .status
                .success()
        );

        let file = Hit::local(at("fixture.docx"));
        let options = Options {
            name: "*.xml".into(),
            name_mode: MatchMode::Mask,
            archives: true,
            text: "Ready &amp;".into(),
            ..Default::default()
        };
        let result = run(
            std::slice::from_ref(&file),
            &options,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(
            result.hits[0].0.member,
            Some(PathBuf::from("word").join("document.xml"))
        );

        let options = Options {
            office_xml: true,
            text: "Ready & waiting".into(),
            ..Default::default()
        };
        let result = run(
            std::slice::from_ref(&file),
            &options,
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(result.hits, [(file, None)]);
    }

    #[test]
    fn searches_opendocument_whitespace() {
        if std::process::Command::new("7z").output().is_err() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("fixture.odt");
        fs::write(root.path().join("content.xml"), r#"<o:document xmlns:o="urn:o" xmlns:t="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><t:p>one<t:s t:c="3"/>two<t:tab/>three<t:line-break/>four</t:p></o:document>"#).unwrap();
        assert!(
            std::process::Command::new("7z")
                .current_dir(root.path())
                .args(["a", "-tzip", "fixture.odt", "content.xml"])
                .output()
                .unwrap()
                .status
                .success()
        );
        let roots = [Hit::local(file)];
        let mut options = Options {
            office_xml: true,
            text: "one   two\tthree\nfour".into(),
            ..Default::default()
        };
        let cancel = Cancellation::default();
        assert_eq!(run(&roots, &options, &cancel).unwrap().hits.len(), 1);
        options.text_absent = true;
        assert!(run(&roots, &options, &cancel).unwrap().hits.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn replacement_preserves_file_acl() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("text");
        fs::write(&path, "before").unwrap();
        use std::os::unix::fs::MetadataExt;
        let uid = path.metadata().unwrap().uid();
        let status = std::process::Command::new("setfacl")
            .args(["-m", &format!("u:{uid}:r--")])
            .arg(&path)
            .status();
        if !status.is_ok_and(|status| status.success()) {
            return;
        }
        let acl = || {
            std::process::Command::new("getfacl")
                .arg("-cpn")
                .arg(&path)
                .output()
                .unwrap()
                .stdout
        };
        let original = acl();
        let options = Options {
            text: "before".into(),
            replacement: Some("after".into()),
            ..Default::default()
        };
        let results = run(
            &[Hit::local(path.clone())],
            &options,
            &Cancellation::default(),
        )
        .unwrap();
        ReplaceContent::compile(&options)
            .unwrap()
            .apply(&results.replacements[&path], &Default::default())
            .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "after");
        assert_eq!(acl(), original);
    }

    #[test]
    fn replacement_rechecks_after_flush() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("target");
        fs::write(&path, "original").unwrap();
        let revision = crate::archives::FileRevision::new(&path.metadata().unwrap());
        let mut staged = tempfile::NamedTempFile::new_in(root.path()).unwrap();
        staged.write_all(b"replacement").unwrap();
        let result = publish_replacement(staged, &path, &revision, &Default::default(), |file| {
            fs::write(&path, "concurrent edit")?;
            file.sync_all()
        });
        assert!(result.unwrap_err().to_string().contains("File changed"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "concurrent edit");
    }

    #[test]
    fn searches_unlocked_archive_headers_and_content() {
        if std::process::Command::new("7z").output().is_err() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("locked.7z");
        fs::write(root.path().join("one.txt"), "same searchable text").unwrap();
        fs::write(root.path().join("two.txt"), "same searchable text").unwrap();
        let output = std::process::Command::new("7z")
            .current_dir(root.path())
            .args([
                "a",
                "-pmock-password",
                "-mhe=on",
                "locked.7z",
                "one.txt",
                "two.txt",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let roots = [Hit {
            path: path.clone(),
            member: Some(PathBuf::new()),
        }];
        let options = Options {
            text: "searchable".into(),
            duplicates: Duplicates {
                content: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let cancel = Cancellation::default();
        assert!(run(&roots, &options, &cancel).unwrap().hits.is_empty());
        let archives = Archives::default();
        archives.0.set_password(&path, "mock-password".into());
        let result = run_with_archives(&roots, &options, &archives, &cancel).unwrap();
        assert_eq!(result.skipped, 0);
        assert_eq!(result.hits.len(), 2);
        assert_eq!(result.hits[0].1, result.hits[1].1);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn local_content_and_duplicate_reads_reject_fifos() {
        use std::os::unix::ffi::OsStrExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pipe");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a NUL-terminated pathname in the test's temporary directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let regular = root.path().join("file");
        fs::write(&regular, "data").unwrap();
        let cancel = Cancellation::default();
        let archives = Archives::default();
        assert!(read_bytes(&path, &cancel).is_err());
        let pipe = Hit::local(path);
        assert!(fingerprint(&pipe, &archives, &cancel).is_err());
        assert!(same_content(&Hit::local(regular), &pipe, &archives, &cancel).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_executable_filter_uses_local_extensions() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("tool.EXE"), "program").unwrap();
        fs::write(root.path().join("tool.txt"), "document").unwrap();
        let options = Options {
            attributes: Attributes::Executable,
            ..Default::default()
        };
        let results = run(
            &[Hit::local(root.path().to_owned())],
            &options,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(
            results.hits,
            [(Hit::local(root.path().join("tool.EXE")), None)]
        );
    }

    #[test]
    fn validates_and_cancels_search() {
        assert!(
            validate(&Options {
                name_mode: MatchMode::Regex,
                name: "(".into(),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate(&Options {
                text_hex: true,
                text: "1".into(),
                ..Default::default()
            })
            .is_err()
        );
        let cancel = Cancellation::default();
        cancel.cancel();
        assert_eq!(
            run(
                &[Hit::local(PathBuf::from("."))],
                &Options::default(),
                &cancel
            )
            .err()
            .unwrap()
            .kind(),
            io::ErrorKind::Interrupted
        );
    }
}
