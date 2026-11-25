//! Native watches are a hint. Registration, teardown and blocked backends live
//! on two dedicated threads, with one replaceable subscription per slot.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode, ModifyKind};
use notify::{Event, EventKind, RecursiveMode, Watcher};

pub(crate) const PATCH_LIMIT: usize = 32;
const DEBOUNCE: Duration = Duration::from_millis(40);
const MAX_DEBOUNCE: Duration = Duration::from_millis(200);
const REGISTER_WAIT: Duration = Duration::from_millis(50);
pub(crate) const WATCHED_INTERVAL: Duration = Duration::from_secs(60);
const RETRY: Duration = Duration::from_secs(4);

type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy)]
enum Status {
    Registering,
    Watching,
    Retry(Instant),
}

#[derive(Default)]
pub(crate) enum Hint {
    #[default]
    Full,
    Paths(Vec<PathBuf>),
}

impl Hint {
    fn merge(&mut self, other: Self) {
        let Self::Paths(paths) = self else { return };
        let Self::Paths(other) = other else {
            *self = Self::Full;
            return;
        };
        for path in other {
            if !paths.contains(&path) {
                if paths.len() == PATCH_LIMIT {
                    *self = Self::Full;
                    return;
                }
                paths.push(path);
            }
        }
    }
}

struct Batch {
    due: Instant,
    until: Instant,
    hint: Hint,
}

struct State {
    path: Option<Arc<Path>>,
    revision: u64,
    change: u64,
    status: Status,
    batch: Option<Batch>,
    closed: bool,
    reconcile_on_connect: bool,
}

struct Slot {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Clone)]
pub(crate) struct Ticket {
    slot: Arc<Slot>,
    revision: u64,
}

#[derive(Clone)]
pub(crate) struct Observation {
    ticket: Ticket,
    change: u64,
}

impl Observation {
    pub fn current(&self) -> bool {
        let state = self.ticket.slot.state.lock().unwrap();
        state.revision == self.ticket.revision && state.change == self.change
    }
}

impl Ticket {
    pub fn observe_clean(self) -> Option<Observation> {
        let state = self.slot.state.lock().unwrap();
        if state.revision != self.revision || state.batch.is_some() {
            return None;
        }
        let change = state.change;
        drop(state);
        Some(Observation {
            ticket: self,
            change,
        })
    }

    pub fn wait_for_registration(&self) {
        let state = self.slot.state.lock().unwrap();
        let (mut state, timeout) = self
            .slot
            .changed
            .wait_timeout_while(state, REGISTER_WAIT, |state| {
                !state.closed
                    && state.revision == self.revision
                    && matches!(state.status, Status::Registering)
            })
            .unwrap();
        if timeout.timed_out()
            && state.revision == self.revision
            && matches!(state.status, Status::Registering)
        {
            state.reconcile_on_connect = true;
        }
    }
}

pub(crate) struct Watches([Arc<Slot>; 2]);

impl Watches {
    pub fn new(wake: Wake) -> Self {
        Self(std::array::from_fn(|_| {
            let slot = Arc::new(Slot {
                state: Mutex::new(State {
                    path: None,
                    revision: 0,
                    change: 0,
                    status: Status::Registering,
                    batch: None,
                    closed: false,
                    reconcile_on_connect: false,
                }),
                changed: Condvar::new(),
            });
            let worker = slot.clone();
            let wake = wake.clone();
            std::thread::spawn(move || worker.run(&wake));
            slot
        }))
    }

