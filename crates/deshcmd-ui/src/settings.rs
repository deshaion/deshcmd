use deshcmd_engine::{Column, Command, Settings, Side};
use eframe::egui::{self, Align, Id, Layout, Ui, gui_zoom::kb_shortcuts, vec2};

use crate::{App, theme::*};

const MIN_ZOOM: u32 = 75;
const MAX_ZOOM: u32 = 125;
const ZOOM_STEP: u32 = 5;

fn zoom_percent(factor: f32) -> u32 {
    let percent = (factor * 100.).clamp(MIN_ZOOM as f32, MAX_ZOOM as f32);
    ((percent / ZOOM_STEP as f32).round() as u32 * ZOOM_STEP).max(MIN_ZOOM)
}

fn zoom_factor(percent: u32) -> f32 {
    percent as f32 / 100.
}

pub(crate) fn install_zoom(ctx: &egui::Context) {
    ctx.options_mut(|options| {
        options.zoom_factor = zoom_factor(zoom_percent(options.zoom_factor));
        options.zoom_with_keyboard = false;
    });
}

pub(crate) fn zoom_shortcuts(ctx: &egui::Context) {
    let consume = |shortcut| ctx.input_mut(|input| input.consume_shortcut(shortcut));
    let current = zoom_percent(ctx.zoom_factor());
    let next = if consume(&kb_shortcuts::ZOOM_RESET) {
        100
    } else if consume(&kb_shortcuts::ZOOM_IN) || consume(&kb_shortcuts::ZOOM_IN_SECONDARY) {
        (current + ZOOM_STEP).min(MAX_ZOOM)
    } else if consume(&kb_shortcuts::ZOOM_OUT) {
        (current - ZOOM_STEP).max(MIN_ZOOM)
    } else {
        return;
    };
    ctx.set_zoom_factor(zoom_factor(next));
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Interface,
    Panels,
    Navigation,
    Confirmations,
    Visibility,
    Sorting,
    Columns,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Self::Interface => "Interface",
            Self::Panels => "Panels",
            Self::Navigation => "Navigation",
            Self::Confirmations => "Confirmations",
            Self::Visibility => "Visibility",
            Self::Sorting => "Sorting",
            Self::Columns => "Columns",
        }
    }
}

pub(crate) struct Preferences {
    workspace: Settings,
    directory_tree: bool,
    show_inline_filter: bool,
    show_places: bool,
    show_bottom_bar: bool,
    ui_font: UiFont,
    alternate_columns: [bool; 2],
    zoom: u32,
    page: Page,
    error: Option<String>,
}

impl Preferences {
    fn tree(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 3.;
        ui.spacing_mut().interact_size.y = 21.;
        for (group, pages) in [
            ("Appearance", &[Page::Interface, Page::Panels][..]),
            ("Workspace", &[Page::Navigation][..]),
            ("File operations", &[Page::Confirmations][..]),
            (
                "File lists",
                &[Page::Visibility, Page::Sorting, Page::Columns][..],
            ),
        ] {
            egui::CollapsingHeader::new(group)
                .default_open(true)
                .show(ui, |ui| {
                    for &page in pages {
                        if ui
                            .selectable_label(self.page == page, page.title())
                            .clicked()
                        {
                            self.page = page;
                        }
                    }
                });
        }
    }

