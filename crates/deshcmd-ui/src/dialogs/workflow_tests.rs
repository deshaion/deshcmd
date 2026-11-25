use super::*;
use crate::tests::TestDesktop;

struct Session {
    _fixture: tempfile::TempDir,
    ctx: egui::Context,
    app: App,
    left: PathBuf,
    right: PathBuf,
}

impl Session {
    fn new() -> Self {
        let fixture = tempfile::tempdir().unwrap();
        let home = fixture.path().canonicalize().unwrap();
        let left = home.join("left");
        let right = home.join("right");
        std::fs::create_dir(&left).unwrap();
        std::fs::create_dir(&right).unwrap();
        std::fs::write(left.join("original.txt"), "abcdef").unwrap();
        let ctx = egui::Context::default();
        let creation = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = App::new(
            &creation,
            Box::new(TestDesktop::default()),
            home,
            [left.clone(), right.clone()],
            true,
        );
        let mut session = Self {
            _fixture: fixture,
            ctx,
            app,
            left,
            right,
        };
        session.settle();
        session.focus(&session.left.join("original.txt"));
        session
    }

    fn settle(&mut self) {
        crate::tests::settle(&mut self.app);
    }

    fn focus(&mut self, path: &Path) {
        self.app.engine.dispatch(Command::Select(
            Side::Left,
            path.into(),
            engine::Select::Focus,
        ));
        assert_eq!(
            self.app.engine.tab(Side::Left).current().unwrap().path,
            path
        );
    }

    fn dialog(&mut self, tool: Tool) -> Dialog {
        self.app.invoke(Side::Left, tool, &self.ctx);
        super::tests::draw_dialog(&mut self.app, &self.ctx, None);
        self.app.dialog.take().expect("tool should open a dialog")
    }

    fn submit(&mut self, dialog: &Dialog) {
        self.app.submit(dialog, false, &self.ctx).unwrap();
        self.settle();
        if let Some(job) = self.app.engine.jobs.last() {
            assert_eq!(
                job.state,
                engine::JobState::Done,
                "{} {:?}",
                job.message,
                job.warnings
            );
        }
    }

    fn invalid(&mut self, dialog: &Dialog, expected: &str) {
        let jobs = self.app.engine.jobs.len();
        let error = self.app.submit(dialog, false, &self.ctx).unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert_eq!(self.app.engine.jobs.len(), jobs);
    }
}

#[test]
fn create_rename_link_and_comment_dialogs_validate_and_apply() {
    let mut session = Session::new();
    let mut folder = session.dialog(Tool::NewFolder);
    folder.primary = "../escape".into();
    session.invalid(&folder, "relative path");
    folder.primary = "nested/child".into();
    session.submit(&folder);
    assert!(session.left.join("nested/child").is_dir());

    let mut file = session.dialog(Tool::NewFile);
    assert_eq!(file.primary, "untitled.txt");
    file.primary = "nested/file".into();
    session.invalid(&file, "single file");
    file.primary = "created.txt".into();
    session.submit(&file);
    let created = session.left.join("created.txt");
    std::fs::write(&created, "new content").unwrap();
    session.focus(&created);

    let mut rename = session.dialog(Tool::Rename);
    assert_eq!(rename.primary, "created.txt");
    rename.primary = "renamed.txt".into();
    session.submit(&rename);
    let renamed = session.left.join("renamed.txt");
    assert!(!created.exists());
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "new content");
    assert_eq!(
        session.app.engine.tab(Side::Left).current().unwrap().path,
        renamed
    );
    assert!(session.app.engine.take_cursor_restored(Side::Left));
    assert!(!session.app.interface.queue);

    session.app.interface.queue = true;
    let hardlink = session.dialog(Tool::Hardlink);
    session.submit(&hardlink);
    assert!(session.app.interface.queue);
    assert_eq!(
        std::fs::read(session.right.join("renamed.txt")).unwrap(),
        b"new content"
    );

    #[cfg(unix)]
    {
        let mut symlink = session.dialog(Tool::Symlink);
        assert!(symlink.flags[0]);
        symlink.primary = session.right.join("link").display().to_string();
        session.submit(&symlink);
        assert_eq!(
            std::fs::read_link(session.right.join("link")).unwrap(),
            Path::new("../left/renamed.txt")
        );
    }
    let mut comment = session.dialog(Tool::Comment);
    comment.primary = "Important fixture".into();
    session.submit(&comment);
    session.focus(&renamed);
    let inspect = session.dialog(Tool::Inspect);
    assert!(inspect.description.contains("Important fixture"));
}

