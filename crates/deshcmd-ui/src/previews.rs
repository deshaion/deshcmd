use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use deshcmd_engine::{
    self as engine, ArchiveSource, Entry, Kind, Preview, Tab,
    read_pool::{Cancellation, ReadPool},
};
use eframe::egui::{self, ColorImage, TextureHandle, vec2};

pub(crate) const QUICK_PIXELS: u32 = 720;
pub(crate) const GALLERY_PIXELS: u32 = 176;
const LOOKAHEAD: usize = 4;
const LOOKBEHIND: usize = 2;
const NEARBY_ROWS: usize = 48;
const MIN_PRELOAD_ROWS: usize = 32;
const MAX_PRELOAD_ROWS: usize = 64;
const WARM_CACHE_BYTES: usize = 16 * 1024 * 1024;
const SPECULATIVE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Hash)]
enum Revision {
    Local(Option<SystemTime>, u64),
    Archive(u64),
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Key {
    path: PathBuf,
    revision: Revision,
    pixels: u32,
}

impl Key {
    fn new(entry: &Entry, pixels: u32) -> Self {
        Self {
            path: entry.path.clone(),
            revision: entry
                .archive
                .as_ref()
                .map_or(Revision::Local(entry.modified, entry.size), |archive| {
                    Revision::Archive(archive.revision)
                }),
            pixels,
        }
    }
}

pub(crate) enum CachedPreview {
    Image(TextureHandle, egui::Vec2),
    Text(String),
    None,
}

type Decode =
    dyn Fn(&Key, Kind, Option<&ArchiveSource>, &Cancellation, bool) -> Preview + Send + Sync;
type Pool = ReadPool<u64, Option<Preview>>;

pub(crate) struct Previews {
    ctx: egui::Context,
    decoder: Arc<Decode>,
    // Keep focused requests independent of speculative decodes and Gallery thumbnails.
    focus_pool: Pool,
    prediction_pool: Pool,
    gallery_pool: Pool,
    focus: Option<Key>,
    focus_pending: bool,
    warm_pending: HashMap<Key, u64>,
    warm_rejected: HashSet<Key>,
    page_done: HashSet<Key>,
    disk_enabled: bool,
    warm_cache: VecDeque<(Key, Preview)>,
    warm_bytes: usize,
    prefetch: bool,
    last_focus: Option<(u64, PathBuf)>,
    backwards: bool,
    gallery_pending: HashMap<Key, u64>,
    gallery_visible: HashSet<u64>,
    gallery_ready: Option<(u64, Preview)>,
    next_id: u64,
    pub(super) cached: HashMap<Key, CachedPreview>,
    order: VecDeque<Key>,
}

impl Previews {
    pub(crate) fn new(ctx: egui::Context, cache_directory: Option<PathBuf>) -> Self {
        let disk = cache_directory
            .filter(|_| std::env::var_os("DESHCMD_DISABLE_PREVIEW_DISK_CACHE").is_none())
            .map(engine::PreviewDiskCache::new);
        let disk_enabled = disk.is_some();
        let mut previews = Self::with_decoder(
            ctx,
            Arc::new(move |key, kind, archive, cancel, background| {
                if let Some(archive) = archive {
                    return key
                        .path
                        .strip_prefix(&archive.file)
                        .map_or(Preview::Unavailable, |member| {
                            archive.preview_cancellable(member, kind, key.pixels, cancel)
                        });
                }
                if kind == Kind::Image
                    && key.pixels == QUICK_PIXELS
                    && let Some(disk) = &disk
                {
                    return disk.preview(&key.path, key.pixels, cancel, background);
                }
                engine::preview_cancellable(&key.path, kind, key.pixels, cancel)
            }),
        );
        previews.disk_enabled = disk_enabled;
        previews
    }

