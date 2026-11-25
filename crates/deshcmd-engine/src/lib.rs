mod archives;
mod changes;
mod listing;
mod navigation;
mod operations;
#[cfg(feature = "perf")]
mod perf;
pub mod read_pool;
mod snapshots;
#[cfg(test)]
mod tests;
mod text_preview;
mod views;
mod watch;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub use archives::{
    ArchiveOperation, ArchiveSelection, ArchiveSource, ExtractLayout, ExtractOptions, PackOptions,
    is_archive, preview_member,
};
pub use listing::{
    Column, Entry, Kind, Location, MatchOptions, Preview, Sort, TEXT_PREVIEW_BYTES, View, preview,
};
pub use operations::{
    Algorithm, Collision, Conflict, ManifestMode, Operation, Progress, TransferOptions,
};
use operations::{Control, Worker};
pub use snapshots::ListingStats;
pub use text_preview::{TextPreviewReader, TextPreviewWindow};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    #[default]
    Left,
    Right,
}
impl Side {
    pub const ALL: [Self; 2] = [Self::Left, Self::Right];
    pub fn index(self) -> usize {
        usize::from(self == Self::Right)
    }
    pub fn other(self) -> Self {
        if self == Self::Left {
            Self::Right
        } else {
            Self::Left
        }
    }
    pub fn label(self) -> &'static str {
        if self == Self::Left { "Left" } else { "Right" }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabLock {
    #[default]
    Normal,
    Locked,
    Reset,
    NewTab,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedTab {
    pub location: Location,
    pub title: String,
    pub lock: TabLock,
    #[serde(default)]
    pub pinned: bool,
    pub home: PathBuf,
    pub view: View,
    pub sort: Vec<Sort>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedPane {
    pub tabs: Vec<SavedTab>,
    pub active: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bookmark {
    pub name: String,
    pub path: PathBuf,
    pub opposite: Option<PathBuf>,
    pub group: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Favorite {
    pub name: String,
    pub panes: [SavedPane; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub active: Side,
    pub panes: [SavedPane; 2],
    pub bookmarks: Vec<Bookmark>,
    pub favorites: Vec<Favorite>,
    pub stacked: bool,
    pub split: f32,
    pub linked: bool,
    pub show_hidden: bool,
    pub natural_sort: bool,
    pub directories_first: bool,
    pub auto_refresh: bool,
    pub confirm_trash: bool,
    pub confirm_delete: bool,
    pub columns: Vec<Column>,
    pub alternate_columns: Vec<Column>,
    pub show_modified_time: bool,
    pub ignore: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            active: Side::Left,
            panes: Default::default(),
            bookmarks: Vec::new(),
            favorites: Vec::new(),
            stacked: false,
            split: 0.5,
            linked: false,
            show_hidden: false,
            natural_sort: true,
            directories_first: true,
            auto_refresh: true,
            confirm_trash: true,
            confirm_delete: true,
            columns: vec![Column::Name, Column::Size, Column::Modified],
            alternate_columns: vec![Column::Name, Column::Kind, Column::Path, Column::Comment],
            show_modified_time: true,
            ignore: String::new(),
        }
    }
}

#[derive(Clone)]
struct HistoryEntry {
    location: Location,
    entered_via: Option<PathBuf>,
}

impl HistoryEntry {
    fn new(location: Location) -> Self {
        Self {
            location,
            entered_via: None,
        }
    }
}

pub struct Tab {
    pub saved: SavedTab,
    pub id: u64,
    view: views::View,
    raw: Option<Arc<views::Raw>>,
    raw_revision: u64,
    view_revision: u64,
    sort_revision: u64,
    eligibility_revision: u64,
    installed_view: Option<views::ViewKey>,
    preparing: Option<views::ViewKey>,
    selected: views::Marks,
    selected_bytes: u128,
    cursor: Option<PathBuf>,
    cursor_position: Option<usize>,
    pending_cursor: Option<PathBuf>,
    cursor_restored: bool,
    anchor: Option<PathBuf>,
    anchor_position: Option<usize>,
    saved_marks: views::Marks,
    filter: Arc<MatchOptions>,
    filter_query: Arc<listing::MatchQuery>,
    history: Vec<HistoryEntry>,
    history_index: usize,
    pub loading: bool,
    scanning: bool,
    scan_serial: u64,
    scan_request: Option<ScanKey>,
    scan_base: Option<Arc<views::Raw>>,
    resolved_directory: Option<Arc<Path>>,
    patch_paths: Option<Arc<[PathBuf]>>,
    patch_watch: Option<watch::Observation>,
    last_scan: Option<Instant>,
    provisional_focus: bool,
    pub error: Option<String>,
    pub archive_locked: bool,
    pub skipped: usize,
    generation: u64,
    nav_revision: u64,
    pending_navigation: Option<PathBuf>,
    cached_history: Option<PathBuf>,
    deferred_recovery: Option<PathBuf>,
    waiting_for_archive: bool,
    dirty: bool,
}

impl Tab {
    fn new(id: u64, saved: SavedTab) -> Self {
        Self {
            id,
            history: vec![HistoryEntry::new(saved.location.clone())],
            saved,
            view: Default::default(),
            raw: None,
            raw_revision: 0,
            view_revision: 0,
            sort_revision: 0,
            eligibility_revision: 0,
            installed_view: None,
            preparing: None,
            selected: Default::default(),
            selected_bytes: 0,
            cursor: None,
            cursor_position: None,
            pending_cursor: None,
            cursor_restored: false,
            anchor: None,
            anchor_position: None,
            saved_marks: Default::default(),
            filter: Arc::new(MatchOptions::filename("")),
            filter_query: Default::default(),
            history_index: 0,
            loading: false,
            scanning: false,
            scan_serial: 0,
            scan_request: None,
            scan_base: None,
            resolved_directory: None,
            patch_paths: None,
            patch_watch: None,
            last_scan: None,
            provisional_focus: false,
            error: None,
            archive_locked: false,
            skipped: 0,
            generation: 0,
            nav_revision: 0,
            pending_navigation: None,
            cached_history: None,
            deferred_recovery: None,
            waiting_for_archive: false,
            dirty: true,
        }
    }
    pub fn location(&self) -> &Location {
        &self.saved.location
    }
    fn ordinary_directory(&self) -> Option<&Path> {
        match self.location() {
            Location::Directory(path) if !self.filter.recursive => Some(path),
            _ => None,
        }
    }
    pub fn base(&self) -> &Path {
        self.location().base()
    }
    pub fn title(&self) -> String {
        if !self.saved.title.is_empty() {
            return self.saved.title.clone();
        }
        match self.location() {
            Location::Directory(path) => display_name(path),
            Location::Archive { file, folder } => {
                if folder.as_os_str().is_empty() {
                    display_name(file)
                } else {
                    display_name(folder)
                }
            }
            Location::Flat { .. } => "Flat view".into(),
            Location::List { .. } => "File list".into(),
        }
    }
    pub fn navigation_stamp(&self) -> (u64, u64) {
        (self.id, self.nav_revision)
    }
    pub fn pending_navigation(&self) -> Option<&Path> {
        self.pending_navigation.as_deref()
    }
    pub fn busy(&self) -> bool {
        self.listing_pending() || self.pending_navigation.is_some()
    }
    pub fn refreshing(&self) -> bool {
        self.scanning && self.installed_view.is_some() && !self.provisional()
    }
    pub fn listing_pending(&self) -> bool {
        self.loading || self.waiting_for_archive || self.preparing.is_some()
    }
    fn cancel_navigation(&mut self) {
        self.nav_revision += 1;
        self.pending_navigation = None;
        self.cached_history = None;
    }
    pub fn can_back(&self) -> bool {
        self.history_index > 0
    }
    pub fn can_forward(&self) -> bool {
        self.history_index + 1 < self.history.len()
    }
    pub fn history(&self) -> impl DoubleEndedIterator<Item = &Location> + ExactSizeIterator {
        self.history.iter().map(|entry| &entry.location)
    }
    pub fn filter(&self) -> &MatchOptions {
        &self.filter
    }
    pub fn provisional(&self) -> bool {
        self.view.raw.partial
    }
    pub fn incomplete_listing(&self) -> bool {
        self.provisional()
            || (self.loading && self.installed_view.is_none())
            || (self.installed_view.is_some()
                && self.view.eligibility_revision != self.eligibility_revision)
    }
    pub fn listing_generation(&self) -> u64 {
        self.generation
    }
    pub fn visible_position(&self, path: &Path) -> Option<usize> {
        self.view.position(path)
    }
    fn scan_key(&self) -> ScanKey {
        (self.id, self.generation, self.scan_serial)
    }
    pub fn entry_count(&self) -> usize {
        self.view.raw.entries.len()
    }
    pub fn visible_count(&self) -> usize {
        self.view.visible.len()
    }
    pub fn visible_entry(&self, position: usize) -> Option<&Entry> {
        self.view
            .visible
            .get(position)
            .map(|&index| &self.view.raw.entries[index])
    }
    pub fn visible_entries(&self) -> impl ExactSizeIterator<Item = &Entry> {
        self.view.visible.iter().map(|&i| &self.view.raw.entries[i])
    }
    pub fn cursor_position(&self) -> Option<usize> {
        self.cursor_position
    }
    pub fn current(&self) -> Option<&Entry> {
        self.cursor_position
            .and_then(|position| self.visible_entry(position))
    }
    pub fn is_marked(&self, path: &Path) -> bool {
        self.selected.contains(path)
    }
    pub fn marked_count(&self) -> usize {
        if self.view.visible.is_empty() {
            0
        } else {
            self.selected.len()
        }
    }
    pub fn marked_entries(&self) -> impl Iterator<Item = &Entry> {
        (self.marked_count() != 0)
            .then(|| {
                self.visible_entries()
                    .filter(|entry| self.is_marked(&entry.path))
            })
            .into_iter()
            .flatten()
    }
    pub fn has_operands(&self) -> bool {
        self.marked_count() != 0 || self.current().is_some()
    }
    pub fn single_operand(&self) -> Option<&Entry> {
        match self.marked_count() {
            0 => self.current(),
            1 => self
                .current()
                .filter(|entry| self.is_marked(&entry.path))
                .or_else(|| self.marked_entries().next()),
            _ => None,
        }
    }
    pub fn operands(&self) -> Vec<PathBuf> {
        if self.marked_count() == 0 {
            return self
                .current()
                .into_iter()
                .map(|entry| entry.path.clone())
                .collect();
        }
        self.marked_entries()
            .map(|entry| entry.path.clone())
            .collect()
    }
    pub fn archive_selection(&self) -> Option<ArchiveSelection> {
        let Location::Archive { file, folder } = self.location() else {
            return None;
        };
        Some(ArchiveSelection {
            file: file.clone(),
            folder: folder.clone(),
            members: self
                .operands()
                .into_iter()
                .filter_map(|path| path.strip_prefix(file).ok().map(Path::to_owned))
                .collect(),
        })
    }
    pub fn selected_bytes(&self) -> u64 {
        self.selected_bytes.min(u128::from(u64::MAX)) as u64
    }
    fn clear_view(&mut self) {
        self.view = Default::default();
        self.provisional_focus = false;
        self.raw = None;
        self.installed_view = None;
        self.preparing = None;
        self.selected_bytes = 0;
        self.cursor_position = None;
        self.anchor_position = None;
    }
    fn clear_marks(&mut self) {
        self.selected = Default::default();
        self.selected_bytes = 0;
    }
    fn collect_marks<'a>(entries: impl Iterator<Item = &'a Entry>) -> (views::Marks, u128) {
        let mut bytes = 0;
        let paths = entries
            .map(|entry| {
                bytes += Self::mark_size(entry);
                Arc::<Path>::from(entry.path.as_path())
            })
            .collect();
        (Arc::new(paths), bytes)
    }
    fn mark_size(entry: &Entry) -> u128 {
        if entry.is_dir() { 0 } else { entry.size.into() }
    }
    fn set_mark(selected: &mut views::Marks, bytes: &mut u128, entry: &Entry, mark: bool) {
        if mark {
            if !selected.contains(entry.path.as_path()) {
                Arc::make_mut(selected).insert(Arc::from(entry.path.as_path()));
                *bytes += Self::mark_size(entry);
            }
        } else if selected.contains(entry.path.as_path()) {
            Arc::make_mut(selected).remove(entry.path.as_path());
            *bytes -= Self::mark_size(entry);
        }
    }
    fn mark_entries(&mut self, mark: bool, matches: impl Fn(&Entry) -> bool) {
        for &index in self.view.visible.iter() {
            let entry = &self.view.raw.entries[index];
            if matches(entry) {
                Self::set_mark(&mut self.selected, &mut self.selected_bytes, entry, mark);
            }
        }
    }
    fn set_initial_location(&mut self, location: Location) {
        self.saved.home = location.base().to_owned();
        self.history = vec![HistoryEntry::new(location.clone())];
        self.history_index = 0;
        self.set_location(location, false);
    }

    fn set_location(&mut self, location: Location, record: bool) {
        if record && self.history[self.history_index].location != location {
            self.history.truncate(self.history_index + 1);
            self.history.push(HistoryEntry::new(location.clone()));
            if self.history.len() > 128 {
                self.history.remove(0);
            }
            self.history_index = self.history.len() - 1;
        }
        self.cancel_navigation();
        self.deferred_recovery = None;
        self.saved.location = location;
        self.resolved_directory = None;
        self.last_scan = None;
        self.clear_view();
        self.clear_marks();
        self.cursor = None;
        self.pending_cursor = None;
        self.cursor_restored = false;
        self.anchor = None;
        let filter = Arc::make_mut(&mut self.filter);
        filter.text.clear();
        filter.recursive = false;
        self.filter_query = Default::default();
        self.invalidate_scan();
    }

    fn recover_directory(&mut self, parent: PathBuf) {
        if self.saved.home.starts_with(self.base()) {
            self.saved.home = parent.clone();
        }
        let location = Location::Directory(parent);
        if self.history_index > 0 && self.history[self.history_index - 1].location == location {
            self.history.remove(self.history_index);
            self.history_index -= 1;
        } else {
            self.history[self.history_index] = HistoryEntry::new(location.clone());
        }
        self.set_location(location, false);
    }

    fn invalidate_scan(&mut self) {
        self.waiting_for_archive = false;
        self.raw = self.installed_view.map(|_| self.view.raw.clone());
        self.preparing = None;
        self.error = None;
        self.archive_locked = false;
        self.generation += 1;
        if let (Some(installed), Some(key)) = (self.installed_view, self.view_key()) {
            self.installed_view = Some(views::ViewKey {
                revision: installed.revision,
                ..key
            });
        }
        self.loading = false;
        self.scanning = false;
        self.scan_request = None;
        self.scan_base = None;
        self.require_scan();
    }
    fn can_patch(&self, paths: usize) -> Option<&Arc<views::Raw>> {
        self.raw.as_ref().filter(|raw| {
            !self.dirty
                && !self.scanning
                && !raw.partial
                && raw.skipped == 0
                && raw.directory.is_some()
                && !raw.has_hard_links
                && paths + raw.links <= watch::PATCH_LIMIT
        })
    }

    fn require_scan(&mut self) {
        self.patch_paths = None;
        self.patch_watch = None;
        self.dirty = true;
    }

    fn entry_in_parent(&self, parent: &Path) -> Option<&Path> {
        if let Location::Archive { file, .. } = self.location() {
            return (file.parent() == Some(parent)).then_some(file.as_path());
        }
        let Location::Directory(directory) = self.location() else {
            return None;
        };
        let entered_via = self.history[self.history_index].entered_via.as_deref();
        entered_via
            .filter(|path| path.parent() == Some(parent))
            .or_else(|| (directory.parent() == Some(parent)).then_some(directory.as_path()))
    }
    fn view_key(&self) -> Option<views::ViewKey> {
        self.raw.as_ref().map(|_| views::ViewKey {
            tab: self.id,
            generation: self.generation,
            raw: self.raw_revision,
            revision: self.view_revision,
        })
    }

    fn install_view(
        &mut self,
        key: views::ViewKey,
        view: views::View,
        marks: Option<views::Reconciled>,
    ) {
        let finishing = self.provisional() && !view.raw.partial;
        if finishing && !self.provisional_focus {
            self.cursor = None;
            self.anchor = None;
        }
        let pending_cursor = if view.raw.partial {
            self.pending_cursor
                .as_ref()
                .filter(|path| view.position(path).is_some())
                .cloned()
        } else if self.scanning {
            None
        } else {
            self.pending_cursor.take()
        };
        if let Some(path) = &pending_cursor
            && view.position(path).is_some()
        {
            self.cursor = Some(path.clone());
        }
        if let Some(marks) = marks {
            self.selected = marks.retained;
            self.selected_bytes = marks.bytes;
        }
        let previous_position = self.cursor_position;
        self.cursor_position = self.cursor.as_deref().and_then(|path| view.position(path));
        let mut neighbor_restored = false;
        if self.cursor_position.is_none()
            && !view.visible.is_empty()
            && !view.raw.partial
            && let Some(path) = self.cursor.as_deref()
            && view.raw.index(path).is_none()
            && let Some(position) = previous_position
            && let Some((path, new_position)) = (position + 1..self.visible_count())
                .chain((0..position).rev())
                .filter_map(|index| self.visible_entry(index))
                .find_map(|entry| view.position(&entry.path).map(|pos| (&entry.path, pos)))
        {
            self.cursor = Some(path.clone());
            self.cursor_position = Some(new_position);
            neighbor_restored = true;
        }
        self.anchor_position = self.anchor.as_deref().and_then(|path| view.position(path));
        self.skipped = view.raw.skipped;
        self.view = view;
        self.finish_view();
        if neighbor_restored || (pending_cursor.is_some() && pending_cursor == self.cursor) {
            self.anchor.clone_from(&self.cursor);
            self.anchor_position = self.cursor_position;
            self.cursor_restored = true;
        }
        self.preparing = None;
        self.installed_view = Some(key);
        self.loading = self.scanning || self.provisional();
        if finishing {
            self.cursor_restored |= self.provisional_focus && self.cursor_position.is_some();
            self.provisional_focus = false;
        }
    }
    fn finish_view(&mut self) {
        if self.cursor_position.is_none() {
            self.cursor = self.visible_entry(0).map(|entry| entry.path.clone());
            self.cursor_position = self.cursor.as_ref().map(|_| 0);
        }
    }
    fn select(&mut self, path: &Path, mode: Select, mark: bool) {
        let position = self.view.position(path);
        if let Some(position) = position {
            self.select_position(position, mode, mark);
        }
    }
    fn choose_provisional_cursor(&mut self) {
        if self.provisional() {
            self.provisional_focus = true;
            self.pending_cursor = None;
        }
    }
    fn select_position(&mut self, position: usize, mode: Select, mark: bool) {
        self.choose_provisional_cursor();
        let index = self.view.visible[position];
        let path = self.view.raw.entries[index].path.clone();
        match mode {
            Select::Replace | Select::Focus => {
                self.clear_marks();
                if mark {
                    let entry = &self.view.raw.entries[index];
                    Arc::make_mut(&mut self.selected).insert(Arc::from(path.as_path()));
                    self.selected_bytes = Self::mark_size(entry);
                }
                self.anchor = Some(path.clone());
                self.anchor_position = Some(position);
            }
            Select::Toggle => {
                let entry = &self.view.raw.entries[index];
                let mark = !self.selected.contains(entry.path.as_path());
                Self::set_mark(&mut self.selected, &mut self.selected_bytes, entry, mark);
                self.anchor = Some(path.clone());
                self.anchor_position = Some(position);
            }
            Select::Extend => {
                let start = self
                    .anchor_position
                    .or(self.cursor_position)
                    .unwrap_or(position);
                if self.anchor.is_none() {
                    self.anchor = self.visible_entry(start).map(|entry| entry.path.clone());
                    self.anchor_position = Some(start);
                }
                let range = start.min(position)..=start.max(position);
                (self.selected, self.selected_bytes) = Self::collect_marks(
                    self.view.visible[range]
                        .iter()
                        .map(|&index| &self.view.raw.entries[index]),
                );
            }
        }
        self.cursor = Some(path);
        self.cursor_position = Some(position);
    }
}

pub struct Pane {
    pub tabs: Vec<Tab>,
    pub active: usize,
}
impl Pane {
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }
    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Select {
    Replace,
    Toggle,
    Extend,
    Focus,
}
#[derive(Clone, Copy, Debug)]
pub enum Step {
    Previous,
    Next,
    First,
    Last,
    PageUp,
    PageDown,
}

pub enum Command {
    Focus(Side),
    UnlockArchive {
        side: Side,
        file: PathBuf,
        password: String,
    },
    Navigate {
        side: Side,
        path: PathBuf,
        new_tab: bool,
        background: bool,
    },
    History(Side, usize),
    CancelNavigation(Side),
    Back(Side),
    Forward(Side),
    Parent(Side),
    Refresh,
    NewTab(Side),
    OpenFocusedDirectoryInTab {
        source: Side,
        destination: Side,
    },
    Tab(Side, usize),
    CloseTab(Side, usize),
    CloseDuplicates(Side),
    ReorderTab(Side, usize, usize),
    SendTab(Side, usize, bool),
    RenameTab(Side, usize, String),
    LockTab(Side, usize, TabLock),
    ToggleTabPin(Side, usize),
    Swap,
    MatchPanes,
    Flat(Side, bool),
    View(Side, View),
    Sort(Side, Column, bool),
    Filter(Side, MatchOptions),
    Find(Side, MatchOptions),
    Ignore(String),
    ToggleHidden,
    Select(Side, PathBuf, Select),
    Step(Side, Step, bool),
    MarkAll(Side),
    UnmarkAll(Side),
    Invert(Side),
    MarkMask(Side, MatchOptions, bool),
    MarkLike(Side, Column, bool),
    RememberMarks(Side),
    RestoreMarks(Side),
    LoadMarks(Side, String),
    LoadList(Side, String),
    AddBookmark(Bookmark),
    EditBookmark(usize, Bookmark),
    ImportBookmarks(Vec<Bookmark>),
    RestoreTabs([SavedPane; 2]),
    RemoveBookmark(usize),
    OpenBookmark(usize),
    SaveFavorite(String),
    LoadFavorite(usize, bool),
    RemoveFavorite(usize),
    Queue(Operation, bool),
    CreateFile {
        side: Side,
        path: PathBuf,
        hold: bool,
    },
    PauseJob(u64),
    ResumeJob(u64),
    CancelJob(u64),
    PrioritizeJob(u64),
    Resolve(u64, Collision, bool),
    CompareConflict(u64),
    ClearJobs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Held,
    Queued,
    Running,
    Paused,
    Conflict,
    Done,
    Failed,
    Cancelled,
}
impl JobState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Held => "On hold",
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Conflict => "Needs a decision",
            Self::Done => "Complete",
            Self::Failed => "Attention",
            Self::Cancelled => "Cancelled",
        }
    }
    pub fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}
