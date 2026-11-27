use super::*;
use eframe::App as _;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Default)]
pub(super) struct DesktopState {
    supports_updates: bool,
    pub(super) files: Vec<PathBuf>,
    pub(super) text: Option<String>,
    pub(super) file_clipboard_unavailable: bool,
    pub(super) terminal: Option<PathBuf>,
    pick: Option<PathBuf>,
    save: Option<PathBuf>,
}

#[derive(Default)]
pub(super) struct TestDesktop {
    opened: Option<mpsc::Sender<PathBuf>>,
    preview_cache: Option<PathBuf>,
    pub(super) state: Rc<RefCell<DesktopState>>,
}

impl Desktop for TestDesktop {
    fn preview_cache_directory(&self) -> Option<PathBuf> {
        self.preview_cache.clone()
    }
    fn supports_updates(&self) -> bool {
        self.state.borrow().supports_updates
    }
    fn open(&mut self, path: &Path) -> Result<(), String> {
        self.opened
            .as_ref()
            .unwrap()
            .send(path.to_path_buf())
            .unwrap();
        Ok(())
    }
    fn open_terminal(&mut self, directory: &Path) -> Result<(), String> {
        self.state.borrow_mut().terminal = Some(directory.into());
        Ok(())
    }
    fn pick_file(&mut self, _: &Path, _: &str) -> Option<PathBuf> {
        self.state.borrow_mut().pick.take()
    }
    fn save_file(&mut self, _: &Path, _: &str, _: &str) -> Option<PathBuf> {
        self.state.borrow_mut().save.take()
    }
    fn pick_directory(&mut self, _: &Path, _: &str) -> Option<PathBuf> {
        self.state.borrow_mut().pick.take()
    }
    fn copy_files(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        if state.file_clipboard_unavailable {
            return Err("System clipboard unavailable".into());
        }
        state.files = paths.to_vec();
        state.text = None;
        Ok(())
    }
    fn clipboard_files(&mut self) -> Vec<PathBuf> {
        self.state.borrow().files.clone()
    }
    fn clipboard_text(&mut self) -> Option<String> {
        self.state.borrow().text.clone()
    }
    fn copy_text(&mut self, text: &str) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        state.text = Some(text.into());
        state.files.clear();
        Ok(())
    }
    fn eject(&mut self, _: &Path) -> Result<(), String> {
        unreachable!()
    }
    fn shortcut(&mut self, _: &Path, _: &Path) -> Result<(), String> {
        unreachable!()
    }
}

#[test]
fn update_item_is_visible_in_help_in_a_short_window() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().to_path_buf();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let desktop = TestDesktop::default();
    desktop.state.borrow_mut().supports_updates = true;
    let mut app = App::new(
        &creation,
        Box::new(desktop),
        path.clone(),
        [path.clone(), path],
        true,
    );
    let mut frame = eframe::Frame::_new_kittest();
    let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(900., 480.));
    let mut draw = |events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            },
            |ui| app.ui(ui, &mut frame),
        );
        output.textures_delta.clear();
        output.shapes
    };
    let menu_button = draw(vec![])
        .into_iter()
        .find_map(|shape| match shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Help" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("Help menu button");
    draw(vec![egui::Event::PointerMoved(menu_button)]);
    for pressed in [true, false] {
        draw(vec![egui::Event::PointerButton {
            pos: menu_button,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }]);
    }
    assert!(egui::Popup::is_any_open(&ctx));
    assert!(
        draw(vec![]).into_iter().any(|shape| match shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Check for Updates…" => {
                let bounds = Rect::from_min_size(text.pos, text.galley.size());
                bounds.bottom() < 140. && shape.clip_rect.contains_rect(bounds)
            }
            _ => false,
        }),
        "Check for Updates should be visible in Help"
    );
}

