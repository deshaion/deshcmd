//! Immutable scan data and the per-tab order and projection built from it.
use std::collections::{HashMap, HashSet, hash_map::RandomState};
use std::hash::BuildHasher;
use std::io;
use std::path::Path;
use std::sync::{Arc, mpsc};

use crate::{Entry, MatchOptions, Sort, listing, read_pool::Cancellation};

const ABSENT: usize = usize::MAX;

pub(crate) type Retire = mpsc::SyncSender<Vec<Entry>>;

pub(crate) fn reclaimer() -> Retire {
    let (send, receive) = mpsc::sync_channel::<Vec<Entry>>(4);
    std::thread::spawn(move || {
        while let Ok(entries) = receive.recv() {
            drop(entries);
        }
    });
    send
}

#[derive(Default)]
pub(crate) struct Raw {
    pub entries: Vec<Entry>,
    pub skipped: usize,
    pub partial: bool,
    pub directory: Option<listing::DirectoryIdentity>,
    pub resolved_directory: Option<Arc<Path>>,
    pub has_hard_links: bool,
    pub links: usize,
    #[cfg(feature = "perf")]
    sampled_bytes: usize,
    by_path: HashMap<u64, usize>,
    bytes: usize,
    hash: RandomState,
    retire: Option<Retire>,
}