    pub fn set_paths(&self, paths: [Option<&Path>; 2], now: Instant) -> Vec<Arc<Path>> {
        let mut removed_dirty = Vec::new();
        // Preserve a subscription when its other pane leaves or when panes swap.
        let old = self
            .0
            .each_ref()
            .map(|slot| slot.state.lock().unwrap().path.clone());
        for (index, slot) in self.0.iter().enumerate() {
            if old[index]
                .as_deref()
                .is_some_and(|path| paths.contains(&Some(path)))
            {
                let mut state = slot.state.lock().unwrap();
                if matches!(state.status, Status::Retry(at) if now >= at) && state.batch.is_none() {
                    state.revision += 1;
                    state.reconcile_on_connect = true;
                    state.status = Status::Registering;
                    slot.changed.notify_all();
                }
                continue;
            }
            let free_before = old[..index]
                .iter()
                .filter(|value| {
                    !value
                        .as_deref()
                        .is_some_and(|path| paths.contains(&Some(path)))
                })
                .count();
            let wanted = paths
                .into_iter()
                .enumerate()
                .filter_map(|(i, path)| {
                    path.filter(|path| {
                        !paths[..i].contains(&Some(path))
                            && !old.iter().any(|value| value.as_deref() == Some(path))
                    })
                })
                .nth(free_before);
            if old[index].as_deref() != wanted {
                let mut state = slot.state.lock().unwrap();
                if state.batch.take().is_some()
                    && let Some(path) = state.path.clone()
                {
                    removed_dirty.push(path);
                }
                state.path = wanted.map(Arc::from);
                state.reconcile_on_connect = false;
                state.revision += 1;
                state.status = Status::Registering;
                slot.changed.notify_all();
            }
        }
        removed_dirty
    }

    pub fn ticket(&self, path: &Path) -> Option<Ticket> {
        self.0.iter().find_map(|slot| {
            let state = slot.state.lock().unwrap();
            (state.path.as_deref() == Some(path)).then(|| Ticket {
                slot: slot.clone(),
                revision: state.revision,
            })
        })
    }

    pub fn reliable(&self, path: &Path) -> bool {
        self.0.iter().any(|slot| {
            let state = slot.state.lock().unwrap();
            state.path.as_deref() == Some(path) && matches!(state.status, Status::Watching)
        })
    }

    pub fn next_batch(&self) -> Option<Duration> {
        self.0
            .iter()
            .filter_map(|slot| {
                let state = slot.state.lock().unwrap();
                state
                    .batch
                    .as_ref()
                    .map(|batch| batch.due.saturating_duration_since(Instant::now()))
            })
            .min()
    }

    pub fn drain(&self, now: Instant) -> Vec<(Arc<Path>, Hint)> {
        self.0
            .iter()
            .filter_map(|slot| {
                let mut state = slot.state.lock().unwrap();
                if state.batch.as_ref().is_some_and(|batch| batch.due <= now) {
                    let hint = state.batch.take().unwrap().hint;
                    return state.path.clone().map(|path| (path, hint));
                }
                None
            })
            .collect()
    }
}

impl Drop for Watches {
    fn drop(&mut self) {
        for slot in &self.0 {
            slot.state.lock().unwrap().closed = true;
            slot.changed.notify_all();
        }
    }
}

impl Slot {
    fn run(self: Arc<Self>, wake: &Wake) {
        let mut previous = 0;
        let mut watcher = None;
        loop {
            let (revision, path) = {
                let state = self
                    .changed
                    .wait_while(self.state.lock().unwrap(), |state| {
                        !state.closed && state.revision == previous
                    })
                    .unwrap();
                if state.closed {
                    break;
                }
                (state.revision, state.path.clone())
            };
            drop(watcher.take());
            previous = revision;
            if let Some(path) = path {
                let callback = self.clone();
                let on_event = wake.clone();
                let coverage = path.clone();
                watcher = notify::recommended_watcher(move |event| {
                    callback.receive(revision, &coverage, event, Instant::now(), &on_event);
                })
                .and_then(|mut watcher| {
                    watcher.watch(&path, RecursiveMode::NonRecursive)?;
                    Ok(watcher)
                })
                .ok();
                let mut state = self.state.lock().unwrap();
                let mut repaint = false;
                if !state.closed
                    && state.revision == revision
                    && matches!(state.status, Status::Registering)
                {
                    let now = Instant::now();
                    state.status = if watcher.is_some() {
                        if state.reconcile_on_connect {
                            repaint = queue(&mut state, Hint::Full, now);
                            state.reconcile_on_connect = false;
                        }
                        Status::Watching
                    } else {
                        Status::Retry(now + RETRY)
                    };
                }
                self.changed.notify_all();
                drop(state);
                if repaint {
                    wake();
                }
            }
        }
    }