#[test]
fn escape_dismisses_an_update_check_and_ignores_its_result() {
    let root = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop::default()),
        root.path().to_path_buf(),
        [root.path().to_path_buf(), root.path().to_path_buf()],
        true,
    );
    let (sender, receiver) = mpsc::channel();
    app.update_check = Some(receiver);
    app.update = Some(updates::UpdateDialog::Checking);
    let draw = |app: &mut App, events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800., 600.))),
                events,
                ..Default::default()
            },
            |ui| app.update_dialog(ui.ctx()),
        );
        output.textures_delta.clear();
    };
    draw(&mut app, vec![]);
    draw(
        &mut app,
        vec![egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert!(
        app.update.is_none(),
        "Escape did not dismiss the update dialog"
    );
    assert!(app.update_check.is_some());
    sender.send(Ok(None)).unwrap();
    app.poll_update();
    assert!(
        app.update.is_none(),
        "A dismissed check reopened its dialog"
    );
    assert!(app.update_check.is_none());
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
    std::fs::write(folder.join("payload.bin"), [0; 2048]).unwrap();
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
            ..Default::default()
        }),
        root.path().to_path_buf(),
        [source.clone(), destination],
        true,
    );
    let mut frame = eframe::Frame::_new_kittest();
    let side = Side::Left;
    let wait_for_listing = |app: &mut App| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.engine.tab(side).busy() {
            assert!(Instant::now() < deadline, "directory listing timed out");
            app.engine.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    wait_for_listing(&mut app);

    let time = Cell::new(0.);
    let draw = |app: &mut App, frame: &mut eframe::Frame, events: Vec<egui::Event>| {
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

    app.interface.show_inline_filter = false;
    press(&mut app, &mut frame, Key::F, ctrl);
    draw(&mut app, &mut frame, Vec::new());
    assert!(app.dialog.is_none());
    assert!(ctx.memory(|memory| memory.has_focus(panes::filter_id(side))));
    draw(
        &mut app,
        &mut frame,
        vec![egui::Event::Text("preview".into())],
    );
    assert_eq!(app.engine.tab(side).filter().text, "preview");
    press(&mut app, &mut frame, Key::Escape, plain);
    assert!(!ctx.memory(|memory| memory.has_focus(panes::filter_id(side))));
    press(&mut app, &mut frame, Key::F, ctrl | egui::Modifiers::SHIFT);
    assert!(app.find_files.open);
    assert!(frame_contains(
        &draw(&mut app, &mut frame, Vec::new()),
        "Find Files"
    ));
    draw(
        &mut app,
        &mut frame,
        vec![egui::Event::Text("preview".into())],
    );
    press(&mut app, &mut frame, Key::Enter, ctrl);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let shapes = draw(&mut app, &mut frame, Vec::new());
        if frame_contains(&shapes, "1 match") {
            assert!(frame_contains(&shapes, "preview.txt"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Find Files did not display the result"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    press(&mut app, &mut frame, Key::Escape, plain);
    assert!(!app.find_files.open);

    app.engine
        .dispatch(Command::Select(side, folder.clone(), engine::Select::Focus));
    press(&mut app, &mut frame, Key::ArrowRight, plain);
    wait_for_listing(&mut app);
    assert_eq!(app.engine.tab(side).base(), folder);
    press(&mut app, &mut frame, Key::ArrowLeft, plain);
    wait_for_listing(&mut app);
    assert_eq!(app.engine.tab(side).base(), source);
    press(&mut app, &mut frame, Key::ArrowLeft, alt);
    wait_for_listing(&mut app);
    assert_eq!(app.engine.tab(side).base(), folder);
    press(&mut app, &mut frame, Key::ArrowRight, alt);
    wait_for_listing(&mut app);
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
    let click = |app: &mut App, frame: &mut eframe::Frame, pos, modifiers| {
        draw(app, frame, vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            draw(
                app,
                frame,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                }],
            );
        }
    };
    click(&mut app, &mut frame, row.center(), plain);
    assert_eq!(app.engine.tab(side).current().unwrap().path, folder);
    assert!(!app.engine.tab(side).is_marked(&folder));
    assert!(app.engine.tab(side).is_marked(&file));

    app.engine.dispatch(Command::UnmarkAll(side));
    let shapes = draw(&mut app, &mut frame, vec![]);
    assert!(!frame_contains(&shapes, "Calculating size…"));
    assert!(!frame_contains(&shapes, "2.0 KB"));
    press(&mut app, &mut frame, Key::Space, plain);
    assert!(app.engine.tab(side).is_marked(&folder));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let shapes = draw(&mut app, &mut frame, vec![]);
        if frame_contains(&shapes, "1 selected · 2.0 KB") {
            assert!(shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text() == "2.0 KB")));
            break;
        }
        assert!(Instant::now() < deadline, "folder size did not appear");
        std::thread::sleep(Duration::from_millis(2));
    }
    app.engine.dispatch(Command::UnmarkAll(side));
    press(&mut app, &mut frame, Key::ArrowDown, egui::Modifiers::SHIFT);
    assert!(app.engine.tab(side).is_marked(&folder));
    assert!(app.engine.tab(side).is_marked(&file));
    assert!(frame_contains(
        &draw(&mut app, &mut frame, vec![]),
        "2 selected · 2.0 KB"
    ));

    app.engine.dispatch(Command::UnmarkAll(side));
    time.set(time.get() + 1.);
    click(&mut app, &mut frame, row.center(), ctrl);
    assert!(app.engine.tab(side).is_marked(&folder));
    let down = focus_outline(press(&mut app, &mut frame, Key::ArrowDown, plain));
    press(&mut app, &mut frame, Key::ArrowUp, plain);
    click(&mut app, &mut frame, down.center(), egui::Modifiers::SHIFT);
    assert!(app.engine.tab(side).is_marked(&folder));
    assert!(app.engine.tab(side).is_marked(&file));
    assert!(frame_contains(
        &draw(&mut app, &mut frame, vec![]),
        "2 selected · 2.0 KB"
    ));

    time.set(time.get() + 1.);
    draw(
        &mut app,
        &mut frame,
        vec![egui::Event::PointerMoved(row.center())],
    );
    time.set(time.get() + 1.);
    draw(&mut app, &mut frame, vec![]);
    let path_hint = folder.display().to_string();
    assert!(frame_contains(
        &draw(&mut app, &mut frame, vec![]),
        &path_hint
    ));
    let shapes = press(&mut app, &mut frame, Key::ArrowUp, plain);
    assert!(!frame_contains(&shapes, &path_hint));
    time.set(time.get() + 1.);
    assert!(!frame_contains(
        &draw(&mut app, &mut frame, vec![]),
        &path_hint
    ));
    draw(
        &mut app,
        &mut frame,
        vec![egui::Event::PointerMoved(row.center() + vec2(4., 0.))],
    );
    time.set(time.get() + 1.);
    draw(&mut app, &mut frame, vec![]);
    assert!(frame_contains(
        &draw(&mut app, &mut frame, vec![]),
        &path_hint
    ));

    let empty_strip = pos2(430., 72.);
    time.set(time.get() + 1.);
    click(&mut app, &mut frame, empty_strip, plain);
    assert_eq!(app.engine.pane(side).tabs.len(), 1);
    click(&mut app, &mut frame, empty_strip, plain);
    assert_eq!(app.engine.pane(side).tabs.len(), 2);
    app.engine.dispatch(Command::CloseTab(side, 1));
    app.engine.settings.double_click_new_tab = false;
    time.set(time.get() + 1.);
    click(&mut app, &mut frame, empty_strip, plain);
    click(&mut app, &mut frame, empty_strip, plain);
    assert_eq!(app.engine.pane(side).tabs.len(), 1);
    wait_for_listing(&mut app);
    press(&mut app, &mut frame, Key::ArrowUp, plain);
    let row = focus_outline(press(&mut app, &mut frame, Key::ArrowDown, plain));
    assert_eq!(app.engine.tab(side).current().unwrap().path, file);
    time.set(time.get() + 1.);
    click(&mut app, &mut frame, row.center(), plain);
    click(&mut app, &mut frame, row.center(), plain);
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
    wait_for_listing(&mut app);
    assert_eq!(app.engine.tab(side).base(), parent);
    assert_eq!(app.engine.tab(side).current().unwrap().path, child);
    assert_eq!(app.engine.tab(side).marked_count(), 0);
    focus_outline(draw(&mut app, &mut frame, vec![]));

    app.engine.dispatch(Command::NewTab(side));
    app.engine.dispatch(Command::NewTab(side));
    for (modifiers, index) in [
        (ctrl, 0),
        (ctrl | egui::Modifiers::SHIFT, 2),
        (ctrl | egui::Modifiers::SHIFT, 1),
        (ctrl | alt, 1),
        (ctrl, 2),
        (ctrl, 0),
    ] {
        press(&mut app, &mut frame, Key::Tab, modifiers);
        assert_eq!(app.engine.pane(side).active, index);
        assert_eq!(app.engine.settings.active, side);
        assert_eq!(app.engine.pane(side.other()).active, 0);
    }
    press(&mut app, &mut frame, Key::Tab, plain);
    assert_eq!(app.engine.settings.active, side.other());
    press(&mut app, &mut frame, Key::Tab, ctrl);
    assert_eq!(app.engine.settings.active, side.other());
    assert_eq!(app.engine.pane(side.other()).active, 0);
    press(&mut app, &mut frame, Key::Tab, plain);
    assert_eq!(app.engine.settings.active, side);

    press(&mut app, &mut frame, Key::L, egui::Modifiers::COMMAND);
    let path_field = panes::path_id(side);
    assert!(ctx.memory(|memory| memory.has_focus(path_field)));
    let pos = ctx.read_response(path_field).unwrap().rect.center() + vec2(0., 60.);
    click(&mut app, &mut frame, pos, plain);
    draw(&mut app, &mut frame, vec![]);
    assert!(!ctx.memory(|memory| memory.has_focus(path_field)));
    draw(&mut app, &mut frame, vec![]);
    assert!(ctx.read_response(path_field).is_none());
    assert_eq!(app.engine.tab(side).base(), parent);

    let initially_hidden = app.engine.settings.show_hidden;
    press(&mut app, &mut frame, Key::Period, ctrl);
    assert_eq!(app.engine.settings.show_hidden, !initially_hidden);
    press(&mut app, &mut frame, Key::Period, ctrl);
    assert_eq!(app.engine.settings.show_hidden, initially_hidden);
    wait_for_listing(&mut app);

    press(&mut app, &mut frame, Key::F4, egui::Modifiers::SHIFT);
    assert!(app.dialog.is_some(), "Shift+F4 did not open New text file");
    app.dialog = None;
    ctx.memory_mut(|memory| {
        if let Some(id) = memory.focused() {
            memory.surrender_focus(id);
        }
    });

    let menu_button = draw(&mut app, &mut frame, vec![])
        .into_iter()
        .find_map(|shape| match shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "File" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("File menu button");
    click(&mut app, &mut frame, menu_button, plain);
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

pub(super) fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.engine.poll();
        if Side::ALL
            .into_iter()
            .all(|side| !app.engine.tab(side).busy())
            && app.engine.jobs.iter().all(|job| job.state.finished())
        {
            return;
        }
        assert!(Instant::now() < deadline, "listing or job timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn workspace_exports_can_be_imported_through_native_file_pickers() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let left = root.join("left");
    let right = root.join("right");
    std::fs::create_dir(&left).unwrap();
    std::fs::create_dir(&right).unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let desktop = TestDesktop::default();
    let native = desktop.state.clone();
    let mut app = App::new(
        &creation,
        Box::new(desktop),
        root.clone(),
        [left.clone(), right.clone()],
        true,
    );
    let bookmark = engine::Bookmark {
        name: "Original bookmarks".into(),
        path: left.clone(),
        opposite: Some(right),
        group: "Fixtures".into(),
    };
    app.engine.dispatch(Command::AddBookmark(bookmark.clone()));
    app.engine.dispatch(Command::NewTab(Side::Left));

    let tabs = root.join("export.tabs");
    native.borrow_mut().save = Some(tabs.clone());
    app.invoke(Side::Left, Tool::ExportTabs, &ctx);
    settle(&mut app);
    assert!(tabs.exists());
    app.engine.dispatch(Command::CloseTab(Side::Left, 1));
    native.borrow_mut().pick = Some(tabs);
    app.invoke(Side::Left, Tool::ImportTabs, &ctx);
    wait_for_dialog_read(&mut app, &ctx, |app| {
        app.engine.pane(Side::Left).tabs.len() == 2
    });
    assert_eq!(app.engine.pane(Side::Left).tabs.len(), 2);

    let bookmarks = root.join("export.bookmarks");
    native.borrow_mut().save = Some(bookmarks.clone());
    app.invoke(Side::Left, Tool::ExportBookmarks, &ctx);
    settle(&mut app);
    app.engine.dispatch(Command::RemoveBookmark(0));
    native.borrow_mut().pick = Some(bookmarks);
    app.invoke(Side::Left, Tool::ImportBookmarks, &ctx);
    wait_for_dialog_read(&mut app, &ctx, |app| {
        !app.engine.settings.bookmarks.is_empty()
    });
    assert_eq!(app.engine.settings.bookmarks, [bookmark]);

    let invalid = root.join("bad.bookmarks");
    std::fs::write(&invalid, "invalid json").unwrap();
    native.borrow_mut().pick = Some(invalid);
    app.invoke(Side::Left, Tool::ImportBookmarks, &ctx);
    wait_for_dialog_read(&mut app, &ctx, |app| {
        app.interface
            .notice
            .as_ref()
            .is_some_and(|notice| notice.0.contains("expected"))
    });
    assert!(
        app.interface
            .notice
            .as_ref()
            .unwrap()
            .0
            .contains("expected")
    );
    assert_eq!(app.engine.settings.bookmarks.len(), 1);
}

#[test]
fn close_request_reveals_the_queue_until_pending_work_is_cancelled() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().to_path_buf();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop::default()),
        root.clone(),
        [root.clone(), root.clone()],
        true,
    );
    settle(&mut app);
    app.apply(
        Action::Command(Command::Queue(
            engine::Operation::Create(root.join("held")),
            true,
        )),
        &ctx,
    );
    assert!(!app.interface.queue);
    app.apply(Action::Close, &ctx);
    assert!(app.interface.queue);
    assert!(!app.interface.closing);
    assert!(
        app.interface
            .notice
            .as_ref()
            .unwrap()
            .0
            .contains("pending operations")
    );
    let job = app.engine.jobs[0].id;
    assert_eq!(app.engine.jobs[0].state, engine::JobState::Held);
    app.apply(Action::Command(Command::CancelJob(job)), &ctx);
    app.apply(Action::Close, &ctx);
    assert!(app.interface.closing);
    assert!(!root.join("held").exists());
}

