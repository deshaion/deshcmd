//! Bounded execution for blocking reads. Workers own their state until the current
//! syscall returns. Dropping the handle only signals cancellation and shutdown.
use std::collections::VecDeque;
use std::io;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Default)]
pub struct Cancellation(
    Arc<AtomicBool>,
    #[cfg(feature = "perf")] Arc<std::sync::OnceLock<std::time::Instant>>,
);

impl Cancellation {
    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn check(&self) -> io::Result<()> {
        if self.cancelled() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Read superseded",
            ))
        } else {
            Ok(())
        }
    }

    pub fn cancel(&self) {
        #[cfg(feature = "perf")]
        if let Some(at) = crate::perf::timestamp() {
            let _ = self.1.set(at);
        }
        self.0.store(true, Ordering::Relaxed);
    }

    #[cfg(feature = "perf")]
    pub(crate) fn cancelled_at(&self) -> Option<std::time::Instant> {
        self.1.get().copied()
    }
}

type Task<R> = Box<dyn FnOnce(&Cancellation) -> R + Send>;

struct State<K, R> {
    closed: bool,
    pending: VecDeque<(K, Task<R>)>,
    running: Vec<(usize, K, Cancellation)>,
    ready: Vec<(K, R)>,
}

struct Shared<K, R> {
    state: Mutex<State<K, R>>,
    changed: Condvar,
    workers: usize,
    capacity: usize,
}

pub struct ReadPool<K, R>(Arc<Shared<K, R>>);

impl<K: Copy + Eq + Send + 'static, R: Send + 'static> ReadPool<K, R> {
    pub fn new(workers: usize, capacity: usize, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                closed: false,
                pending: VecDeque::new(),
                running: Vec::new(),
                ready: Vec::new(),
            }),
            changed: Condvar::new(),
            workers,
            capacity,
        });
        for worker in 0..workers {
            let shared = shared.clone();
            let wake = wake.clone();
            std::thread::spawn(move || {
                loop {
                    let mut state = shared
                        .changed
                        .wait_while(shared.state.lock().unwrap(), |state| {
                            !state.closed
                                && (state.pending.is_empty()
                                    || state.running.len() + state.ready.len() >= shared.workers
                                    // Reuse the first available worker for serial reads.
                                    || (0..worker).any(|index| !state.running.iter().any(|(active, _, _)| *active == index)))
                        })
                        .unwrap();
                    if state.closed {
                        break;
                    }
                    let (key, task) = state.pending.pop_front().unwrap();
                    let token = Cancellation::default();
                    state.running.push((worker, key, token.clone()));
                    if !state.pending.is_empty() {
                        shared.changed.notify_all();
                    }
                    drop(state);
                    let result = task(&token);
                    let mut state = shared.state.lock().unwrap();
                    state.running.retain(|(active, _, _)| *active != worker);
                    let notify = if token.cancelled() || state.closed {
                        false
                    } else {
                        let notify = state.ready.is_empty();
                        state.ready.push((key, result));
                        notify
                    };
                    shared.changed.notify_all();
                    drop(state);
                    if notify {
                        wake();
                    }
                }
            });
        }
        Self(shared)
    }

    pub fn submit(&self, key: K, run: impl FnOnce(&Cancellation) -> R + Send + 'static) -> bool {
        self.submit_reserving(key, 0, run)
    }

    pub(crate) fn submit_reserving(
        &self,
        key: K,
        reserve: usize,
        run: impl FnOnce(&Cancellation) -> R + Send + 'static,
    ) -> bool {
        self.retain(|old| old != key);
        let mut state = self.0.state.lock().unwrap();
        if state.pending.len() >= self.0.capacity - reserve {
            return false;
        }
        state.pending.push_back((key, Box::new(run)));
        self.0.changed.notify_all();
        true
    }

    /// Remove a request that has not started so a higher-priority worker can take it.
    pub fn remove_pending(&self, key: K) -> bool {
        let mut state = self.0.state.lock().unwrap();
        if let Some(index) = state
            .pending
            .iter()
            .position(|(pending, _)| *pending == key)
        {
            state.pending.remove(index);
            true
        } else {
            false
        }
    }

    pub fn prioritize(&self, key: K) {
        let mut state = self.0.state.lock().unwrap();
        if let Some(index) = state
            .pending
            .iter()
            .position(|(pending, _)| *pending == key)
        {
            let request = state.pending.remove(index).unwrap();
            state.pending.push_front(request);
        }
    }

    pub fn retain(&self, keep: impl Fn(K) -> bool) {
        let mut state = self.0.state.lock().unwrap();
        state.pending.retain(|(key, _)| keep(*key));
        for (_, key, token) in &state.running {
            if !keep(*key) {
                token.cancel();
            }
        }
        let mut removed = Vec::new();
        let mut i = 0;
        while i < state.ready.len() {
            if keep(state.ready[i].0) {
                i += 1;
            } else {
                removed.push(state.ready.remove(i));
            }
        }
        if !removed.is_empty() {
            self.0.changed.notify_all();
        }
        drop(state);
        drop(removed);
    }

    pub fn drain(&self) -> Vec<(K, R)> {
        let ready = std::mem::take(&mut self.0.state.lock().unwrap().ready);
        if !ready.is_empty() {
            self.0.changed.notify_all();
        }
        ready
    }
}