    fn page(&mut self, ui: &mut Ui) {
        ui.heading(self.page.title());
        match self.page {
            Page::Interface => {
                ui.label("Interface zoom");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Slider::new(&mut self.zoom, MIN_ZOOM..=MAX_ZOOM)
                            .step_by(f64::from(ZOOM_STEP))
                            .suffix("%"),
                    );
                    if ui
                        .button("Reset")
                        .on_hover_text("Reset zoom to 100%")
                        .clicked()
                    {
                        self.zoom = 100;
                    }
                });
                ui.label("Interface font");
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.ui_font, UiFont::UbuntuLight, "Ubuntu Light");
                    ui.selectable_value(
                        &mut self.ui_font,
                        UiFont::DejaVuSansMono,
                        "DejaVu Sans Mono",
                    );
                });
            }
            Page::Panels => {
                ui.label("Panel orientation");
                ui.radio_value(&mut self.workspace.stacked, false, "Side by side");
                ui.radio_value(&mut self.workspace.stacked, true, "Stacked")
                    .on_hover_text(
                        "Short windows use side-by-side panels when stacking cannot fit.",
                    );
                let mut split = self.workspace.split * 100.;
                if ui
                    .add(
                        egui::Slider::new(&mut split, 25.0..=75.0)
                            .fixed_decimals(0)
                            .suffix("%")
                            .text("First panel size"),
                    )
                    .changed()
                {
                    self.workspace.split = split / 100.;
                }
                ui.checkbox(&mut self.directory_tree, "Show the directory tree");
                ui.checkbox(&mut self.show_inline_filter, "Show the inline filter");
                ui.checkbox(&mut self.show_places, "Show the Places sidebar");
                ui.checkbox(&mut self.show_bottom_bar, "Show the bottom bar");
            }
            Page::Navigation => {
                ui.checkbox(&mut self.workspace.linked, "Synchronize navigation");
                ui.label(label(
                    "Navigate matching subfolders in both panels.",
                    12.,
                    SUB,
                ));
                ui.checkbox(
                    &mut self.workspace.auto_refresh,
                    "Refresh active folders automatically",
                );
                ui.checkbox(
                    &mut self.workspace.double_click_new_tab,
                    "Double-click empty tab bar space to open a new tab",
                );
            }
            Page::Confirmations => {
                ui.checkbox(
                    &mut self.workspace.confirm_trash,
                    "Confirm before moving items to Trash",
                );
                ui.checkbox(
                    &mut self.workspace.confirm_delete,
                    "Confirm before permanently deleting items",
                );
            }
            Page::Visibility => {
                ui.checkbox(&mut self.workspace.show_hidden, "Show hidden files");
                ui.label("Ignore masks");
                if ui
                    .text_edit_singleline(&mut self.workspace.ignore)
                    .changed()
                {
                    self.error = None;
                }
            }
            Page::Sorting => {
                ui.checkbox(
                    &mut self.workspace.natural_sort,
                    "Sort numbers in filenames naturally",
                );
                ui.checkbox(
                    &mut self.workspace.directories_first,
                    "List folders before files",
                );
            }
            Page::Columns => {
                ui.checkbox(
                    &mut self.workspace.show_modified_time,
                    "Show time in the Modified column",
                );
                ui.separator();
                for (title, columns) in [
                    ("Default columns", &mut self.workspace.columns),
                    ("Alternate columns", &mut self.workspace.alternate_columns),
                ] {
                    ui.label(label(title, 13., TEXT).strong());
                    ui.horizontal_wrapped(|ui| {
                        for column in Column::ALL.into_iter().skip(1) {
                            let mut enabled = columns.contains(&column);
                            if ui.checkbox(&mut enabled, column.label()).changed() {
                                if enabled {
                                    columns.push(column);
                                } else {
                                    columns.retain(|value| *value != column);
                                }
                            }
                        }
                    });
                }
                ui.separator();
                for side in Side::ALL {
                    ui.checkbox(
                        &mut self.alternate_columns[side.index()],
                        format!(
                            "Use alternate columns in the {} panel",
                            side.label().to_lowercase()
                        ),
                    );
                }
            }
        }
    }
}

impl App {
    pub(crate) fn open_settings(&mut self, ctx: &egui::Context) {
        self.preferences = Some(Preferences {
            workspace: self.engine.settings.clone(),
            directory_tree: self.interface.tree,
            show_inline_filter: self.interface.show_inline_filter,
            show_places: self.interface.show_places,
            show_bottom_bar: self.interface.show_bottom_bar,
            ui_font: self.interface.ui_font,
            alternate_columns: self.interface.alternate_columns,
            zoom: zoom_percent(ctx.zoom_factor()),
            page: Page::Interface,
            error: None,
        });
    }