#[derive(Default)]
struct MemoryStorage(HashMap<String, String>);

impl eframe::Storage for MemoryStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }

    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }

    fn flush(&mut self) {}
}

#[test]
fn app_storage_restores_workspace_and_independent_display_preferences() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let left = root.join("left");
    let right = root.join("right");
    std::fs::create_dir(&left).unwrap();
    std::fs::create_dir(&right).unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop::default()),
        root.clone(),
        [left.clone(), right.clone()],
        true,
    );
    app.engine.dispatch(Command::NewTab(Side::Right));
    app.engine
        .dispatch(Command::RenameTab(Side::Right, 1, "Saved tab".into()));
    app.engine.settings.stacked = true;
    app.interface.tree = true;
    app.interface.alternate_columns = [true, false];
    app.interface.show_inline_filter = false;
    app.interface.show_places = false;
    app.interface.show_bottom_bar = false;
    app.interface.ui_font = theme::UiFont::DejaVuSansMono;

    let mut storage = MemoryStorage::default();
    app.save(&mut storage);
    let mut creation = eframe::CreationContext::_new_kittest(egui::Context::default());
    creation.storage = Some(&storage);
    let reopen = |fresh| {
        App::new(
            &creation,
            Box::new(TestDesktop::default()),
            root.clone(),
            [left.clone(), right.clone()],
            fresh,
        )
    };
    let restored = reopen(false);
    assert!(restored.engine.settings.stacked);
    assert_eq!(restored.engine.tab(Side::Right).title(), "Saved tab");
    assert!(restored.interface.tree);
    assert_eq!(restored.interface.alternate_columns, [true, false]);
    assert!(!restored.interface.show_inline_filter);
    assert!(!restored.interface.show_places);
    assert!(!restored.interface.show_bottom_bar);
    assert!(restored.interface.ui_font == theme::UiFont::DejaVuSansMono);
    let fresh = reopen(true);
    assert_eq!(fresh.engine.pane(Side::Right).tabs.len(), 1);
    assert!(!fresh.engine.settings.stacked);
    assert!(!fresh.interface.show_places);
}