pub struct Job {
    pub id: u64,
    pub title: &'static str,
    pub state: JobState,
    pub progress: Progress,
    pub started: Option<Instant>,
    pub message: String,
    pub warnings: Vec<String>,
    pub conflict: Option<Conflict>,
    operation: Option<Operation>,
    focus_on_success: Option<(u64, PathBuf)>,
    control: Arc<Control>,
}

enum Event {
    JobProgress(u64, Progress),
    Conflict(u64, Conflict),
    ConflictCompared(u64, PathBuf, PathBuf, io::Result<bool>),
    Done {
        id: u64,
        result: io::Result<String>,
        warnings: Vec<String>,
        renamed: Option<(PathBuf, PathBuf)>,
        effects: changes::Effects,
    },
}

type ScanKey = (u64, u64, u64);
#[cfg(any(test, feature = "perf"))]
type FirstPageHook = Arc<dyn Fn(ScanKey) -> io::Result<()> + Send + Sync>;
type ScanResult = (io::Result<Arc<views::Raw>>, Option<PathBuf>);

/// Read-only rows from a history destination while its validation is pending.
pub struct HistoryPreview<'a> {
    pub path: &'a Path,
    view: &'a views::View,
}

impl<'a> HistoryPreview<'a> {
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &'a Entry> + '_ {
        self.view
            .visible
            .iter()
            .map(|&index| &self.view.raw.entries[index])
    }
}

