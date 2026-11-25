use super::*;
use eframe::App as _;
use engine::{Column, Step};

use super::tests::TestDesktop;

fn sample(label: &str, elapsed: Duration) {
    eprintln!("sample,{label},{:.6}", elapsed.as_secs_f64() * 1000.);
}

#[test]
#[ignore = "requires the generated performance fixture and a release build"]
fn phase1_ui() {
    let root = PathBuf::from(std::env::var_os("DESHCMD_PERF_ROOT").expect("set fixture path"))
        .canonicalize()
        .unwrap();
    let empty = root.join("empty");
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &cc,
        Box::new(TestDesktop::default()),
        root.clone(),
        [empty.clone(), empty],
        true,
    );
    app.engine.settings.auto_refresh = false;
    let mut frame = eframe::Frame::_new_kittest();
    let side = Side::Left;
    let draw = |app: &mut App, frame: &mut eframe::Frame, events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280., 800.))),
                events,
                ..Default::default()
            },
            |ui| app.ui(ui, frame),
        );
        output.textures_delta.clear();
        output
    };
    for size in [100, 10_000, 100_000] {
        app.logic(&ctx, &mut frame);
        let _ = draw(&mut app, &mut frame, vec![]);
        let start = Instant::now();
        let target = root.join(size.to_string());
        app.engine.dispatch(Command::Navigate {
            side,
            path: target.clone(),
            new_tab: false,
            background: false,
        });
        let mut feedback = false;
        let mut first_draw = false;
        loop {
            let frame_start = Instant::now();
            app.logic(&ctx, &mut frame);
            let _ = draw(&mut app, &mut frame, vec![]);
            sample(&format!("ui/{size}/loading_frame"), frame_start.elapsed());
            if !first_draw
                && app.engine.tab(side).base() == target
                && app.engine.tab(side).visible_count() > 0
            {
                first_draw = true;
                sample(&format!("ui/{size}/first_draw"), start.elapsed());
            }
            if !app.engine.tab(side).busy() {
                assert!(first_draw);
                assert!(app.engine.tab(side).visible_entries().count() >= size);
                sample(&format!("ui/{size}/populated_draw"), start.elapsed());
                break;
            }
            if !feedback {
                sample(&format!("ui/{size}/loading_draw"), start.elapsed());
                feedback = true;
            }
            assert!(start.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = draw(&mut app, &mut frame, vec![]);
        sample(&format!("ui/{size}/settled_draw"), start.elapsed());

        app.engine.dispatch(Command::Step(side, Step::First, false));
        for _ in 0..50 {
            let at = Instant::now();
            app.logic(&ctx, &mut frame);
            let _ = draw(&mut app, &mut frame, vec![]);
            sample(&format!("ui/{size}/none_head"), at.elapsed());
        }
        app.engine.dispatch(Command::Step(side, Step::Last, false));
        for (label, marks) in [
            ("none_end", None),
            ("one_end", Some(false)),
            ("all_end", Some(true)),
        ] {
            match marks {
                Some(false) => {
                    let path = app.engine.tab(side).current().unwrap().path.clone();
                    app.engine
                        .dispatch(Command::Select(side, path, engine::Select::Replace));
                }
                Some(true) => app.engine.dispatch(Command::MarkAll(side)),
                None => {}
            }
            for _ in 0..50 {
                let at = Instant::now();
                app.logic(&ctx, &mut frame);
                let _ = draw(&mut app, &mut frame, vec![]);
                sample(&format!("ui/{size}/{label}"), at.elapsed());
            }
        }
        let middle = app
            .engine
            .tab(side)
            .visible_entries()
            .nth(size / 2)
            .unwrap()
            .path
            .clone();
        app.engine.dispatch(Command::Step(side, Step::First, false));
        app.engine
            .dispatch(Command::Select(side, middle, engine::Select::Focus));
        app.engine.dispatch(Command::MarkAll(side));
        app.interface.focus_scroll = Some(side);
        let _ = draw(&mut app, &mut frame, vec![]);
        for _ in 0..50 {
            let start = Instant::now();
            app.logic(&ctx, &mut frame);
            let _ = draw(&mut app, &mut frame, vec![]);
            sample(&format!("ui/{size}/all_middle"), start.elapsed());
        }
        for _ in 0..50 {
            let at = Instant::now();
            let events = vec![
                egui::Event::PointerMoved(egui::pos2(350., 420.)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0., -24.),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                },
            ];
            app.logic(&ctx, &mut frame);
            let _ = draw(&mut app, &mut frame, events);
            sample(&format!("ui/{size}/scroll_all"), at.elapsed());
        }
        app.engine.dispatch(Command::UnmarkAll(side));
        if size == 10_000 || std::env::var_os("DESHCMD_PERF_EXPORT_100K").is_some() {
            app.engine.settings.columns = vec![Column::Name];
            for _ in 0..5 {
                let at = Instant::now();
                app.invoke(side, Tool::ExportDetails, &ctx);
                sample(&format!("ui/{size}/export_all"), at.elapsed());
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "release benchmark on Linux; uses mkfifo"]
fn phase2_blocked_listing_ui() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let blocked = root.join("blocked");
    std::fs::create_dir(&blocked).unwrap();
    let fifo = blocked.join("descript.ion");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &cc,
        Box::new(TestDesktop::default()),
        root.clone(),
        [root.clone(), root],
        true,
    );
    app.engine.settings.auto_refresh = false;
    let mut frame = eframe::Frame::_new_kittest();
    let mut draw = |app: &mut App| {
        app.logic(&ctx, &mut frame);
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280., 800.))),
                ..Default::default()
            },
            |ui| app.ui(ui, &mut frame),
        )
        .textures_delta
        .clear();
    };
    for _ in 0..10 {
        draw(&mut app);
    }
    app.engine.dispatch(Command::Navigate {
        side: Side::Left,
        path: blocked,
        new_tab: false,
        background: false,
    });
    let (opened, writer) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = opened.send(std::fs::OpenOptions::new().write(true).open(fifo));
    });
    let started = Instant::now();
    let hold = loop {
        draw(&mut app);
        if let Ok(file) = writer.try_recv() {
            break file.unwrap();
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    };
    for _ in 0..100 {
        let start = Instant::now();
        draw(&mut app);
        sample("blocked_ui_frame", start.elapsed());
        assert!(app.engine.tab(Side::Left).loading);
    }
    drop(hold);
}