pub(crate) fn frame_contains(shapes: &[egui::epaint::ClippedShape], expected: &str) -> bool {
    fn matches(shape: &egui::Shape, expected: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text().contains(expected),
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| matches(shape, expected)),
            _ => false,
        }
    }
    shapes.iter().any(|shape| matches(&shape.shape, expected))
}

#[test]
fn workspace_renders_tree_gallery_help_and_a_transfer_conflict() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let left = root.join("left");
    let right = root.join("right");
    let source = left.join("shared.txt");
    std::fs::create_dir_all(left.join("directory")).unwrap();
    std::fs::create_dir(&right).unwrap();
    std::fs::write(&source, "incoming").unwrap();
    std::fs::write(right.join("shared.txt"), "existing").unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop::default()),
        root,
        [left.clone(), right.clone()],
        true,
    );
    settle(&mut app);
    app.places = vec![
        Place {
            name: "Fixture place".into(),
            path: left,
            drive: false,
            ejectable: false,
        },
        Place {
            name: "Fixture drive".into(),
            path: right.clone(),
            drive: true,
            ejectable: true,
        },
    ]
    .into();
    app.interface.tree = true;
    app.engine.settings.stacked = true;

    let mut frame = eframe::Frame::_new_kittest();
    let mut draw = |app: &mut App| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280., 800.))),
                ..Default::default()
            },
            |ui| app.ui(ui, &mut frame),
        );
        output.textures_delta.clear();
        output.shapes
    };
    app.engine
        .dispatch(Command::View(Side::Left, engine::View::Gallery));
    app.engine
        .dispatch(Command::View(Side::Right, engine::View::Compact));
    let queue_highlighted = |shapes: &[egui::epaint::ClippedShape], title: &str| {
        let text = shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == title => Some(text.pos),
                _ => None,
            })
            .expect("expected the Queue toolbar button");
        shapes.iter().any(|shape| matches!(
            &shape.shape, egui::Shape::Rect(rect) if rect.fill == ACCENT && rect.rect.contains(text)
        ))
    };
    let shapes = draw(&mut app);
    assert!(!queue_highlighted(&shapes, "Queue"));
    assert!(frame_contains(&shapes, "Fixture place"));
    assert!(frame_contains(&shapes, "Fixture drive"));
    assert!(frame_contains(&shapes, "directory"));
    assert!(frame_contains(&shapes, "shared.txt"));
    app.interface.show_help = true;
    draw(&mut app);
    assert!(frame_contains(&draw(&mut app), "Keyboard shortcuts"));
    app.interface.show_help = false;

    app.apply(
        Action::Command(Command::QueueFor(
            Side::Right,
            engine::Operation::Statistics(vec![right.clone()]),
            true,
        )),
        &ctx,
    );
    app.engine.jobs[0].progress.item_total = Some(4);
    app.engine.jobs[0].progress.items = 1;
    let shapes = draw(&mut app);
    assert!(!app.interface.queue);
    assert!(queue_highlighted(&shapes, "Queue  1"));
    assert!(frame_contains(&shapes, "Calculate size · On hold"));
    assert!(frame_contains(&shapes, "25%"));
    assert!(shapes.iter().any(|shape| matches!(
        &shape.shape, egui::Shape::Rect(rect) if rect.fill == OPERATION_SURFACE
    )));
    app.apply(
        Action::Command(Command::QueueFor(
            Side::Right,
            engine::Operation::Benchmark,
            true,
        )),
        &ctx,
    );
    let shapes = draw(&mut app);
    assert!(frame_contains(&shapes, "Calculate size · On hold"));
    assert!(frame_contains(&shapes, "Hash benchmark · On hold"));
    assert!(!frame_contains(&shapes, "25%"));
    app.apply(Action::ShowQueue, &ctx);
    let shapes = draw(&mut app);
    assert!(frame_contains(&shapes, "Operations"));
    assert!(frame_contains(&shapes, "25%"));
    let right_summary = format!("right · {}", right.parent().unwrap().display());
    assert!(frame_contains(&shapes, &right_summary));
    assert!(!frame_contains(&shapes, "Calculate size · On hold"));
    app.interface.queue = false;
    for id in app.engine.jobs.iter().map(|job| job.id).collect::<Vec<_>>() {
        app.engine.dispatch(Command::CancelJob(id));
    }
    app.engine.dispatch(Command::ClearJobs);
    assert!(app.engine.jobs.is_empty());
    assert_eq!(clock(3661), "01:01:01");

    let operation = engine::Operation::Transfer {
        paths: vec![source.clone()],
        destination: right.clone(),
        moving: false,
        options: engine::TransferOptions::default(),
    };
    app.apply(Action::Command(Command::Queue(operation, false)), &ctx);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.engine.jobs[0].state != engine::JobState::Conflict {
        app.engine.poll();
        assert!(
            Instant::now() < deadline,
            "transfer conflict was not reported"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    draw(&mut app);
    let shapes = draw(&mut app);
    assert!(frame_contains(&shapes, "shared.txt"));
    assert!(frame_contains(&shapes, "Skip"));
    assert!(!app.interface.queue);
    let id = app.engine.jobs[0].id;
    app.apply(
        Action::Command(Command::Resolve(id, engine::Collision::Skip, false)),
        &ctx,
    );
    settle(&mut app);
    assert_eq!(
        std::fs::read(right.join("shared.txt")).unwrap(),
        b"existing"
    );
    assert!(!app.interface.queue);
    assert!(!queue_highlighted(&draw(&mut app), "Queue"));
    app.interface.queue = true;
    let shapes = draw(&mut app);
    assert!(queue_highlighted(&shapes, "Queue"));
    assert!(frame_contains(&shapes, "1 skipped"));
    assert!(frame_contains(
        &shapes,
        &format!("shared.txt · {}", source.parent().unwrap().display())
    ));
    assert!(frame_contains(&shapes, &right_summary));
    app.invoke(Side::Right, Tool::JobDetails(id), &ctx);
    draw(&mut app);
    let shapes = draw(&mut app);
    assert!(frame_contains(
        &shapes,
        &format!("Sources\n{}", source.display())
    ));
    assert!(frame_contains(
        &shapes,
        &format!("Destination\n{}", right.display())
    ));
}

// A one-pixel RGBA PNG, independent of the system image libraries.
const PREVIEW_TEST_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 184, 147, 34, 249, 31, 0, 5,
    210, 2, 89, 77, 189, 91, 80, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[test]
fn opening_a_folder_fills_its_disk_preview_page_without_moving_the_cursor() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let photos = root.join("photos");
    let cache = root.join("cache");
    let thumbnails = cache.join("previews-v1");
    std::fs::create_dir_all(photos.join("12800x7200")).unwrap();
    for index in 0..25 {
        std::fs::write(photos.join(format!("{index:02}.png")), PREVIEW_TEST_PNG).unwrap();
    }

    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop {
            preview_cache: Some(cache),
            ..Default::default()
        }),
        root.clone(),
        [root.clone(), root],
        true,
    );
    app.open_initial_directories(&ctx, [Some(photos.clone()), None]);
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut frame = eframe::Frame::_new_kittest();
    loop {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1220., 540.))),
                ..Default::default()
            },
            |ui| {
                app.logic(&ctx, &mut frame);
                app.ui(ui, &mut frame);
            },
        );
        output.textures_delta.clear();
        let count = std::fs::read_dir(&thumbnails).map_or(0, |entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
                .count()
        });
        if count == 25 {
            break;
        }
        assert!(Instant::now() < deadline, "wrote {count} of 25 thumbnails");
        std::thread::sleep(Duration::from_millis(5));
    }
    let tab = app.engine.tab(Side::Left);
    assert_eq!(tab.base(), photos);
    assert_eq!(tab.current().unwrap().path, photos.join("12800x7200"));
    assert!(app.interface.quick.is_none());
}