pub struct Engine {
    pub settings: Settings,
    pub panes: [Pane; 2],
    pub jobs: Vec<Job>,
    pub home: PathBuf,
    pub notice: Option<String>,
    ids: u64,
    ignore: Option<Arc<globset::GlobSet>>,
    sender: mpsc::Sender<Event>,
    receiver: mpsc::Receiver<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
    snapshots: snapshots::Cache,
    watches: watch::Watches,
    running: Option<u64>,
    archive_cache: Arc<archives::ArchiveCache>,
    scans: read_pool::ReadPool<ScanKey, ScanResult>,
    navigation: navigation::Navigation,
    views: read_pool::ReadPool<views::ViewKey, io::Result<views::Prepared>>,
    retire: views::Retire,
    pages: Arc<views::FirstPages>,
    #[cfg(any(test, feature = "perf"))]
    first_page_hook: Option<FirstPageHook>,
    focus_revision: u64,
}

impl Engine {
    pub fn new(
        home: PathBuf,
        initial: [PathBuf; 2],
        mut settings: Settings,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let saved = std::mem::take(&mut settings.panes);
        let mut ids = 0;
        let panes = saved
            .into_iter()
            .zip(initial)
            .map(|(pane, path)| make_pane(pane, path, &mut ids))
            .collect::<Vec<_>>()
            .try_into()
            .ok()
            .unwrap();
        let ignore = listing::compile_mask(&settings.ignore, false)
            .unwrap_or_default()
            .map(Arc::new);
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let mut engine = Self {
            settings,
            panes,
            jobs: Vec::new(),
            home,
            notice: None,
            ids,
            ignore,
            sender,
            receiver,
            scans: read_pool::ReadPool::new(2, 2, wake.clone()),
            navigation: navigation::Navigation::new(wake.clone()),
            views: read_pool::ReadPool::new(2, 2, wake.clone()),
            retire: views::reclaimer(),
            pages: Default::default(),
            #[cfg(any(test, feature = "perf"))]
            first_page_hook: None,
            focus_revision: 0,
            wake: wake.clone(),
            snapshots: Default::default(),
            watches: watch::Watches::new(wake.clone()),
            running: None,
            archive_cache: Arc::default(),
        };
        engine.start_scans();
        engine
    }
    pub fn history_preview(&self, side: Side) -> Option<HistoryPreview<'_>> {
        let tab = self.tab(side);
        let path = tab.cached_history.as_deref()?;
        let mut options = view_options(&self.settings, &self.ignore, tab);
        if !options.criteria.text.is_empty() || options.criteria.recursive {
            let criteria = Arc::make_mut(&mut options.criteria);
            criteria.text.clear();
            criteria.recursive = false;
        }
        let view = self.snapshots.ready(path, &options)?;
        Some(HistoryPreview { path, view })
    }

    pub fn listing_wake_after(&self) -> Option<Duration> {
        self.watches.next_batch()
    }

    pub fn listing_stats(&self) -> ListingStats {
        self.snapshots.stats()
    }

    pub fn pane(&self, side: Side) -> &Pane {
        &self.panes[side.index()]
    }
    pub fn tab(&self, side: Side) -> &Tab {
        self.pane(side).tab()
    }
    pub fn archive_password(&self, file: &Path) -> Option<Arc<str>> {
        self.archive_cache.password(file)
    }
    #[cfg(feature = "perf")]
    #[doc(hidden)]
    pub fn set_first_page_test_hook(
        &mut self,
        hook: impl Fn() -> io::Result<()> + Send + Sync + 'static,
    ) {
        self.first_page_hook = Some(Arc::new(move |_| hook()));
    }
    pub fn keep_provisional_focus(&mut self, side: Side) {
        let tab = self.tab_mut(side);
        tab.provisional_focus |= tab.provisional();
    }
    /// Report a restored cursor once so a view can scroll it into sight.
    pub fn take_cursor_restored(&mut self, side: Side) -> bool {
        std::mem::take(&mut self.tab_mut(side).cursor_restored)
    }
    fn pane_mut(&mut self, side: Side) -> &mut Pane {
        &mut self.panes[side.index()]
    }
    fn tab_mut(&mut self, side: Side) -> &mut Tab {
        self.pane_mut(side).tab_mut()
    }
    pub fn snapshot(&self) -> Settings {
        let mut settings = self.settings.clone();
        settings.panes = self.saved_panes();
        settings
    }
    fn saved_panes(&self) -> [SavedPane; 2] {
        self.panes.each_ref().map(|pane| SavedPane {
            tabs: pane.tabs.iter().map(|tab| tab.saved.clone()).collect(),
            active: pane.active,
        })
    }
    pub fn dispatch(&mut self, command: Command) {
        if matches!(
            command,
            Command::Focus(_) | Command::Tab(..) | Command::Select(..)
        ) {
            self.focus_revision += 1;
        }
        let previous_focus = (self.settings.active, self.tab(self.settings.active).id);
        if let Err(error) = self.execute(command) {
            self.notice = Some(error.to_string());
        }
        if previous_focus != (self.settings.active, self.tab(self.settings.active).id) {
            self.focus_revision += 1;
        }
        self.retain_navigation();
        self.start_scans();
        self.start_views();
        self.start_job();
    }

    pub fn set_ignore(&mut self, mask: String) -> io::Result<()> {
        self.ignore = listing::compile_mask(&mask, false)?.map(Arc::new);
        self.settings.ignore = mask;
        self.invalidate_views(false);
        self.start_views();
        Ok(())
    }

    fn execute(&mut self, command: Command) -> io::Result<()> {
        let needs_complete = match &command {
            Command::Select(side, _, Select::Extend)
            | Command::Step(side, _, true)
            | Command::MarkAll(side)
            | Command::Invert(side)
            | Command::Find(side, _)
            | Command::MarkMask(side, ..)
            | Command::MarkLike(side, ..)
            | Command::RestoreMarks(side)
            | Command::LoadMarks(side, _) => Some(*side),
            _ => None,
        };
        if needs_complete.is_some_and(|side| self.tab(side).incomplete_listing()) {
            return Err(io::Error::other("Wait for the directory to finish loading"));
        }

        match command {
            Command::Focus(side) => self.settings.active = side,
            Command::UnlockArchive {
                side,
                file,
                password,
            } => {
                if !matches!(self.tab(side).location(), Location::Archive { file: current, .. } if current == &file)
                {
                    return Err(io::Error::other("The selected archive is no longer open"));
                }
                self.archive_cache.set_password(&file, password);
                for pane in &mut self.panes {
                    for tab in &mut pane.tabs {
                        if matches!(tab.location(), Location::Archive { file: current, .. } if current == &file)
                        {
                            tab.clear_view();
                            tab.invalidate_scan();
                        }
                    }
                }
            }
            Command::Navigate {
                side,
                path,
                new_tab,
                background,
            } => self.navigate(side, &path, new_tab, background, true)?,
            Command::CancelNavigation(side) => self.tab_mut(side).cancel_navigation(),
            Command::History(side, index) => self.request_history(side, Some(index), false)?,
            Command::Back(side) => self.request_history(side, None, true)?,
            Command::Forward(side) => self.request_history(side, None, false)?,
            Command::Parent(side) => {
                let path = match self.tab(side).location() {
                    Location::Directory(path) => path.parent().map(Path::to_path_buf),
                    Location::Archive { file, folder } => folder
                        .parent()
                        .map(|parent| {
                            if parent.as_os_str().is_empty() {
                                file.clone()
                            } else {
                                file.join(parent)
                            }
                        })
                        .or_else(|| file.parent().map(Path::to_owned)),
                    other => Some(other.base().to_path_buf()),
                };
                if let Some(path) = path {
                    let linked = !matches!(self.tab(side).location(), Location::Archive { .. });
                    self.navigate(side, &path, false, false, linked)?;
                }
            }
            Command::Refresh => {
                self.archive_cache.clear_inventories();
                self.mark_dirty();
            }
            Command::NewTab(side) => {
                let tab = self.tab(side);
                let mut saved = default_tab(tab.base().to_path_buf());
                if matches!(tab.location(), Location::Archive { .. }) {
                    saved.location = tab.location().clone();
                }
                self.add_tab(side, saved, false);
            }
            Command::OpenFocusedDirectoryInTab {
                source,
                destination,
            } => {
                let path = self
                    .tab(source)
                    .current()
                    .filter(|entry| entry.is_dir())
                    .ok_or_else(|| io::Error::other("Focus a folder first"))?
                    .path
                    .clone();
                self.navigate(destination, &path, true, false, false)?;
            }
            Command::Tab(side, index) => self.switch_tab(side, index),
            Command::CloseTab(side, index) => self.close_tab(side, index),
            Command::CloseDuplicates(side) => {
                let pane = self.pane_mut(side);
                let mut best = HashMap::new();
                for (index, tab) in pane.tabs.iter().enumerate() {
                    let rank = (
                        tab.saved.pinned,
                        tab.saved.lock != TabLock::Normal,
                        !tab.saved.title.is_empty(),
                        index == pane.active,
                    );
                    let current = best.entry(tab.location()).or_insert((rank, tab.id));
                    if rank > current.0 {
                        *current = (rank, tab.id);
                    }
                }
                let old = pane.tab().id;
                let replacement = best[pane.tab().location()].1;
                let keep: HashSet<_> = best.into_values().map(|(_, id)| id).collect();
                pane.tabs
                    .retain(|tab| tab.saved.pinned || keep.contains(&tab.id));
                pane.active = pane
                    .tabs
                    .iter()
                    .position(|tab| tab.id == old)
                    .or_else(|| pane.tabs.iter().position(|tab| tab.id == replacement))
                    .unwrap_or(0);
            }
            Command::ReorderTab(side, from, to) => {
                let pane = self.pane_mut(side);
                if from < pane.tabs.len() && to < pane.tabs.len() {
                    let active = pane.tab().id;
                    let tab = pane.tabs.remove(from);
                    pane.tabs.insert(to, tab);
                    pane.active = pane.tabs.iter().position(|tab| tab.id == active).unwrap();
                }
            }
            Command::SendTab(side, index, moving) => {
                if let Some(tab) = self.pane(side).tabs.get(index)
                    && !(moving && tab.saved.pinned)
                {
                    let mut saved = tab.saved.clone();
                    saved.pinned = false;
                    self.add_tab(side.other(), saved, false);
                    if moving {
                        self.close_tab(side, index);
                    }
                }
            }
            Command::RenameTab(side, index, title) => {
                if let Some(tab) = self.pane_mut(side).tabs.get_mut(index) {
                    tab.saved.title = title.trim().to_owned();
                }
            }
            Command::LockTab(side, index, lock) => {
                if let Some(tab) = self.pane_mut(side).tabs.get_mut(index) {
                    tab.saved.home = tab.base().to_path_buf();
                    tab.saved.lock = lock;
                    tab.cancel_navigation();
                }
            }
            Command::ToggleTabPin(side, index) => {
                if let Some(tab) = self.pane_mut(side).tabs.get_mut(index) {
                    tab.saved.pinned = !tab.saved.pinned;
                }
            }
            Command::Swap => {
                self.panes.swap(0, 1);
                self.settings.active = self.settings.active.other();
            }
            Command::MatchPanes => {
                let side = self.settings.active;
                let path = self.tab(side).base().to_path_buf();
                self.navigate_without_focus(side.other(), &path)?;
            }
            Command::Flat(side, selection) => {
                let tab = self.tab_mut(side);
                if matches!(tab.location(), Location::Archive { .. }) {
                    return Err(io::Error::other("Flat view needs a local folder"));
                }
                let base = tab.base().to_path_buf();
                let roots = if selection {
                    tab.operands()
                } else {
                    vec![base.clone()]
                };
                let location = if matches!(tab.location(), Location::Flat { .. }) {
                    Location::Directory(base)
                } else {
                    Location::Flat { base, roots }
                };
                tab.set_location(location, true);
            }
            Command::View(side, view) => self.tab_mut(side).saved.view = view,
            Command::Sort(side, column, append) => {
                let sorting = &mut self.tab_mut(side).saved.sort;
                if let Some(sort) = sorting.iter_mut().find(|sort| sort.column == column) {
                    sort.descending = !sort.descending;
                    if !append {
                        let value = *sort;
                        *sorting = vec![value];
                    }
                } else if append {
                    sorting.push(Sort {
                        column,
                        descending: false,
                    });
                } else {
                    *sorting = vec![Sort {
                        column,
                        descending: false,
                    }];
                }
                let tab = self.tab_mut(side);
                tab.view_revision += 1;
                tab.sort_revision += 1;
            }
            Command::Filter(side, mut criteria) => {
                let query = criteria.compile()?;
                let tab = self.tab_mut(side);
                criteria.recursive &= matches!(
                    tab.location(),
                    Location::Directory(_) | Location::Archive { .. }
                );
                let scope_changed = tab.filter.recursive != criteria.recursive;
                tab.filter = Arc::new(criteria);
                tab.filter_query = Arc::new(query);
                tab.view_revision += 1;
                tab.eligibility_revision += 1;
                if scope_changed {
                    tab.clear_view();
                    tab.invalidate_scan();
                }
            }
            Command::Find(side, criteria) => {
                let tab = self.tab(side);
                let matches = criteria.matching(tab.visible_entries())?;
                if matches.is_empty() {
                    return Err(io::Error::other("No names match"));
                }
                let current = tab
                    .cursor
                    .as_ref()
                    .and_then(|cursor| matches.iter().position(|entry| &entry.path == cursor));
                let next = matches[current.map_or(0, |index| (index + 1) % matches.len())]
                    .path
                    .clone();
                self.tab_mut(side).select(&next, Select::Focus, false);
            }
            Command::Ignore(mask) => self.set_ignore(mask)?,
            Command::ToggleHidden => {
                self.settings.show_hidden = !self.settings.show_hidden;
                self.invalidate_views(false);
            }
            Command::Select(side, path, mode) => {
                self.settings.active = side;
                self.tab_mut(side)
                    .select(&path, mode, !matches!(mode, Select::Focus));
            }
            Command::Step(side, step, extend) => {
                let tab = self.tab_mut(side);
                if !tab.view.visible.is_empty() {
                    let last = tab.view.visible.len() - 1;
                    let index = tab.cursor_position().unwrap_or(0);
                    let next = match step {
                        Step::Previous => index.saturating_sub(1),
                        Step::Next => (index + 1).min(last),
                        Step::First => 0,
                        Step::Last => last,
                        Step::PageUp => index.saturating_sub(12),
                        Step::PageDown => (index + 12).min(last),
                    };
                    tab.choose_provisional_cursor();
                    if extend {
                        tab.select_position(next, Select::Extend, false);
                    } else {
                        let path = tab.view.raw.entries[tab.view.visible[next]].path.clone();
                        tab.cursor = Some(path.clone());
                        tab.cursor_position = Some(next);
                        tab.anchor = Some(path);
                        tab.anchor_position = Some(next);
                    }
                }
            }
            Command::MarkAll(side) => {
                let tab = self.tab_mut(side);
                (tab.selected, tab.selected_bytes) = Tab::collect_marks(tab.visible_entries());
            }
            Command::UnmarkAll(side) => self.tab_mut(side).clear_marks(),
            Command::Invert(side) => {
                let tab = self.tab_mut(side);
                (tab.selected, tab.selected_bytes) = Tab::collect_marks(
                    tab.visible_entries()
                        .filter(|entry| !tab.is_marked(&entry.path)),
                );
            }
            Command::MarkMask(side, criteria, mark) => {
                let tab = self.tab_mut(side);
                let matches = criteria
                    .matching(tab.view.visible.iter().map(|&i| &tab.view.raw.entries[i]))?;
                for entry in matches {
                    Tab::set_mark(&mut tab.selected, &mut tab.selected_bytes, entry, mark);
                }
            }
            Command::MarkLike(side, column, mark) => {
                let tab = self.tab_mut(side);
                if let Some(current) = tab.current() {
                    let extension = current.extension.clone();
                    let path = current.path.clone();
                    tab.mark_entries(mark, |entry| match column {
                        Column::Extension => entry.extension == extension,
                        Column::Path => entry.path.parent() == path.parent(),
                        _ => entry.path.file_stem() == path.file_stem(),
                    });
                }
            }
            Command::RememberMarks(side) => {
                let tab = self.tab_mut(side);
                tab.saved_marks = tab.selected.clone();
                self.notice = Some(format!("Remembered {} marks", tab.saved_marks.len()));
            }
            Command::RestoreMarks(side) => {
                let tab = self.tab_mut(side);
                (tab.selected, tab.selected_bytes) = Tab::collect_marks(
                    tab.visible_entries()
                        .filter(|entry| tab.saved_marks.contains(entry.path.as_path())),
                );
            }
            Command::LoadMarks(side, text) => {
                let tab = self.tab_mut(side);
                let input: HashSet<_> = lines(&text).collect();
                (tab.selected, tab.selected_bytes) =
                    Tab::collect_marks(tab.visible_entries().filter(|entry| {
                        input.contains(entry.name.as_str())
                            || entry.path.to_str().is_some_and(|path| input.contains(path))
                    }));
            }
            Command::LoadList(side, text) => {
                let base = self.tab(side).base().to_path_buf();
                let paths = lines(&text).map(|line| base.join(line)).collect();
                let tab = self.tab_mut(side);
                tab.set_location(Location::List { base, paths }, true);
            }
            Command::AddBookmark(bookmark) => {
                if !self.settings.bookmarks.contains(&bookmark) {
                    self.settings.bookmarks.push(bookmark);
                }
            }
            Command::EditBookmark(index, bookmark) => {
                if let Some(existing) = self.settings.bookmarks.get_mut(index) {
                    *existing = bookmark;
                }
            }
            Command::ImportBookmarks(bookmarks) => {
                for bookmark in bookmarks {
                    if !self.settings.bookmarks.contains(&bookmark) {
                        self.settings.bookmarks.push(bookmark);
                    }
                }
            }
            Command::RestoreTabs(panes) => {
                for (side, saved) in Side::ALL.into_iter().zip(panes) {
                    let fallback = self.tab(side).base().to_path_buf();
                    self.panes[side.index()] = make_pane(saved, fallback, &mut self.ids);
                }
            }
            Command::RemoveBookmark(index) => {
                if index < self.settings.bookmarks.len() {
                    self.settings.bookmarks.remove(index);
                }
            }
            Command::OpenBookmark(index) => {
                if let Some(bookmark) = self.settings.bookmarks.get(index).cloned() {
                    let side = self.settings.active;
                    self.navigate_bookmark(side, &bookmark.path, bookmark.opposite)?;
                }
            }
            Command::SaveFavorite(name) => {
                let panes = self.saved_panes();
                self.settings
                    .favorites
                    .retain(|favorite| favorite.name != name);
                self.settings.favorites.push(Favorite { name, panes });
            }
            Command::LoadFavorite(index, keep) => {
                if let Some(favorite) = self.settings.favorites.get(index).cloned() {
                    for (side, saved) in Side::ALL.into_iter().zip(favorite.panes) {
                        if keep {
                            for tab in saved.tabs {
                                self.add_tab(side, tab, true);
                            }
                        } else {
                            let fallback = self.tab(side).base().to_path_buf();
                            self.panes[side.index()] = make_pane(saved, fallback, &mut self.ids);
                        }
                    }
                }
            }
            Command::RemoveFavorite(index) => {
                if index < self.settings.favorites.len() {
                    self.settings.favorites.remove(index);
                }
            }
            Command::Queue(operation, hold) => self.queue_job(operation, hold, None),
            Command::CreateFile { side, path, hold } => {
                let focus = (self.tab(side).id, path.clone());
                self.queue_job(Operation::Create(path), hold, Some(focus));
            }
            Command::PauseJob(id) => {
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    match job.state {
                        JobState::Running => {
                            job.control.pause(true);
                            job.state = JobState::Paused;
                        }
                        JobState::Queued => job.state = JobState::Held,
                        _ => {}
                    }
                }
            }
            Command::ResumeJob(id) => {
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    match job.state {
                        JobState::Paused => {
                            job.control.pause(false);
                            job.state = JobState::Running;
                        }
                        JobState::Held => job.state = JobState::Queued,
                        _ => {}
                    }
                }
            }
            Command::CancelJob(id) => {
                if let Some(job) = self
                    .jobs
                    .iter_mut()
                    .find(|j| j.id == id && !j.state.finished())
                {
                    job.control.cancel();
                    if self.running != Some(id) {
                        job.operation = None;
                        job.state = JobState::Cancelled;
                    }
                }
            }
            Command::PrioritizeJob(id) => {
                if let Some(index) = self
                    .jobs
                    .iter()
                    .position(|job| job.id == id && job.operation.is_some())
                {
                    let job = self.jobs.remove(index);
                    self.jobs.insert(0, job);
                }
            }
            Command::CompareConflict(id) => {
                if let Some(job) = self
                    .jobs
                    .iter_mut()
                    .find(|job| job.id == id && job.message.is_empty())
                    && let Some(conflict) =
                        job.conflict.clone().filter(|conflict| conflict.can_append)
                {
                    job.message = "Comparing…".into();
                    let (sender, wake, control) =
                        (self.sender.clone(), self.wake.clone(), job.control.clone());
                    std::thread::spawn(move || {
                        let result =
                            operations::compare_files(&conflict.source, &conflict.target, &control);
                        let _ = sender.send(Event::ConflictCompared(
                            id,
                            conflict.source,
                            conflict.target,
                            result,
                        ));
                        wake();
                    });
                }
            }
            Command::Resolve(id, choice, all) => {
                if let Some(job) = self
                    .jobs
                    .iter_mut()
                    .find(|j| j.id == id && j.conflict.is_some())
                {
                    if choice == Collision::Cancel {
                        job.control.cancel();
                    } else {
                        job.control.answer(choice, all);
                    }
                    job.message.clear();
                    job.conflict = None;
                    job.state = JobState::Running;
                }
            }
            Command::ClearJobs => self.jobs.retain(|job| !job.state.finished()),
        }
        Ok(())
    }

    fn add_tab(&mut self, side: Side, saved: SavedTab, background: bool) -> &mut Tab {
        self.ids += 1;
        let index = self.pane(side).tabs.len();
        let id = self.ids;
        self.pane_mut(side).tabs.push(Tab::new(id, saved));
        if !background {
            self.switch_tab(side, index);
        }
        &mut self.pane_mut(side).tabs[index]
    }
    fn switch_tab(&mut self, side: Side, index: usize) {
        if self.select_tab(side, index) {
            self.settings.active = side;
        }
    }
    fn select_tab(&mut self, side: Side, index: usize) -> bool {
        let pane = self.pane_mut(side);
        if index >= pane.tabs.len() {
            return false;
        }
        if pane.active != index {
            let tab = pane.tab_mut();
            if tab.saved.lock == TabLock::Reset && tab.saved.home != tab.base() {
                tab.set_location(Location::Directory(tab.saved.home.clone()), true);
            }
            pane.active = index;
        }
        true
    }
    fn close_tab(&mut self, side: Side, index: usize) {
        let pane = self.pane_mut(side);
        if pane.tabs.len() <= 1 || pane.tabs.get(index).is_none_or(|tab| tab.saved.pinned) {
            return;
        }
        pane.tabs.remove(index);
        if index < pane.active {
            pane.active -= 1;
        }
        pane.active = pane.active.min(pane.tabs.len() - 1);
    }
    pub fn set_sort_preferences(&mut self, natural: bool, directories_first: bool) {
        if self.settings.natural_sort != natural
            || self.settings.directories_first != directories_first
        {
            self.settings.natural_sort = natural;
            self.settings.directories_first = directories_first;
            self.invalidate_views(true);
            self.start_views();
        }
    }

    fn invalidate_views(&mut self, sort: bool) {
        for pane in &mut self.panes {
            for tab in &mut pane.tabs {
                tab.view_revision += 1;
                if sort {
                    tab.sort_revision += 1;
                } else {
                    tab.eligibility_revision += 1;
                }
            }
        }
    }

    fn start_views(&mut self) {
        let keys = self.panes.each_ref().map(|pane| pane.tab().view_key());
        self.views.retain(|key| keys.contains(&Some(key)));
        for pane in &mut self.panes {
            for (index, tab) in pane.tabs.iter_mut().enumerate() {
                if index != pane.active {
                    tab.preparing = None;
                }
            }
            let tab = pane.tab_mut();
            let Some(key) = tab.view_key() else { continue };
            if tab.installed_view == Some(key) || tab.preparing == Some(key) {
                continue;
            }
            let raw = tab.raw.as_ref().unwrap().clone();
            let order = Arc::ptr_eq(&tab.view.raw, &raw).then(|| {
                (
                    tab.view.order.clone(),
                    tab.view.sort_revision == tab.sort_revision,
                )
            });
            let options = view_options(&self.settings, &self.ignore, tab);
            let marks = (!Arc::ptr_eq(&tab.view.raw, &raw)
                || tab.view.eligibility_revision != tab.eligibility_revision)
                .then(|| tab.selected.clone());
            if self.views.submit(key, move |cancel| {
                let view = options.prepare(raw, order, cancel)?;
                views::Prepared::new(view, marks, cancel)
            }) {
                tab.preparing = Some(key);
            }
        }
    }

    fn install_views(&mut self) {
        for (key, result) in self.views.drain() {
            for pane in &mut self.panes {
                let tab = pane.tab_mut();
                if tab.view_key() != Some(key) || tab.preparing != Some(key) {
                    continue;
                }
                #[cfg(feature = "perf")]
                let install = perf::timestamp();
                match result {
                    Ok(views::Prepared { view, mut marks }) => {
                        #[cfg(feature = "perf")]
                        let label = if view.raw.partial {
                            "partial_install"
                        } else {
                            "install"
                        };
                        if marks
                            .as_ref()
                            .is_some_and(|marks| !Arc::ptr_eq(&marks.source, &tab.selected))
                        {
                            let current = tab.selected.clone();
                            if current.len() > 2048 {
                                tab.preparing = self
                                    .views
                                    .submit(key, move |cancel| {
                                        views::Prepared::new(view, Some(current), cancel)
                                    })
                                    .then_some(key);
                                break;
                            }
                            marks = Some(
                                view.reconcile(current, &Default::default())
                                    .expect("a fresh token cannot cancel mark reconciliation"),
                            );
                        }
                        if let Some(path) = tab.ordinary_directory()
                            && !view.raw.partial
                        {
                            let options = view_options(&self.settings, &self.ignore, tab);
                            self.snapshots.remember_view(path, options, &view);
                        }
                        tab.install_view(key, view, marks);
                        #[cfg(feature = "perf")]
                        perf::elapsed(label, install);
                    }
                    Err(error) => {
                        tab.preparing = None;
                        tab.loading = tab.scanning || tab.provisional();
                        tab.error = Some(error.to_string());
                    }
                }
                break;
            }
        }
    }
    fn mark_dirty(&mut self) {
        self.snapshots.invalidate(|_, _| true);
        for pane in &mut self.panes {
            for tab in &mut pane.tabs {
                tab.require_scan();
            }
        }
    }
    fn invalidate_effects(&mut self, effects: &changes::Effects) {
        if effects.is_empty() {
            return;
        }
        self.snapshots.invalidate(|path, resolved| {
            effects.directory(path) || resolved.is_some_and(|path| effects.directory(path))
        });
        self.archive_cache
            .invalidate_inventories(|path| effects.item(path));
        for pane in &mut self.panes {
            for tab in &mut pane.tabs {
                let resolved_affected = tab
                    .resolved_directory
                    .as_deref()
                    .is_some_and(|path| effects.directory_listing(path, tab.filter.recursive));
                let unresolved_scan = tab.scanning
                    && matches!(tab.location(), Location::Directory(_))
                    && tab.resolved_directory.is_none();
                if resolved_affected
                    || unresolved_scan
                    || effects.listing(tab.location(), tab.filter.recursive)
                {
                    tab.require_scan();
                }
            }
        }
    }

    fn apply_rename_to_tabs(&mut self, source: &Path, target: &Path) {
        for pane in &mut self.panes {
            for tab in &mut pane.tabs {
                let paths = match &mut tab.saved.location {
                    Location::List { paths, .. } | Location::Flat { roots: paths, .. } => {
                        paths.as_mut_slice()
                    }
                    _ => &mut [],
                };
                let mut location_changed = false;
                for path in paths.iter_mut().filter(|path| *path == source) {
                    *path = target.to_owned();
                    location_changed = true;
                }
                if location_changed {
                    tab.history[tab.history_index].location = tab.saved.location.clone();
                }
                let focused = tab.cursor.as_deref() == Some(source)
                    || tab.pending_cursor.as_deref() == Some(source)
                    || (tab.selected.len() == 1 && tab.selected.contains(source));
                if focused {
                    tab.pending_cursor = Some(target.to_owned());
                }
                if focused || location_changed {
                    // A scan started before the rename can still contain the old path.
                    tab.invalidate_scan();
                }
            }
        }
    }
    fn dirty_directory(&mut self, path: &Path, hint: watch::Hint) {
        self.snapshots.invalidate(|cached, _| cached == path);
        let mut tabs = self
            .panes
            .iter()
            .map(Pane::tab)
            .filter(|tab| tab.ordinary_directory() == Some(path));
        let (coverage_clear, observation) = match self.watches.ticket(path) {
            Some(ticket) => {
                let observed = ticket.observe_clean();
                (observed.is_some(), observed)
            }
            None => (true, None),
        };
        let patch_paths = match (hint, tabs.next()) {
            (watch::Hint::Paths(paths), Some(tab)) if coverage_clear => tab
                .can_patch(paths.len())
                .filter(|raw| {
                    tabs.all(|tab| {
                        tab.can_patch(paths.len())
                            .is_some_and(|other| Arc::ptr_eq(raw, other))
                    })
                })
                .map(|_| Arc::<[PathBuf]>::from(paths)),
            _ => None,
        };
        for pane in &mut self.panes {
            let active = pane.active;
            for (index, tab) in pane.tabs.iter_mut().enumerate() {
                if tab.ordinary_directory() == Some(path) {
                    tab.dirty = true;
                    tab.patch_paths = if active == index {
                        patch_paths.clone()
                    } else {
                        None
                    };
                    tab.patch_watch = tab.patch_paths.as_ref().and(observation.clone());
                }
            }
        }
    }

    fn start_scans(&mut self) {
        for pane in &mut self.panes {
            let tab = pane.tab_mut();
            if tab.pending_navigation.is_none()
                && let Some(parent) = tab.deferred_recovery.take()
            {
                tab.recover_directory(parent);
            }
        }
        for pane in &mut self.panes {
            for (index, tab) in pane.tabs.iter_mut().enumerate() {
                if index != pane.active {
                    tab.waiting_for_archive = false;
                    if tab.loading {
                        tab.invalidate_scan();
                    }
                    if tab.raw.is_some() {
                        tab.clear_view();
                        tab.require_scan();
                    }
                }
            }
        }
        let watched = self.panes.each_ref().map(|pane| {
            self.settings
                .auto_refresh
                .then(|| pane.tab().ordinary_directory())
                .flatten()
        });
        let had_coverage = watched.map(|path| path.is_some_and(|path| self.watches.reliable(path)));
        for path in self.watches.set_paths(watched, Instant::now()) {
            if self.settings.auto_refresh {
                self.dirty_directory(&path, watch::Hint::Full);
            } else {
                self.snapshots
                    .invalidate(|cached, _| cached == path.as_ref());
            }
        }
        let active = self.panes.each_ref().map(|pane| pane.tab().scan_request);
        self.scans.retain(|key| active.contains(&Some(key)));
        self.pages.set_active(active.map(Option::unwrap_or_default));
        for (slot, had_coverage) in had_coverage.into_iter().enumerate() {
            let [pane, peer] = self.panes.get_disjoint_mut([slot, 1 - slot]).unwrap();
            let tab = pane.tab_mut();
            if tab.scanning || !tab.dirty {
                continue;
            }
            if tab.raw.is_none()
                && let Some(path) = tab.ordinary_directory()
            {
                let options = view_options(&self.settings, &self.ignore, tab);
                if let Some((raw, ready, validated)) = self.snapshots.get(path, &options)
                    && (tab.selected.is_empty()
                        || validated
                            .zip(tab.last_scan)
                            .is_some_and(|(cached, previous)| cached >= previous))
                {
                    tab.raw_revision += 1;
                    tab.resolved_directory.clone_from(&raw.resolved_directory);
                    tab.raw = Some(raw);
                    tab.last_scan = validated;
                    // A new subscription cannot tell us what changed while we were away.
                    tab.dirty = !had_coverage
                        || validated.is_none_or(|at| at.elapsed() >= snapshots::UNWATCHED_INTERVAL);
                    tab.loading = true;
                    if let Some(view) = ready
                        && tab.selected.is_empty()
                    {
                        tab.install_view(tab.view_key().unwrap(), view, None);
                    }
                }
            }
            if !tab.dirty {
                continue;
            }
            let other = peer.tab();
            if let Some(path) = tab.ordinary_directory()
                && other.ordinary_directory() == Some(path)
                && let Some(key) = other.scan_request
            {
                tab.scan_request = Some(key);
                tab.scan_base = other.scan_base.clone();
                tab.patch_paths = None;
                tab.patch_watch = other.patch_watch.clone();
                self.pages.expect(slot, key);
                tab.dirty = other.dirty
                    || tab.scan_base.as_ref().is_some_and(|base| {
                        !tab.raw.as_ref().is_some_and(|raw| Arc::ptr_eq(base, raw))
                    });
                tab.loading = true;
                tab.scanning = true;
                continue;
            }
            // An archive reservation survives cancellation until its worker leaves
            // the blocking read. A duplicate stays dirty without taking a worker.
            let reservation = if let Location::Archive { file, .. } = tab.location() {
                let Some(reservation) = self.archive_cache.reserve_listing(file) else {
                    tab.waiting_for_archive = true;
                    continue;
                };
                Some(reservation)
            } else {
                None
            };
            let location = tab.saved.location.clone();
            let recursive = tab.filter.recursive;
            // Retained marks need the complete scope for reconciliation.
            let first_page = (tab.installed_view.is_none()
                && tab.selected.is_empty()
                && matches!(location, Location::Directory(_))
                && !recursive)
                .then(|| view_options(&self.settings, &self.ignore, tab));
            let ticket = tab
                .ordinary_directory()
                .and_then(|path| self.watches.ticket(path));
            let ordinary = tab.ordinary_directory().is_some();
            let patch = tab.patch_paths.take().zip(tab.raw.clone());
            let base = patch.as_ref().map(|(_, raw)| raw.clone());
            tab.scan_serial += 1;
            let key = tab.scan_key();
            self.pages.expect(slot, key);
            let pages = self.pages.clone();
            #[cfg(any(test, feature = "perf"))]
            let first_page_hook = self.first_page_hook.clone();
            let archive_cache = self.archive_cache.clone();
            let wake = self.wake.clone();
            let retire = self.retire.clone();
            #[cfg(feature = "perf")]
            let scheduled = perf::timestamp();
            let accepted = self.scans.submit(key, move |cancel| {
                #[cfg(feature = "perf")]
                perf::elapsed("worker_start", scheduled);
                if let Some(ticket) = ticket {
                    ticket.wait_for_registration();
                }
                if let Some((paths, base)) = patch
                    && let Location::Directory(path) = &location
                {
                    let result = listing::patch_directory(path, &base, &paths, cancel)
                        .and_then(|scan| views::Raw::new(scan, false, retire, cancel))
                        .map(|mut raw| {
                            raw.directory = base.directory;
                            raw.resolved_directory.clone_from(&base.resolved_directory);
                            Arc::new(raw)
                        });
                    return (result, None);
                }
                let directory = ordinary
                    .then(|| listing::directory_identity(location.base()))
                    .flatten();
                let resolved_directory = if matches!(location, Location::Directory(_)) {
                    cancel
                        .check()
                        .and_then(|()| location.base().canonicalize())
                        .ok()
                        .map(Arc::from)
                } else {
                    None
                };
                let mut did_publish = false;
                #[cfg(any(test, feature = "perf"))]
                let mut page_error = Ok(());
                let result = match &location {
                    Location::Archive { file, folder } => {
                        archives::scan(&archive_cache, file, folder, recursive, cancel)
                    }
                    _ => {
                        let mut sample = Vec::new();
                        let first_deadline = Instant::now() + Duration::from_millis(16);
                        listing::scan_cancellable(&location, recursive, cancel, |entry| {
                            if did_publish || cancel.cancelled() {
                                return;
                            }
                            let Some(options) = &first_page else { return };
                            if options.shown(entry)
                                && options.query.matches(&options.criteria, entry)
                            {
                                sample.push(entry.clone());
                            }
                            if !sample.is_empty()
                                && (sample.len() >= 64 || Instant::now() >= first_deadline)
                            {
                                let scan = listing::Scan {
                                    entries: std::mem::take(&mut sample),
                                    skipped: 0,
                                    has_hard_links: false,
                                };
                                if let Ok(raw) = views::Raw::new(scan, true, retire.clone(), cancel)
                                    && pages.publish(key, Arc::new(raw), cancel)
                                {
                                    did_publish = true;
                                    wake();
                                    #[cfg(any(test, feature = "perf"))]
                                    if let Some(hook) = &first_page_hook {
                                        page_error = hook(key);
                                    }
                                }
                            }
                        })
                    }
                };
                #[cfg(any(test, feature = "perf"))]
                let result = result.and_then(|scan| page_error.map(|()| scan));
                let result = result.and_then(|scan| {
                    if did_publish && let Location::Directory(path) = &location {
                        cancel.check()?;
                        if !path.metadata()?.is_dir() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotADirectory,
                                "Directory was replaced while loading",
                            ));
                        }
                    }
                    Ok(scan)
                });
                let fallback = match (&location, &result) {
                    (Location::Directory(path), Err(error))
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) =>
                    {
                        path.ancestors()
                            .skip(1)
                            .take_while(|_| !cancel.cancelled())
                            .find(|parent| parent.is_dir())
                            .map(Path::to_owned)
                    }
                    _ => None,
                };
                let result = result
                    .and_then(|scan| views::Raw::new(scan, false, retire, cancel))
                    .map(|mut raw| {
                        raw.directory = directory;
                        raw.resolved_directory = resolved_directory;
                        Arc::new(raw)
                    });
                #[cfg(feature = "perf")]
                if cancel.cancelled() {
                    perf::elapsed("listing_cancel_observed", cancel.cancelled_at());
                } else {
                    perf::elapsed("publication", scheduled);
                }
                if let Some(reservation) = reservation {
                    drop(reservation);
                    if cancel.cancelled() {
                        wake();
                    }
                }
                (result, fallback)
            });
            if accepted {
                if ordinary && base.is_none() {
                    self.snapshots.record_scan();
                }
                tab.scan_base = base;
                tab.scan_request = Some(key);
                tab.waiting_for_archive = false;
                tab.loading = true;
                tab.scanning = true;
                tab.dirty = false;

                let peer = peer.tab_mut();
                if ordinary
                    && peer.ordinary_directory() == tab.ordinary_directory()
                    && peer.raw.is_some()
                    && !peer.scanning
                {
                    peer.scan_request = Some(key);
                    peer.scan_base.clone_from(&tab.scan_base);
                    peer.patch_watch.clone_from(&tab.patch_watch);
                    peer.patch_paths = None;
                    peer.dirty = false;
                    peer.loading = true;
                    peer.scanning = true;
                    self.pages.expect(1 - slot, key);
                }
            }
        }
    }
    fn queue_job(
        &mut self,
        operation: Operation,
        hold: bool,
        focus_on_success: Option<(u64, PathBuf)>,
    ) {
        self.ids += 1;
        self.jobs.push(Job {
            id: self.ids,
            title: operation.label(),
            state: if hold {
                JobState::Held
            } else {
                JobState::Queued
            },
            progress: Progress::default(),
            started: None,
            message: String::new(),
            warnings: Vec::new(),
            conflict: None,
            operation: Some(operation),
            focus_on_success,
            control: Arc::default(),
        });
    }

    fn start_job(&mut self) {
        if self.running.is_some() {
            return;
        }
        let Some(job) = self.jobs.iter_mut().find(|j| j.state == JobState::Queued) else {
            return;
        };
        let id = job.id;
        let Some(operation) = job.operation.take() else {
            return;
        };
        job.started = Some(Instant::now());
        job.state = JobState::Running;
        self.running = Some(id);
        let (sender, wake, control) = (self.sender.clone(), self.wake.clone(), job.control.clone());
        std::thread::spawn(move || {
            let report = |progress| {
                let _ = sender.send(Event::JobProgress(id, progress));
                wake();
            };
            let conflict = |conflict| {
                let _ = sender.send(Event::Conflict(id, conflict));
                wake();
            };
            let mut worker = Worker {
                control,
                progress: Default::default(),
                report: &report,
                conflict: &conflict,
                last_report: Instant::now(),
                warnings: Vec::new(),
            };
            let renamed = match &operation {
                Operation::Rename { source, target } => Some((source.clone(), target.clone())),
                _ => None,
            };
            let effects = if worker.control.is_cancelled() {
                changes::Effects::default()
            } else {
                changes::Effects::for_operation(&operation)
            };
            let result = worker.run(operation);
            let _ = sender.send(Event::Done {
                id,
                result,
                warnings: worker.warnings,
                renamed,
                effects,
            });
            wake();
        });
    }

    fn install_pages(&mut self) {
        for (key, raw) in self.pages.drain() {
            for pane in &mut self.panes {
                let tab = pane.tab_mut();
                if tab.scan_request == Some(key) && tab.raw.is_none() && tab.selected.is_empty() {
                    tab.raw_revision += 1;
                    tab.raw = Some(raw.clone());
                }
            }
        }
    }

    fn install_scans(&mut self) {
        for (key, (result, fallback)) in self.scans.drain() {
            let subscribers = self
                .panes
                .each_ref()
                .map(|pane| pane.tab().scan_request == Some(key));
            let patch = self
                .panes
                .iter()
                .zip(subscribers)
                .any(|(pane, subscribed)| subscribed && pane.tab().scan_base.is_some());
            if patch {
                let acceptable = result.is_ok()
                    && self
                        .panes
                        .iter()
                        .zip(subscribers)
                        .filter(|(_, subscribed)| *subscribed)
                        .all(|(pane, _)| {
                            let tab = pane.tab();
                            !tab.dirty
                                && tab
                                    .patch_watch
                                    .as_ref()
                                    .is_none_or(watch::Observation::current)
                                && tab
                                    .raw
                                    .as_ref()
                                    .zip(tab.scan_base.as_ref())
                                    .is_some_and(|(raw, base)| Arc::ptr_eq(raw, base))
                        });
                if !acceptable {
                    for (pane, subscribed) in self.panes.iter_mut().zip(subscribers) {
                        if subscribed {
                            let tab = pane.tab_mut();
                            tab.scan_base = None;
                            tab.scan_request = None;
                            tab.scanning = false;
                            tab.require_scan();
                        }
                    }
                    continue;
                }
                self.snapshots.record_patch();
            }
            let completed = Instant::now();
            if let Some(path) = self
                .panes
                .iter()
                .zip(subscribers)
                .find_map(|(pane, subscribed)| {
                    subscribed
                        .then(|| pane.tab().ordinary_directory())
                        .flatten()
                })
            {
                match &result {
                    Ok(raw) => {
                        let clean = !self
                            .panes
                            .iter()
                            .zip(subscribers)
                            .any(|(pane, subscribed)| subscribed && pane.tab().dirty);
                        let last_validation = if patch {
                            self.panes
                                .iter()
                                .zip(subscribers)
                                .find_map(|(pane, subscribed)| {
                                    subscribed.then(|| pane.tab().last_scan).flatten()
                                })
                        } else {
                            Some(completed)
                        };
                        self.snapshots.remember(
                            path,
                            raw.clone(),
                            if clean { last_validation } else { None },
                        );
                    }
                    Err(_) => self.snapshots.remove(path),
                }
            }
            for (pane, subscribed) in self.panes.iter_mut().zip(subscribers) {
                if !subscribed {
                    continue;
                }
                let tab = pane.tab_mut();
                tab.scan_request = None;
                tab.scan_base = None;
                tab.patch_watch = None;
                if !patch {
                    tab.last_scan = Some(completed);
                }
                tab.scanning = false;
                tab.deferred_recovery.clone_from(&fallback);
                match &result {
                    Ok(raw) => {
                        tab.raw_revision += 1;
                        tab.resolved_directory.clone_from(&raw.resolved_directory);
                        tab.raw = Some(raw.clone());
                        tab.error = None;
                        tab.archive_locked = false;
                    }
                    Err(error) => {
                        tab.loading = false;
                        if tab.deferred_recovery.is_some() && !tab.provisional() {
                            if tab.raw.as_ref().is_some_and(|raw| raw.partial) {
                                tab.clear_view();
                            }
                            continue;
                        }
                        tab.pending_cursor = None;
                        tab.archive_locked = archives::is_password_error(error);
                        tab.error = Some(error.to_string());
                        tab.clear_view();
                        tab.clear_marks();
                        tab.cursor = None;
                    }
                }
            }
        }
    }

    pub fn poll(&mut self) {
        self.install_navigation();
        self.retain_navigation();
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Event::JobProgress(id, progress) => {
                    if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                        job.progress = progress;
                    }
                }
                Event::Conflict(id, conflict) => {
                    if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id)
                        && !job.control.is_cancelled()
                    {
                        job.conflict = Some(conflict);
                        job.state = JobState::Conflict;
                    }
                }
                Event::ConflictCompared(id, source, target, result) => {
                    if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id)
                        && job.conflict.as_ref().is_some_and(|conflict| {
                            conflict.source == source && conflict.target == target
                        })
                    {
                        job.message = match result {
                            Ok(true) => "Files are identical".into(),
                            Ok(false) => "File contents differ".into(),
                            Err(error) => error.to_string(),
                        };
                    }
                }
                Event::Done {
                    id,
                    result,
                    warnings,
                    renamed,
                    effects,
                } => {
                    if result.is_ok()
                        && let Some((source, target)) = renamed
                    {
                        self.apply_rename_to_tabs(&source, &target);
                    }
                    let mut created_focus = None;
                    if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                        let focus = job.focus_on_success.take();
                        if result.is_ok() && warnings.is_empty() {
                            created_focus = focus;
                        }
                        job.conflict = None;
                        job.warnings = warnings;
                        match result {
                            Ok(message) => {
                                job.state = if job.warnings.is_empty() {
                                    JobState::Done
                                } else {
                                    JobState::Failed
                                };
                                job.message = message;
                            }
                            Err(error) => {
                                job.state = if job.control.is_cancelled()
                                    || error.to_string() == "Cancelled"
                                {
                                    JobState::Cancelled
                                } else {
                                    JobState::Failed
                                };
                                job.message = error.to_string();
                            }
                        }
                        self.notice = Some(format!(
                            "{} · {}",
                            job.title,
                            job.message.lines().next().unwrap_or_default()
                        ));
                    }
                    if let Some((tab_id, path)) = created_focus
                        && let Some(tab) = self
                            .panes
                            .iter_mut()
                            .flat_map(|pane| &mut pane.tabs)
                            .find(|tab| tab.id == tab_id)
                        && path.parent() == Some(tab.base())
                        && effects.listing(tab.location(), tab.filter.recursive)
                    {
                        tab.pending_cursor = Some(path);
                        tab.invalidate_scan();
                    }
                    self.running = None;
                    self.invalidate_effects(&effects);
                }
            }
        }
        for (path, hint) in self.watches.drain(Instant::now()) {
            if self.settings.auto_refresh {
                self.dirty_directory(&path, hint);
            }
        }
        self.install_pages();
        self.install_scans();
        self.install_views();
        if self.settings.auto_refresh {
            for pane in &mut self.panes {
                let tab = pane.tab_mut();
                let interval = if tab
                    .ordinary_directory()
                    .is_some_and(|path| self.watches.reliable(path))
                {
                    watch::WATCHED_INTERVAL
                } else {
                    snapshots::UNWATCHED_INTERVAL
                };
                if !tab.scanning
                    && tab.last_scan.is_some_and(|at| at.elapsed() >= interval)
                    && !matches!(
                        tab.location(),
                        Location::Archive { .. } | Location::Flat { .. }
                    )
                {
                    tab.require_scan();
                }
            }
        }
        self.start_scans();
        self.start_views();
        self.start_job();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        #[cfg(feature = "perf")]
        perf::cache_stats(self.listing_stats());
        for job in &self.jobs {
            job.control.cancel();
        }
    }
}