impl Raw {
    pub fn new(
        scan: listing::Scan,
        partial: bool,
        retire: Retire,
        cancel: &Cancellation,
    ) -> io::Result<Self> {
        let mut raw = Self {
            by_path: HashMap::with_capacity(scan.entries.len()),
            #[cfg(feature = "perf")]
            sampled_bytes: crate::perf::snapshot_created(scan.entries.capacity()),
            entries: scan.entries,
            partial,
            directory: None,
            resolved_directory: None,
            has_hard_links: scan.has_hard_links,
            links: 0,
            skipped: scan.skipped,
            retire: Some(retire),
            bytes: 0,
            hash: RandomState::new(),
        };
        raw.bytes = size_of::<Self>()
            + raw.entries.capacity() * size_of::<Entry>()
            + raw.by_path.capacity() * (size_of::<(u64, usize)>() + 1);
        for (index, entry) in raw.entries.iter().enumerate() {
            cancel.check()?;
            raw.links += usize::from(entry.link);
            raw.bytes += entry.path.capacity()
                + entry.name.capacity()
                + entry.extension.capacity()
                + entry.comment.capacity()
                + entry.attributes.capacity();
            raw.by_path
                .entry(raw.hash.hash_one(&entry.path))
                .and_modify(|index| *index = ABSENT)
                .or_insert(index);
        }
        Ok(raw)
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn index(&self, path: &Path) -> Option<usize> {
        let &index = self.by_path.get(&self.hash.hash_one(path))?;
        if index == ABSENT {
            // Native path equality remains authoritative in the event of a collision.
            self.entries.iter().position(|entry| entry.path == path)
        } else {
            (self.entries[index].path == path).then_some(index)
        }
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        #[cfg(feature = "perf")]
        crate::perf::snapshot_dropped(self.sampled_bytes);
        if self.entries.len() >= 4096
            && let Some(retire) = &self.retire
        {
            // The queue bounds retained snapshots if destruction falls behind.
            let _ = retire.try_send(std::mem::take(&mut self.entries));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ViewKey {
    pub tab: u64,
    pub generation: u64,
    pub raw: u64,
    pub revision: u64,
}

#[derive(Clone, Default)]
pub(crate) struct View {
    pub raw: Arc<Raw>,
    pub order: Arc<Vec<usize>>,
    pub sort_revision: u64,
    pub eligibility_revision: u64,
    pub visible: Arc<Vec<usize>>,
    positions: Arc<Vec<usize>>,
}

impl View {
    pub fn index_bytes(&self) -> usize {
        (self.order.capacity() + self.visible.capacity() + self.positions.capacity())
            * size_of::<usize>()
    }

    pub fn position(&self, path: &Path) -> Option<usize> {
        let position = self.positions[self.raw.index(path)?];
        (position != ABSENT).then_some(position)
    }
}

pub(crate) struct Options {
    pub sort: Vec<Sort>,
    pub directories_first: bool,
    pub natural: bool,
    pub hidden: bool,
    pub ignore: Option<Arc<globset::GlobSet>>,
    pub criteria: Arc<MatchOptions>,
    pub query: Arc<listing::MatchQuery>,
    pub sort_revision: u64,
    pub eligibility_revision: u64,
}

pub(crate) struct ViewSpec {
    sort: Vec<Sort>,
    directories_first: bool,
    natural: bool,
    hidden: bool,
    ignore: Option<Arc<globset::GlobSet>>,
    criteria: Arc<MatchOptions>,
}

impl ViewSpec {
    pub fn matches(&self, options: &Options) -> bool {
        self.sort == options.sort
            && self.directories_first == options.directories_first
            && self.natural == options.natural
            && self.hidden == options.hidden
            && self.criteria == options.criteria
            && match (&self.ignore, &options.ignore) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }

    pub fn bytes(&self) -> usize {
        self.sort.capacity() * size_of::<Sort>() + self.criteria.text.capacity()
    }
}

impl Options {
    pub fn spec(self) -> ViewSpec {
        ViewSpec {
            sort: self.sort,
            directories_first: self.directories_first,
            natural: self.natural,
            hidden: self.hidden,
            ignore: self.ignore,
            criteria: self.criteria,
        }
    }

    pub fn shown(&self, entry: &Entry) -> bool {
        self.shown_name(&entry.name, entry.hidden)
    }

    pub fn shown_name(&self, name: &str, hidden: bool) -> bool {
        (self.hidden || !hidden)
            && !self
                .ignore
                .as_ref()
                .is_some_and(|ignore| ignore.is_match(listing::fold(name, false).as_ref()))
    }

    pub fn prepare(
        self,
        raw: Arc<Raw>,
        order: Option<(Arc<Vec<usize>>, bool)>,
        cancel: &Cancellation,
    ) -> io::Result<View> {
        let order = match order {
            Some((order, true)) => order,
            previous => {
                let order = listing::sorted_order_from(
                    &raw.entries,
                    &self.sort,
                    self.directories_first,
                    self.natural,
                    previous.as_ref().map(|(order, _)| order.as_slice()),
                    cancel,
                )?;
                #[cfg(feature = "perf")]
                crate::perf::view_sort(raw.partial, order.len());
                Arc::new(order)
            }
        };
        #[cfg(feature = "perf")]
        let started = crate::perf::timestamp();
        let mut excluded_dirs = HashSet::new();
        if self.criteria.recursive {
            for entry in &raw.entries {
                cancel.check()?;
                if entry.is_dir() && !self.shown(entry) {
                    excluded_dirs.insert(entry.path.as_path());
                }
            }
        }
        let mut visible = Vec::new();
        let mut positions = vec![ABSENT; raw.entries.len()];
        for &index in order.iter() {
            cancel.check()?;
            let entry = &raw.entries[index];
            let shown = if self.criteria.recursive && entry.is_dir() {
                !excluded_dirs.contains(entry.path.as_path())
            } else {
                self.shown(entry)
            };
            if shown
                && (excluded_dirs.is_empty()
                    || !entry
                        .path
                        .ancestors()
                        .skip(1)
                        .any(|path| excluded_dirs.contains(path)))
                && self.query.matches(&self.criteria, entry)
            {
                positions[index] = visible.len();
                visible.push(index);
            }
        }
        #[cfg(feature = "perf")]
        crate::perf::elapsed("projection", started);
        Ok(View {
            raw,
            order,
            sort_revision: self.sort_revision,
            eligibility_revision: self.eligibility_revision,
            visible: Arc::new(visible),
            positions: Arc::new(positions),
        })
    }
}

pub(crate) type Marks = Arc<HashSet<Arc<Path>>>;

pub(crate) struct Reconciled {
    pub source: Marks,
    pub retained: Marks,
    pub bytes: u128,
}

pub(crate) struct Prepared {
    pub view: View,
    pub marks: Option<Reconciled>,
}

impl Prepared {
    pub fn new(view: View, marks: Option<Marks>, cancel: &Cancellation) -> io::Result<Self> {
        let marks = marks
            .map(|source| view.reconcile(source, cancel))
            .transpose()?;
        Ok(Self { view, marks })
    }
}

impl View {
    pub fn reconcile(&self, source: Marks, cancel: &Cancellation) -> io::Result<Reconciled> {
        let mut bytes = 0;
        let mut removed = None;
        for path in source.iter() {
            cancel.check()?;
            if let Some(position) = self.position(path) {
                bytes += crate::Tab::mark_size(&self.raw.entries[self.visible[position]]);
            } else {
                removed.get_or_insert_with(Vec::new).push(path.clone());
            }
        }
        let retained = if let Some(removed) = removed {
            if removed.len() == source.len() {
                Marks::default()
            } else {
                let mut retained = source.clone();
                let retained_set = Arc::make_mut(&mut retained);
                for path in removed {
                    cancel.check()?;
                    retained_set.remove(path.as_ref());
                }
                retained
            }
        } else {
            source.clone()
        };
        Ok(Reconciled {
            source,
            retained,
            bytes,
        })
    }
}

/// An ordinary scan can deliver one small page without occupying a final-result slot.
#[derive(Default)]
pub(crate) struct FirstPages(std::sync::Mutex<PageState>);

#[derive(Default)]
struct PageState {
    active: [crate::ScanKey; 2],
    ready: Vec<(crate::ScanKey, Arc<Raw>)>,
}

impl FirstPages {
    pub fn set_active(&self, active: [crate::ScanKey; 2]) {
        let mut state = self.0.lock().unwrap();
        state.active = active;
        state.ready.retain(|(key, _)| active.contains(key));
    }

    pub fn expect(&self, slot: usize, key: crate::ScanKey) {
        let mut state = self.0.lock().unwrap();
        let previous = std::mem::replace(&mut state.active[slot], key);
        if !state.active.contains(&previous) {
            state.ready.retain(|(key, _)| *key != previous);
        }
    }

    pub fn publish(&self, key: crate::ScanKey, raw: Arc<Raw>, cancel: &Cancellation) -> bool {
        let mut state = self.0.lock().unwrap();
        if cancel.cancelled()
            || !state.active.contains(&key)
            || state.ready.iter().any(|(ready, _)| *ready == key)
        {
            return false;
        }
        state.ready.push((key, raw));
        true
    }

    pub fn drain(&self) -> Vec<(crate::ScanKey, Arc<Raw>)> {
        std::mem::take(&mut self.0.lock().unwrap().ready)
    }
}