#[test]
fn quick_view_changes_between_a_large_log_image_and_unavailable_file() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let log = root.join("large.log");
    let image = root.join("pixel.png");
    let binary = root.join("binary.bin");
    std::fs::write(&log, "Live log text\n".repeat(4_000)).unwrap();
    std::fs::write(&binary, b"\0\xff").unwrap();
    std::fs::write(&image, PREVIEW_TEST_PNG).unwrap();

    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop::default()),
        root.clone(),
        [root.clone(), root],
        true,
    );
    let mut frame = eframe::Frame::_new_kittest();
    settle(&mut app);
    app.apply(Action::ToggleQuick(Side::Left), &ctx);
    let mut await_preview =
        |app: &mut App,
         path: &Path,
         ready: &dyn Fn(&App, &[egui::epaint::ClippedShape]) -> bool| {
            app.engine.dispatch(Command::Select(
                Side::Left,
                path.into(),
                engine::Select::Focus,
            ));
            assert_eq!(app.engine.tab(Side::Left).current().unwrap().path, path);
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280., 800.))),
                        ..Default::default()
                    },
                    |ui| {
                        app.logic(&ctx, &mut frame);
                        app.ui(ui, &mut frame);
                    },
                );
                output.textures_delta.clear();
                if ready(app, &output.shapes) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Quick View timed out for {}",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        };
    await_preview(&mut app, &log, &|_, shapes| {
        frame_contains(shapes, "Live log text")
    });
    await_preview(&mut app, &image, &|app, shapes| {
        frame_contains(shapes, "pixel.png")
            && app
                .previews
                .cached
                .values()
                .any(|item| matches!(item, previews::CachedPreview::Image(_, size) if size.x == size.y && size.x > 0.))
    });
    await_preview(&mut app, &binary, &|_, shapes| {
        frame_contains(shapes, "Preview unavailable for this file.")
    });
}