fn view_options(
    settings: &Settings,
    ignore: &Option<Arc<globset::GlobSet>>,
    tab: &Tab,
) -> views::Options {
    views::Options {
        sort: tab.saved.sort.clone(),
        directories_first: settings.directories_first,
        natural: settings.natural_sort,
        hidden: settings.show_hidden,
        ignore: ignore.clone(),
        criteria: tab.filter.clone(),
        query: tab.filter_query.clone(),
        sort_revision: tab.sort_revision,
        eligibility_revision: tab.eligibility_revision,
    }
}

fn default_tab(path: PathBuf) -> SavedTab {
    SavedTab {
        home: path.clone(),
        location: Location::Directory(path),
        title: String::new(),
        lock: TabLock::Normal,
        pinned: false,
        view: View::Details,
        sort: vec![Sort::default()],
    }
}
fn make_pane(saved: SavedPane, fallback: PathBuf, ids: &mut u64) -> Pane {
    let mut tabs = saved
        .tabs
        .into_iter()
        .map(|mut saved| {
            if saved.lock == TabLock::Reset {
                saved.location = Location::Directory(saved.home.clone());
            }
            *ids += 1;
            Tab::new(*ids, saved)
        })
        .collect::<Vec<_>>();
    if tabs.is_empty() {
        *ids += 1;
        tabs.push(Tab::new(*ids, default_tab(fallback)));
    }
    Pane {
        active: saved.active.min(tabs.len() - 1),
        tabs,
    }
}

