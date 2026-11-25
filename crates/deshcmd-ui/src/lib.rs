mod dialogs;
mod panes;
mod settings;
mod theme;
mod window;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use deshcmd_engine::{self as engine, Command, Engine, Entry, JobState, Settings, Side, Step};
use eframe::egui;
use egui::{
    Align, ColorImage, Id, Key, Layout, Rect, Sense, TextureHandle, Ui, UiBuilder, pos2, vec2,
};

use dialogs::{Dialog, Tool};
use panes::PanelUi;
use theme::*;

#[derive(Clone)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    pub drive: bool,
    pub ejectable: bool,
}

pub trait Desktop {
    fn open(&mut self, path: &Path) -> Result<(), String>;
    fn open_terminal(&mut self, directory: &Path) -> Result<(), String>;
    #[cfg(any(target_os = "linux", windows))]
    fn install_menu_entry(&mut self) -> Result<(), String> {
        Err("Application menu integration is unavailable".into())
    }
    fn pick_file(&mut self, initial: &Path, title: &str) -> Option<PathBuf>;
    fn save_file(&mut self, initial: &Path, name: &str, title: &str) -> Option<PathBuf>;
    fn pick_directory(&mut self, initial: &Path, title: &str) -> Option<PathBuf>;
    fn copy_files(&mut self, paths: &[PathBuf]) -> Result<(), String>;
    fn clipboard_files(&mut self) -> Vec<PathBuf>;
    fn clipboard_text(&mut self) -> Option<String>;
    fn copy_text(&mut self, text: &str) -> Result<(), String>;
    fn places(&mut self) -> Vec<Place>;
    fn eject(&mut self, path: &Path) -> Result<(), String>;
    fn shortcut(&mut self, target: &Path, source: &Path) -> Result<(), String>;
}

enum Action {
    Command(Command),
    Tool(Side, Tool),
    Open(Side, Entry, bool),
    Drop {
        side: Side,
        paths: Vec<PathBuf>,
        destination: PathBuf,
        moving: bool,
    },
    Path(Side, PathBuf),
    ToggleQuick(Side),
    ClipboardText(String),
    Eject(PathBuf),
    Close,
}

struct FileClipboard {
    paths: Vec<PathBuf>,
    cutting: bool,
    published: bool,
}

#[derive(Default)]
struct Interface {
    panes: [PanelUi; 2],
    queue: bool,
    tree: bool,
    show_inline_filter: bool,
    show_places: bool,
    show_bottom_bar: bool,
    alternate_columns: [bool; 2],
    quick: Option<Side>,
    clipboard: Option<FileClipboard>,
    notice: Option<(String, Instant)>,
    focus_scroll: Option<Side>,
    closing: bool,
    show_help: bool,
}

pub struct App {
    pub(crate) engine: Engine,
    desktop: Box<dyn Desktop>,
    interface: Interface,
    dialog: Option<Dialog>,
    preferences: Option<settings::Preferences>,
    places: Vec<Place>,
    spaces: HashMap<PathBuf, (u64, u64)>,
    last_places: Instant,
    previews: Previews,
}

