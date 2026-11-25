use super::*;
use eframe::App as _;
use std::cell::Cell;

#[derive(Default)]
pub(super) struct TestDesktop {
    opened: Option<mpsc::Sender<PathBuf>>,
}

impl Desktop for TestDesktop {
    fn open(&mut self, path: &Path) -> Result<(), String> {
        self.opened
            .as_ref()
            .unwrap()
            .send(path.to_path_buf())
            .unwrap();
        Ok(())
    }
    fn open_terminal(&mut self, _: &Path) -> Result<(), String> {
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

#[test]
fn panel_navigation_and_preview_shortcuts() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let folder = source.join("folder");
    let destination = root.path().join("destination");
    let file = source.join("preview.txt");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(&file, "Example preview").unwrap();
    for index in 0..40 {
        std::fs::write(source.join(format!("z-{index:02}.txt")), "").unwrap();
    }

    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let (opened, launches) = mpsc::channel();
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop {
            opened: Some(opened),
        }),
        root.path().to_path_buf(),
        [source.clone(), destination],
        true,
    );
    let mut frame = eframe::Frame::_new_kittest();
    let side = Side::Left;
    let wait_for_listing = |app: &mut App| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.engine.tab(side).loading {
            assert!(Instant::now() < deadline, "directory listing timed out");
            app.engine.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    wait_for_listing(&mut app);

    let time = Cell::new(0.);
    let draw = |app: &mut App, frame: &mut eframe::Frame, events| {
        time.set(time.get() + 1. / 60.);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280., 800.))),
                time: Some(time.get()),
                events,
                ..Default::default()
            },
            |ui| app.ui(ui, frame),
        );
        output.textures_delta.clear();
        output.shapes
    };
    let press = |app: &mut App, frame: &mut eframe::Frame, key, modifiers| {
        let events = [true, false]
            .map(|pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            })
            .into();
        draw(app, frame, events)
    };
    let plain = egui::Modifiers::NONE;
    let ctrl = egui::Modifiers::CTRL;
    let alt = egui::Modifiers::ALT;

    app.engine
        .dispatch(Command::Select(side, folder.clone(), engine::Select::Focus));
    press(&mut app, &mut frame, Key::ArrowRight, plain);
    assert_eq!(app.engine.tab(side).base(), folder);
    press(&mut app, &mut frame, Key::ArrowLeft, plain);
    assert_eq!(app.engine.tab(side).base(), source);
    press(&mut app, &mut frame, Key::ArrowLeft, alt);
    assert_eq!(app.engine.tab(side).base(), folder);
    press(&mut app, &mut frame, Key::ArrowRight, alt);
    assert_eq!(app.engine.tab(side).base(), source);

    wait_for_listing(&mut app);
    app.engine
        .dispatch(Command::Select(side, file.clone(), engine::Select::Focus));
    press(&mut app, &mut frame, Key::ArrowRight, plain);
    assert_eq!(app.engine.tab(side).base(), source);
    press(&mut app, &mut frame, Key::Q, ctrl);
    assert_eq!(app.interface.quick, Some(side));
    assert_eq!(app.engine.tab(side).current().unwrap().path, file);
    press(&mut app, &mut frame, Key::Q, ctrl);
    assert_eq!(app.interface.quick, None);
    assert!(launches.try_recv().is_err());
    press(&mut app, &mut frame, Key::Enter, plain);
    assert_eq!(launches.try_recv().unwrap(), file);

    let focus_outline = |shapes: Vec<egui::epaint::ClippedShape>| {
        shapes
            .into_iter()
            .find_map(|shape| match shape.shape {
                egui::Shape::Rect(rect)
                    if rect.stroke == egui::Stroke::new(2., ACCENT)
                        && shape.clip_rect.contains_rect(rect.rect) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("the focused entry should have a visible outline")
    };
    for view in [
        engine::View::Details,
        engine::View::Compact,
        engine::View::Gallery,
    ] {
        app.engine.dispatch(Command::View(side, view));
        app.engine
            .dispatch(Command::Select(side, file.clone(), engine::Select::Replace));

        let up = focus_outline(press(&mut app, &mut frame, Key::ArrowUp, plain));
        assert_eq!(app.engine.tab(side).current().unwrap().path, folder);
        let down = focus_outline(press(&mut app, &mut frame, Key::ArrowDown, plain));
        assert_eq!(app.engine.tab(side).current().unwrap().path, file);
        assert!(down.top() > up.top() || down.left() > up.left());
        assert!(app.engine.tab(side).is_marked(&file));

        focus_outline(press(&mut app, &mut frame, Key::End, plain));
        focus_outline(press(&mut app, &mut frame, Key::ArrowUp, plain));
        focus_outline(press(&mut app, &mut frame, Key::Home, plain));
    }

    app.engine
        .dispatch(Command::View(side, engine::View::Details));
    let row = focus_outline(press(&mut app, &mut frame, Key::Home, plain));
    let click = |app: &mut App, frame: &mut eframe::Frame, pos| {
        draw(app, frame, vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            draw(
                app,
                frame,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: plain,
                }],
            );
        }
    };
    click(&mut app, &mut frame, row.center());
    assert!(app.engine.tab(side).is_marked(&folder));
    press(&mut app, &mut frame, Key::ArrowUp, plain);
    let row = focus_outline(press(&mut app, &mut frame, Key::ArrowDown, plain));
    assert_eq!(app.engine.tab(side).current().unwrap().path, file);
    time.set(time.get() + 1.);
    click(&mut app, &mut frame, row.center());
    click(&mut app, &mut frame, row.center());
    assert_eq!(launches.try_recv().unwrap(), file);
    assert!(launches.try_recv().is_err());
    press(&mut app, &mut frame, Key::ArrowUp, plain);

    ctx.memory_mut(|memory| memory.request_focus(panes::filter_id(side)));
    draw(&mut app, &mut frame, vec![]);
    press(&mut app, &mut frame, Key::ArrowLeft, plain);
    press(&mut app, &mut frame, Key::ArrowDown, plain);
    assert_eq!(app.engine.tab(side).base(), source);
    assert_eq!(app.engine.tab(side).current().unwrap().path, folder);

    let parent = source.join("long-list");
    let child = parent.join("z-folder");
    std::fs::create_dir_all(&child).unwrap();
    for index in 0..40 {
        std::fs::create_dir(parent.join(format!("a-{index:02}"))).unwrap();
    }
    ctx.memory_mut(|memory| memory.surrender_focus(panes::filter_id(side)));
    app.engine.dispatch(Command::Navigate {
        side,
        path: child.clone(),
        new_tab: false,
        background: false,
    });
    wait_for_listing(&mut app);
    press(&mut app, &mut frame, Key::ArrowLeft, plain);
    assert_eq!(app.engine.tab(side).base(), parent);
    wait_for_listing(&mut app);
    assert_eq!(app.engine.tab(side).current().unwrap().path, child);
    assert_eq!(app.engine.tab(side).marked_count(), 0);
    focus_outline(draw(&mut app, &mut frame, vec![]));

    let menu_button = draw(&mut app, &mut frame, vec![])
        .into_iter()
        .find_map(|shape| match shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "File" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("File menu button");
    click(&mut app, &mut frame, menu_button);
    assert!(egui::Popup::is_any_open(&ctx));
    press(&mut app, &mut frame, Key::ArrowRight, plain);
    assert_eq!(app.engine.tab(side).base(), parent);
    press(&mut app, &mut frame, Key::ArrowLeft, plain);
    assert_eq!(app.engine.tab(side).base(), parent);
}

#[test]
fn held_delete_shortcuts_only_dispatch_on_the_initial_press() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().to_owned();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop::default()),
        path.clone(),
        [path.clone(), path],
        true,
    );
    app.engine.settings.confirm_trash = false;
    app.engine.settings.confirm_delete = false;

    for (key, modifiers, permanent) in [
        (Key::Delete, egui::Modifiers::SHIFT, true),
        (Key::Delete, egui::Modifiers::NONE, false),
        (Key::F8, egui::Modifiers::NONE, false),
    ] {
        let mut input = |pressed| {
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |_| app.keyboard(&ctx, &mut actions),
            );
            output.textures_delta.clear();
            actions
        };
        let actions = input(true);
        if permanent {
            assert!(matches!(
                actions.as_slice(),
                [Action::Tool(Side::Left, Tool::Delete)]
            ));
        } else {
            assert!(matches!(
                actions.as_slice(),
                [Action::Tool(Side::Left, Tool::Trash)]
            ));
        }
        assert!(input(true).is_empty());
        assert!(input(false).is_empty());
    }
}
