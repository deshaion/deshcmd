//! Compare engine-ready milestones on an identical Phase 3 and Phase 4 fixture.
#![cfg(feature = "perf")]

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use deshcmd_engine::{Command, Engine, Settings, Side};

fn sample(label: &str, duration: Duration) {
    eprintln!("sample,{label},{:.6}", duration.as_secs_f64() * 1000.);
}

fn navigate(side: Side, path: &Path) -> Command {
    Command::Navigate {
        side,
        path: path.to_owned(),
        new_tab: false,
        background: false,
    }
}

fn idle(engine: &Engine) -> bool {
    Side::ALL.into_iter().all(|side| !engine.tab(side).busy())
}

fn wait(engine: &mut Engine, wakes: &mpsc::Receiver<()>, mut done: impl FnMut(&Engine) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        engine.poll();
        if done(engine) {
            break;
        }
        assert!(Instant::now() < deadline, "benchmark timed out");
        let _ = wakes.recv_timeout(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires a disposable release benchmark fixture"]
fn phase4_cache_and_refresh() {
    let fixture = PathBuf::from(std::env::var_os("DESHCMD_PERF_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    let (notify, wakes) = mpsc::channel();
    let size = std::env::var("DESHCMD_PERF_SIZE").unwrap_or_else(|_| "100000".into());
    let directory = fixture.join(&size);
    let empty = fixture.join("empty");
    let mut engine = Engine::new(
        fixture.clone(),
        [empty.clone(), empty.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        move || {
            let _ = notify.send(());
        },
    );
    wait(&mut engine, &wakes, idle);
    let left = Side::Left;
    let right = Side::Right;
    for iteration in 0..6 {
        let label = if iteration == 0 { "first" } else { "revisit" };
        eprintln!("begin,{label}");
        let start = Instant::now();
        engine.dispatch(navigate(left, &directory));
        let mut first = None;
        wait(&mut engine, &wakes, |e| {
            if e.tab(left).base() == directory && e.tab(left).visible_count() != 0 {
                first.get_or_insert_with(|| start.elapsed());
            }
            idle(e)
        });
        sample(&format!("{label}/first_rows"), first.unwrap());
        sample(&format!("{label}/complete"), start.elapsed());
        eprintln!("begin,return");
        engine.dispatch(navigate(left, &empty));
        wait(&mut engine, &wakes, idle);
    }

    let fresh = fixture.join("plain");
    eprintln!("begin,both");
    engine.dispatch(navigate(left, &fresh));
    engine.dispatch(navigate(right, &fresh));
    wait(&mut engine, &wakes, idle);

    eprintln!("begin,watch_setup");
    engine.settings.auto_refresh = true;
    engine.dispatch(navigate(right, &empty));
    engine.dispatch(navigate(left, &directory));
    wait(&mut engine, &wakes, idle);
    engine.dispatch(Command::Refresh);
    wait(&mut engine, &wakes, idle);
    eprintln!("begin,idle_9s");
    let start = Instant::now();
    wait(&mut engine, &wakes, |_| {
        start.elapsed() >= Duration::from_secs(9)
    });
    wait(&mut engine, &wakes, idle);

    let marker = directory.join("phase4-external.tmp");
    assert!(!marker.exists());
    for i in 0..3 {
        let label = if i == 0 { "create" } else { "modify" };
        eprintln!("begin,{label}");
        let start = Instant::now();
        std::fs::write(&marker, vec![42; i + 1]).unwrap();
        wait(&mut engine, &wakes, |e| {
            let tab = e.tab(left);
            tab.visible_position(&marker)
                .and_then(|position| tab.visible_entry(position))
                .is_some_and(|entry| entry.size == (i + 1) as u64)
        });
        sample(label, start.elapsed());
        wait(&mut engine, &wakes, idle);
    }
    eprintln!("begin,move");
    let moved = empty.join(marker.file_name().unwrap());
    let start = Instant::now();
    std::fs::rename(&marker, &moved).unwrap();
    wait(&mut engine, &wakes, |e| {
        e.tab(left).visible_position(&marker).is_none()
            && e.tab(right).visible_position(&moved).is_some()
    });
    sample("move", start.elapsed());
    wait(&mut engine, &wakes, idle);
    std::fs::remove_file(moved).unwrap();

    eprintln!("begin,burst");
    let start = Instant::now();
    let paths: Vec<_> = (0..10_000)
        .map(|i| directory.join(format!("phase4-burst-{i}.tmp")))
        .collect();
    for path in &paths {
        std::fs::write(path, b"burst").unwrap();
    }
    wait(&mut engine, &wakes, |e| {
        paths
            .iter()
            .all(|path| e.tab(left).visible_position(path).is_some())
    });
    sample("burst", start.elapsed());
    wait(&mut engine, &wakes, idle);
    // Let closely spaced duplicate hints drain before ending the measurement.
    let quiet = Instant::now();
    wait(&mut engine, &wakes, |_| {
        quiet.elapsed() >= Duration::from_millis(500)
    });
    wait(&mut engine, &wakes, idle);
    eprintln!("begin,cleanup");
    drop(engine);
    for path in paths {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
#[ignore = "runs a one-minute idle trace on a release benchmark fixture"]
fn phase4_idle_scan_rate() {
    let fixture = PathBuf::from(std::env::var_os("DESHCMD_PERF_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    let (notify, wakes) = mpsc::channel();
    let mut engine = Engine::new(
        fixture.clone(),
        [fixture.join("100000"), fixture.join("empty")],
        Settings::default(),
        move || {
            let _ = notify.send(());
        },
    );
    wait(&mut engine, &wakes, idle);
    eprintln!("begin,idle_minute");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        // Match the desktop's one-second idle heartbeat.
        let _ = wakes.recv_timeout(Duration::from_secs(1));
        if start.elapsed() < Duration::from_secs(60) {
            engine.poll();
        }
    }
    eprintln!("begin,idle_complete");
}
