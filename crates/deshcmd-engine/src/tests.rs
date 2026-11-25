use super::*;

#[test]
fn auto_refresh_updates_local_tabs_but_keeps_flat_views_stable() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let (wake, notifications) = mpsc::channel();
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root.clone()],
        Settings::default(),
        move || {
            let _ = wake.send(());
        },
    );
    let settle = |engine: &mut Engine| {
        while Side::ALL.into_iter().any(|side| engine.tab(side).loading) {
            notifications.recv_timeout(Duration::from_secs(5)).unwrap();
            engine.poll();
        }
    };
    settle(&mut engine);
    std::fs::write(root.join("first.txt"), "first").unwrap();
    engine.dispatch(Command::Flat(Side::Right, false));
    settle(&mut engine);
    assert_eq!(engine.tab(Side::Right).visible_count(), 1);

    std::fs::write(root.join("second.txt"), "second").unwrap();
    engine.last_refresh = Instant::now() - Duration::from_secs(4);
    engine.poll();
    settle(&mut engine);
    assert_eq!(engine.tab(Side::Left).visible_count(), 2);
    assert_eq!(engine.tab(Side::Right).visible_count(), 1);

    engine.dispatch(Command::Refresh);
    settle(&mut engine);
    assert_eq!(engine.tab(Side::Right).visible_count(), 2);
}

#[test]
fn locked_history_navigation_preserves_the_original_tab() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    let side = Side::Left;
    let mut engine = Engine::new(
        root.clone(),
        [root.clone(), root.clone()],
        Settings::default(),
        || {},
    );
    let wait = |engine: &mut Engine, done: &dyn Fn(&Engine) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            engine.poll();
            if done(engine) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    };
    engine.dispatch(Command::Navigate {
        side,
        path: folder.clone(),
        new_tab: false,
        background: false,
    });
    wait(&mut engine, &|engine| !engine.tab(side).busy());
    engine.dispatch(Command::LockTab(side, 0, TabLock::Locked));
    engine.dispatch(Command::Back(side));
    wait(&mut engine, &|engine| engine.pane(side).tabs.len() == 2);
    assert_eq!(engine.pane(side).tabs[0].base(), folder);
    assert_eq!(engine.tab(side).base(), root);
    assert!(!engine.tab(side).can_forward());

    let index = engine.pane(side).active;
    engine.dispatch(Command::Navigate {
        side,
        path: folder.clone(),
        new_tab: true,
        background: true,
    });
    wait(&mut engine, &|engine| engine.pane(side).tabs.len() == 3);
    assert_eq!(engine.pane(side).active, index);
    assert_eq!(engine.pane(side).tabs[2].base(), folder);
}
