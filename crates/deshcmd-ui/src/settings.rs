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
            Self::Visibility => "Visibility",
            Self::Sorting => "Sorting",
            Self::Columns => "Columns",
        }
    }
}

pub(crate) struct Preferences {
    workspace: Settings,
    directory_tree: bool,
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
                        vec2(146., height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(146.);
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
        let refresh = settings.directories_first != draft.directories_first
            || settings.natural_sort != draft.natural_sort;
        let toggle_hidden = settings.show_hidden != draft.show_hidden;
        settings.stacked = draft.stacked;
        settings.split = draft.split;
        settings.linked = draft.linked;
        settings.auto_refresh = draft.auto_refresh;
        settings.natural_sort = draft.natural_sort;
        settings.directories_first = draft.directories_first;
        settings.columns.clone_from(&draft.columns);
        settings
            .alternate_columns
            .clone_from(&draft.alternate_columns);
        self.interface.tree = preferences.directory_tree;
        self.interface.alternate_columns = preferences.alternate_columns;
        ctx.set_zoom_factor(zoom_factor(preferences.zoom));

        if toggle_hidden {
            self.engine.dispatch(Command::ToggleHidden);
        }
        if refresh {
            self.engine.dispatch(Command::Refresh);
        }
        Ok(())
    }
}