impl App {
    pub fn new(
        context: &eframe::CreationContext<'_>,
        mut desktop: Box<dyn Desktop>,
        home: PathBuf,
        initial: [PathBuf; 2],
        fresh: bool,
    ) -> Self {
        theme::install(&context.egui_ctx);
        settings::install_zoom(&context.egui_ctx);
        #[cfg(not(target_os = "macos"))]
        context.egui_ctx.options_mut(|options| {
            // Reserve Ctrl+Q for Quick View.
            let quit = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::Q);
            options.quit_shortcuts.retain(|shortcut| *shortcut != quit);
        });
        let (tree, alternate_columns) = context
            .storage
            .and_then(|storage| eframe::get_value(storage, "panel_display"))
            .unwrap_or_default();
        let settings = if fresh {
            Settings::default()
        } else {
            context
                .storage
                .and_then(|storage| eframe::get_value(storage, "workspace"))
                .unwrap_or_default()
        };
        let ctx = context.egui_ctx.clone();
        let engine = Engine::new(home, initial, settings, move || ctx.request_repaint());
        Self {
            engine,
            places: desktop.places(),
            desktop,
            interface: Interface {
                tree,
                alternate_columns,
                show_inline_filter: context
                    .storage
                    .and_then(|storage| eframe::get_value(storage, "show_inline_filter"))
                    .unwrap_or(true),
                show_places: context
                    .storage
                    .and_then(|storage| eframe::get_value(storage, "show_places"))
                    .unwrap_or(true),
                show_bottom_bar: context
                    .storage
                    .and_then(|storage| eframe::get_value(storage, "show_bottom_bar"))
                    .unwrap_or(true),
                ..Default::default()
            },
            dialog: None,
            preferences: None,
            spaces: HashMap::new(),
            last_places: Instant::now(),
            previews: Previews::new(context.egui_ctx.clone()),
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Command(command) => {
                if let Command::Step(side, ..) | Command::Find(side, ..) = command {
                    self.interface.focus_scroll = Some(side);
                    ctx.request_repaint();
                }
                if matches!(command, Command::Queue(..)) {
                    self.interface.queue = true;
                }
                self.engine.dispatch(command);
            }
            Action::Tool(side, tool) => self.invoke(side, tool, ctx),
            Action::Open(side, entry, background) => {
                if entry.is_dir() {
                    self.engine.dispatch(Command::Navigate {
                        side,
                        path: entry.path,
                        new_tab: background,
                        background,
                    });
                } else if let Err(error) = self.desktop.open(&entry.path) {
                    self.notice(error);
                }
            }
            Action::Drop {
                side,
                paths,
                destination,
                moving,
            } => {
                self.dialog = Some(Dialog::transfer(side, paths, destination, moving));
            }
            Action::Path(side, path) => self.engine.dispatch(Command::Navigate {
                side,
                path,
                new_tab: false,
                background: false,
            }),
            Action::ToggleQuick(side) => {
                self.interface.quick = if self.interface.quick == Some(side) {
                    None
                } else {
                    Some(side)
                }
            }
            Action::ClipboardText(text) => {
                if text.is_empty() {
                    self.notice("There are no items to copy");
                } else {
                    ctx.copy_text(text);
                    self.notice("Copied to clipboard");
                }
            }
            Action::Eject(path) => {
                self.dialog = Some(Dialog::eject(path));
            }
            Action::Close => {
                if self.engine.jobs.iter().any(|job| !job.state.finished()) {
                    self.interface.queue = true;
                    self.notice("Finish or cancel pending operations before closing.");
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                } else {
                    self.interface.closing = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn notice(&mut self, message: impl Into<String>) {
        self.interface.notice = Some((message.into(), Instant::now()));
    }

    fn record_notice(&mut self) {
        if let Some(message) = self.engine.notice.take() {
            self.notice(message);
        }
    }

    fn header(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let side = self.engine.settings.active;
        ui.painter().rect_filled(rect, 0., RAIL);
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1., LINE),
        );
        let brand_x = rect.left() + 25.;
        #[cfg(target_os = "macos")]
        let brand_x = brand_x + 76. / ui.ctx().zoom_factor();
        let brand = Rect::from_center_size(pos2(brand_x, rect.center().y), vec2(26., 26.));
        ui.painter().rect_filled(brand, 7., ACCENT);
        Icon::Brand.draw(ui, brand.center(), 16., BASE);
        let compact = rect.width() < 850.;
        let area = Rect::from_min_max(
            pos2(brand.right() + 9., rect.top() + 6.),
            rect.right_bottom() - vec2(9., 6.),
        );
        let mut toolbar = child(
            ui,
            Id::unique("toolbar"),
            area,
            Layout::left_to_right(Align::Center),
        );
        toolbar.spacing_mut().item_spacing.x = 5.;
        if action(
            &mut toolbar,
            Icon::Plus,
            if compact { "" } else { "New" },
            "Create a folder or file",
            true,
            true,
        )
        .clicked()
        {
            actions.push(Action::Tool(side, Tool::NewFolder));
        }
        toolbar.menu_button("File", |ui| dialogs::file_menu(ui, side, actions));
        toolbar.menu_button("Select", |ui| dialogs::selection_menu(ui, side, actions));
        toolbar.menu_button("View", |ui| self.view_menu(ui, side, actions));
        toolbar.menu_button("Tools", |ui| dialogs::tools_menu(ui, side, actions));
        let drag_start = toolbar.cursor().left();
        let drag_end = toolbar
            .with_layout(Layout::right_to_left(Align::Center), |ui| {
                #[cfg(not(target_os = "macos"))]
                {
                    if window::controls(ui) {
                        actions.push(Action::Close);
                    }
                    ui.separator();
                }
                let pending = self
                    .engine
                    .jobs
                    .iter()
                    .filter(|job| !job.state.finished())
                    .count();
                let title = if compact {
                    String::new()
                } else if pending > 0 {
                    format!("Queue  {pending}")
                } else {
                    "Queue".into()
                };
                let queue_label = if pending > 0 {
                    format!("Operation queue · {pending} pending")
                } else {
                    "Operation queue and log".into()
                };
                if action(
                    ui,
                    Icon::Queue,
                    &title,
                    &queue_label,
                    true,
                    self.interface.queue,
                )
                .clicked()
                {
                    self.interface.queue = !self.interface.queue;
                }
                if icon_button(ui, Icon::Settings, "Settings", true).clicked() {
                    actions.push(Action::Tool(side, Tool::Settings));
                }
                if icon_button(
                    ui,
                    if self.engine.settings.stacked {
                        Icon::Stack
                    } else {
                        Icon::Columns
                    },
                    "Switch panel orientation",
                    true,
                )
                .clicked()
                {
                    self.engine.settings.stacked = !self.engine.settings.stacked;
                }
                if icon_button(ui, Icon::Refresh, "Refresh active folders · Ctrl+R", true).clicked()
                {
                    actions.push(Action::Command(Command::Refresh));
                }
                if icon_button(ui, Icon::Swap, "Swap panels", true).clicked() {
                    actions.push(Action::Command(Command::Swap));
                }
                let places_label = if self.interface.show_places {
                    "Hide Places sidebar"
                } else {
                    "Show Places sidebar"
                };
                if action(
                    ui,
                    Icon::Sidebar,
                    "",
                    places_label,
                    true,
                    self.interface.show_places,
                )
                .clicked()
                {
                    self.interface.show_places = !self.interface.show_places;
                }
                ui.cursor().right()
            })
            .inner;
        let drag_area =
            Rect::from_min_max(pos2(drag_start, area.top()), pos2(drag_end, rect.bottom()));
        window::drag_region(ui, drag_area);
    }

    fn view_menu(&mut self, ui: &mut Ui, side: Side, actions: &mut Vec<Action>) {
        use engine::View;
        for (title, view) in [
            ("Details", View::Details),
            ("Compact list", View::Compact),
            ("Gallery", View::Gallery),
        ] {
            if ui
                .selectable_label(self.engine.tab(side).saved.view == view, title)
                .clicked()
            {
                actions.push(Action::Command(Command::View(side, view)));
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Flat view").clicked() {
            actions.push(Action::Command(Command::Flat(side, false)));
            ui.close();
        }
        if ui.button("Flat view from selection").clicked() {
            actions.push(Action::Command(Command::Flat(side, true)));
            ui.close();
        }
        if ui.button("Quick View · F3").clicked() {
            actions.push(Action::ToggleQuick(side));
            ui.close();
        }
        if ui
            .selectable_label(self.interface.tree, "Directory tree")
            .clicked()
        {
            self.interface.tree = !self.interface.tree;
            ui.close();
        }
        if ui
            .selectable_label(self.engine.settings.show_hidden, "Hidden files")
            .clicked()
        {
            actions.push(Action::Command(Command::ToggleHidden));
            ui.close();
        }
        if ui
            .selectable_label(
                self.interface.alternate_columns[side.index()],
                "Alternate column set",
            )
            .clicked()
        {
            self.interface.alternate_columns[side.index()] =
                !self.interface.alternate_columns[side.index()];
            ui.close();
        }
        if ui.button("Refresh · Ctrl+R").clicked() {
            actions.push(Action::Command(Command::Refresh));
            ui.close();
        }
    }

    fn rail(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        ui.painter().rect_filled(rect, 0., RAIL);
        ui.painter().vline(
            rect.right() - 0.5,
            rect.y_range(),
            egui::Stroke::new(1., LINE),
        );
        let mut nav = child(
            ui,
            Id::unique("places"),
            rect.shrink2(vec2(12., 13.)),
            Layout::top_down(Align::Min),
        );
        let side = self.engine.settings.active;
        let current = self.engine.tab(side).base();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut nav, |ui| {
                ui.spacing_mut().item_spacing.y = 3.;
                ui.label(label("PLACES", 11., MUTED).strong());
                ui.add_space(3.);
                let home = self.engine.home.clone();
                if place(
                    ui,
                    Icon::Home,
                    "Home",
                    &home.display().to_string(),
                    current == home,
                )
                .clicked()
                {
                    actions.push(Action::Path(side, home));
                }
                for entry in self.places.iter().filter(|entry| !entry.drive) {
                    if entry.path == self.engine.home {
                        continue;
                    }
                    if place(
                        ui,
                        Icon::Folder,
                        &entry.name,
                        &entry.path.display().to_string(),
                        current == entry.path,
                    )
                    .clicked()
                    {
                        actions.push(Action::Path(side, entry.path.clone()));
                    }
                }
                ui.add_space(13.);
                ui.horizontal(|ui| {
                    ui.label(label("BOOKMARKS", 11., MUTED).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, Icon::Plus, "Add bookmark", true).clicked() {
                            actions.push(Action::Tool(side, Tool::Bookmark));
                        }
                    });
                });
                if self.engine.settings.bookmarks.is_empty()
                    && place(
                        ui,
                        Icon::Star,
                        "Add current folder",
                        "Bookmark the active location",
                        false,
                    )
                    .clicked()
                {
                    actions.push(Action::Tool(side, Tool::Bookmark));
                }
                let mut bookmarks: Vec<_> =
                    self.engine.settings.bookmarks.iter().enumerate().collect();
                bookmarks.sort_by(|a, b| a.1.group.cmp(&b.1.group));
                let mut group = "";
                for (index, entry) in bookmarks {
                    if entry.group != group && !entry.group.is_empty() {
                        ui.label(label(&entry.group, 11., MUTED));
                    }
                    group = &entry.group;
                    let response = place(
                        ui,
                        Icon::Star,
                        &entry.name,
                        &entry.path.display().to_string(),
                        current == entry.path,
                    );
                    if response.clicked() {
                        actions.push(Action::Command(Command::OpenBookmark(index)));
                    }
                    response.context_menu(|ui| {
                        if ui.button("Edit bookmark").clicked() {
                            actions.push(Action::Tool(side, Tool::EditBookmark(index)));
                            ui.close();
                        }
                        if ui.button("Remove bookmark").clicked() {
                            actions.push(Action::Command(Command::RemoveBookmark(index)));
                            ui.close();
                        }
                    });
                }
                ui.add_space(13.);
                ui.label(label("VOLUMES", 11., MUTED).strong());
                for entry in self.places.iter().filter(|entry| entry.drive) {
                    let response = place(
                        ui,
                        Icon::Drive,
                        &entry.name,
                        &entry.path.display().to_string(),
                        current == entry.path,
                    );
                    if response.clicked() {
                        actions.push(Action::Path(side, entry.path.clone()));
                    }
                    if entry.ejectable {
                        response.context_menu(|ui| {
                            if ui.button("Unmount").clicked() {
                                actions.push(Action::Eject(entry.path.clone()));
                                ui.close();
                            }
                        });
                    }
                }
                ui.add_space(10.);
                if place(
                    ui,
                    Icon::Tree,
                    "Directory tree",
                    "Show the current filesystem hierarchy",
                    self.interface.tree,
                )
                .clicked()
                {
                    self.interface.tree = !self.interface.tree;
                }
                if self.interface.tree {
                    let root = current.ancestors().last().unwrap_or(current).to_path_buf();
                    panes::tree(ui, side, &root, current, 0, actions);
                }
            });
    }

    fn footer(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        ui.painter().rect_filled(rect, 0., RAIL);
        ui.painter().hline(
            rect.x_range(),
            rect.top() + 0.5,
            egui::Stroke::new(1., LINE),
        );
        let mut footer = child(
            ui,
            Id::unique("status"),
            rect.shrink2(vec2(13., 3.)),
            Layout::left_to_right(Align::Center),
        );
        footer.spacing_mut().item_spacing.x = 6.;
        let side = self.engine.settings.active;
        let has_items = self.engine.tab(side).has_operands();
        for (key, title, icon, command, enabled) in [
            (
                "F3",
                "Preview",
                Icon::Eye,
                Action::ToggleQuick(side),
                has_items,
            ),
            (
                "F5",
                "Copy",
                Icon::Copy,
                Action::Tool(side, Tool::Copy),
                has_items,
            ),
            (
                "F6",
                "Move",
                Icon::Move,
                Action::Tool(side, Tool::Move),
                has_items,
            ),
            (
                "F7",
                "Folder",
                Icon::Plus,
                Action::Tool(side, Tool::NewFolder),
                true,
            ),
            (
                "F8",
                "Trash",
                Icon::Trash,
                Action::Tool(side, Tool::Trash),
                has_items,
            ),
        ] {
            if action(
                &mut footer,
                icon,
                title,
                &format!("{title} · {key}"),
                enabled,
                false,
            )
            .clicked()
            {
                actions.push(command);
            }
        }
        footer.separator();
        if footer.button(label("Shortcuts", 12., SUB)).clicked() {
            self.interface.show_help = true;
        }
        footer.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some((message, _)) = &self.interface.notice {
                let response = ui.add(egui::Label::new(label(message, 12., AMBER)).truncate());
                response.on_hover_text(message);
            } else {
                ui.label(mono(
                    format!("{} PANEL", side.label().to_uppercase()),
                    11.,
                    ACCENT,
                ));
                if self.engine.settings.linked {
                    ui.label(label("Linked", 11., SUB));
                }
            }
        });
    }

    fn queue(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        card(ui, rect, SURFACE);
        let compact = rect.height() < 80.;
        let mut frame = child(
            ui,
            Id::unique("queue-drawer"),
            rect.shrink2(vec2(13., if compact { 4. } else { 8. })),
            Layout::top_down(Align::Min),
        );
        if compact {
            frame.spacing_mut().item_spacing.y = 2.;
        }
        frame.horizontal(|ui| {
            ui.label(label("Operations", 15., TEXT).strong());
            ui.label(mono(
                format!("{} recorded", self.engine.jobs.len()),
                11.,
                MUTED,
            ));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, Icon::Close, "Hide operation queue", true).clicked() {
                    self.interface.queue = false;
                }
                if ui.small_button("Clear finished").clicked() {
                    actions.push(Action::Command(Command::ClearJobs));
                }
            });
        });
        if self.engine.jobs.is_empty() {
            frame.label(label("No file operations yet.", 13., MUTED));
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut frame, |ui| {
                for job in &self.engine.jobs {
                    ui.horizontal(|ui| {
                        ui.label(label(job.title, 13., TEXT).strong());
                        let ink = match job.state {
                            JobState::Failed => RED,
                            JobState::Done => ACCENT,
                            JobState::Conflict => AMBER,
                            _ => SUB,
                        };
                        ui.label(label(job.state.label(), 12., ink));
                        if let Some(total) = job.progress.total.filter(|total| *total > 0) {
                            ui.add(
                                egui::ProgressBar::new(
                                    (job.progress.bytes as f32 / total as f32).min(1.),
                                )
                                .desired_width(85.),
                            );
                            if !job.state.finished()
                                && let Some(start) = job.started
                            {
                                let rate = job.progress.bytes as f64
                                    / start.elapsed().as_secs_f64().max(0.1);
                                if rate > 0. {
                                    let eta = (total.saturating_sub(job.progress.bytes) as f64
                                        / rate)
                                        as u64;
                                    ui.label(mono(
                                        format!("{}/s · {eta}s", metric(rate as u64)),
                                        10.,
                                        MUTED,
                                    ));
                                }
                            }
                        }
                        if job.progress.file_total > 0 && !job.state.finished() {
                            ui.add(
                                egui::ProgressBar::new(
                                    (job.progress.file_bytes as f32
                                        / job.progress.file_total as f32)
                                        .min(1.),
                                )
                                .desired_width(45.),
                            )
                            .on_hover_text(&job.progress.current);
                        }
                        if !job.message.is_empty() || !job.warnings.is_empty() {
                            let mut detail = job.message.clone();
                            for warning in &job.warnings {
                                detail.push('\n');
                                detail.push_str(warning);
                            }
                            if ui.small_button("Details").on_hover_text(detail).clicked() {
                                actions.push(Action::Tool(
                                    self.engine.settings.active,
                                    Tool::JobDetails(job.id),
                                ));
                            }
                            ui.label(label(
                                job.message.lines().next().unwrap_or_default(),
                                12.,
                                SUB,
                            ));
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !job.state.finished() && ui.small_button("Cancel").clicked() {
                                actions.push(Action::Command(Command::CancelJob(job.id)));
                            }
                            if matches!(job.state, JobState::Held | JobState::Paused)
                                && ui.small_button("Resume").clicked()
                            {
                                actions.push(Action::Command(Command::ResumeJob(job.id)));
                            }
                            if matches!(job.state, JobState::Running | JobState::Queued)
                                && ui.small_button("Pause").clicked()
                            {
                                actions.push(Action::Command(Command::PauseJob(job.id)));
                            }
                            if matches!(job.state, JobState::Queued | JobState::Held)
                                && ui.small_button("Move to front").clicked()
                            {
                                actions.push(Action::Command(Command::PrioritizeJob(job.id)));
                            }
                        });
                    });
                }
            });
    }

    fn text_field_focused(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|memory| {
            Side::ALL.into_iter().any(|side| {
                memory.has_focus(panes::filter_id(side)) || memory.has_focus(panes::path_id(side))
            })
        })
    }

    fn clipboard_shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let (copy, cut, paste) = ctx.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::COMMAND, Key::C),
                input.consume_key(egui::Modifiers::COMMAND, Key::X),
                input.consume_key(egui::Modifiers::COMMAND, Key::V),
            )
        });
        if !(copy || cut || paste) {
            return;
        }
        let text_context = self.text_field_focused(ctx)
            || self.dialog.is_some()
            || self.preferences.is_some()
            || self.interface.show_help
            || self.engine.jobs.iter().any(|job| job.conflict.is_some());
        if text_context {
            let event = if copy {
                Some(egui::Event::Copy)
            } else if cut {
                Some(egui::Event::Cut)
            } else {
                self.desktop
                    .clipboard_text()
                    .map(|text| egui::Event::Paste(text.replace("\r\n", "\n")))
            };
            if let Some(event) = event {
                ctx.input_mut(|input| input.events.push(event));
            }
        } else {
            let tool = if copy {
                Tool::CopyClipboard
            } else if cut {
                Tool::CutClipboard
            } else {
                Tool::Paste
            };
            actions.push(Action::Tool(self.engine.settings.active, tool));
        }
    }

    fn publish_text_clipboard(&mut self, ctx: &egui::Context) {
        let texts = ctx.output_mut(|output| {
            let mut texts = Vec::new();
            output.commands.retain(|command| {
                if let egui::OutputCommand::CopyText(text) = command {
                    texts.push(text.clone());
                    false
                } else {
                    true
                }
            });
            texts
        });
        if let Some(text) = texts.last()
            && self.desktop.copy_text(text).is_err()
        {
            ctx.copy_text(text.clone());
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if self.dialog.is_some()
            || self.preferences.is_some()
            || self.interface.show_help
            || egui::Popup::is_any_open(ctx)
            || self.engine.jobs.iter().any(|job| job.conflict.is_some())
        {
            return;
        }
        let side = self.engine.settings.active;
        let editing = self.text_field_focused(ctx);
        let pressed = |mods, key| ctx.input_mut(|input| input.consume_key(mods, key));
        let just = |key| pressed(egui::Modifiers::NONE, key);
        let command = |key| pressed(egui::Modifiers::COMMAND, key);
        let shift = |key| pressed(egui::Modifiers::SHIFT, key);
        let alt = |key| pressed(egui::Modifiers::ALT, key);
        // egui records directional widget navigation before the app consumes keys.
        let stop_widget_navigation =
            || ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
        let exact = |modifiers, key| {
            ctx.input_mut(|input| {
                let before = input.events.len();
                input.events.retain(|event| {
                    !matches!(
                        event,
                        egui::Event::Key {
                            key: event_key,
                            modifiers: event_modifiers,
                            pressed: true,
                            ..
                        } if *event_key == key && event_modifiers.matches_exact(modifiers)
                    )
                });
                before != input.events.len()
            })
        };
        if just(Key::F1) {
            self.interface.show_help = true;
            return;
        }
        if ctx.input(|input| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: Key::F9,
                        modifiers: egui::Modifiers::NONE,
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            })
        }) {
            actions.push(Action::Tool(side, Tool::OpenTerminal));
        }
        if command(Key::L) {
            self.interface.panes[side.index()].start_path();
            return;
        }
        if command(Key::F) {
            actions.push(Action::Tool(side, Tool::FilterOptions));
            return;
        }
        if editing {
            return;
        }

        if just(Key::Tab) {
            actions.push(Action::Command(Command::Focus(side.other())));
        }
        let tab_shortcut = |modifiers, key| {
            ctx.input_mut(|input| {
                let mut pressed = false;
                input.events.retain(|event| {
                    if let egui::Event::Key {
                        key: event_key,
                        modifiers: event_modifiers,
                        pressed: true,
                        repeat,
                        ..
                    } = event
                        && *event_key == key
                        && event_modifiers.matches_exact(modifiers)
                    {
                        pressed |= !repeat;
                        return false;
                    }
                    true
                });
                pressed
            })
        };
        if tab_shortcut(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::T) {
            let index = self.engine.pane(side).active;
            actions.push(Action::Command(Command::SendTab(side, index, false)));
        }
        if command(Key::T) {
            actions.push(Action::Command(Command::NewTab(side)));
        }
        if tab_shortcut(egui::Modifiers::CTRL, Key::ArrowUp) {
            actions.push(Action::Command(Command::OpenFocusedDirectoryInTab {
                source: side,
                destination: side,
            }));
        }
        if tab_shortcut(egui::Modifiers::CTRL, Key::ArrowRight) && side == Side::Left {
            actions.push(Action::Command(Command::OpenFocusedDirectoryInTab {
                source: side,
                destination: Side::Right,
            }));
        }
        if command(Key::W) {
            let index = self.engine.pane(side).active;
            actions.push(Action::Command(Command::CloseTab(side, index)));
        }
        for (index, key) in [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ]
        .into_iter()
        .enumerate()
        {
            if alt(key) {
                actions.push(Action::Command(Command::Tab(side, index)));
                break;
            }
        }
        if alt(Key::ArrowLeft) {
            actions.push(Action::Command(Command::Back(side)));
        }
        if alt(Key::ArrowRight) {
            actions.push(Action::Command(Command::Forward(side)));
        }
        let left = exact(egui::Modifiers::NONE, Key::ArrowLeft);
        if left {
            stop_widget_navigation();
        }
        if alt(Key::ArrowUp) || just(Key::Backspace) || left {
            actions.push(Action::Command(Command::Parent(side)));
        }
        if command(Key::R) {
            actions.push(Action::Command(Command::Refresh));
        }
        if command(Key::A) {
            actions.push(Action::Command(Command::MarkAll(side)));
        }
        if command(Key::I) {
            actions.push(Action::Command(Command::Invert(side)));
        }
        if just(Key::Escape) {
            actions.push(Action::Command(Command::UnmarkAll(side)));
            self.interface.quick = None;
        }
        for (key, step) in [
            (Key::ArrowUp, Step::Previous),
            (Key::ArrowDown, Step::Next),
            (Key::Home, Step::First),
            (Key::End, Step::Last),
            (Key::PageUp, Step::PageUp),
            (Key::PageDown, Step::PageDown),
        ] {
            let extend = shift(key);
            if extend || just(key) {
                if matches!(key, Key::ArrowUp | Key::ArrowDown) {
                    stop_widget_navigation();
                }
                actions.push(Action::Command(Command::Step(side, step, extend)));
            }
        }
        if (just(Key::Space) || just(Key::Insert))
            && let Some(entry) = self.engine.tab(side).current()
        {
            actions.push(Action::Command(Command::Select(
                side,
                entry.path.clone(),
                engine::Select::Toggle,
            )));
        }
        if just(Key::Enter)
            && let Some(entry) = self.engine.tab(side).current().cloned()
        {
            actions.push(Action::Open(side, entry, false));
        }
        if exact(egui::Modifiers::NONE, Key::ArrowRight) {
            stop_widget_navigation();
            if let Some(entry) = self
                .engine
                .tab(side)
                .current()
                .filter(|entry| entry.is_dir())
            {
                actions.push(Action::Path(side, entry.path.clone()));
            }
        }
        if just(Key::F3) || exact(egui::Modifiers::CTRL, Key::Q) {
            actions.push(Action::ToggleQuick(side));
        }
        if shift(Key::F5) {
            actions.push(Action::Tool(side, Tool::CopyHere));
        }
        if shift(Key::Delete) {
            actions.push(Action::Tool(side, Tool::Delete));
        }
        for (key, tool) in [
            (Key::F2, Tool::Rename),
            (Key::F5, Tool::Copy),
            (Key::F6, Tool::Move),
            (Key::F7, Tool::NewFolder),
            (Key::F8, Tool::Trash),
            (Key::Delete, Tool::Trash),
        ] {
            if just(key) {
                actions.push(Action::Tool(side, tool));
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        egui::Window::new("Keyboard shortcuts")
            .open(&mut self.interface.show_help)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0., 0.])
            .show(ctx, |ui| {
                ui.set_min_width(426.);
                egui_extras::TableBuilder::new(ui)
                    .striped(true)
                    .max_scroll_height((ctx.content_rect().height() - 72.).max(26.))
                    .column(egui_extras::Column::exact(145.))
                    .column(egui_extras::Column::remainder())
                    .body(|mut body| {
                        for (keys, purpose) in [
                            ("Tab", "Switch active panel"),
                            ("Alt + Left / Right", "Go back or forward"),
                            ("Left / Alt + Up", "Go to parent"),
                            ("Right", "Open focused directory"),
                            ("Ctrl + L", "Edit location"),
                            ("Ctrl + T / W", "Duplicate or close current tab"),
                            ("Ctrl + Shift + T", "Copy current tab to other panel"),
                            ("Ctrl + Up", "Open focused folder in new tab"),
                            (
                                "Ctrl + Right",
                                "From left, open focused folder in a right tab",
                            ),
                            ("Alt + 1–9", "Switch to tab"),
                            ("Enter", "Open focused item"),
                            ("Up / Down · Space", "Move focus · toggle mark"),
                            ("Shift + Up / Down", "Extend selection"),
                            ("Ctrl + A / I", "Mark all or invert marks"),
                            ("Ctrl + F / R", "Open filter options or refresh"),
                            ("F2", "Rename"),
                            ("Ctrl + Q / F3", "Toggle Quick View"),
                            ("F5 / F6", "Copy or move to opposite panel"),
                            ("Shift + F5", "Copy in same folder"),
                            ("F7 / F8", "Create folder or move to Trash"),
                            ("F9", "Open terminal in active folder"),
                            ("Shift + Delete", "Delete permanently"),
                            ("Ctrl + C / X / V", "Copy, cut, or paste files"),
                        ] {
                            body.row(26., |mut row| {
                                row.col(|ui| {
                                    ui.label(mono(keys, 12., ACCENT));
                                });
                                row.col(|ui| {
                                    ui.label(label(purpose, 13., TEXT));
                                });
                            });
                        }
                    });
            });
    }

    fn content(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let (root, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        ui.painter().rect_filled(root, 0., BASE);
        let top = Rect::from_min_size(root.min, vec2(root.width(), window::HEADER_HEIGHT));
        self.header(ui, top, actions);
        let workspace_bottom = if self.interface.show_bottom_bar {
            let bottom =
                Rect::from_min_max(pos2(root.left(), root.bottom() - 41.), root.right_bottom());
            self.footer(ui, bottom, actions);
            bottom.top()
        } else {
            root.bottom()
        };
        let rail_w = if !self.interface.show_places {
            0.
        } else if root.width() > 1050. {
            184.
        } else {
            158.
        };
        if self.interface.show_places {
            let rail = Rect::from_min_max(
                pos2(root.left(), top.bottom()),
                pos2(root.left() + rail_w, workspace_bottom),
            );
            self.rail(ui, rail, actions);
        }
        let gap = 12.;
        let vertical_margin = if root.height() < 560. { 6. } else { 14. };
        let workspace = Rect::from_min_max(
            pos2(root.left() + rail_w + 14., top.bottom() + vertical_margin),
            pos2(root.right() - 14., workspace_bottom - vertical_margin),
        );
        let queue_gap = 11.;
        let min_queue_height = 61. + queue_gap;
        let min_stacked_height = 2. * panes::MIN_HEIGHT + gap;
        let queue_space = if self.interface.queue {
            min_queue_height
        } else {
            0.
        };
        let stacked =
            self.engine.settings.stacked && workspace.height() >= min_stacked_height + queue_space;
        let queue_height = if self.interface.queue {
            let preferred = (workspace.height() * 0.35).clamp(108., 180.) + queue_gap;
            let pane_space = if stacked {
                min_stacked_height
            } else {
                panes::MIN_HEIGHT
            };
            preferred.min((workspace.height() - pane_space).max(0.))
        } else {
            0.
        };
        let body = Rect::from_min_max(workspace.min, workspace.max - vec2(0., queue_height));
        if queue_height > queue_gap {
            let queue = Rect::from_min_max(
                pos2(workspace.left(), body.bottom() + queue_gap),
                workspace.max,
            );
            self.queue(ui, queue, actions);
        }

        let ratio = self.engine.settings.split.clamp(0.25, 0.75);
        let (panel1, divider, panel2) = if stacked {
            let pane_height = (body.height() - gap).max(0.);
            let min_height = panes::MIN_HEIGHT.min(pane_height / 2.);
            let split =
                body.top() + (pane_height * ratio).clamp(min_height, pane_height - min_height);
            (
                Rect::from_min_max(body.min, pos2(body.right(), split)),
                Rect::from_min_max(pos2(body.left(), split), pos2(body.right(), split + gap)),
                Rect::from_min_max(pos2(body.left(), split + gap), body.max),
            )
        } else {
            let split = body.left() + (body.width() - gap) * ratio;
            (
                Rect::from_min_max(body.min, pos2(split, body.bottom())),
                Rect::from_min_max(pos2(split, body.top()), pos2(split + gap, body.bottom())),
                Rect::from_min_max(pos2(split + gap, body.top()), body.max),
            )
        };
        let grip = ui
            .interact(
                divider,
                Id::unique("resizable-divider"),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(if stacked {
                egui::CursorIcon::ResizeVertical
            } else {
                egui::CursorIcon::ResizeHorizontal
            });
        if grip.dragged()
            && let Some(pos) = grip.interact_pointer_pos()
        {
            self.engine.settings.split = if stacked {
                (pos.y - body.top() - gap / 2.) / (body.height() - gap)
            } else {
                (pos.x - body.left() - gap / 2.) / (body.width() - gap)
            }
            .clamp(0.25, 0.75);
        }
        if grip.double_clicked() {
            self.engine.settings.split = 0.5;
        }
        let line = if grip.hovered() || grip.dragged() {
            ACCENT
        } else {
            LINE
        };
        if stacked {
            ui.painter().hline(
                divider.center().x - 14. ..=divider.center().x + 14.,
                divider.center().y,
                egui::Stroke::new(2., line),
            );
        } else {
            ui.painter().vline(
                divider.center().x,
                divider.center().y - 14. ..=divider.center().y + 14.,
                egui::Stroke::new(2., line),
            );
        }

        for (side, rect) in Side::ALL.into_iter().zip([panel1, panel2]) {
            let quick = self.interface.quick.filter(|source| source.other() == side);
            let parent_focus_restored =
                quick.is_none() && self.engine.take_parent_focus_restored(side);
            let pane = self.engine.pane(side);
            let root = pane.tab().base();
            let space = self
                .spaces
                .get(root)
                .copied()
                .or_else(|| engine::space(root));
            if let Some(space) = space {
                self.spaces.insert(root.to_owned(), space);
            }
            let column_set = if self.interface.alternate_columns[side.index()] {
                &self.engine.settings.alternate_columns
            } else {
                &self.engine.settings.columns
            };
            let settings = panes::PaneScreen {
                pane,
                side,
                active: self.engine.settings.active == side,
                home: &self.engine.home,
                space,
                columns: column_set,
                show_modified_time: self.engine.settings.show_modified_time,
                show_inline_filter: self.interface.show_inline_filter,
                reveal_cursor: self.interface.focus_scroll == Some(side) || parent_focus_restored,
                preview: quick.map(|source| (source, self.engine.tab(source).current())),
            };
            self.interface.panes[side.index()].show(
                ui,
                rect,
                settings,
                &mut self.previews,
                actions,
            );
        }
        self.interface.focus_scroll = None;
        let dropped = ui.ctx().input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .filter(|path| !path.as_os_str().is_empty())
                .collect::<Vec<_>>()
        });
        if !dropped.is_empty()
            && let Some(pointer) = ui.ctx().pointer_latest_pos()
        {
            let side = if panel2.contains(pointer) {
                Side::Right
            } else if panel1.contains(pointer) {
                Side::Left
            } else {
                self.engine.settings.active
            };
            actions.push(Action::Drop {
                side,
                paths: dropped,
                destination: self.engine.tab(side).base().to_path_buf(),
                moving: false,
            });
        }
        #[cfg(not(target_os = "macos"))]
        window::resize_edges(ui, root);
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.engine.poll();
        self.record_notice();
        self.previews.poll();
        if self.last_places.elapsed() >= Duration::from_secs(10) {
            self.places = self.desktop.places();
            self.spaces.clear();
            self.last_places = Instant::now();
        }
        if self
            .interface
            .notice
            .as_ref()
            .is_some_and(|(_, start)| start.elapsed() >= Duration::from_secs(7))
        {
            self.interface.notice = None;
        }
        if ctx.input(|input| input.viewport().close_requested())
            && !self.interface.closing
            && self.engine.jobs.iter().any(|job| !job.state.finished())
        {
            self.apply(Action::Close, ctx);
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(target_os = "macos")]
        _frame.set_traffic_lights_position(
            &ctx,
            egui::Rangef::new(0., window::HEADER_HEIGHT),
            12. / ctx.zoom_factor(),
        );
        let mut actions = Vec::new();
        if self.preferences.is_none() {
            settings::zoom_shortcuts(&ctx);
        }
        self.clipboard_shortcuts(&ctx, &mut actions);
        self.keyboard(&ctx, &mut actions);
        for action in actions.drain(..) {
            self.apply(action, &ctx);
        }
        self.content(ui, &mut actions);
        for action in actions {
            self.apply(action, &ctx);
        }
        if self.preferences.is_some() {
            self.show_settings(&ctx);
        } else {
            self.dialogs(&ctx);
        }
        if self.interface.show_help {
            self.shortcuts(&ctx);
        }
        self.publish_text_clipboard(&ctx);
        self.record_notice();
        if !self.interface.show_bottom_bar
            && let Some((message, _)) = &self.interface.notice
        {
            egui::Area::new(Id::unique("status-notice"))
                .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-14., -12.))
                .interactable(false)
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(360.);
                        ui.label(label(message, 12., AMBER));
                    });
                });
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "workspace", &self.engine.snapshot());
        eframe::set_value(
            storage,
            "show_inline_filter",
            &self.interface.show_inline_filter,
        );
        eframe::set_value(storage, "show_places", &self.interface.show_places);
        eframe::set_value(storage, "show_bottom_bar", &self.interface.show_bottom_bar);
        eframe::set_value(
            storage,
            "panel_display",
            &(self.interface.tree, self.interface.alternate_columns),
        );
    }
}

