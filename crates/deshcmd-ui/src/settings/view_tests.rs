use super::*;
use crate::tests::TestDesktop;

#[test]
fn settings_pages_render_and_saving_applies_display_and_visibility_changes() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().canonicalize().unwrap();
    std::fs::write(root.join("visible.txt"), "sample").unwrap();
    std::fs::write(root.join(".hidden.txt"), "sample").unwrap();
    std::fs::write(root.join("ignore.tmp"), "sample").unwrap();
    let ctx = egui::Context::default();
    let creation = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = App::new(
        &creation,
        Box::new(TestDesktop::default()),
        root.clone(),
        [root.clone(), root],
        true,
    );
    crate::tests::settle(&mut app);
    app.open_settings(&ctx);
    for page in [
        Page::Interface,
        Page::Panels,
        Page::Navigation,
        Page::Confirmations,
        Page::Visibility,
        Page::Sorting,
        Page::Columns,
    ] {
        app.preferences.as_mut().unwrap().page = page;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(1200., 800.),
                )),
                ..Default::default()
            },
            |_| app.show_settings(&ctx),
        );
        output.textures_delta.clear();
        assert!(!output.shapes.is_empty(), "{}", page.title());
        assert!(
            app.preferences
                .as_ref()
                .is_some_and(|preferences| preferences.page == page)
        );
    }
    let mut draft = app.preferences.take().unwrap();
    draft.workspace.ignore = "[".into();
    assert!(app.save_settings(&draft, &ctx).is_err());
    assert_eq!(app.engine.settings.ignore, "");
    assert!(!app.engine.settings.show_hidden);

    draft.workspace.ignore = "*.tmp".into();
    draft.workspace.show_hidden = true;
    draft.workspace.natural_sort = false;
    draft.workspace.stacked = true;
    draft.workspace.split = 0.6;
    draft.directory_tree = true;
    draft.show_places = true;
    draft.show_inline_filter = true;
    draft.show_bottom_bar = false;
    draft.alternate_columns = [true, false];
    draft.ui_font = UiFont::DejaVuSansMono;
    draft.zoom = 115;
    app.save_settings(&draft, &ctx).unwrap();
    crate::tests::settle(&mut app);
    assert!(app.engine.settings.stacked);
    assert_eq!(app.engine.settings.split, 0.6);
    assert!(app.interface.tree && app.interface.show_places && app.interface.show_inline_filter);
    assert!(!app.interface.show_bottom_bar);
    assert_eq!(app.interface.alternate_columns, [true, false]);
    assert!(app.interface.ui_font == UiFont::DejaVuSansMono);
    assert_eq!(
        app.engine
            .tab(Side::Left)
            .visible_entries()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        [".hidden.txt", "visible.txt"]
    );

    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
    output.textures_delta.clear();
    assert_eq!(zoom_percent(ctx.zoom_factor()), 115);
}

#[test]
fn zoom_settings_round_to_steps_and_clamp_to_supported_limits() {
    assert_eq!(zoom_percent(0.1), MIN_ZOOM);
    assert_eq!(zoom_percent(1.08), 110);
    assert_eq!(zoom_percent(2.), MAX_ZOOM);
}
