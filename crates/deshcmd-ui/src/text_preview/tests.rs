use super::*;

fn entry(path: PathBuf, size: u64) -> Entry {
    Entry {
        path,
        archive: None,
        name: "log.txt".into(),
        extension: "txt".into(),
        kind: deshcmd_engine::Kind::Text,
        size,
        modified: None,
        created: None,
        accessed: None,
        read_only: false,
        link: false,
        hidden: false,
        comment: String::new(),
        attributes: String::new(),
    }
}

fn draw(view: &mut LargeTextView, ctx: &egui::Context, events: Vec<egui::Event>) -> Rect {
    let mut rect = Rect::NOTHING;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400., 220.))),
            events,
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                rect = ui.available_rect_before_wrap();
                view.show(ui);
            });
        },
    );
    output.textures_delta.clear();
    rect
}

fn render_until(
    view: &mut LargeTextView,
    ctx: &egui::Context,
    ready: impl Fn(&LargeTextView) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ready(view) {
        assert!(
            std::time::Instant::now() < deadline,
            "Quick View did not load its text window"
        );
        draw(view, ctx, Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn large_text_preview_scrolls_and_loads_a_window_at_the_end() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("log.txt");
    let mut log = "a line from the beginning of the log\n".repeat(20_000);
    log.push_str("MARKER AT END OF THE LOG\n");
    std::fs::write(&path, &log).unwrap();
    let mut file = entry(path, log.len() as u64);
    let ctx = egui::Context::default();
    let mut view = LargeTextView::new(&file, ctx.clone());
    assert!(view.matches(&file));
    file.modified = Some(SystemTime::UNIX_EPOCH);
    assert!(!view.matches(&file));

    let rect = draw(&mut view, &ctx, Vec::new());
    render_until(&mut view, &ctx, |view| view.window.is_some());
    assert_eq!(view.offset, 0);
    draw(
        &mut view,
        &ctx,
        vec![
            egui::Event::PointerMoved(rect.center()),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                delta: vec2(0., -120.),
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert!(view.offset > 0);
    let position = pos2(rect.right() - 5., rect.bottom() - 2.);
    let button = |pressed| egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    draw(
        &mut view,
        &ctx,
        vec![egui::Event::PointerMoved(position), button(true)],
    );
    draw(&mut view, &ctx, vec![button(false)]);
    render_until(&mut view, &ctx, |view| {
        view.window
            .as_ref()
            .is_some_and(|window| window.bytes.end == view.len)
    });
    assert!(view.offset > view.len / 2);
    assert_eq!(
        view.window.as_ref().unwrap().rows.last().unwrap().1,
        "MARKER AT END OF THE LOG"
    );
}

#[test]
fn missing_file_switches_preview_to_a_read_error() {
    let fixture = tempfile::tempdir().unwrap();
    let file = entry(fixture.path().join("missing"), 50_000);
    let ctx = egui::Context::default();
    let mut view = LargeTextView::new(&file, ctx.clone());
    render_until(&mut view, &ctx, |view| view.failed);
    assert!(view.window.is_none());
    draw(&mut view, &ctx, Vec::new());
}