    fn with_decoder(ctx: egui::Context, decoder: Arc<Decode>) -> Self {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new({
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        });
        Self {
            ctx,
            decoder,
            focus_pool: Pool::new(1, 1, wake.clone()),
            prediction_pool: Pool::new(
                2,
                MAX_PRELOAD_ROWS + 1 + LOOKAHEAD + LOOKBEHIND,
                wake.clone(),
            ),
            gallery_pool: Pool::new(1, 32, wake),
            focus: None,
            focus_pending: false,
            warm_pending: HashMap::new(),
            warm_rejected: HashSet::new(),
            page_done: HashSet::new(),
            disk_enabled: false,
            warm_cache: VecDeque::new(),
            warm_bytes: 0,
            prefetch: std::env::var_os("DESHCMD_DISABLE_PREVIEW_PREFETCH").is_none(),
            last_focus: None,
            backwards: false,
            gallery_pending: HashMap::new(),
            gallery_visible: HashSet::new(),
            gallery_ready: None,
            next_id: 0,
            cached: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(crate) fn update(&mut self, tab: &Tab, open: bool, page: Option<Range<usize>>) {
        let focused = tab.current();
        let mut neighbors = Vec::new();
        if let (Some(position), Some(entry)) = (tab.cursor_position(), focused) {
            match &self.last_focus {
                Some((id, old)) if *id == tab.id => {
                    if let Some(previous) = tab.visible_position(old)
                        && previous != position
                    {
                        self.backwards = position < previous;
                    }
                }
                _ => self.backwards = false,
            }
            if self.prefetch {
                let previewable = |entry: &&Entry| {
                    entry.kind == Kind::Image
                        || open && matches!(entry.kind, Kind::Text | Kind::Code)
                };
                let forward = (position + 1..tab.visible_count())
                    .take(NEARBY_ROWS)
                    .filter_map(|i| tab.visible_entry(i))
                    .filter(previewable);
                let backward = (0..position)
                    .rev()
                    .take(NEARBY_ROWS)
                    .filter_map(|i| tab.visible_entry(i))
                    .filter(previewable);
                if self.backwards {
                    neighbors.extend(backward.take(LOOKAHEAD));
                    neighbors.extend(forward.take(LOOKBEHIND));
                } else {
                    neighbors.extend(forward.take(LOOKAHEAD));
                    neighbors.extend(backward.take(LOOKBEHIND));
                }
            }
            if self
                .last_focus
                .as_ref()
                .is_none_or(|(id, path)| *id != tab.id || *path != entry.path)
            {
                self.last_focus = Some((tab.id, entry.path.clone()));
            }
        } else {
            self.last_focus = None;
        }
        let page: Vec<_> = page
            .filter(|_| self.prefetch && self.disk_enabled)
            .into_iter()
            .flat_map(|rows| {
                let count = rows.len().clamp(MIN_PRELOAD_ROWS, MAX_PRELOAD_ROWS);
                rows.start..rows.start.saturating_add(count).min(tab.visible_count())
            })
            .filter_map(|index| tab.visible_entry(index))
            .filter(|entry| entry.kind == Kind::Image && entry.archive.is_none())
            .collect();
        self.update_quick(
            focused.filter(|entry| !entry.is_dir()),
            &neighbors,
            open,
            &page,
        );
        self.poll_gallery();
    }

    fn poll_gallery(&mut self) {
        self.gallery_ready = self
            .gallery_pool
            .drain()
            .pop()
            .and_then(|(id, result)| result.map(|preview| (id, preview)));
    }

    fn update_quick(
        &mut self,
        focused: Option<&Entry>,
        neighbors: &[&Entry],
        open: bool,
        page: &[&Entry],
    ) {
        let current = focused.map(|entry| Key::new(entry, QUICK_PIXELS));
        let focus = current.as_ref().filter(|_| open);
        if self.focus.as_ref() != focus {
            self.focus_pool.retain(|_| false);
            self.focus_pending = false;
            self.focus = focus.cloned();
        }

        let mut warm: Vec<_> = focused
            .into_iter()
            .filter(|entry| self.prefetch && !open && entry.kind == Kind::Image)
            .chain(neighbors.iter().copied().filter(|_| self.prefetch))
            .map(|entry| (Key::new(entry, QUICK_PIXELS), entry))
            .collect();
        let near = warm.len();
        let mut wanted = HashSet::with_capacity(warm.len() + page.len());
        wanted.extend(warm.iter().map(|(key, _)| key.clone()));
        let page_keys: HashSet<_> = page
            .iter()
            .map(|entry| Key::new(entry, QUICK_PIXELS))
            .collect();
        self.page_done.retain(|key| page_keys.contains(key));
        for entry in page {
            let key = Key::new(entry, QUICK_PIXELS);
            if focus != Some(&key) && wanted.insert(key.clone()) {
                warm.push((key, entry));
            }
        }
        self.warm_rejected
            .retain(|key| focus == Some(key) || wanted.contains(key));
        self.warm_pending
            .retain(|key, _| focus == Some(key) || wanted.contains(key));
        self.prediction_pool
            .retain(|id| self.warm_pending.values().any(|&wanted| id == wanted));

        if let Some((_, result)) = self.focus_pool.drain().pop() {
            self.focus_pending = false;
            if let (Some(key), Some(preview)) = (focus, result) {
                self.cache(key.clone(), preview);
            }
        }
        for (id, result) in self.prediction_pool.drain() {
            let key = self
                .warm_pending
                .iter()
                .find(|(_, pending)| **pending == id)
                .map(|(key, _)| key.clone());
            if let Some(key) = key {
                self.warm_pending.remove(&key);
                if page_keys.contains(&key) {
                    self.page_done.insert(key.clone());
                }
                if let Some(preview) = result {
                    if focus == Some(&key)
                        || warm[..near].iter().any(|(candidate, _)| candidate == &key)
                    {
                        self.store_warm(key, preview);
                    }
                } else {
                    self.warm_rejected.insert(key);
                }
            }
        }

        if let (Some(entry), Some(key)) = (focused, focus)
            && !self.cached.contains_key(key)
        {
            if let Some(index) = self.warm_cache.iter().position(|(cached, _)| cached == key) {
                let (key, preview) = self.warm_cache.remove(index).unwrap();
                self.warm_bytes -= payload_bytes(&preview);
                self.cache(key, preview);
            } else if !self.focus_pending {
                let waiting = self.warm_pending.get(key).copied();
                if waiting.is_none()
                    || waiting.is_some_and(|id| self.prediction_pool.remove_pending(id))
                {
                    self.warm_pending.remove(key);
                    self.focus_pending = Self::submit(
                        &self.focus_pool,
                        &self.decoder,
                        0,
                        key.clone(),
                        entry,
                        false,
                    );
                }
            }
        }

        // Let an uncached visible preview start before filling the neighborhood again.
        if focus.is_some_and(|key| !self.cached.contains_key(key)) {
            return;
        }
        for (key, entry) in &warm {
            if !self.cached.contains_key(key)
                && !self.warm_pending.contains_key(key)
                && !self.warm_rejected.contains(key)
                && !self.page_done.contains(key)
                && !self.warm_cache.iter().any(|(cached, _)| cached == key)
            {
                self.next_id += 1;
                let id = self.next_id;
                if Self::submit(
                    &self.prediction_pool,
                    &self.decoder,
                    id,
                    key.clone(),
                    entry,
                    true,
                ) {
                    self.warm_pending.insert(key.clone(), id);
                }
            }
        }
        for (key, _) in warm.iter().rev() {
            if let Some(id) = self.warm_pending.get(key) {
                self.prediction_pool.prioritize(*id);
            }
        }
    }

    fn store_warm(&mut self, key: Key, preview: Preview) {
        self.warm_bytes += payload_bytes(&preview);
        self.warm_cache.push_back((key, preview));
        while self.warm_bytes > WARM_CACHE_BYTES || self.warm_cache.len() > 32 {
            let (_, preview) = self.warm_cache.pop_front().unwrap();
            self.warm_bytes -= payload_bytes(&preview);
        }
    }

    fn submit(
        pool: &Pool,
        decoder: &Arc<Decode>,
        id: u64,
        key: Key,
        entry: &Entry,
        speculative: bool,
    ) -> bool {
        let decoder = decoder.clone();
        let kind = entry.kind;
        let archive = entry.archive.clone();
        pool.submit(id, move |cancel| {
            let preview = decoder(&key, kind, archive.as_deref(), cancel, speculative);
            // Enforce the budget before a result enters the pool’s ready slot.
            if speculative && payload_bytes(&preview) > SPECULATIVE_BYTES {
                return None;
            }
            Some(preview)
        })
    }

    pub(crate) fn get(&mut self, entry: &Entry, size: u32) -> Option<&CachedPreview> {
        let key = Key::new(entry, size);
        if size == GALLERY_PIXELS && !self.cached.contains_key(&key) {
            if let Some(&id) = self.gallery_pending.get(&key) {
                if self
                    .gallery_ready
                    .as_ref()
                    .is_some_and(|(ready, _)| *ready == id)
                {
                    let (_, preview) = self.gallery_ready.take().unwrap();
                    self.gallery_pending.remove(&key);
                    self.cache(key.clone(), preview);
                } else {
                    self.gallery_visible.insert(id);
                }
            } else {
                self.next_id += 1;
                let id = self.next_id;
                if Self::submit(
                    &self.gallery_pool,
                    &self.decoder,
                    id,
                    key.clone(),
                    entry,
                    false,
                ) {
                    self.gallery_pending.insert(key.clone(), id);
                    self.gallery_visible.insert(id);
                }
            }
        }
        self.cached.get(&key)
    }

    pub(crate) fn finish_frame(&mut self) {
        let pending = self.gallery_pending.len();
        self.gallery_ready = None;
        self.gallery_pool
            .retain(|id| self.gallery_visible.contains(&id));
        self.gallery_pending
            .retain(|_, id| self.gallery_visible.contains(id));
        self.gallery_visible.clear();
        if self.gallery_pending.len() < pending {
            self.ctx.request_repaint();
        }
    }

    fn cache(&mut self, key: Key, preview: Preview) {
        let cached = match preview {
            Preview::Image { size, rgba } => {
                let image = ColorImage::from_rgba_unmultiplied(size, &rgba);
                let texture = self.ctx.load_texture(
                    key.path.display().to_string(),
                    image,
                    egui::TextureOptions::LINEAR,
                );
                CachedPreview::Image(texture, vec2(size[0] as f32, size[1] as f32))
            }
            Preview::Text(text) => CachedPreview::Text(text),
            Preview::Unavailable => CachedPreview::None,
        };
        self.cached.insert(key.clone(), cached);
        self.order.push_back(key);
        while self.order.len() > 150 {
            if let Some(old) = self.order.pop_front() {
                self.cached.remove(&old);
            }
        }
    }
}

fn payload_bytes(preview: &Preview) -> usize {
    match preview {
        Preview::Image { rgba, .. } => rgba.capacity(),
        Preview::Text(text) => text.capacity(),
        Preview::Unavailable => 0,
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "perf"))]
mod benchmark;
