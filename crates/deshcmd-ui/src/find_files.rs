use std::path::PathBuf;
use std::sync::Arc;

use deshcmd_engine::read_pool::ReadPool;
use deshcmd_engine::search::{self, Attributes, Hit, MatchMode, Options, Results};
use deshcmd_engine::{Command, Location, Side};
use eframe::egui::{self, Id, TextEdit, Ui};
use serde::{Deserialize, Serialize};

use crate::{App, dialogs::absolute_input, theme::*};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Standard,
    Advanced,
    Templates,
    Results,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Scope {
    #[default]
    Locations,
    Selected,
    Tabs,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Template {
    name: String,
    options: Options,
}

struct SearchRun {
    title: String,
    result: Result<Results, String>,
    options: Options,
}

#[derive(Default)]
pub(crate) struct FindFiles {
    pub open: bool,
    side: Side,
    scope: Scope,
    page: Page,
    locations: String,
    initial_location: String,
    initial_roots: Vec<Hit>,
    options: Options,
    pool: Option<ReadPool<(), SearchRun>>,
    running: bool,
    history: Vec<SearchRun>,
    current: usize,
    next_run: u64,
    selected: Option<usize>,
    error: String,
    template_name: String,
    pub templates: Vec<Template>,
    focus_name: bool,
    confirm_replace: bool,
}

enum Action {
    Start,
    Cancel,
    ChooseDirectory,
    Open(Hit),
    Reveal(Hit, bool),
    Preview(Hit),
    Replace(Vec<search::Replacement>, Box<Options>),
    Panel(Vec<PathBuf>),
}

fn roots(location: &Location) -> Vec<Hit> {
    match location {
        Location::Directory(path) => vec![Hit::local(path.clone())],
        Location::Archive { file, folder } => vec![Hit {
            path: file.clone(),
            member: Some(folder.clone()),
        }],
        Location::Flat { roots, .. } => roots.iter().cloned().map(Hit::local).collect(),
        Location::List { paths, .. } => paths.iter().cloned().map(Hit::local).collect(),
    }
}

impl App {
    pub(crate) fn open_find_files(&mut self, side: Side) {
        let location = self.engine.tab(side).location();
        let find = &mut self.find_files;
        find.open = true;
        find.side = side;
        find.page = Page::Standard;
        find.scope = Scope::Locations;
        find.locations = location.display_path().display().to_string();
        find.initial_location.clone_from(&find.locations);
        find.initial_roots = roots(location);
        find.error.clear();
        find.confirm_replace = false;
        find.focus_name = true;
    }

    pub(crate) fn show_find_files(&mut self, ctx: &egui::Context) {
        let find = &mut self.find_files;
        if !find.open {
            return;
        }
        if let Some(pool) = &find.pool {
            for (_, result) in pool.drain() {
                find.accept_result(result);
            }
        }
        let viewport = ctx.content_rect();
        let width = (viewport.width() - 55.).clamp(200., 720.);
        let height = (viewport.height() - 158.).clamp(145., 425.);
        let mut action = None;
        let response = egui::Modal::new(Id::unique("find-files")).show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(label("Find Files", 22., TEXT).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, Icon::Close, "Close Find Files", true).clicked() {
                        find.open = false;
                    }
                });
            });
            ui.horizontal(|ui| {
                for (page, text) in [
                    (Page::Standard, "Standard"),
                    (Page::Advanced, "Advanced"),
                    (Page::Templates, "Load/Save"),
                    (Page::Results, "Results"),
                ] {
                    if ui.selectable_label(find.page == page, text).clicked() {
                        find.page = page;
                    }
                }
            });
            ui.separator();
            if find.page == Page::Results {
                find.results(ui, height, &mut action);
            } else {
                egui::ScrollArea::vertical()
                    .min_scrolled_height(height)
                    .max_height(height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| match find.page {
                        Page::Standard => find.standard(ui, &mut action),
                        Page::Advanced => find.advanced(ui),
                        Page::Templates => find.templates(ui),
                        Page::Results => unreachable!(),
                    });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if find.running {
                    if ui.button("Cancel Search").clicked() {
                        action = Some(Action::Cancel);
                    }
                    ui.spinner();
                    ui.label("Searching…");
                } else if ui.button("Start Search").clicked()
                    || ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::CTRL, egui::Key::Enter)
                    })
                {
                    action = Some(Action::Start);
                }
                if ui
                    .add_enabled(!find.running, egui::Button::new("New Search"))
                    .clicked()
                {
                    find.options = Options::default();
                    find.scope = Scope::Locations;
                    find.locations.clone_from(&find.initial_location);
                    find.page = Page::Standard;
                    find.error.clear();
                    find.focus_name = true;
                }
                if !find.error.is_empty() {
                    ui.add(egui::Label::new(label(&find.error, 12., RED)).truncate())
                        .on_hover_text(&find.error);
                }
            });
        });
        if response.should_close() {
            self.find_files.open = false;
        }
        let side = self.find_files.side;
        match action {
            Some(Action::Start) => {
                if let Err(error) = self.start_file_search(ctx) {
                    self.find_files.error = error;
                }
            }
            Some(Action::Cancel) => self.find_files.cancel(),
            Some(Action::ChooseDirectory) => {
                if let Some(path) = self
                    .desktop
                    .pick_directory(self.engine.tab(side).base(), "Add search location")
                {
                    let find = &mut self.find_files;
                    if !find.locations.is_empty() && find.locations != find.initial_location {
                        find.locations.push('\n');
                    } else {
                        find.locations.clear();
                    }
                    find.locations.push_str(&path.display().to_string());
                }
            }
            Some(Action::Open(hit)) => {
                if let Err(error) = self.desktop.open(&hit.path) {
                    self.find_files.error = error;
                }
            }
            Some(Action::Preview(hit)) => {
                self.find_files.open = false;
                self.interface.quick = None;
                self.engine.dispatch(Command::RevealPreview(side, hit));
            }
            Some(Action::Replace(files, options)) => {
                self.find_files.open = false;
                self.engine.dispatch(Command::QueueFor(
                    side,
                    deshcmd_engine::Operation::ReplaceContent { files, options },
                    false,
                ));
            }
            Some(Action::Reveal(hit, background)) => {
                if !background {
                    self.find_files.open = false;
                }
                self.engine.dispatch(Command::Reveal {
                    side,
                    hit,
                    background,
                });
            }
            Some(Action::Panel(paths)) => {
                self.find_files.open = false;
                self.engine.dispatch(Command::LoadPaths(side, paths));
            }
            None => {}
        }
        if !self.find_files.open {
            self.find_files.cancel();
        }
    }

    fn start_file_search(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let find = &mut self.find_files;
        let side = find.side;
        let tab = self.engine.tab(side);
        let mut locations: Vec<Hit> = match find.scope {
            Scope::Locations if find.locations == find.initial_location => {
                find.initial_roots.clone()
            }
            Scope::Locations => find
                .locations
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| absolute_input(tab.base(), &self.engine.home, line).map(Hit::local))
                .collect::<Result<_, _>>()?,
            Scope::Selected => {
                if let Some(archive) = tab.archive_selection() {
                    archive
                        .members
                        .into_iter()
                        .map(|member| Hit {
                            path: archive.file.clone(),
                            member: Some(member),
                        })
                        .collect()
                } else {
                    tab.operands().into_iter().map(Hit::local).collect()
                }
            }
            Scope::Tabs => Side::ALL
                .into_iter()
                .flat_map(|side| &self.engine.pane(side).tabs)
                .flat_map(|tab| roots(tab.location()))
                .collect(),
        };
        let mut seen = std::collections::HashSet::new();
        locations.retain(|root| seen.insert(root.clone()));
        if locations.is_empty() {
            return Err("Choose at least one search location or item".into());
        }
        search::validate(&find.options).map_err(|error| error.to_string())?;
        let options = find.options.clone();
        find.next_run += 1;
        let title = format!(
            "{} · {}",
            find.next_run,
            if !options.name.is_empty() {
                options.name.clone()
            } else if !options.text.is_empty() {
                options.text.clone()
            } else {
                "All files".into()
            }
        );
        let archives = self.engine.search_archives();
        let pool = find.pool.get_or_insert_with(|| {
            let ctx = ctx.clone();
            ReadPool::new(1, 1, Arc::new(move || ctx.request_repaint()))
        });
        pool.submit((), move |cancel| SearchRun {
            title,
            result: search::run_with_archives(&locations, &options, &archives, cancel)
                .map_err(|error| error.to_string()),
            options,
        });
        find.running = true;
        find.confirm_replace = false;
        find.page = Page::Results;
        find.error.clear();
        Ok(())
    }
}

