mod archives;
mod listing;
mod operations;
#[cfg(feature = "perf")]
mod perf;
mod text_preview;

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

    fn is_available(&self) -> bool {
        match &self.location {
            Location::Archive { file, .. } => file.is_file(),
            _ => self.location.base().is_dir(),
        }
    }
}

pub struct Tab {
    pub saved: SavedTab,
    pub id: u64,
    entries: Vec<Entry>,
    visible: Vec<usize>,
    selected: HashSet<PathBuf>,
    selected_bytes: u128,
    cursor: Option<PathBuf>,
    cursor_position: Option<usize>,
    pending_parent_focus: Option<PathBuf>,
    parent_focus_restored: bool,
    anchor: Option<PathBuf>,
    anchor_position: Option<usize>,
    saved_marks: HashSet<PathBuf>,
    filter: MatchOptions,
    filter_query: listing::MatchQuery,
    history: Vec<HistoryEntry>,
    history_index: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub archive_locked: bool,
    pub skipped: usize,
    generation: u64,
    dirty: bool,
}

impl Tab {
    fn new(id: u64, saved: SavedTab) -> Self {
        Self {
            id,
            history: vec![HistoryEntry::new(saved.location.clone())],
            saved,
            entries: Vec::new(),
            visible: Vec::new(),
            selected: Default::default(),
            selected_bytes: 0,
            cursor: None,
            cursor_position: None,
            pending_parent_focus: None,
            parent_focus_restored: false,
            anchor: None,
            anchor_position: None,
            saved_marks: Default::default(),
            filter: MatchOptions::filename(""),
            filter_query: Default::default(),
            history_index: 0,
            loading: false,
            error: None,
            archive_locked: false,
            skipped: 0,
            generation: 0,
            dirty: true,
        }
    }
    pub fn location(&self) -> &Location {
        &self.saved.location
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
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }
    pub fn visible_entry(&self, position: usize) -> Option<&Entry> {
        self.visible
            .get(position)
            .map(|&index| &self.entries[index])
    }
    pub fn visible_entries(&self) -> impl ExactSizeIterator<Item = &Entry> {
        self.visible.iter().map(|&i| &self.entries[i])
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
        if self.visible.is_empty() {
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
        self.entries.clear();
        self.visible.clear();
        self.selected_bytes = 0;
        self.cursor_position = None;
        self.anchor_position = None;
    }
    fn clear_marks(&mut self) {
        self.selected.clear();
        self.selected_bytes = 0;
    }
    fn collect_marks<'a>(entries: impl Iterator<Item = &'a Entry>) -> (HashSet<PathBuf>, u128) {
        let mut bytes = 0;
        let paths = entries
            .map(|entry| {
                bytes += Self::mark_size(entry);
                entry.path.clone()
            })
            .collect();
        (paths, bytes)
    }
    fn mark_size(entry: &Entry) -> u128 {
        if entry.is_dir() { 0 } else { entry.size.into() }
    }
    fn set_mark(selected: &mut HashSet<PathBuf>, bytes: &mut u128, entry: &Entry, mark: bool) {
        if mark {
            if !selected.contains(&entry.path) {
                selected.insert(entry.path.clone());
                *bytes += Self::mark_size(entry);
            }
        } else if selected.remove(&entry.path) {
            *bytes -= Self::mark_size(entry);
        }
    }
    fn mark_entries(&mut self, mark: bool, matches: impl Fn(&Entry) -> bool) {
        for &index in &self.visible {
            let entry = &self.entries[index];
            if matches(entry) {
                Self::set_mark(&mut self.selected, &mut self.selected_bytes, entry, mark);
            }
        }
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
        self.saved.location = location;
        self.clear_view();
        self.clear_marks();
        self.cursor = None;
        self.pending_parent_focus = None;
        self.parent_focus_restored = false;
        self.anchor = None;
        self.filter.text.clear();
        self.filter_query = Default::default();
        self.filter.recursive = false;
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

    fn history_step(&self, backwards: bool) -> Option<usize> {
        let available = |index: &usize| {
            let entry = &self.history[*index];
            &entry.location != self.location() && entry.is_available()
        };
        if backwards {
            (0..self.history_index).rev().find(available)
        } else {
            (self.history_index + 1..self.history.len()).find(available)
        }
    }
    fn invalidate_scan(&mut self) {
        self.error = None;
        self.archive_locked = false;
        self.generation += 1;
        self.loading = false;
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
    fn filter_entries(&mut self, hidden: bool, ignore: Option<&globset::GlobSet>) {
        #[cfg(feature = "perf")]
        let started = perf::timestamp();
        let shown = |entry: &Entry| {
            (hidden || !entry.hidden)
                && !ignore.is_some_and(|ignore| {
                    ignore.is_match(listing::fold(&entry.name, false).as_ref())
                })
        };
        let mut excluded_dirs = HashSet::new();
        if self.filter.recursive {
            excluded_dirs.extend(
                self.entries
                    .iter()
                    .filter(|entry| entry.is_dir() && !shown(entry))
                    .map(|entry| entry.path.as_path()),
            );
        }

        self.visible.clear();
        self.cursor_position = None;
        self.anchor_position = None;
        self.selected_bytes = 0;
        let mut surviving_marks = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            let path = entry.path.as_path();
            let shown = if self.filter.recursive && entry.is_dir() {
                !excluded_dirs.contains(path)
            } else {
                shown(entry)
            };
            if !shown
                || (!excluded_dirs.is_empty()
                    && path
                        .ancestors()
                        .skip(1)
                        .any(|ancestor| excluded_dirs.contains(ancestor)))
                || !self.filter_query.matches(&self.filter, entry)
            {
                continue;
            }
            let position = self.visible.len();
            self.visible.push(index);
            if self.cursor_position.is_none() && self.cursor.as_deref() == Some(path) {
                self.cursor_position = Some(position);
            }
            if self.anchor_position.is_none() && self.anchor.as_deref() == Some(path) {
                self.anchor_position = Some(position);
            }
            if !self.selected.is_empty() && self.selected.contains(path) {
                surviving_marks.push(path);
                self.selected_bytes += Self::mark_size(entry);
            }
        }
        if surviving_marks.is_empty() {
            self.selected.clear();
        } else if surviving_marks.len() != self.selected.len() {
            let surviving: HashSet<_> = surviving_marks.into_iter().collect();
            self.selected
                .retain(|path| surviving.contains(path.as_path()));
        }
        self.finish_view();
        #[cfg(feature = "perf")]
        perf::elapsed("projection", started);
    }
    fn finish_view(&mut self) {
        if self.cursor_position.is_none() {
            self.cursor = self.visible_entry(0).map(|entry| entry.path.clone());
            self.cursor_position = self.cursor.as_ref().map(|_| 0);
        }
    }
    fn select(&mut self, path: &Path, mode: Select, mark: bool) {
        let position = self.visible_entries().position(|entry| entry.path == path);
        if let Some(position) = position {
            self.select_position(position, mode, mark);
        }
    }
    fn select_position(&mut self, position: usize, mode: Select, mark: bool) {
        let index = self.visible[position];
        let path = self.entries[index].path.clone();
        match mode {
            Select::Replace | Select::Focus => {
                self.clear_marks();
                if mark {
                    let entry = &self.entries[index];
                    self.selected.insert(path.clone());
                    self.selected_bytes = Self::mark_size(entry);
                }
                self.anchor = Some(path.clone());
                self.anchor_position = Some(position);
            }
            Select::Toggle => {
                let entry = &self.entries[index];
                let mark = !self.selected.contains(&entry.path);
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
                    self.visible[range]
                        .iter()
                        .map(|&index| &self.entries[index]),
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
    control: Arc<Control>,
}

enum Event {
    Listed(u64, u64, io::Result<listing::Scan>, Option<PathBuf>),
    JobProgress(u64, Progress),
    Conflict(u64, Conflict),
    ConflictCompared(u64, PathBuf, PathBuf, io::Result<bool>),
    Done(u64, io::Result<String>, Vec<String>),
}

pub struct Engine {
    pub settings: Settings,
    pub panes: [Pane; 2],
    pub jobs: Vec<Job>,
    pub home: PathBuf,
    pub notice: Option<String>,
    ids: u64,
    ignore: Option<globset::GlobSet>,
    sender: mpsc::Sender<Event>,
    receiver: mpsc::Receiver<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
    last_refresh: Instant,
    running: Option<u64>,
    archive_cache: Arc<archives::ArchiveCache>,
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
        let ignore = listing::compile_mask(&settings.ignore, false).unwrap_or_default();
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
            wake: Arc::new(wake),
            last_refresh: Instant::now(),
            running: None,
            archive_cache: Arc::default(),
        };
        engine.start_scans();
        engine
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
    /// Report a restored cursor once so a view can scroll it into sight.
    pub fn take_parent_focus_restored(&mut self, side: Side) -> bool {
        std::mem::take(&mut self.tab_mut(side).parent_focus_restored)
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
        if let Err(error) = self.execute(command) {
            self.notice = Some(error.to_string());
        }
        self.start_scans();
        self.start_job();
    }

    pub fn set_ignore(&mut self, mask: String) -> io::Result<()> {
        self.ignore = listing::compile_mask(&mask, false)?;
        self.settings.ignore = mask;
        for side in Side::ALL {
            self.refilter(side);
        }
        Ok(())
    }

    fn execute(&mut self, command: Command) -> io::Result<()> {
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
            Command::History(side, index) => self.history(side, index)?,
            Command::Back(side) => {
                if let Some(index) = self.tab(side).history_step(true) {
                    self.history(side, index)?;
                }
            }
            Command::Forward(side) => {
                if let Some(index) = self.tab(side).history_step(false) {
                    self.history(side, index)?;
                }
            }
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
                self.navigate(side.other(), &path, false, false, false)?;
                self.settings.active = side;
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
                self.resort(side);
            }
            Command::Filter(side, mut criteria) => {
                let query = criteria.compile()?;
                let tab = self.tab_mut(side);
                criteria.recursive &= matches!(
                    tab.location(),
                    Location::Directory(_) | Location::Archive { .. }
                );
                let scope_changed = tab.filter.recursive != criteria.recursive;
                tab.filter = criteria;
                tab.filter_query = query;
                if scope_changed {
                    tab.clear_view();
                    tab.invalidate_scan();
                } else {
                    self.refilter(side);
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
                for side in Side::ALL {
                    self.refilter(side);
                }
            }
            Command::Select(side, path, mode) => {
                self.settings.active = side;
                self.tab_mut(side)
                    .select(&path, mode, !matches!(mode, Select::Focus));
            }
            Command::Step(side, step, extend) => {
                let tab = self.tab_mut(side);
                if !tab.visible.is_empty() {
                    let last = tab.visible.len() - 1;
                    let index = tab.cursor_position().unwrap_or(0);
                    let next = match step {
                        Step::Previous => index.saturating_sub(1),
                        Step::Next => (index + 1).min(last),
                        Step::First => 0,
                        Step::Last => last,
                        Step::PageUp => index.saturating_sub(12),
                        Step::PageDown => (index + 12).min(last),
                    };
                    if extend {
                        tab.select_position(next, Select::Extend, false);
                    } else {
                        let path = tab.entries[tab.visible[next]].path.clone();
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
                let matches = criteria.matching(tab.visible.iter().map(|&i| &tab.entries[i]))?;
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
                        .filter(|entry| tab.saved_marks.contains(&entry.path)),
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
                    self.navigate(side, &bookmark.path, false, false, false)?;
                    if let Some(other) = bookmark.opposite {
                        self.navigate(side.other(), &other, false, false, false)?;
                    }
                    self.settings.active = side;
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
            Command::Queue(operation, hold) => {
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
                    control: Arc::default(),
                });
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

    fn navigate(
        &mut self,
        side: Side,
        path: &Path,
        new_tab: bool,
        background: bool,
        linked: bool,
    ) -> io::Result<()> {
        let base = self.tab(side).base().to_path_buf();
        let resolved = if path == Path::new("~") {
            self.home.clone()
        } else if let Ok(suffix) = path.strip_prefix("~/") {
            self.home.join(suffix)
        } else {
            base.join(path)
        };
        if !resolved.is_dir() {
            if let Some(file) = resolved
                .ancestors()
                .find(|path| is_archive(path) && path.is_file())
            {
                let folder = resolved.strip_prefix(file).map_err(io::Error::other)?;
                if folder
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
                {
                    return Err(io::Error::other("Invalid archive folder"));
                }
                let file = file.to_owned();
                let tab = self.tab(side);
                let previous_folder = match tab.location() {
                    Location::Archive {
                        file: previous_file,
                        folder: previous_folder,
                    } if previous_file == &file && previous_folder.parent() == Some(folder) => {
                        Some(previous_file.join(previous_folder))
                    }
                    _ => None,
                };
                let location = Location::Archive {
                    file,
                    folder: folder.into(),
                };
                let opens_tab = new_tab
                    || matches!(tab.saved.lock, TabLock::Locked | TabLock::NewTab)
                        && tab.location() != &location;
                if opens_tab {
                    let mut saved = default_tab(location.base().to_owned());
                    saved.location = location;
                    self.add_tab(side, saved, background);
                } else {
                    let tab = self.tab_mut(side);
                    tab.set_location(location, true);
                    tab.pending_parent_focus = previous_folder;
                    self.settings.active = side;
                }
                return Ok(());
            }
            return Err(io::Error::other(format!(
                "{} is unavailable or is not a directory",
                resolved.display()
            )));
        }
        let path = resolved.canonicalize()?;
        let tab = self.tab(side);
        let previous = tab.base().to_path_buf();
        let parent_focus = tab
            .entry_in_parent(&resolved)
            .or_else(|| tab.entry_in_parent(&path))
            .and_then(Path::file_name)
            .map(|name| path.join(name));
        let location = Location::Directory(path.clone());
        let location_changed = tab.location() != &location;
        let lock = tab.saved.lock;
        let opens_tab = new_tab
            || matches!(lock, TabLock::Locked | TabLock::NewTab)
                && (path != previous || matches!(tab.location(), Location::Archive { .. }));
        let tab = if opens_tab {
            self.add_tab(side, default_tab(path.clone()), background)
        } else {
            self.settings.active = side;
            let tab = self.tab_mut(side);
            tab.set_location(location, true);
            tab
        };
        tab.pending_parent_focus = parent_focus;
        if opens_tab || location_changed {
            tab.history[tab.history_index].entered_via = Some(resolved);
        }
        if self.settings.linked
            && linked
            && !background
            && !opens_tab
            && !matches!(self.tab(side.other()).location(), Location::Archive { .. })
        {
            let target = self.tab(side.other()).base();
            let paired = if let Some(parent) = previous.parent().filter(|parent| parent == &path) {
                let _ = parent;
                target.parent().map(Path::to_path_buf)
            } else {
                path.strip_prefix(&previous)
                    .ok()
                    .map(|suffix| target.join(suffix))
            };
            if let Some(paired) = paired
                && paired.is_dir()
            {
                let _ = self.navigate(side.other(), &paired, false, false, false);
                self.settings.active = side;
            }
        }
        Ok(())
    }
    fn history(&mut self, side: Side, index: usize) -> io::Result<()> {
        let tab = self.tab_mut(side);
        if let Some(entry) = tab.history.get(index) {
            if !entry.is_available() {
                return Err(io::Error::other("This history location is unavailable"));
            }
            let HistoryEntry {
                location,
                entered_via,
            } = entry.clone();
            let parent_focus = match &location {
                Location::Directory(parent) => tab.entry_in_parent(parent).map(Path::to_path_buf),
                _ => None,
            };
            let tab = if matches!(tab.saved.lock, TabLock::Locked | TabLock::NewTab)
                && (location.base() != tab.base()
                    || (matches!(&location, Location::Archive { .. })
                        || matches!(tab.location(), Location::Archive { .. }))
                        && &location != tab.location())
            {
                let mut saved = default_tab(location.base().to_path_buf());
                if matches!(&location, Location::Archive { .. }) {
                    saved.location = location;
                }
                let tab = self.add_tab(side, saved, false);
                tab.history[0].entered_via = entered_via;
                tab
            } else {
                tab.history_index = index;
                tab.set_location(location, false);
                tab
            };
            tab.pending_parent_focus = parent_focus;
            self.settings.active = side;
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
        let pane = self.pane_mut(side);
        if index >= pane.tabs.len() {
            return;
        }
        if pane.active != index {
            let tab = pane.tab_mut();
            if tab.saved.lock == TabLock::Reset && tab.saved.home != tab.base() {
                tab.set_location(Location::Directory(tab.saved.home.clone()), true);
            }
            pane.active = index;
        }
        self.settings.active = side;
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
    fn resort(&mut self, side: Side) {
        let hidden = self.settings.show_hidden;
        let first = self.settings.directories_first;
        let natural = self.settings.natural_sort;
        let tab = self.panes[side.index()].tab_mut();
        listing::sort(&mut tab.entries, &tab.saved.sort, first, natural);
        tab.filter_entries(hidden, self.ignore.as_ref());
    }
    fn refilter(&mut self, side: Side) {
        self.panes[side.index()]
            .tab_mut()
            .filter_entries(self.settings.show_hidden, self.ignore.as_ref());
    }
    fn mark_dirty(&mut self) {
        for pane in &mut self.panes {
            for tab in &mut pane.tabs {
                tab.dirty = true;
            }
        }
    }
    fn start_scans(&mut self) {
        for pane in &mut self.panes {
            let tab = pane.tab_mut();
            if tab.loading || !tab.dirty {
                continue;
            }
            tab.loading = true;
            tab.dirty = false;
            let (id, generation, location) = (tab.id, tab.generation, tab.saved.location.clone());
            let recursive = tab.filter.recursive;
            let archive_cache = self.archive_cache.clone();
            let (sender, wake) = (self.sender.clone(), self.wake.clone());
            #[cfg(feature = "perf")]
            let scheduled = perf::timestamp();
            std::thread::spawn(move || {
                #[cfg(feature = "perf")]
                perf::elapsed("worker_start", scheduled);
                let result = match &location {
                    Location::Archive { file, folder } => {
                        archives::scan(&archive_cache, file, folder, recursive)
                    }
                    _ => listing::scan(&location, recursive),
                };
                let fallback = match (&location, &result) {
                    (Location::Directory(path), Err(error))
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) =>
                    {
                        path.ancestors()
                            .skip(1)
                            .find(|parent| parent.is_dir())
                            .map(Path::to_owned)
                    }
                    _ => None,
                };
                let _ = sender.send(Event::Listed(id, generation, result, fallback));
                #[cfg(feature = "perf")]
                perf::elapsed("publication", scheduled);
                wake();
            });
        }
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
            let result = worker.run(operation);
            let _ = sender.send(Event::Done(id, result, worker.warnings));
            wake();
        });
    }

    pub fn poll(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Event::Listed(id, generation, result, fallback) => {
                    for pane in &mut self.panes {
                        let Some(tab) = pane
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.id == id && tab.generation == generation)
                        else {
                            continue;
                        };
                        tab.loading = false;
                        let parent_focus = tab.pending_parent_focus.take();
                        match result {
                            Ok(listing) => {
                                #[cfg(feature = "perf")]
                                let install = perf::timestamp();
                                tab.entries = listing.entries;
                                tab.skipped = listing.skipped;
                                tab.error = None;
                                tab.archive_locked = false;
                                listing::sort(
                                    &mut tab.entries,
                                    &tab.saved.sort,
                                    self.settings.directories_first,
                                    self.settings.natural_sort,
                                );
                                if let Some(path) = &parent_focus {
                                    tab.cursor = Some(path.clone());
                                }
                                tab.filter_entries(self.settings.show_hidden, self.ignore.as_ref());
                                if parent_focus.is_some() && parent_focus == tab.cursor {
                                    tab.anchor.clone_from(&tab.cursor);
                                    tab.anchor_position = tab.cursor_position;
                                    tab.parent_focus_restored = true;
                                }
                                #[cfg(feature = "perf")]
                                perf::elapsed("install", install);
                            }
                            Err(error) => {
                                if let Some(parent) = fallback {
                                    tab.recover_directory(parent);
                                    break;
                                }
                                tab.archive_locked = archives::is_password_error(&error);
                                tab.error = Some(error.to_string());
                                tab.clear_view();
                                tab.clear_marks();
                                tab.cursor = None;
                            }
                        }
                        break;
                    }
                }
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
                Event::Done(id, result, warnings) => {
                    if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
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
                    self.running = None;
                    self.mark_dirty();
                }
            }
        }
        if self.settings.auto_refresh && self.last_refresh.elapsed() >= Duration::from_secs(4) {
            self.last_refresh = Instant::now();
            for pane in &mut self.panes {
                let tab = pane.tab_mut();
                if !tab.filter.recursive
                    && !matches!(
                        tab.location(),
                        Location::Flat { .. } | Location::Archive { .. }
                    )
                {
                    tab.dirty = true;
                }
            }
        }
        self.start_scans();
        self.start_job();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        for job in &self.jobs {
            job.control.cancel();
        }
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
