//! Release benchmark. See docs/performance-phase1-results.md for the fixture and invocation.
#![cfg(feature = "perf")]

use std::{
    hint::black_box,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use deshcmd_engine::{
    Column, Command, Engine, JobState, MatchOptions, Operation, Settings, Side, Step,
};

const LEFT: Side = Side::Left;

fn navigate(side: Side, path: &Path) -> Command {
    Command::Navigate {
        side,
        path: path.into(),
        new_tab: false,
        background: false,
    }
}

fn measure<T>(label: &str, run: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = run();
    sample(label, start.elapsed());
    result
}

fn sample(label: &str, time: Duration) {
    eprintln!("sample,{label},{:.6}", time.as_secs_f64() * 1000.);
}

fn settle(engine: &mut Engine, wakes: &mpsc::Receiver<()>, label: &str, start: Instant) {
    let mut longest = Duration::ZERO;
    let mut polls = 0;
    loop {
        let poll = Instant::now();
        engine.poll();
        longest = longest.max(poll.elapsed());
        polls += 1;
        if Side::ALL.into_iter().all(|side| !engine.tab(side).busy()) {
            break;
        }
        wakes.recv_timeout(Duration::from_secs(30)).unwrap();
    }
    sample(&format!("{label}/ready"), start.elapsed());
    sample(&format!("{label}/poll_max"), longest);
    eprintln!("polls,{label},{polls}");
}

#[test]
#[ignore = "requires the generated performance fixture and a release build"]
fn phase1_engine() {
    let root = PathBuf::from(std::env::var_os("DESHCMD_PERF_ROOT").expect("set fixture path"));
    let mode = std::env::var("DESHCMD_PERF_CASE").unwrap_or_else(|_| "mixed".into());
    let sizes: &[&str] = match mode.as_str() {
        "plain" => &["plain"],
        "100k" => &["100000"],
        _ => &["100", "10000", "100000"],
    };
    let runs = std::env::var("DESHCMD_PERF_RUNS").map_or(5, |n| n.parse().unwrap());
    let empty = root.join("empty");
    let (wake, wakes) = mpsc::channel();
    let mut engine = Engine::new(
        root.clone(),
        [empty.clone(), empty.clone()],
        Settings {
            auto_refresh: false,
            ..Settings::default()
        },
        move || {
            let _ = wake.send(());
        },
    );
    settle(&mut engine, &wakes, "startup", Instant::now());
    engine.dispatch(navigate(LEFT, &empty));
    settle(&mut engine, &wakes, "empty", Instant::now());

    for &size in sizes {
        let path = root.join(size);
        let expected = if size == "plain" {
            100_000
        } else {
            size.parse::<usize>().unwrap()
                + 2
                + ["file-link", "broken-link"]
                    .into_iter()
                    .filter(|name| path.join(name).symlink_metadata().is_ok())
                    .count()
        };
        for run in 0..runs {
            let visit = if run == 0 { "first" } else { "repeat" };
            let label = format!("{size}/{visit}");
            eprintln!("begin,{label}");
            let start = Instant::now();
            measure(&format!("{label}/dispatch"), || {
                engine.dispatch(navigate(LEFT, &path))
            });
            settle(&mut engine, &wakes, &label, start);
            assert_eq!(engine.tab(LEFT).visible_entries().count(), expected);
            if size != "plain" {
                measure(&format!("{size}/sort_size"), || {
                    engine.dispatch(Command::Sort(LEFT, Column::Size, false))
                });
                measure(&format!("{size}/sort_name"), || {
                    engine.dispatch(Command::Sort(LEFT, Column::Name, false))
                });
                measure(&format!("{size}/filter"), || {
                    engine.dispatch(Command::Filter(
                        LEFT,
                        MatchOptions::filename("file1*.txt;child?.txt"),
                    ));
                });
                assert!(engine.tab(LEFT).visible_entries().count() < expected);
                measure(&format!("{size}/clear_filter"), || {
                    engine.dispatch(Command::Filter(LEFT, MatchOptions::filename("")))
                });
                measure(&format!("{size}/mark_all"), || {
                    engine.dispatch(Command::MarkAll(LEFT))
                });
                engine.dispatch(Command::Step(LEFT, Step::Last, false));
                measure(&format!("{size}/step_marked"), || {
                    for _ in 0..100 {
                        engine.dispatch(Command::Step(LEFT, Step::Previous, false));
                        black_box(engine.tab(LEFT).current());
                    }
                });
            }
            eprintln!("begin,{size}/return");
            let start = Instant::now();
            measure(&format!("{size}/return_dispatch"), || {
                engine.dispatch(navigate(LEFT, &empty))
            });
            settle(&mut engine, &wakes, &format!("{size}/return"), start);
        }
        if size != "plain" {
            eprintln!("begin,{size}/both");
            let start = Instant::now();
            engine.dispatch(navigate(LEFT, &path));
            engine.dispatch(navigate(Side::Right, &path));
            settle(&mut engine, &wakes, &format!("{size}/both"), start);
            assert!(
                Side::ALL
                    .into_iter()
                    .all(|side| engine.tab(side).visible_entries().count() == expected)
            );
            eprintln!("begin,{size}/recursive");
            let start = Instant::now();
            let mut filter = MatchOptions::filename("child");
            filter.recursive = true;
            engine.dispatch(Command::Filter(LEFT, filter));
            settle(&mut engine, &wakes, &format!("{size}/recursive"), start);
            assert_eq!(
                engine.tab(LEFT).visible_entries().next().unwrap().name,
                "child.txt"
            );
            engine.dispatch(navigate(LEFT, &empty));
            engine.dispatch(navigate(Side::Right, &empty));
            settle(&mut engine, &wakes, "reset", Instant::now());
        }
    }

    if mode == "mixed" {
        // A collision keeps a real transfer active while navigating, independently of I/O speed.
        engine.dispatch(Command::Queue(
            Operation::Transfer {
                paths: vec![root.join("100/file0.txt")],
                destination: root.join("10000"),
                moving: false,
                options: Default::default(),
            },
            false,
        ));
        loop {
            engine.poll();
            match engine.jobs.last().unwrap().state {
                JobState::Conflict => break,
                state => assert!(!state.finished(), "{state:?}"),
            }
            wakes.recv_timeout(Duration::from_secs(30)).unwrap();
        }
        let start = Instant::now();
        eprintln!("begin,transfer");
        engine.dispatch(navigate(LEFT, &root.join("100000")));
        settle(&mut engine, &wakes, "transfer", start);
        let job = engine.jobs.last().unwrap();
        assert_eq!(job.state, JobState::Conflict);
        engine.dispatch(Command::CancelJob(job.id));
    }
}
