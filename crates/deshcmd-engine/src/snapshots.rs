//! Retained ordinary-directory snapshots. Active tabs own their published views independently.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::views::{Options, Raw, View, ViewSpec};

pub(crate) const UNWATCHED_INTERVAL: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, Default)]
pub struct ListingStats {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub full_scans: u64,
    pub patches: u64,
    pub retained_bytes: usize,
    pub retained_directories: usize,
}

struct Snapshot {
    raw: Arc<Raw>,
    ready: Option<(ViewSpec, View)>,
    validated: Option<Instant>,
    used: u64,
    bytes: usize,
}

pub(crate) struct Cache {
    entries: HashMap<PathBuf, Snapshot>,
    stats: ListingStats,
    time: u64,
    byte_limit: usize,
    directory_limit: usize,
}

impl Default for Cache {
    fn default() -> Self {
        Self::new(64 * 1024 * 1024, 32)
    }
}

impl Cache {
    fn new(byte_limit: usize, directory_limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            stats: ListingStats::default(),
            time: 0,
            byte_limit,
            directory_limit,
        }
    }

    pub fn stats(&self) -> ListingStats {
        ListingStats {
            retained_directories: self.entries.len(),
            ..self.stats
        }
    }

    pub fn record_scan(&mut self) {
        self.stats.full_scans += 1;
    }

    pub fn record_patch(&mut self) {
        self.stats.patches += 1;
    }

    pub fn get(
        &mut self,
        path: &Path,
        options: &Options,
    ) -> Option<(Arc<Raw>, Option<View>, Option<Instant>)> {
        let Some(snapshot) = self.entries.get_mut(path) else {
            self.stats.cache_misses += 1;
            return None;
        };
        self.time += 1;
        snapshot.used = self.time;
        self.stats.cache_hits += 1;
        let ready = snapshot
            .ready
            .as_ref()
            .filter(|(spec, _)| spec.matches(options))
            .map(|(_, view)| {
                let mut view = view.clone();
                view.sort_revision = options.sort_revision;
                view.eligibility_revision = options.eligibility_revision;
                view
            });
        Some((snapshot.raw.clone(), ready, snapshot.validated))
    }

    pub fn ready(&self, path: &Path, options: &Options) -> Option<&View> {
        let (spec, view) = self.entries.get(path)?.ready.as_ref()?;
        spec.matches(options).then_some(view)
    }

    pub fn remember(&mut self, path: &Path, raw: Arc<Raw>, validated: Option<Instant>) {
        self.remove(path);
        let resolved_bytes = raw
            .resolved_directory
            .as_ref()
            .map_or(0, |resolved| resolved.as_os_str().len());
        let bytes = raw.bytes() + path.as_os_str().len() + resolved_bytes + size_of::<Snapshot>();
        if bytes > self.byte_limit {
            return;
        }
        self.time += 1;
        self.stats.retained_bytes += bytes;
        self.entries.insert(
            path.to_owned(),
            Snapshot {
                raw,
                ready: None,
                validated,
                used: self.time,
                bytes,
            },
        );
        self.evict();
    }

    pub fn remember_view(&mut self, path: &Path, options: Options, view: &View) {
        if let Some(snapshot) = self.entries.get_mut(path)
            && Arc::ptr_eq(&snapshot.raw, &view.raw)
        {
            let previous = snapshot.ready.take();
            let previous_bytes = previous
                .as_ref()
                .map_or(0, |(spec, view)| spec.bytes() + view.index_bytes());
            let spec = options.spec();
            let bytes = view.index_bytes() + spec.bytes();
            snapshot.bytes -= previous_bytes;
            self.stats.retained_bytes -= previous_bytes;
            if snapshot.bytes + bytes <= self.byte_limit {
                snapshot.bytes += bytes;
                self.stats.retained_bytes += bytes;
                snapshot.ready = Some((spec, view.clone()));
            }
            self.evict();
        }
    }

    pub fn invalidate(&mut self, affected: impl Fn(&Path, Option<&Path>) -> bool) {
        for (path, snapshot) in &mut self.entries {
            if affected(path, snapshot.raw.resolved_directory.as_deref()) {
                snapshot.validated = None;
            }
        }
    }

    pub fn remove(&mut self, path: &Path) {
        if let Some(snapshot) = self.entries.remove(path) {
            self.stats.retained_bytes -= snapshot.bytes;
        }
    }

    fn evict(&mut self) {
        while self.stats.retained_bytes > self.byte_limit
            || self.entries.len() > self.directory_limit
        {
            let path = self
                .entries
                .iter()
                .min_by_key(|(_, value)| value.used)
                .unwrap()
                .0
                .clone();
            self.remove(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_releases_cache_ownership_and_skips_oversized_snapshots() {
        let raw = Arc::new(Raw::default());
        let mut cache = Cache::new(1024, 1);
        cache.remember(Path::new("/a"), raw.clone(), Some(Instant::now()));
        cache.remember(Path::new("/b"), Arc::new(Raw::default()), None);
        assert_eq!(Arc::strong_count(&raw), 1);
        assert_eq!(cache.stats().retained_directories, 1);
        assert!(cache.stats().retained_bytes <= 1024);
        let mut small = Cache::new(1, 32);
        small.remember(Path::new("/large"), raw.clone(), None);
        assert_eq!(small.stats().retained_bytes, 0);
        assert_eq!(Arc::strong_count(&raw), 1);
    }
}
