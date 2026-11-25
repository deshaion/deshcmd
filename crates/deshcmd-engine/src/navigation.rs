use super::*;
use crate::read_pool::{Cancellation, ReadPool};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    tab: u64,
    revision: u64,
    independent: bool,
}

pub(super) struct Navigation {
    pool: ReadPool<Key, (Intent, io::Result<Option<Destination>>)>,
}

impl Navigation {
    pub(super) fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            pool: ReadPool::new(2, 8, wake),
        }
    }
}

enum Destination {
    Path(Location),
    History(usize),
}

struct Intent {
    lock: TabLock,
    focus: Option<u64>,
    open_tab: bool,
    background: bool,
    path: PathBuf,
    peer: Option<Peer>,
    quiet: bool,
    startup: bool,
}

struct Peer {
    tab: u64,
    revision: u64,
    path: Option<PathBuf>,
}

fn validate_path(path: &Path, cancel: &Cancellation) -> io::Result<Location> {
    cancel.check()?;
    if path.is_dir() {
        cancel.check()?;
        return path.canonicalize().map(Location::Directory);
    }
    for file in path.ancestors() {
        cancel.check()?;
        if !is_archive(file) || !file.is_file() {
            continue;
        }
        let folder = path.strip_prefix(file).map_err(io::Error::other)?;
        if folder
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(io::Error::other("Invalid archive folder"));
        }
        return Ok(Location::Archive {
            file: file.into(),
            folder: folder.into(),
        });
    }
    Err(io::Error::other(format!(
        "{} is unavailable or is not a directory",
        path.display()
    )))
}

impl Engine {
    pub fn request_initial_directory(&mut self, side: Side, path: PathBuf) {
        let intent = Intent {
            lock: TabLock::Normal,
            focus: None,
            open_tab: false,
            background: false,
            path,
            peer: None,
            quiet: true,
            startup: true,
        };
        let _ = self.submit_navigation(side, intent, |intent, cancel| {
            cancel.check()?;
            if !intent.path.is_dir() {
                return Ok(None);
            }
            cancel.check()?;
            intent
                .path
                .canonicalize()
                .map(|path| Some(Destination::Path(Location::Directory(path))))
        });
    }

    fn resolve_path(&self, side: Side, path: &Path) -> PathBuf {
        if path == Path::new("~") {
            self.home.clone()
        } else if let Ok(suffix) = path.strip_prefix("~/") {
            self.home.join(suffix)
        } else {
            self.tab(side).base().join(path)
        }
    }

    fn capture_peer(&self, side: Side, path: Option<PathBuf>) -> Peer {
        let tab = self.tab(side.other());
        Peer {
            tab: tab.id,
            revision: tab.nav_revision,
            path,
        }
    }

    pub(super) fn navigate(
        &mut self,
        side: Side,
        path: &Path,
        new_tab: bool,
        background: bool,
        linked: bool,
    ) -> io::Result<()> {
        let peer = (self.settings.linked && linked && !background && !new_tab)
            .then(|| self.capture_peer(side, None));
        self.request_path(side, path, new_tab, background, peer, true, false)
    }

    pub(super) fn navigate_without_focus(&mut self, side: Side, path: &Path) -> io::Result<()> {
        self.request_path(side, path, false, false, None, false, false)
    }