    fn receive(
        &self,
        revision: u64,
        path: &Path,
        event: notify::Result<Event>,
        now: Instant,
        wake: &Wake,
    ) {
        let (hint, reset) = match event {
            Ok(event) => {
                if matches!(event.kind, EventKind::Access(kind) if kind != AccessKind::Close(AccessMode::Write))
                    && !event.need_rescan()
                {
                    return;
                }
                let reset = event.need_rescan()
                    || event.paths.is_empty()
                    || event.paths.iter().any(|item| item == path);
                let patchable = !reset
                    && event.paths.len() <= PATCH_LIMIT
                    && matches!(
                        event.kind,
                        EventKind::Create(_)
                            | EventKind::Remove(_)
                            | EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_))
                            | EventKind::Access(AccessKind::Close(AccessMode::Write))
                    )
                    && event.paths.iter().all(|item| {
                        item.parent() == Some(path)
                            && !item
                                .file_name()
                                .is_some_and(|name| name.eq_ignore_ascii_case("descript.ion"))
                    });
                (
                    if patchable {
                        Hint::Paths(event.paths)
                    } else {
                        Hint::Full
                    },
                    reset,
                )
            }
            Err(_) => (Hint::Full, true),
        };
        let mut state = self.state.lock().unwrap();
        if state.closed || state.revision != revision {
            return;
        }
        if reset {
            state.status = Status::Retry(now + RETRY);
        }
        let repaint = queue(&mut state, hint, now);
        drop(state);
        if repaint {
            wake();
        }
    }
}

fn queue(state: &mut State, hint: Hint, now: Instant) -> bool {
    state.change += 1;
    if let Some(batch) = &mut state.batch {
        batch.hint.merge(hint);
        batch.due = (now + DEBOUNCE).max(batch.due).min(batch.until);
        false
    } else {
        state.batch = Some(Batch {
            due: now + DEBOUNCE,
            until: now + MAX_DEBOUNCE,
            hint,
        });
        true
    }
}

#[cfg(test)]
impl Watches {
    pub fn inject(&self, path: &Path, event: notify::Result<Event>) {
        let ticket = self.ticket(path).expect("test path has a subscription");
        let wake: Wake = Arc::new(|| {});
        ticket.slot.receive(
            ticket.revision,
            path,
            event,
            Instant::now() - DEBOUNCE,
            &wake,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_duplicate_notifications_and_fences_previous_subscriptions() {
        let wake: Wake = Arc::new(|| {});
        let watches = Watches::new(wake.clone());
        let dir = Path::new("/first");
        let other = Path::new("/second");
        let now = Instant::now();
        watches.set_paths([Some(dir), Some(dir)], now);
        let ticket = watches.ticket(dir).unwrap();
        let peer_ticket = watches.ticket(dir).unwrap();
        assert!(Arc::ptr_eq(&ticket.slot, &peer_ticket.slot));
        let child = || {
            Event::new(EventKind::Modify(notify::event::ModifyKind::Any)).add_path(dir.join("file"))
        };
        ticket
            .slot
            .receive(ticket.revision, dir, Ok(child()), now, &wake);
        ticket
            .slot
            .receive(ticket.revision, dir, Ok(child()), now, &wake);
        assert!(watches.drain(now).is_empty());
        assert_eq!(watches.drain(now + DEBOUNCE)[0].0.as_ref(), dir);
        assert!(watches.drain(now + DEBOUNCE).is_empty());

        watches.set_paths([None, Some(other)], now);
        ticket
            .slot
            .receive(ticket.revision, dir, Ok(child()), now, &wake);
        assert!(watches.drain(now + DEBOUNCE).is_empty());

        let other_ticket = watches.ticket(other).unwrap();
        other_ticket.slot.receive(
            other_ticket.revision,
            other,
            Err(notify::Error::generic("overflow")),
            now,
            &wake,
        );
        assert!(!watches.reliable(other));
        assert_eq!(watches.drain(now + DEBOUNCE)[0].0.as_ref(), other);
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn overflowing_a_batch_uses_full_reconciliation() {
        let mut hint = Hint::Paths(vec![PathBuf::from("duplicate")]);
        for _ in 0..100 {
            hint.merge(Hint::Paths(vec![PathBuf::from("duplicate")]));
        }
        assert!(matches!(&hint, Hint::Paths(paths) if paths.len() == 1));
        hint.merge(Hint::Paths(
            (0..PATCH_LIMIT)
                .map(|index| PathBuf::from(index.to_string()))
                .collect(),
        ));
        assert!(matches!(hint, Hint::Full));
        hint.merge(Hint::Paths(vec![PathBuf::from("late")]));
        assert!(matches!(hint, Hint::Full));
    }
}
