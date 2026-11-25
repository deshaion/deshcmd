use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};

use deshcmd_engine::read_pool::ReadPool;
use eframe::egui;

pub(crate) fn repaint(ctx: &egui::Context) -> Arc<dyn Fn() + Send + Sync> {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

struct Cached<V> {
    value: Option<Arc<V>>,
    pending: bool,
    touched: Instant,
    touched_pass: Option<u64>,
    retry: Instant,
}

pub(crate) struct ReadCache<K, V> {
    pool: ReadPool<u64, (K, Option<V>)>,
    entries: HashMap<K, Cached<V>>,
    sequence: u64,
    ttl: Duration,
    limit: usize,
    render_pass: Option<u64>,
}

impl<K: Clone + Eq + Hash + Send + 'static, V: Send + 'static> ReadCache<K, V> {
    pub(crate) fn new(ctx: &egui::Context, workers: usize, ttl: Duration, limit: usize) -> Self {
        Self {
            pool: ReadPool::new(workers, 16, repaint(ctx)),
            entries: HashMap::new(),
            sequence: 0,
            ttl,
            limit,
            render_pass: None,
        }
    }

    pub(crate) fn begin_render_pass(&mut self, pass: u64) {
        self.render_pass = Some(pass);
    }

    pub(crate) fn request(
        &mut self,
        key: K,
        read: impl FnOnce(&K) -> Option<V> + Send + 'static,
    ) -> Option<Arc<V>> {
        let now = Instant::now();
        if self.entries.len() == self.limit && !self.entries.contains_key(&key) {
            let victim = self
                .entries
                .iter()
                .filter(|(_, entry)| {
                    !entry.pending
                        && (self.render_pass.is_none() || entry.touched_pass != self.render_pass)
                })
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(key, _)| key.clone());
            self.entries.remove(&victim?);
        }
        let entry = self.entries.entry(key.clone()).or_insert(Cached {
            value: None,
            pending: false,
            touched: now,
            touched_pass: None,
            retry: now,
        });
        entry.touched = now;
        entry.touched_pass = self.render_pass;
        if !entry.pending && now >= entry.retry {
            self.sequence += 1;
            entry.pending = self.pool.submit(self.sequence, move |_| {
                let value = read(&key);
                (key, value)
            });
            if !entry.pending {
                entry.retry = now + Duration::from_millis(250);
            }
        }
        entry.value.clone()
    }

    pub(crate) fn poll(&mut self) {
        let now = Instant::now();
        for (_, (key, result)) in self.pool.drain() {
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.pending = false;
                if let Some(result) = result {
                    entry.value = Some(Arc::new(result));
                    entry.retry = now + self.ttl;
                } else {
                    entry.retry = now + Duration::from_secs(2);
                }
            }
        }
    }

    pub(crate) fn expire(&mut self, key: &K) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.retry = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_blocked_key_is_coalesced_and_a_failure_is_cached() {
        let ctx = egui::Context::default();
        let mut cache = ReadCache::new(&ctx, 2, Duration::from_secs(5), 8);
        let (release, gate) = mpsc::channel();
        let (started, starts) = mpsc::channel();
        assert_eq!(
            cache.request(0, move |_| {
                started.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(5)).unwrap();
                None
            }),
            None
        );
        starts.recv_timeout(Duration::from_secs(5)).unwrap();
        for _ in 0..100 {
            assert!(
                cache
                    .request(0, |_| -> Option<u64> { panic!("duplicate read") })
                    .is_none()
            );
        }
        cache.request(1, |_| Some(42));
        let deadline = Instant::now() + Duration::from_secs(5);
        while cache.request(1, |_| panic!("duplicate read")).is_none() {
            cache.poll();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        release.send(()).unwrap();
        while cache.entries[&0].pending {
            cache.poll();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(
            cache
                .request(0, |_| panic!("failure retried immediately"))
                .is_none()
        );
    }

    #[test]
    fn review_tree_overflow_keeps_nodes_used_in_the_same_pass() {
        let ctx = egui::Context::default();
        let mut cache = ReadCache::new(&ctx, 1, Duration::from_secs(5), 2);
        let now = Instant::now();
        cache.entries.insert(
            0,
            Cached {
                value: Some(Arc::new(0)),
                pending: false,
                touched: now,
                touched_pass: None,
                retry: now + Duration::from_secs(5),
            },
        );
        ctx.run_ui(Default::default(), |_| {
            cache.begin_render_pass(ctx.cumulative_pass_nr());
            assert_eq!(
                cache
                    .request(0, |_| panic!("cached root reread"))
                    .as_deref(),
                Some(&0)
            );
            cache.request(1, |_| Some(1));
            cache.request(2, |_| Some(2));
            assert_eq!(cache.request(0, |_| Some(0)).as_deref(), Some(&0));
        })
        .textures_delta
        .clear();
    }
}