    pub(super) fn navigate_bookmark(
        &mut self,
        side: Side,
        path: &Path,
        opposite: Option<PathBuf>,
    ) -> io::Result<()> {
        let peer = opposite.map(|opposite| {
            self.capture_peer(side, Some(self.resolve_path(side.other(), &opposite)))
        });
        self.request_path(side, path, false, false, peer, true, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn request_path(
        &mut self,
        side: Side,
        path: &Path,
        open_tab: bool,
        background: bool,
        peer: Option<Peer>,
        focus: bool,
        quiet: bool,
    ) -> io::Result<()> {
        if focus && !background {
            self.focus_revision += 1;
        }
        let intent = Intent {
            lock: self.tab(side).saved.lock,
            focus: (focus && !background).then_some(self.focus_revision),
            open_tab,
            background,
            path: self.resolve_path(side, path),
            peer,
            quiet,
            startup: false,
        };
        self.submit_navigation(side, intent, |intent, cancel| {
            validate_path(&intent.path, cancel).map(|location| Some(Destination::Path(location)))
        })
    }

    pub(super) fn request_history(
        &mut self,
        side: Side,
        exact: Option<usize>,
        backwards: bool,
    ) -> io::Result<()> {
        let tab = self.tab(side);
        let range: Box<dyn Iterator<Item = usize>> = if let Some(index) = exact {
            Box::new(std::iter::once(index))
        } else if backwards {
            Box::new((0..tab.history_index).rev())
        } else {
            Box::new(tab.history_index + 1..tab.history.len())
        };
        let candidates: Vec<_> = range
            .filter_map(|index| {
                let location = &tab.history.get(index)?.location;
                if exact.is_none() && location == tab.location() {
                    return None;
                }
                let (path, archive) = match location {
                    Location::Archive { file, .. } => (file.as_path(), true),
                    _ => (location.base(), false),
                };
                Some((index, path.to_owned(), archive))
            })
            .collect();
        let Some((_, path, _)) = candidates.first() else {
            self.tab_mut(side).cancel_navigation();
            return Ok(());
        };
        self.focus_revision += 1;
        let intent = Intent {
            lock: self.tab(side).saved.lock,
            focus: Some(self.focus_revision),
            open_tab: false,
            background: false,
            path: path.clone(),
            peer: None,
            quiet: false,
            startup: false,
        };
        self.submit_navigation(side, intent, move |_, cancel| {
            for (index, path, archive) in candidates {
                cancel.check()?;
                if if archive {
                    path.is_file()
                } else {
                    path.is_dir()
                } {
                    return Ok(Some(Destination::History(index)));
                }
            }
            if exact.is_some() {
                Err(io::Error::other("This history location is unavailable"))
            } else {
                Ok(None)
            }
        })
    }

    fn submit_navigation(
        &mut self,
        side: Side,
        intent: Intent,
        validate: impl FnOnce(&Intent, &Cancellation) -> io::Result<Option<Destination>>
        + Send
        + 'static,
    ) -> io::Result<()> {
        let independent = intent.open_tab && intent.background;
        let key = if independent {
            self.ids += 1;
            Key {
                tab: self.tab(side).id,
                revision: self.ids,
                independent,
            }
        } else {
            let tab = self.tab_mut(side);
            tab.cancel_navigation();
            tab.pending_navigation = Some(intent.path.clone());
            Key {
                tab: tab.id,
                revision: tab.nav_revision,
                independent,
            }
        };
        self.retain_navigation();
        let reserve = if independent { 2 } else { 0 };
        if self
            .navigation
            .pool
            .submit_reserving(key, reserve, move |cancel| {
                let result = validate(&intent, cancel);
                (intent, result)
            })
        {
            Ok(())
        } else {
            if !independent {
                self.tab_mut(side).cancel_navigation();
            }
            Err(io::Error::other("Too many pending navigation requests"))
        }
    }

    pub(super) fn retain_navigation(&mut self) {
        let active = self.panes.each_ref().map(|pane| pane.tab().id);
        for pane in &mut self.panes {
            for tab in &mut pane.tabs {
                if !active.contains(&tab.id) && tab.pending_navigation.is_some() {
                    tab.cancel_navigation();
                }
            }
        }
        self.navigation.pool.retain(|key| {
            self.panes.iter().any(|pane| {
                if key.independent {
                    pane.tabs.iter().any(|tab| tab.id == key.tab)
                } else {
                    let tab = pane.tab();
                    tab.id == key.tab && tab.nav_revision == key.revision
                }
            })
        });
    }

    pub(super) fn install_navigation(&mut self) {
        for (key, (intent, result)) in self.navigation.pool.drain() {
            let Some(side) = Side::ALL.into_iter().find(|&side| {
                let pane = self.pane(side);
                if key.independent {
                    pane.tabs.iter().any(|tab| tab.id == key.tab)
                } else {
                    let tab = pane.tab();
                    tab.id == key.tab && tab.nav_revision == key.revision
                }
            }) else {
                continue;
            };
            if !key.independent {
                self.tab_mut(side).pending_navigation = None;
            }
            match result {
                Ok(Some(destination)) => self.commit_navigation(side, intent, destination),
                Err(error) if !intent.quiet => self.notice = Some(error.to_string()),
                _ => {}
            }
        }
    }

    fn commit_navigation(&mut self, side: Side, intent: Intent, destination: Destination) {
        if intent.startup {
            if let Destination::Path(location) = destination {
                let tab = self.tab_mut(side);
                tab.saved.home = location.base().to_owned();
                tab.history = vec![HistoryEntry::new(location.clone())];
                tab.history_index = 0;
                tab.set_location(location, false);
            }
            return;
        }
        let tab = self.tab(side);
        let previous = tab.base().to_owned();
        let before = tab.location();
        let (location, history) = match destination {
            Destination::Path(location) => (location, None),
            Destination::History(index) => (tab.history[index].location.clone(), Some(index)),
        };
        let changed = &location != before;
        let parent_focus = match &location {
            Location::Directory(path) if history.is_some() => {
                tab.entry_in_parent(path).map(Path::to_owned)
            }
            Location::Directory(path) => tab
                .entry_in_parent(&intent.path)
                .or_else(|| tab.entry_in_parent(path))
                .and_then(Path::file_name)
                .map(|name| path.join(name)),
            Location::Archive { file, folder } => match before {
                Location::Archive {
                    file: previous_file,
                    folder: previous_folder,
                } if previous_file == file && previous_folder.parent() == Some(folder) => {
                    Some(file.join(previous_folder))
                }
                _ => None,
            },
            _ => None,
        };
        let locked = matches!(intent.lock, TabLock::Locked | TabLock::NewTab);
        let opens_tab = intent.open_tab
            || (locked
                && changed
                && (location.base() != previous
                    || matches!(location, Location::Archive { .. })
                    || matches!(before, Location::Archive { .. })));
        let can_focus = intent.focus == Some(self.focus_revision);
        let entered_via = history.map_or_else(
            || matches!(location, Location::Directory(_)).then(|| intent.path.clone()),
            |index| tab.history[index].entered_via.clone(),
        );
        let peer_path = intent.peer.and_then(|peer| {
            let other = self.tab(side.other());
            if other.id != peer.tab
                || other.nav_revision != peer.revision
                || other.pending_navigation.is_some()
            {
                return None;
            }
            if let Some(path) = peer.path {
                return Some((path, false));
            }
            let Location::Directory(path) = &location else {
                return None;
            };
            if opens_tab
                || !self.settings.linked
                || matches!(other.location(), Location::Archive { .. })
            {
                return None;
            }
            if previous.parent() == Some(path) {
                other.base().parent().map(|path| (path.to_owned(), true))
            } else {
                path.strip_prefix(&previous)
                    .ok()
                    .map(|suffix| (other.base().join(suffix), true))
            }
        });
        let tab = if opens_tab {
            let mut saved = default_tab(location.base().to_owned());
            if history.is_none() || matches!(location, Location::Archive { .. }) {
                saved.location = location;
            }
            self.add_tab(side, saved, intent.background || !can_focus)
        } else {
            if can_focus {
                self.settings.active = side;
            }
            let tab = self.tab_mut(side);
            if let Some(index) = history {
                tab.history_index = index;
            }
            tab.set_location(location, history.is_none());
            tab
        };
        if !intent.background {
            tab.pending_parent_focus = parent_focus;
        }
        if opens_tab || history.is_none() && changed {
            tab.history[tab.history_index].entered_via = entered_via;
        }
        if let Some((path, quiet)) = peer_path {
            let _ = self.request_path(side.other(), &path, false, false, None, false, quiet);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(engine: &mut Engine, done: impl Fn(&Engine) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(engine) {
            engine.poll();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    fn gated(
        engine: &mut Engine,
        path: PathBuf,
        peer: Option<Peer>,
    ) -> (mpsc::Sender<()>, mpsc::Receiver<bool>) {
        let (release, gate) = mpsc::channel();
        let (started, starts) = mpsc::channel();
        let (finished, finishes) = mpsc::channel();
        engine.focus_revision += 1;
        engine
            .submit_navigation(
                Side::Left,
                Intent {
                    lock: TabLock::Normal,
                    focus: Some(engine.focus_revision),
                    open_tab: false,
                    background: false,
                    path,
                    peer,
                    quiet: false,
                    startup: false,
                },
                move |intent, token| {
                    started.send(()).unwrap();
                    gate.recv_timeout(Duration::from_secs(5)).unwrap();
                    finished.send(token.cancelled()).unwrap();
                    Ok(Some(Destination::Path(Location::Directory(
                        intent.path.clone(),
                    ))))
                },
            )
            .unwrap();
        starts.recv_timeout(Duration::from_secs(5)).unwrap();
        (release, finishes)
    }

    #[test]
    fn slow_validation_preserves_marks_and_newer_peer_navigation() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let [left, right] = [root.join("left"), root.join("right")];
        for path in [&left, &right] {
            std::fs::create_dir_all(path.join("child")).unwrap();
        }
        std::fs::create_dir(right.join("newer")).unwrap();
        std::fs::write(left.join("marked.txt"), "marked").unwrap();
        let mut engine = Engine::new(
            root,
            [left.clone(), right.clone()],
            Settings::default(),
            || {},
        );
        wait(&mut engine, |e| {
            Side::ALL.into_iter().all(|side| !e.tab(side).busy())
        });
        engine.dispatch(Command::MarkAll(Side::Left));
        engine.settings.linked = true;
        let peer = engine.capture_peer(Side::Left, None);
        let (release, finish) = gated(&mut engine, left.join("child"), Some(peer));
        assert_eq!(engine.tab(Side::Left).base(), left);
        assert_eq!(engine.tab(Side::Left).selected_bytes(), 6);
        engine.dispatch(Command::Navigate {
            side: Side::Right,
            path: "newer".into(),
            new_tab: false,
            background: false,
        });
        wait(&mut engine, |e| !e.tab(Side::Right).busy());
        release.send(()).unwrap();
        assert!(!finish.recv_timeout(Duration::from_secs(5)).unwrap());
        wait(&mut engine, |e| !e.tab(Side::Left).busy());
        assert_eq!(engine.tab(Side::Right).base(), right.join("newer"));
        assert_eq!(engine.settings.active, Side::Right);
        assert_eq!(engine.tab(Side::Left).base(), left.join("child"));
    }

    #[test]
    fn newer_failure_cancels_old_success_and_restore_invalidates_intent() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let older = root.join("older");
        std::fs::create_dir(&older).unwrap();
        let mut engine = Engine::new(
            root.clone(),
            [root.clone(), root.clone()],
            Settings::default(),
            || {},
        );
        let (release, finish) = gated(&mut engine, older.clone(), None);
        engine.dispatch(Command::Navigate {
            side: Side::Left,
            path: "missing".into(),
            new_tab: false,
            background: false,
        });
        wait(&mut engine, |e| !e.tab(Side::Left).busy());
        assert!(
            engine
                .notice
                .as_ref()
                .is_some_and(|s| s.contains("missing"))
        );
        assert_eq!(engine.tab(Side::Left).history().count(), 1);
        release.send(()).unwrap();
        assert!(finish.recv_timeout(Duration::from_secs(5)).unwrap());
        let (release, finish) = gated(&mut engine, older, None);
        let old_id = engine.tab(Side::Left).id;
        engine.dispatch(Command::RestoreTabs(engine.snapshot().panes));
        release.send(()).unwrap();
        assert!(finish.recv_timeout(Duration::from_secs(5)).unwrap());
        wait(&mut engine, |e| !e.tab(Side::Left).busy());
        assert_ne!(engine.tab(Side::Left).id, old_id);
        assert_eq!(engine.tab(Side::Left).base(), root);
    }

    #[test]
    fn closing_a_tab_and_overlapping_history_cannot_commit_obsolete_requests() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let child = root.join("child");
        std::fs::create_dir(&child).unwrap();
        let mut engine = Engine::new(
            root.clone(),
            [root.clone(), root.clone()],
            Settings::default(),
            || {},
        );
        let side = Side::Left;
        engine.dispatch(Command::NewTab(side));
        let (release, finish) = gated(&mut engine, child.clone(), None);
        engine.dispatch(Command::CloseTab(side, 1));
        release.send(()).unwrap();
        assert!(finish.recv_timeout(Duration::from_secs(5)).unwrap());
        wait(&mut engine, |engine| !engine.tab(side).busy());
        assert_eq!(engine.pane(side).tabs.len(), 1);
        assert_eq!(engine.tab(side).base(), root);
        engine.dispatch(Command::Navigate {
            side,
            path: child.clone(),
            new_tab: false,
            background: false,
        });
        wait(&mut engine, |engine| !engine.tab(side).busy());

        // Forward at the committed history tip supersedes the uncommitted Back.
        engine.dispatch(Command::Back(side));
        assert!(engine.tab(side).pending_navigation().is_some());
        engine.dispatch(Command::Forward(side));
        wait(&mut engine, |engine| !engine.tab(side).busy());
        assert_eq!(engine.tab(side).base(), child);
        assert_eq!(engine.tab(side).history().count(), 2);
        engine.dispatch(Command::Back(side));
        wait(&mut engine, |engine| !engine.tab(side).busy());
        assert_eq!(engine.tab(side).base(), root);
        assert!(engine.tab(side).can_forward());
    }

    #[test]
    fn startup_directory_validation_keeps_independent_fallbacks_and_no_extra_history() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let folder = root.join("valid");
        std::fs::create_dir(&folder).unwrap();
        let mut engine = Engine::new(
            root.clone(),
            [root.clone(), root.clone()],
            Settings::default(),
            || {},
        );
        engine.request_initial_directory(Side::Left, folder.clone());
        engine.request_initial_directory(Side::Right, root.join("missing"));
        wait(&mut engine, |engine| {
            Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
        });
        assert_eq!(engine.tab(Side::Left).base(), folder);
        assert_eq!(engine.tab(Side::Left).saved.home, folder);
        assert!(!engine.tab(Side::Left).can_back());
        assert_eq!(engine.tab(Side::Right).base(), root);
        assert!(engine.notice.is_none());
    }
}