#[test]
fn clipboard_shortcuts_route_to_files_or_text_based_on_focus_context() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().to_path_buf();
    let first = root.join("first.txt");
    let second = root.join("second.txt");
    std::fs::write(&first, "").unwrap();
    std::fs::write(&second, "").unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let desktop = TestDesktop::default();
    let clipboard = desktop.state.clone();
    let mut app = App::new(
        &creation,
        Box::new(desktop),
        root.clone(),
        [root.clone(), root],
        true,
    );
    let route = |app: &mut App, key, modifiers| {
        let mut actions = Vec::new();
        let mut events = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: [true, false]
                    .map(|pressed| egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    })
                    .into(),
                ..Default::default()
            },
            |_| {
                app.clipboard_shortcuts(&ctx, &mut actions);
                events = ctx.input(|input| input.events.clone());
            },
        );
        output.textures_delta.clear();
        (actions, events)
    };
    let command = egui::Modifiers::COMMAND;
    let copy_path = COPY_PATH_SHORTCUT.modifiers;
    let (actions, _) = route(&mut app, Key::C, command);
    assert!(matches!(
        actions.as_slice(),
        [Action::Tool(Side::Left, Tool::CopyClipboard)]
    ));

    settle(&mut app);
    let side = Side::Left;
    app.engine
        .dispatch(Command::Select(side, first.clone(), engine::Select::Focus));
    let copy_paths = |app: &mut App| {
        let (actions, _) = route(app, Key::C, copy_path);
        assert!(matches!(
            actions.as_slice(),
            [Action::Tool(Side::Left, Tool::CopyPaths)]
        ));
        let mut actions = actions.into_iter();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {
            for action in actions.by_ref() {
                app.apply(action, &ctx);
            }
            app.publish_text_clipboard(&ctx);
        });
        output.textures_delta.clear();
        clipboard.borrow().text.clone().unwrap()
    };
    assert_eq!(copy_paths(&mut app), first.display().to_string());
    for path in [first.clone(), second.clone()] {
        app.engine
            .dispatch(Command::Select(side, path, engine::Select::Toggle));
    }
    assert_eq!(
        copy_paths(&mut app),
        format!("{}\n{}", first.display(), second.display())
    );

    clipboard.borrow_mut().text = Some("first\r\nsecond".into());
    app.open_settings(&ctx);
    for (key, modifiers, expected) in [
        (Key::C, command, egui::Event::Copy),
        (Key::C, copy_path, egui::Event::Copy),
        (Key::X, command, egui::Event::Cut),
        (Key::V, command, egui::Event::Paste("first\nsecond".into())),
    ] {
        let (actions, events) = route(&mut app, key, modifiers);
        assert!(actions.is_empty());
        assert!(events.contains(&expected), "{events:?}");
    }
}