pub fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
pub fn lines(input: &str) -> impl Iterator<Item = &str> {
    input
        .trim_start_matches('\u{feff}')
        .lines()
        .filter(|line| !line.trim().is_empty())
}
pub fn read_list(path: &Path) -> io::Result<String> {
    use std::io::Read;
    let mut output = String::new();
    std::fs::File::open(path)?
        .take(16 * 1024 * 1024 + 1)
        .read_to_string(&mut output)?;
    if output.len() > 16 * 1024 * 1024 {
        return Err(io::Error::other("File lists are limited to 16 MiB"));
    }
    Ok(output)
}
pub fn space(path: &Path) -> Option<(u64, u64)> {
    Some((
        fs2::available_space(path).ok()?,
        fs2::total_space(path).ok()?,
    ))
}
pub fn tree_folders(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut folders: Vec<_> = std::fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|item| item.path())
        .collect();
    folders.sort_by_cached_key(|path| display_name(path).to_lowercase());
    Ok(folders)
}

pub fn numbered_parts(path: &Path) -> Vec<PathBuf> {
    let Some(folder) = path.parent() else {
        return vec![path.to_path_buf()];
    };
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return vec![path.to_path_buf()];
    };
    if extension.len() < 3 || !extension.bytes().all(|byte| byte.is_ascii_digit()) {
        return vec![path.to_path_buf()];
    }
    let mut parts = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|candidate| {
            candidate.file_stem() == path.file_stem()
                && candidate
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        ext.len() == extension.len()
                            && ext.bytes().all(|byte| byte.is_ascii_digit())
                    })
        })
        .collect::<Vec<_>>();
    parts.sort();
    if parts.is_empty() {
        parts.push(path.to_path_buf());
    }
    parts
}
pub fn local_properties(path: &Path) -> Option<String> {
    let metadata = path.symlink_metadata().ok()?;
    if metadata.is_symlink() {
        return std::fs::read_link(path)
            .ok()
            .map(|target| format!("Link target  {}", target.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(format!(
            "Allocated  {} bytes\nLinks  {}\nOwner UID  {} · Group GID  {}",
            metadata.blocks() * 512,
            metadata.nlink(),
            metadata.uid(),
            metadata.gid()
        ))
    }
    #[cfg(not(unix))]
    None
}