    pub(crate) fn show_settings(&mut self, ctx: &egui::Context) {
        let Some(mut preferences) = self.preferences.take() else {
            return;
        };
        let viewport = ctx.content_rect().size();
        let width = (viewport.x - 36.).min(620.);
        let height = (viewport.y - 154.).min(330.);
        let tree_width = if self.interface.ui_font == UiFont::DejaVuSansMono {
            168.
        } else {
            146.
        };
        let mut cancel = false;
        let mut save = false;

        let modal = egui::Modal::new(Id::unique("deshcmd-settings")).show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(label("Settings", 21., TEXT).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    cancel = icon_button(ui, Icon::Close, "Close Settings", true).clicked();
                });
            });
            ui.separator();
            ui.allocate_ui_with_layout(
                vec2(width, height),
                Layout::left_to_right(Align::Min),
                |ui| {
                    ui.set_height(height);
                    ui.allocate_ui_with_layout(
                        vec2(tree_width, height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(tree_width);
                            egui::ScrollArea::vertical()
                                .id_salt("settings-tree")
                                .max_height(height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| preferences.tree(ui));
                        },
                    );
                    ui.separator();
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt(("settings-page", preferences.page.title()))
                                .max_height(height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| preferences.page(ui));
                        },
                    );
                },
            );
            if let Some(error) = &preferences.error {
                ui.label(label(error, 12., RED));
            }
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                save = ui
                    .add(egui::Button::new(label("Save", 13., BASE).strong()).fill(ACCENT))
                    .clicked();
                cancel |= ui.button("Cancel").clicked();
            });
        });
        if save {
            match self.save_settings(&preferences, ctx) {
                Ok(()) => return,
                Err(error) => {
                    preferences.page = Page::Visibility;
                    preferences.error = Some(error.to_string());
                }
            }
        }
        if !cancel && !modal.should_close() {
            self.preferences = Some(preferences);
        }
    }

    fn save_settings(
        &mut self,
        preferences: &Preferences,
        ctx: &egui::Context,
    ) -> std::io::Result<()> {
        let draft = &preferences.workspace;
        if self.engine.settings.ignore != draft.ignore {
            self.engine.set_ignore(draft.ignore.clone())?;
        }

        let settings = &mut self.engine.settings;
        let toggle_hidden = settings.show_hidden != draft.show_hidden;
        settings.stacked = draft.stacked;
        settings.split = draft.split;
        settings.linked = draft.linked;
        settings.auto_refresh = draft.auto_refresh;
        settings.double_click_new_tab = draft.double_click_new_tab;
        settings.confirm_trash = draft.confirm_trash;
        settings.confirm_delete = draft.confirm_delete;
        settings.show_modified_time = draft.show_modified_time;
        settings.columns.clone_from(&draft.columns);
        settings
            .alternate_columns
            .clone_from(&draft.alternate_columns);
        self.interface.tree = preferences.directory_tree;
        self.interface.show_inline_filter = preferences.show_inline_filter;
        self.interface.show_places = preferences.show_places;
        self.interface.show_bottom_bar = preferences.show_bottom_bar;
        self.interface.alternate_columns = preferences.alternate_columns;
        ctx.set_zoom_factor(zoom_factor(preferences.zoom));
        if self.interface.ui_font != preferences.ui_font {
            self.interface.ui_font = preferences.ui_font;
            set_ui_font(ctx, preferences.ui_font);
        }

        if toggle_hidden {
            self.engine.dispatch(Command::ToggleHidden);
        }
        self.engine
            .set_sort_preferences(draft.natural_sort, draft.directories_first);
        ctx.request_repaint();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TestDesktop;

    #[test]
    fn deletion_confirmations_default_on_and_can_be_saved_independently() {
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
        let older_settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(older_settings.confirm_trash && older_settings.confirm_delete);
        app.engine.settings.confirm_delete = false;
        app.open_settings(&ctx);
        let mut draft = app.preferences.take().unwrap();
        draft.workspace.confirm_trash = false;
        draft.workspace.confirm_delete = true;
        app.save_settings(&draft, &ctx).unwrap();

        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&app.engine.snapshot()).unwrap()).unwrap();
        assert!(!restored.confirm_trash && restored.confirm_delete);
    }

    #[test]
    fn modified_time_preference_is_enabled_by_default_and_can_be_saved_off() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_path_buf();
        let ctx = egui::Context::default();
        let creation = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(
            &creation,
            Box::new(TestDesktop::default()),
            path.clone(),
            [path.clone(), path],
            true,
        );

        let older_settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(older_settings.show_modified_time);
        app.open_settings(&ctx);
        let mut draft = app.preferences.take().unwrap();
        assert!(draft.workspace.show_modified_time);
        draft.workspace.show_modified_time = false;
        app.save_settings(&draft, &ctx).unwrap();

        let saved = serde_json::to_string(&app.engine.snapshot()).unwrap();
        let restored: Settings = serde_json::from_str(&saved).unwrap();
        assert!(!restored.show_modified_time);
    }
}

#[cfg(test)]
mod view_tests;