#[test]
fn transfer_split_and_checksum_forms_queue_expected_work() {
    let mut session = Session::new();
    let original = session.left.join("original.txt");
    let mut copy = session.dialog(Tool::Copy);
    assert_eq!(copy.primary, session.right.display().to_string());
    copy.primary = "missing".into();
    assert!(
        session
            .app
            .probe_dialog_destination(&mut copy, false)
            .unwrap()
    );
    session.app.dialog = Some(copy);
    crate::tests::wait_for_dialog_read(&mut session.app, &session.ctx, |app| {
        app.dialog.as_ref().is_some_and(|dialog| {
            dialog
                .error
                .as_ref()
                .is_some_and(|error| error.contains("existing destination"))
        })
    });
    let mut copy = session.app.dialog.take().unwrap();
    assert!(session.app.engine.jobs.is_empty());
    copy.primary = "~/right".into();
    session.submit(&copy);
    assert_eq!(
        std::fs::read(session.right.join("original.txt")).unwrap(),
        b"abcdef"
    );

    session.focus(&original);
    let mut split = session.dialog(Tool::Split);
    split.numeric = "0".into();
    session.invalid(&split, "positive");
    split.numeric = "2".into();
    split.flags[0] = true;
    split.primary = "~".into();
    session.submit(&split);
    assert_eq!(
        std::fs::read(session.app.engine.home.join("original.txt.001")).unwrap(),
        b"abc"
    );

    session.focus(&original);
    let mut verify = session.dialog(Tool::Verify);
    session.invalid(&verify, "expected checksum");
    verify.algorithm = Algorithm::Md5;
    verify.primary = " e80b5017098950fc58aad83c8c14978e ".into();
    session.submit(&verify);
    assert!(
        session
            .app
            .engine
            .jobs
            .last()
            .unwrap()
            .message
            .contains("Verified original.txt")
    );

    session.focus(&original);
    let mut hash = session.dialog(Tool::Hash);
    hash.manifest = ManifestMode::PerFile;
    hash.algorithm = Algorithm::Md5;
    session.submit(&hash);
    let manifest = session.left.join("original.txt.md5");
    assert_eq!(
        std::fs::read_to_string(&manifest).unwrap(),
        "e80b5017098950fc58aad83c8c14978e  original.txt\n"
    );
    session.focus(&manifest);
    let verify_manifest = session.dialog(Tool::Verify);
    assert!(verify_manifest.flags[0]);
    assert_eq!(verify_manifest.algorithm, Algorithm::Md5);
    session.submit(&verify_manifest);
}

#[test]
fn criteria_bookmarks_attributes_and_wipe_dialogs_report_invalid_input() {
    let mut session = Session::new();
    let original = session.left.join("original.txt");
    let mut filter = session.dialog(Tool::FilterOptions);
    filter.criteria = engine::MatchOptions::filename("[");
    session.invalid(&filter, "glob");
    filter.criteria = engine::MatchOptions::filename("*.txt");
    session.submit(&filter);
    assert_eq!(session.app.engine.tab(Side::Left).filter().text, "*.txt");

    let mut mark = session.dialog(Tool::MarkMask);
    mark.criteria = engine::MatchOptions::filename("orig*");
    session.submit(&mark);
    assert_eq!(session.app.engine.tab(Side::Left).marked_count(), 1);
    let mut unmark = session.dialog(Tool::UnmarkMask);
    unmark.criteria = mark.criteria;
    session.submit(&unmark);
    assert_eq!(session.app.engine.tab(Side::Left).marked_count(), 0);

    let mut bookmark = session.dialog(Tool::Bookmark);
    bookmark.primary.clear();
    session.invalid(&bookmark, "bookmark name");
    bookmark.primary = "Working pair".into();
    bookmark.flags[0] = true;
    bookmark.description = "~/right".into();
    session.submit(&bookmark);
    assert_eq!(
        session.app.engine.settings.bookmarks[0].opposite.as_ref(),
        Some(&session.right)
    );

    let mut edit = session.dialog(Tool::EditBookmark(0));
    assert_eq!(edit.primary, "Working pair");
    edit.group = "  Tests  ".into();
    session.submit(&edit);
    assert_eq!(session.app.engine.settings.bookmarks[0].group, "Tests");

    session.focus(&original);
    let mut attributes = session.dialog(Tool::ChangeAttributes);
    session.invalid(&attributes, "attribute or timestamp");
    attributes.flags[2] = true;
    attributes.primary = "not a date".into();
    session.invalid(&attributes, "valid local date");
    attributes.primary = "2024-07-01 12:30:10".into();
    session.submit(&attributes);

    session.focus(&original);
    let mut wipe = session.dialog(Tool::Wipe);
    wipe.numeric = "17".into();
    session.invalid(&wipe, "1 to 16");
    assert_eq!(std::fs::read(original).unwrap(), b"abcdef");
}

