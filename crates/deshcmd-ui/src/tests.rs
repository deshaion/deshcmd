use super::*;
use eframe::App as _;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Default)]
pub(super) struct DesktopState {
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
    pub(super) state: Rc<RefCell<DesktopState>>,
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
    wait_for_listing(&mut app);
    assert_eq!(app.engine.tab(side).base(), parent);
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
    app.interface.queue = false;
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

fn frame_contains(shapes: &[egui::epaint::ClippedShape], expected: &str) -> bool {
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
    let shapes = draw(&mut app);
    assert!(frame_contains(&shapes, "Fixture place"));
    assert!(frame_contains(&shapes, "Fixture drive"));
    assert!(frame_contains(&shapes, "directory"));
    assert!(frame_contains(&shapes, "shared.txt"));
    app.interface.show_help = true;
    draw(&mut app);
    assert!(frame_contains(&draw(&mut app), "Keyboard shortcuts"));
    app.interface.show_help = false;

    let operation = engine::Operation::Transfer {
        paths: vec![source],
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
    assert!(frame_contains(&draw(&mut app), "1 skipped"));
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
    // A one-pixel RGBA PNG, independent of the system image libraries.
    const PIXEL: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 184, 147, 34, 249,
        31, 0, 5, 210, 2, 89, 77, 189, 91, 80, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    std::fs::write(&image, PIXEL).unwrap();

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
                .any(|item| matches!(item, CachedPreview::Image(_, size) if size.x == size.y && size.x > 0.))
    });
    await_preview(&mut app, &binary, &|_, shapes| {
        frame_contains(shapes, "Preview unavailable for this file.")
    });
}

#[test]
fn clipboard_shortcuts_route_to_files_or_text_based_on_focus_context() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().to_path_buf();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let desktop = TestDesktop::default();
    desktop.state.borrow_mut().text = Some("first\r\nsecond".into());
    let mut app = App::new(
        &creation,
        Box::new(desktop),
        root.clone(),
        [root.clone(), root],
        true,
    );
    let route = |app: &mut App, key| {
        let modifiers = egui::Modifiers::COMMAND;
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
    let (actions, _) = route(&mut app, Key::C);
    assert!(matches!(
        actions.as_slice(),
        [Action::Tool(Side::Left, Tool::CopyClipboard)]
    ));

    app.open_settings(&ctx);
    for (key, expected) in [
        (Key::C, egui::Event::Copy),
        (Key::X, egui::Event::Cut),
        (Key::V, egui::Event::Paste("first\nsecond".into())),
    ] {
        let (actions, events) = route(&mut app, key);
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