#[test]
fn provisional_rows_render_in_every_view_and_keep_pointer_identity_during_reorder() {
    let fixture = tempfile::tempdir().unwrap();
    let (root, target, expected) = if let Some(root) = std::env::var_os("DESHCMD_PERF_LARGE_TEST") {
        let root = PathBuf::from(root).canonicalize().unwrap();
        let target = root.join("plain");
        (root, target, 100_000)
    } else {
        let root = fixture.path().canonicalize().unwrap();
        let target = root.join("large");
        std::fs::create_dir(root.join("empty")).unwrap();
        std::fs::create_dir(&target).unwrap();
        for number in 0..256 {
            std::fs::write(target.join(format!("file{number:03}.txt")), "Preview text").unwrap();
        }
        (root, target, 256)
    };
    let empty = root.join("empty");
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &cc,
        Box::new(TestDesktop::default()),
        root,
        [empty.clone(), empty],
        true,
    );
    app.engine.settings.auto_refresh = false;
    crate::tests::settle(&mut app);
    let (release, gate) = mpsc::channel();
    let gate = std::sync::Mutex::new(gate);
    app.engine.set_first_page_test_hook(move || {
        gate.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .map_err(std::io::Error::other)
    });
    let side = Side::Left;
    app.engine.dispatch(Command::Navigate {
        side,
        path: target,
        new_tab: false,
        background: false,
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.engine.tab(side).provisional() {
        app.engine.poll();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }

    let mut frame = eframe::Frame::_new_kittest();
    let mut draw = |app: &mut App, events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280., 800.))),
                events,
                ..Default::default()
            },
            |ui| app.ui(ui, &mut frame),
        );
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.pos + text.galley.size() / 2.,
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    app.invoke(side, Tool::ExportDetails, &ctx);
    assert!(
        app.interface
            .notice
            .as_ref()
            .is_some_and(|(text, _)| text.contains("finish loading"))
    );
    app.engine.dispatch(Command::Filter(
        side,
        engine::MatchOptions::filename("no-match"),
    ));
    while app.engine.tab(side).visible_count() != 0 {
        app.engine.poll();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let drawn = draw(&mut app, vec![]);
    assert!(drawn.iter().any(|(label, _)| label == "Loading files…"));
    assert!(!drawn.iter().any(|(label, _)| label == "No matching items"));
    app.engine
        .dispatch(Command::Filter(side, engine::MatchOptions::filename("")));
    while app.engine.tab(side).visible_count() == 0 {
        app.engine.poll();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let first_name = app.engine.tab(side).visible_entry(0).unwrap().name.clone();
    for view in [
        engine::View::Gallery,
        engine::View::Compact,
        engine::View::Details,
    ] {
        app.engine.dispatch(Command::View(side, view));
        let drawn = draw(&mut app, vec![]);
        assert!(
            drawn
                .iter()
                .any(|(label, _)| label.contains("shown · loading"))
        );
        assert!(drawn.iter().any(|(label, _)| label == &first_name));
    }
    let pointer = draw(&mut app, vec![])
        .into_iter()
        .find_map(|(label, point)| (label == first_name).then_some(point))
        .unwrap();
    let pointer_event = |pressed| egui::Event::PointerButton {
        pos: pointer,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    draw(
        &mut app,
        vec![egui::Event::PointerMoved(pointer), pointer_event(true)],
    );
    app.engine
        .dispatch(Command::Sort(side, Column::Name, false));
    while app.engine.tab(side).visible_entry(0).unwrap().name == first_name {
        app.engine.poll();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    draw(&mut app, vec![pointer_event(false)]);
    assert!(
        app.engine
            .tab(side)
            .marked_entries()
            .all(|entry| entry.name == first_name)
    );
    app.apply(Action::ToggleQuick(side), &ctx);
    let drawn = draw(&mut app, vec![]);
    assert!(drawn.iter().any(|(label, _)| label == "Quick View"));
    release.send(()).unwrap();
    crate::tests::settle(&mut app);
    assert_eq!(app.engine.tab(side).visible_count(), expected);
}
