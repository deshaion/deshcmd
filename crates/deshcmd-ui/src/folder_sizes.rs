use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use deshcmd_engine::{self as engine, ArchiveSource, Entry, Tab};
use eframe::egui;

use crate::{probes::ReadCache, theme::metric};

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    modified: Option<SystemTime>,
    archive_revision: Option<u64>,
}

impl From<&Entry> for Key {
    fn from(entry: &Entry) -> Self {
        Self {
            path: entry.path.clone(),
            modified: entry.modified,
            archive_revision: entry.archive.as_ref().map(|source| source.revision),
        }
    }
}

type Folder = (Key, Option<Arc<ArchiveSource>>);

#[derive(Default)]
pub(crate) struct FolderSizes {
    cache: Option<ReadCache<Key, Option<u64>>>,
    scope: Option<(u64, u64)>,
    visible_revision: Option<(u64, u64)>,
    marks: Option<Weak<HashSet<Arc<Path>>>>,
    folders: Vec<Folder>,
    footer: Option<String>,
}

impl FolderSizes {
    pub fn update(&mut self, ctx: &egui::Context, tab: &Tab) {
        let scope = (tab.id, tab.listing_generation());
        if self.scope != Some(scope) {
            self.scope = Some(scope);
            self.marks = None;
            if let Some(cache) = &mut self.cache {
                cache.clear();
            }
        }

        let revision = tab.visible_revision();
        let marks = tab.mark_snapshot();
        let snapshot = Arc::downgrade(marks);
        if self.visible_revision != revision
            || self
                .marks
                .as_ref()
                .is_none_or(|previous| !Weak::ptr_eq(previous, &snapshot))
        {
            self.visible_revision = revision;
            self.marks = Some(snapshot);
            self.folders = marks
                .iter()
                .filter_map(|path| {
                    tab.visible_position(path)
                        .and_then(|position| tab.visible_entry(position))
                })
                .filter(|entry| entry.is_dir())
                .map(|entry| (Key::from(entry), entry.archive.clone()))
                .collect();
        }

        if let Some(cache) = &mut self.cache {
            cache.poll();
        }
        let current = tab.current().filter(|entry| entry.is_dir());
        let current_size =
            current.map(|entry| self.request(ctx, &Key::from(entry), &entry.archive));
        self.footer = if tab.marked_count() == 0 {
            current_size.map(|size| match size {
                Some(Some(bytes)) => metric(bytes),
                Some(None) => "Size unavailable".into(),
                None => "Calculating size…".into(),
            })
        } else {
            let mut bytes = tab.selected_bytes();
            let mut pending = false;
            let mut unavailable = false;
            for (key, archive) in &self.folders {
                // Prioritize the current folder when it is also marked.
                let size = if current.is_some_and(|entry| entry.path == key.path) {
                    current_size.flatten()
                } else {
                    request(
                        self.cache.get_or_insert_with(|| folder_cache(ctx)),
                        key,
                        archive,
                    )
                };
                match size {
                    Some(Some(size)) => bytes = bytes.saturating_add(size),
                    Some(None) => unavailable = true,
                    None => pending = true,
                }
            }
            Some(if pending {
                "Calculating size…".into()
            } else if unavailable {
                format!("{} · size incomplete", metric(bytes))
            } else {
                metric(bytes)
            })
        };
    }

    fn request(
        &mut self,
        ctx: &egui::Context,
        key: &Key,
        archive: &Option<Arc<ArchiveSource>>,
    ) -> Option<Option<u64>> {
        request(
            self.cache.get_or_insert_with(|| folder_cache(ctx)),
            key,
            archive,
        )
    }

    pub fn column(&self, tab: &Tab, entry: &Entry) -> String {
        if let Some(size) = self
            .cache
            .as_ref()
            .and_then(|cache| cache.get(&Key::from(entry)))
        {
            return size.map_or_else(|| "—".into(), metric);
        }
        if tab.is_marked(&entry.path)
            || tab
                .current()
                .is_some_and(|current| current.path == entry.path)
        {
            "…".into()
        } else {
            "—".into()
        }
    }

    pub fn footer(&self) -> Option<&str> {
        self.footer.as_deref()
    }
}

// The cache is cleared when the panel changes listings. Keep all marked folders
// eligible for the total, including selections larger than the read queue.
fn folder_cache(ctx: &egui::Context) -> ReadCache<Key, Option<u64>> {
    ReadCache::new(ctx, 2, Duration::from_secs(15), usize::MAX)
}

fn request(
    cache: &mut ReadCache<Key, Option<u64>>,
    key: &Key,
    archive: &Option<Arc<ArchiveSource>>,
) -> Option<Option<u64>> {
    let archive = archive.clone();
    cache
        .request_cancellable(key.clone(), move |key, cancel| {
            let result = if let Some(archive) = archive {
                archive.folder_size(&key.path, cancel)
            } else {
                engine::folder_size(&key.path, cancel)
            };
            Some(result.ok())
        })
        .as_deref()
        .copied()
}