pub(crate) fn wait_for_dialog_read(
    app: &mut App,
    ctx: &egui::Context,
    done: impl Fn(&App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(app) {
        app.poll_dialog_reads(ctx);
        assert!(Instant::now() < deadline, "dialog read timed out");
        std::thread::yield_now();
    }
}

#[test]
fn a_blocked_dialog_read_does_not_stall_frames_and_cannot_update_a_new_location() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop::default()),
        root.clone(),
        [root.clone(), root],
        true,
    );
    let mut frame = eframe::Frame::_new_kittest();
    let (release, gate) = mpsc::channel();
    let (started, starts) = mpsc::channel();
    let (applied, applications) = mpsc::channel();
    app.read_for_tab(
        Side::Left,
        move || {
            started.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(5)).unwrap();
        },
        move |_, _, ()| applied.send(()).unwrap(),
    );
    starts.recv_timeout(Duration::from_secs(5)).unwrap();
    app.engine.dispatch(Command::Navigate {
        side: Side::Left,
        path: folder.clone(),
        new_tab: false,
        background: false,
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.logic(&ctx, &mut frame);
        ctx.run_ui(Default::default(), |ui| app.ui(ui, &mut frame))
            .textures_delta
            .clear();
        if !app.engine.tab(Side::Left).busy() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(app.engine.tab(Side::Left).base(), folder);
    release.send(()).unwrap();
    loop {
        if let Some((_, apply)) = app.dialog_reads.drain().pop() {
            apply(&mut app, &ctx);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(applications.try_recv().is_err());
}

#[test]
fn review_startup_home_resolution_preserves_defaults_and_valid_cli_arguments() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let cwd = root.join("cwd");
    std::fs::create_dir(&cwd).unwrap();
    let home = root.join("home");
    let cli = root.join("arg");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&cli).unwrap();
    for (configured, argument, expected_home, expected_right) in [
        (
            root.join("missing-home"),
            Some(root.join("missing-argument")),
            &cwd,
            &cwd,
        ),
        (root.join("missing-home"), None, &cwd, &cwd),
        (home.join("..").join("home"), None, &home, &home),
        (home.clone(), Some(cli.clone()), &home, &cli),
    ] {
        let ctx = egui::Context::default();
        let creation = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(
            &creation,
            Box::new(TestDesktop::default()),
            configured,
            [cwd.clone(), cwd.clone()],
            true,
        );
        app.open_initial_directories(&ctx, [None, argument]);
        let mut frame = eframe::Frame::_new_kittest();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.logic(&ctx, &mut frame);
            if app.startup_home.is_none()
                && Side::ALL
                    .into_iter()
                    .all(|side| !app.engine.tab(side).busy())
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "startup kept home at {}",
                app.engine.home.display()
            );
            std::thread::yield_now();
        }
        assert_eq!(&app.engine.home, expected_home);
        assert_eq!(app.engine.tab(Side::Right).base(), expected_right);
        assert!(!app.engine.tab(Side::Right).can_back());
    }
}

#[test]
fn review_history_preview_keeps_the_committed_action_target_visible() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let history = root.join("history");
    let current = root.join("current");
    for dir in [&history, &current] {
        std::fs::create_dir(dir).unwrap();
    }
    std::fs::write(history.join("cached-item.txt"), "old").unwrap();
    let target = current.join("committed-item.txt");
    std::fs::write(&target, "current").unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop::default()),
        root.clone(),
        [history, root],
        true,
    );
    app.engine.settings.auto_refresh = false;
    settle(&mut app);
    let side = Side::Left;
    app.engine.dispatch(Command::Navigate {
        side,
        path: current,
        new_tab: false,
        background: false,
    });
    settle(&mut app);
    app.engine.dispatch(Command::Back(side));
    assert!(app.engine.history_preview(side).is_some());
    assert_eq!(app.engine.tab(side).operands(), [target]);
    let mut frame = eframe::Frame::_new_kittest();
    // Render before polling validation so the history request stays pending.
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1280., 800.))),
            ..Default::default()
        },
        |ui| app.ui(ui, &mut frame),
    );
    output.textures_delta.clear();
    assert!(frame_contains(&output.shapes, "cached-item.txt"));
    assert!(frame_contains(&output.shapes, "committed-item.txt"));
}