fn child(ui: &mut Ui, id: Id, rect: Rect, layout: Layout) -> Ui {
    let clip = ui.clip_rect().intersect(rect);
    let mut child = ui.new_child(UiBuilder::new().id_salt(id).max_rect(rect).layout(layout));
    child.set_clip_rect(clip);
    child
}

type PreviewKey = (PathBuf, Option<SystemTime>, u32);
enum CachedPreview {
    Image(TextureHandle, egui::Vec2),
    Text(String),
    None,
}

struct Previews {
    ctx: egui::Context,
    requests: mpsc::SyncSender<(PreviewKey, engine::Kind)>,
    results: mpsc::Receiver<(PreviewKey, engine::Preview)>,
    pending: HashSet<PreviewKey>,
    cached: HashMap<PreviewKey, CachedPreview>,
    order: std::collections::VecDeque<PreviewKey>,
}

impl Previews {
    fn new(ctx: egui::Context) -> Self {
        let (requests, receive) = mpsc::sync_channel::<(PreviewKey, engine::Kind)>(32);
        let (send, results) = mpsc::channel();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            while let Ok((key, kind)) = receive.recv() {
                let preview = engine::preview(&key.0, kind, key.2);
                if send.send((key, preview)).is_err() {
                    break;
                }
                wake.request_repaint();
            }
        });
        Self {
            ctx,
            requests,
            results,
            pending: HashSet::new(),
            cached: HashMap::new(),
            order: Default::default(),
        }
    }
    fn get(&mut self, entry: &Entry, size: u32) -> Option<&CachedPreview> {
        let key = (entry.path.clone(), entry.modified, size);
        if !self.cached.contains_key(&key)
            && !self.pending.contains(&key)
            && self.requests.try_send((key.clone(), entry.kind)).is_ok()
        {
            self.pending.insert(key.clone());
        }
        self.cached.get(&key)
    }
    fn poll(&mut self) {
        while let Ok((key, preview)) = self.results.try_recv() {
            self.pending.remove(&key);
            let cached = match preview {
                engine::Preview::Image { size, rgba } => {
                    let image = ColorImage::from_rgba_unmultiplied(size, &rgba);
                    let texture = self.ctx.load_texture(
                        key.0.display().to_string(),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    CachedPreview::Image(texture, vec2(size[0] as f32, size[1] as f32))
                }
                engine::Preview::Text(text) => CachedPreview::Text(text),
                engine::Preview::Unavailable => CachedPreview::None,
            };
            self.cached.insert(key.clone(), cached);
            self.order.push_back(key);
            while self.order.len() > 150 {
                if let Some(old) = self.order.pop_front() {
                    self.cached.remove(&old);
                }
            }
        }
    }
}

#[cfg(all(test, feature = "perf"))]
mod performance;

#[cfg(test)]
mod tests;
