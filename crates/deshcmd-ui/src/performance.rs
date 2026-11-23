use super::*;
use eframe::App as _;
use engine::{Column, Step};

struct BenchmarkDesktop;

impl Desktop for BenchmarkDesktop {
    fn open(&mut self, _: &Path) -> Result<(), String> {
        unreachable!()
    }
    fn pick_file(&mut self, _: &Path, _: &str) -> Option<PathBuf> {
        unreachable!()
    }
    fn save_file(&mut self, _: &Path, _: &str, _: &str) -> Option<PathBuf> {
        None
    }
    fn pick_directory(&mut self, _: &Path, _: &str) -> Option<PathBuf> {
        unreachable!()
    }
    fn copy_files(&mut self, _: &[PathBuf]) -> Result<(), String> {
        unreachable!()
    }
    fn clipboard_files(&mut self) -> Vec<PathBuf> {
        vec![]
    }
    fn clipboard_text(&mut self) -> Option<String> {
        None
    }
    fn copy_text(&mut self, _: &str) -> Result<(), String> {
        unreachable!()
    }
    fn places(&mut self) -> Vec<Place> {
        vec![]
    }
    fn eject(&mut self, _: &Path) -> Result<(), String> {
        unreachable!()
    }
    fn shortcut(&mut self, _: &Path, _: &Path) -> Result<(), String> {
        unreachable!()
    }
}

fn sample(label: &str, elapsed: Duration) {
    eprintln!("sample,{label},{:.6}", elapsed.as_secs_f64() * 1000.);
}

#[test]
#[ignore = "requires the generated performance fixture and a release build"]
fn phase1_ui() {
    let root = PathBuf::from(std::env::var_os("DESHCMD_PERF_ROOT").expect("set fixture path"));
    let empty = root.join("empty");
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &cc,
        Box::new(BenchmarkDesktop),
        root.clone(),
        [empty.clone(), empty],
        true,
    );
    app.engine.settings.auto_refresh = false;
    let mut frame = eframe::Frame::_new_kittest();
    let side = Side::Left;
    let draw = |app: &mut App, frame: &mut eframe::Frame, events| {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280., 800.))),
                events,
                ..Default::default()
            },
            |ui| app.ui(ui, frame),
        )
    };
    for size in [100, 10_000, 100_000] {
        app.logic(&ctx, &mut frame);
        let _ = draw(&mut app, &mut frame, vec![]);
        let start = Instant::now();
        app.engine.dispatch(Command::Navigate {
            side,
            path: root.join(size.to_string()),
            new_tab: false,
            background: false,
        });
        let mut feedback = false;
        loop {
            app.logic(&ctx, &mut frame);
            let _ = draw(&mut app, &mut frame, vec![]);
            if !app.engine.tab(side).loading {
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