impl<K, R> Drop for ReadPool<K, R> {
    fn drop(&mut self) {
        let (pending, ready) = {
            let mut state = self.0.state.lock().unwrap();
            state.closed = true;
            for (_, _, token) in &state.running {
                token.cancel();
            }
            self.0.changed.notify_all();
            (
                std::mem::take(&mut state.pending),
                std::mem::take(&mut state.ready),
            )
        };
        drop((pending, ready));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn replaces_pending_reads_and_discards_running_results() {
        let timeout = Duration::from_secs(5);
        let (wake, wakes) = mpsc::channel();
        let pool = ReadPool::new(
            2,
            2,
            Arc::new(move || {
                let _ = wake.send(());
            }),
        );
        let mut releases = Vec::new();
        let (started, starts) = mpsc::channel();
        let (cancelled, cancellations) = mpsc::channel();
        for key in 0..2 {
            let (release, gate) = mpsc::channel();
            releases.push(release);
            let started = started.clone();
            let cancelled = cancelled.clone();
            assert!(pool.submit(key, move |token| {
                started.send(key).unwrap();
                gate.recv_timeout(timeout).unwrap();
                cancelled.send((key, token.cancelled())).unwrap();
                0
            }));
        }
        starts.recv_timeout(timeout).unwrap();
        starts.recv_timeout(timeout).unwrap();
        for i in 1..=100 {
            assert!(pool.submit(0, move |_| i));
            assert!(pool.submit(1, move |_| i));
            let state = pool.0.state.lock().unwrap();
            assert_eq!(state.running.len(), 2);
            assert_eq!(state.pending.len(), 2);
            assert!(state.ready.is_empty());
        }
        releases.pop().unwrap().send(()).unwrap();
        assert_eq!(cancellations.recv_timeout(timeout).unwrap(), (1, true));
        wakes.recv_timeout(timeout).unwrap();
        let first = pool.drain();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].1, 100);
        releases.pop().unwrap().send(()).unwrap();
        assert_eq!(cancellations.recv_timeout(timeout).unwrap(), (0, true));
        wakes.recv_timeout(timeout).unwrap();
        let second = pool.drain();
        assert_eq!(second.len(), 1);
        assert_ne!(first[0].0, second[0].0);
        assert_eq!(second[0].1, 100);
    }

    #[test]
    fn dropping_pool_does_not_wait_for_a_blocked_read() {
        let timeout = Duration::from_secs(5);
        let pool = ReadPool::new(1, 1, Arc::new(|| {}));
        let (started, starts) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (finished, finishes) = mpsc::channel();
        assert!(pool.submit(0, move |token| {
            started.send(()).unwrap();
            gate.recv_timeout(timeout).unwrap();
            finished.send(token.cancelled()).unwrap();
        }));
        starts.recv_timeout(timeout).unwrap();
        drop(pool);
        release.send(()).unwrap();
        assert!(finishes.recv_timeout(timeout).unwrap());
    }
}