impl FindFiles {
    fn accept_result(&mut self, result: SearchRun) {
        self.running = false;
        self.history.push(result);
        if self.history.len() > 12 {
            self.history.remove(0);
        }
        self.current = self.history.len() - 1;
        self.selected = None;
        self.confirm_replace = false;
    }

    pub fn new(storage: Option<&dyn eframe::Storage>) -> Self {
        Self {
            templates: storage
                .and_then(|storage| eframe::get_value(storage, "find_templates"))
                .unwrap_or_default(),
            ..Self::default()
        }
    }

    fn cancel(&mut self) {
        if self.running {
            if let Some(pool) = &self.pool {
                pool.retain(|()| false);
            }
            self.running = false;
        }
    }

    fn standard(&mut self, ui: &mut Ui, action: &mut Option<Action>) {
        ui.spacing_mut().item_spacing.y = 4.;
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            ui.label(label("Directories", 14., TEXT).strong());
            ui.horizontal_wrapped(|ui| {
                ui.radio_value(&mut self.scope, Scope::Locations, "Start locations");
                ui.radio_value(&mut self.scope, Scope::Tabs, "All open tabs");
                ui.radio_value(&mut self.scope, Scope::Selected, "Selected items");
                ui.checkbox(&mut self.options.follow_links, "Follow symlinks");
            });
            if self.scope == Scope::Locations {
                ui.horizontal(|ui| {
                    ui.add(
                        TextEdit::multiline(&mut self.locations)
                            .desired_rows(1)
                            .desired_width((ui.available_width() - 38.).max(80.))
                            .hint_text("One path per line"),
                    );
                    if ui
                        .button("…")
                        .on_hover_text("Choose a search directory")
                        .clicked()
                    {
                        *action = Some(Action::ChooseDirectory);
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Subdirectories");
                let mut limit = self.options.depth.is_some();
                if ui.checkbox(&mut limit, "Limit depth").changed() {
                    self.options.depth = limit.then_some(0);
                }
                if let Some(depth) = &mut self.options.depth {
                    ui.add(egui::DragValue::new(depth).range(0..=999))
                        .on_hover_text("Use 0 to search only the starting location");
                }
                ui.label("Exclude folders");
                ui.add(
                    TextEdit::singleline(&mut self.options.exclude_folders)
                        .hint_text("build;node_modules")
                        .desired_width(185.),
                );
            });
        });
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            ui.label(label("Files", 14., TEXT).strong());
            egui::Grid::new("find-file-fields")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("File name");
                    let hint = match self.options.name_mode {
                        MatchMode::Contains | MatchMode::Exact => "File name",
                        MatchMode::Mask => "*.txt;*.md",
                        MatchMode::Regex => "Regular expression",
                    };
                    let response = ui.add(
                        TextEdit::singleline(&mut self.options.name)
                            .hint_text(hint)
                            .desired_width((ui.available_width() - 85.).max(125.)),
                    );
                    if std::mem::take(&mut self.focus_name) {
                        response.request_focus();
                    }
                    ui.end_row();
                    ui.label("Exclude files");
                    ui.add(
                        TextEdit::singleline(&mut self.options.exclude_files)
                            .hint_text("*.bak;*.tmp")
                            .desired_width((ui.available_width() - 85.).max(125.)),
                    );
                    ui.end_row();
                });
            ui.horizontal_wrapped(|ui| {
                combo(
                    ui,
                    "file-match",
                    &mut self.options.name_mode,
                    &[
                        (MatchMode::Contains, "Partial name"),
                        (MatchMode::Exact, "Exact name"),
                        (MatchMode::Mask, "Wildcard mask"),
                        (MatchMode::Regex, "Regular expression"),
                    ],
                );
                ui.checkbox(&mut self.options.name_case, "Case sensitive");
                ui.checkbox(&mut self.options.archives, "Search in archives");
            });
        });
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            ui.label(label("File contents", 14., TEXT).strong());
            ui.add(
                TextEdit::singleline(&mut self.options.text)
                    .hint_text("Find text in files")
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .checkbox(&mut self.options.text_absent, "Does not contain")
                    .changed()
                    && self.options.text_absent
                {
                    self.options.replacement = None;
                }
                ui.checkbox(&mut self.options.text_hex, "Hexadecimal");
                ui.add_enabled_ui(!self.options.text_hex, |ui| {
                    ui.checkbox(&mut self.options.text_case, "Case sensitive");
                    ui.checkbox(&mut self.options.text_regex, "Regular expression");
                    ui.checkbox(&mut self.options.office_xml, "Office XML");
                });
            });
            ui.horizontal_wrapped(|ui| {
                let mut replacing = self.options.replacement.is_some();
                if ui
                    .add_enabled(
                        !self.options.text_absent,
                        egui::Checkbox::new(&mut replacing, "Replace with"),
                    )
                    .changed()
                {
                    self.options.replacement = replacing.then(String::new);
                }
                if let Some(value) = &mut self.options.replacement {
                    ui.add(
                        TextEdit::singleline(value)
                            .hint_text(if self.options.text_hex {
                                "Hex bytes (blank deletes matches)"
                            } else {
                                "Literal text (blank deletes matches)"
                            })
                            .desired_width(245.),
                    );
                }
                ui.label("Encoding");
                ui.add_enabled(
                    !self.options.text_hex,
                    TextEdit::singleline(&mut self.options.encoding)
                        .hint_text("Default (UTF-8 or BOM)")
                        .desired_width(175.),
                )
                .on_hover_text("Examples: windows-1252, shift_jis, utf-16le");
            });
        });
    }

    fn advanced(&mut self, ui: &mut Ui) {
        let options = &mut self.options;
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            ui.label(label("Size and modification", 14., TEXT).strong());
            egui::Grid::new("file-find-advanced").show(ui, |ui| {
                field(ui, "Size from", &mut options.size_from, "Minimum");
                field(ui, "Size to", &mut options.size_to, "Maximum");
                if options.size_unit == 0 {
                    options.size_unit = 1;
                }
                combo(
                    ui,
                    "size-unit",
                    &mut options.size_unit,
                    &[
                        (1, "Bytes"),
                        (1024, "KiB"),
                        (1024 * 1024, "MiB"),
                        (1024 * 1024 * 1024, "GiB"),
                    ],
                );
                ui.end_row();
                field(ui, "Date from", &mut options.modified_from, "YYYY-MM-DD");
                field(ui, "Date to", &mut options.modified_to, "YYYY-MM-DD");
                ui.end_row();
                field(ui, "Time from", &mut options.time_from, "HH:MM:SS");
                field(ui, "Time to", &mut options.time_to, "HH:MM:SS");
                ui.end_row();
                field(ui, "Not older than", &mut options.age_days, "Days");
                ui.end_row();
            });
        });
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(label("Attributes", 14., TEXT).strong());
                combo(
                    ui,
                    "attributes",
                    &mut options.attributes,
                    &[
                        (Attributes::Any, "Any"),
                        (Attributes::ReadOnly, "Read-only"),
                        (Attributes::Writable, "Writable"),
                        (Attributes::Hidden, "Hidden"),
                        (Attributes::Executable, "Executable"),
                    ],
                );
            });
        });
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            ui.label(label("Find duplicate files", 14., TEXT).strong());
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut options.duplicates.name, "Same name");
                ui.checkbox(&mut options.duplicates.size, "Same size");
                ui.checkbox(&mut options.duplicates.hash, "Same hash");
                ui.checkbox(&mut options.duplicates.content, "Same content");
            });
        });
    }

    fn templates(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut self.template_name).hint_text("Template name"));
            let name = self.template_name.trim();
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new("Save Criteria"))
                .clicked()
            {
                let template = Template {
                    name: name.into(),
                    options: self.options.clone(),
                };
                if let Some(existing) = self.templates.iter_mut().find(|item| item.name == name) {
                    *existing = template;
                } else {
                    self.templates.push(template);
                }
            }
        });
        let mut remove = None;
        for (index, template) in self.templates.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .button(&template.name)
                    .on_hover_text("Load search criteria")
                    .clicked()
                {
                    self.options.clone_from(&template.options);
                    self.template_name.clone_from(&template.name);
                    self.page = Page::Standard;
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            self.templates.remove(index);
        }
    }

    fn results(&mut self, ui: &mut Ui, height: f32, action: &mut Option<Action>) {
        if self.history.is_empty() {
            ui.allocate_space(egui::vec2(1., height));
            return;
        }
        ui.horizontal(|ui| {
            let current = self.current;
            egui::ComboBox::from_id_salt("search-history")
                .selected_text(&self.history[current].title)
                .width(205.)
                .show_ui(ui, |ui| {
                    for (index, run) in self.history.iter().enumerate() {
                        ui.selectable_value(&mut self.current, index, &run.title);
                    }
                });
            if current != self.current {
                self.selected = None;
                self.confirm_replace = false;
            }
            if let Ok(result) = &self.history[self.current].result {
                let count = result.hits.len();
                ui.label(if count == 1 {
                    "1 match".into()
                } else {
                    format!("{count} matches")
                });
                if result.skipped > 0 {
                    ui.label(format!("{} skipped", result.skipped))
                        .on_hover_text("Unreadable, unsupported, or oversized files were skipped");
                }
            }
        });
        let run = &mut self.history[self.current];
        let Ok(results) = &mut run.result else {
            if let Err(error) = &run.result {
                ui.label(label(error, 12., RED));
            }
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt(self.current)
            .max_height(
                (height
                    - if run.options.replacement.is_some() {
                        88.
                    } else {
                        59.
                    })
                .max(72.),
            )
            .auto_shrink([false, false])
            .show_rows(ui, 23., results.hits.len(), |ui, range| {
                for index in range {
                    let (hit, group) = &results.hits[index];
                    let path = hit.display_path().display().to_string();
                    let text = group.map_or_else(
                        || path.clone(),
                        |number| format!("Group {number}    {path}"),
                    );
                    let response = ui
                        .add_sized(
                            [ui.available_width(), 23.],
                            egui::Button::selectable(self.selected == Some(index), text).truncate(),
                        )
                        .on_hover_text(path);
                    if response.clicked() {
                        self.selected = Some(index);
                    }
                    if response.double_clicked() {
                        *action = Some(Action::Reveal(hit.clone(), false));
                    }
                }
            });
        ui.horizontal_wrapped(|ui| {
            if let Some((hit, _)) = self.selected.and_then(|index| results.hits.get(index)) {
                if ui
                    .add_enabled(hit.member.is_none(), egui::Button::new("Open"))
                    .clicked()
                {
                    *action = Some(Action::Open(hit.clone()));
                }
                if ui.button("Quick View").clicked() {
                    *action = Some(Action::Preview(hit.clone()));
                }
                if ui.button("Reveal").clicked() {
                    *action = Some(Action::Reveal(hit.clone(), false));
                }
                if ui.button("Open Folder in Background Tab").clicked() {
                    *action = Some(Action::Reveal(hit.clone(), true));
                }
                if ui.button("Remove Result").clicked() {
                    let (removed, _) = results.hits.remove(self.selected.take().unwrap());
                    if removed.member.is_none() {
                        results.replacements.remove(&removed.path);
                    }
                    self.confirm_replace = false;
                }
            }
            if ui
                .add_enabled(
                    results.hits.iter().any(|(hit, _)| hit.member.is_none()),
                    egui::Button::new("Send Local Files to Panel"),
                )
                .clicked()
            {
                *action = Some(Action::Panel(
                    results
                        .hits
                        .iter()
                        .filter(|(hit, _)| hit.member.is_none())
                        .map(|(hit, _)| hit.path.clone())
                        .collect(),
                ));
            }
        });
        if run.options.replacement.is_some() {
            let eligible = || {
                results
                    .hits
                    .iter()
                    .filter(|(hit, _)| hit.member.is_none())
                    .filter_map(|(hit, _)| results.replacements.get(&hit.path))
            };
            ui.horizontal_wrapped(|ui| {
                let count = eligible().count();
                if self.confirm_replace && count > 0 {
                    ui.label(label(
                        format!("Write replacements to {count} matching files?"),
                        12.,
                        AMBER,
                    ));
                    if ui.button("Confirm Replace").clicked() {
                        *action = Some(Action::Replace(
                            eligible().cloned().collect(),
                            Box::new(run.options.clone()),
                        ));
                        self.confirm_replace = false;
                    }
                    if ui.button("Keep Files").clicked() {
                        self.confirm_replace = false;
                    }
                } else if ui
                    .add_enabled(
                        count > 0,
                        egui::Button::new(format!("Replace in {count} Files")),
                    )
                    .clicked()
                {
                    self.confirm_replace = true;
                }
            });
        }
    }
}

fn field(ui: &mut Ui, title: &str, value: &mut String, hint: &str) {
    ui.label(title);
    ui.add_sized([108., 25.], TextEdit::singleline(value).hint_text(hint));
}

fn combo<T: Copy + PartialEq>(ui: &mut Ui, id: &str, value: &mut T, choices: &[(T, &str)]) {
    let text = choices
        .iter()
        .find(|(candidate, _)| candidate == value)
        .map_or("", |(_, name)| *name);
    egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .show_ui(ui, |ui| {
            for &(candidate, name) in choices {
                ui.selectable_value(value, candidate, name);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_search_invalidates_previous_replace_confirmation() {
        let run = |title: &str| SearchRun {
            title: title.into(),
            result: Ok(Results::default()),
            options: Options {
                replacement: Some(title.into()),
                ..Default::default()
            },
        };
        let mut find = FindFiles::default();
        find.accept_result(run("First"));
        find.running = true;
        find.confirm_replace = true;
        find.accept_result(run("Second"));
        assert_eq!(find.history[find.current].title, "Second");
        assert!(!find.confirm_replace);
    }
}
