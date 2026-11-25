//! Linux FIFO listing-pressure benchmark, compatible with the Phase 1 engine API.
#![cfg(all(feature = "perf", target_os = "linux"))]

use deshcmd_engine::{Command, Engine, Operation, Settings, Side};
use std::fs::{self, OpenOptions};
use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);

fn navigate(side: Side, path: &Path) -> Command {
    Command::Navigate {
        side,
        path: path.to_owned(),
        new_tab: false,
        background: false,
    }
}

fn wait(engine: &mut Engine, done: impl Fn(&Engine) -> bool) {
    let start = Instant::now();
    loop {
        engine.poll();
        if done(engine) {
            break;
        }
        assert!(start.elapsed() < TIMEOUT);
        std::thread::yield_now();
    }
}

fn sample(name: &str, time: Duration) {
    eprintln!("sample,{name},{:.6}", time.as_secs_f64() * 1000.);
}

#[test]
#[ignore = "release benchmark on Linux"]
fn phase2_listing_pressure() {
    let root = tempfile::tempdir().unwrap();
    let [empty, blocked, accessible] =
        ["empty", "blocked", "accessible"].map(|name| root.path().join(name));
    for path in [&empty, &blocked, &accessible] {
        fs::create_dir(path).unwrap();
    }
    for i in 0..128 {
        fs::write(blocked.join(format!("file{i:04}.txt")), []).unwrap();
    }
    fs::write(accessible.join("visible.txt"), []).unwrap();
    let fifo = blocked.join("descript.ion");
    let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

    let mut engine = Engine::new(
        root.path().into(),
        [empty.clone(), empty.clone()],
        Settings {
            auto_refresh: false,
            ..Default::default()
        },
        || {},
    );
    wait(&mut engine, |e| {
        Side::ALL.into_iter().all(|side| !e.tab(side).loading)
    });
    engine.dispatch(navigate(Side::Left, &blocked));
    let start = Instant::now();
    let writer = loop {
        engine.poll();
        match OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
        {
            Ok(file) => break file,
            Err(error)
                if error.raw_os_error() == Some(libc::ENXIO) && start.elapsed() < TIMEOUT =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("FIFO rendezvous: {error}"),
        }
    };
    let mut peak_threads = 0;
    for _ in 0..100 {
        let at = Instant::now();
        engine.dispatch(navigate(Side::Left, &blocked));
        sample("rapid_dispatch", at.elapsed());
        peak_threads = peak_threads.max(fs::read_dir("/proc/self/task").unwrap().count());
    }
    eprintln!("count,process_threads_peak,{peak_threads}");

    assert!(engine.tab(Side::Left).loading);
    // Supersede the burst before resuming the consumer. On the bounded engine
    // this leaves only the original scanner blocked and one free listing slot.
    let final_path: PathBuf = root.path().join("empty");
    engine.dispatch(navigate(Side::Left, &final_path));
    let at = Instant::now();
    engine.dispatch(navigate(Side::Right, &accessible));
    wait(&mut engine, |e| {
        e.tab(Side::Right).base() == accessible && !e.tab(Side::Right).loading
    });
    assert_eq!(
        engine.tab(Side::Right).current().unwrap().name,
        "visible.txt"
    );
    sample("opposite_ready", at.elapsed());

    let job_path = root.path().join("created");
    engine.dispatch(Command::Queue(Operation::Mkdir(job_path.clone()), false));
    wait(&mut engine, |e| e.jobs.last().unwrap().state.finished());
    assert!(job_path.is_dir());
    for _ in 0..100 {
        let at = Instant::now();
        engine.poll();
        sample("heartbeat_poll", at.elapsed());
    }

    wait(&mut engine, |e| {
        e.tab(Side::Left).base() == final_path && !e.tab(Side::Left).loading
    });
    // Future opens see a regular file. Existing readers still hold the old inode.
    fs::remove_file(&fifo).unwrap();
    fs::write(&fifo, []).unwrap();
    eprintln!("begin,release");
    drop(writer);
    // Give obsolete workers a bounded observation window to emit scan counters.
    let end = Instant::now() + Duration::from_millis(300);
    while Instant::now() < end {
        engine.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
}