#[test]
fn file_clipboard_distinguishes_local_cut_from_external_copies_and_text() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let left = root.join("left");
    let right = root.join("right");
    std::fs::create_dir(&left).unwrap();
    std::fs::create_dir(&right).unwrap();
    let source = left.join("sample.txt");
    std::fs::write(&source, b"clipboard fixture").unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let desktop = TestDesktop::default();
    let native = desktop.state.clone();
    let mut app = App::new(
        &creation,
        Box::new(desktop),
        root,
        [left.clone(), right.clone()],
        true,
    );
    crate::tests::settle(&mut app);
    app.engine.dispatch(Command::Select(
        Side::Left,
        source.clone(),
        engine::Select::Focus,
    ));
    app.invoke(Side::Left, Tool::OpenTerminal, &ctx);
    assert_eq!(native.borrow().terminal.as_ref(), Some(&left));

    app.invoke(Side::Left, Tool::CutClipboard, &ctx);
    assert_eq!(
        native.borrow().files.as_slice(),
        std::slice::from_ref(&source)
    );
    app.invoke(Side::Right, Tool::Paste, &ctx);
    let transfer = app.dialog.take().unwrap();
    assert!(matches!(transfer.tool, Tool::Move));
    assert_eq!(transfer.paths, std::slice::from_ref(&source));
    assert_eq!(transfer.primary, right.display().to_string());

    let external = left.join("external.txt");
    native.borrow_mut().files = vec![external.clone()];
    app.invoke(Side::Right, Tool::Paste, &ctx);
    let transfer = app.dialog.take().unwrap();
    assert!(matches!(transfer.tool, Tool::Copy));
    assert_eq!(transfer.paths, [external]);

    native.borrow_mut().files.clear();
    native.borrow_mut().file_clipboard_unavailable = true;
    app.invoke(Side::Left, Tool::CutClipboard, &ctx);
    app.invoke(Side::Right, Tool::Paste, &ctx);
    assert!(matches!(app.dialog.take().unwrap().tool, Tool::Move));

    native.borrow_mut().text = Some("unrelated text".into());
    app.invoke(Side::Right, Tool::Paste, &ctx);
    assert!(app.dialog.is_none());
    assert_eq!(
        app.interface.notice.as_ref().unwrap().0,
        "Clipboard does not contain files"
    );

    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {
        app.invoke(Side::Left, Tool::CopyNames, &ctx);
        app.publish_text_clipboard(&ctx);
    });
    output.textures_delta.clear();
    assert_eq!(native.borrow().text.as_deref(), Some("sample.txt"));
    assert!(app.interface.clipboard.is_none());
}

#[test]
fn dropping_into_an_archive_offers_pack_and_rejects_moving() {
    let mut session = Session::new();
    let source = session.left.join("original.txt");
    let archive = session.right.join("target.zip");
    std::fs::write(&archive, "archive path fixture").unwrap();
    let internal = archive.join("folder");
    for moving in [true, false] {
        session.app.apply(
            Action::Drop {
                side: Side::Left,
                paths: vec![source.clone()],
                destination: internal.clone(),
                moving,
            },
            &session.ctx,
        );
        if moving {
            assert!(session.app.dialog.is_none());
            assert_eq!(
                session.app.interface.notice.as_ref().unwrap().0,
                "Use Copy to add files to an archive"
            );
        } else {
            let dialog = session.app.dialog.take().unwrap();
            assert!(matches!(dialog.kind, Kind::Pack));
            assert_eq!(
                dialog.archive_target,
                Some((archive.clone(), "folder".into()))
            );
            assert!(dialog.pack.update);
            assert_eq!(dialog.paths, std::slice::from_ref(&source));
        }
    }
    session.app.apply(
        Action::Drop {
            side: Side::Right,
            paths: vec![source],
            destination: session.right.clone(),
            moving: false,
        },
        &session.ctx,
    );
    let dialog = session.app.dialog.take().unwrap();
    assert!(matches!(dialog.kind, Kind::Transfer));
    assert_eq!(dialog.primary, session.right.display().to_string());
    assert!(session.app.engine.jobs.is_empty());
}

#[test]
fn named_workspaces_and_numbered_parts_use_dialog_defaults() {
    let mut session = Session::new();
    let first = session.left.join("parts.001");
    std::fs::write(&first, "first").unwrap();
    std::fs::write(session.left.join("parts.002"), "second").unwrap();
    session.app.engine.dispatch(Command::Refresh);
    session.settle();
    session.focus(&first);
    session.app.invoke(Side::Left, Tool::Combine, &session.ctx);
    crate::tests::wait_for_dialog_read(&mut session.app, &session.ctx, |app| app.dialog.is_some());
    let combine = session.app.dialog.take().unwrap();
    assert_eq!(combine.paths, [first, session.left.join("parts.002")]);
    session.submit(&combine);
    assert_eq!(
        std::fs::read(session.right.join("parts.joined")).unwrap(),
        b"firstsecond"
    );

    let mut rename_tab = session.dialog(Tool::RenameTab(0));
    rename_tab.primary = "Work".into();
    session.submit(&rename_tab);
    assert_eq!(session.app.engine.tab(Side::Left).title(), "Work");
    let mut save_favorite = session.dialog(Tool::SaveFavorite);
    session.invalid(&save_favorite, "workspace");
    save_favorite.primary = "  My workspace  ".into();
    session.submit(&save_favorite);
    assert_eq!(
        session.app.engine.settings.favorites[0].name,
        "My workspace"
    );
    assert!(matches!(
        session.dialog(Tool::Favorites).kind,
        Kind::Favorites
    ));
}
