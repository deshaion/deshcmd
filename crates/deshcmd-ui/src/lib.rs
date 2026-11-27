mod about;
mod dialogs;
mod find_files;
mod folder_sizes;
mod panes;
mod previews;
mod probes;
mod settings;
mod text_preview;
mod theme;
mod updates;
mod window;

#[cfg(test)]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::mpsc;
use std::time::{Duration, Instant};

use deshcmd_engine::{self as engine, Command, Engine, Entry, JobState, Settings, Side, Step};
use eframe::egui;
use egui::{Align, Id, Key, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};

use dialogs::{Dialog, Tool};
use panes::PanelUi;
use previews::Previews;
use theme::*;
pub use updates::{DesktopUpdate, UpdateCheck};

const COPY_PATH_SHORTCUT: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    Key::C,
);

#[derive(Clone)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    pub drive: bool,
    pub ejectable: bool,
}

pub trait Desktop {
    fn preview_cache_directory(&self) -> Option<PathBuf> {
        None
    }
    fn supports_updates(&self) -> bool {
        false
    }
    fn check_for_update(&mut self, _ctx: egui::Context) -> Result<UpdateCheck, String> {
        Err("Standalone updates are unavailable".into())
    }
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
    fn places_reader(&self) -> fn() -> Vec<Place> {
        Vec::new
    }
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
    ShowQueue,
    ClipboardText(String),
    Eject(PathBuf),
    CheckForUpdates,
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
    ui_font: UiFont,
    alternate_columns: [bool; 2],
    quick: Option<Side>,
    clipboard: Option<FileClipboard>,
    notice: Option<(String, Instant)>,
    focus_scroll: Option<Side>,
    hide_file_tooltips: bool,
    closing: bool,
    show_help: bool,
    about: Option<about::About>,
}

type DialogRead = Box<dyn FnOnce(&mut App, &egui::Context) + Send>;

struct StartupHome {
    read: Option<engine::read_pool::ReadPool<(), PathBuf>>,
    right_stamp: Option<(u64, u64)>,
}

pub struct App {
    pub(crate) engine: Engine,
    desktop: Box<dyn Desktop>,
    interface: Interface,
    dialog: Option<Dialog>,
    find_files: find_files::FindFiles,
    preferences: Option<settings::Preferences>,
    places: std::sync::Arc<Vec<Place>>,
    read_places: fn() -> Vec<Place>,
    places_cache: probes::ReadCache<(), Vec<Place>>,
    spaces: probes::ReadCache<PathBuf, (u64, u64)>,
    tree_folders: probes::ReadCache<PathBuf, Vec<PathBuf>>,
    dialog_reads: engine::read_pool::ReadPool<(), DialogRead>,
    dialog_revision: u64,
    startup_defaults: Option<(PathBuf, bool)>,
    startup_home: Option<StartupHome>,
    previews: Previews,
    update: Option<updates::UpdateDialog>,
    update_check: Option<UpdateCheck>,
}

impl App {
    pub fn new(
        context: &eframe::CreationContext<'_>,
        desktop: Box<dyn Desktop>,
        home: PathBuf,
        initial: [PathBuf; 2],
        fresh: bool,
    ) -> Self {
        let ui_font = context
            .storage
            .and_then(|storage| eframe::get_value(storage, "ui_font"))
            .unwrap_or_default();
        theme::install(&context.egui_ctx, ui_font);
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
        let startup_defaults = (
            initial[0].clone(),
            settings.panes[Side::Right.index()].tabs.is_empty(),
        );
        let preview_cache = desktop.preview_cache_directory();
        let ctx = context.egui_ctx.clone();
        let engine = Engine::new(home, initial, settings, move || ctx.request_repaint());
        Self {
            engine,
            places: Default::default(),
            read_places: desktop.places_reader(),
            desktop,
            interface: Interface {
                tree,
                alternate_columns,
                ui_font,
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
            find_files: find_files::FindFiles::new(context.storage),
            preferences: None,
            places_cache: probes::ReadCache::new(&context.egui_ctx, 1, Duration::from_secs(10), 1),
            spaces: probes::ReadCache::new(&context.egui_ctx, 2, Duration::from_secs(10), 32),
            tree_folders: probes::ReadCache::new(&context.egui_ctx, 2, Duration::from_secs(5), 192),
            dialog_reads: engine::read_pool::ReadPool::new(
                1,
                1,
                probes::repaint(&context.egui_ctx),
            ),
            dialog_revision: 0,
            startup_defaults: Some(startup_defaults),
            startup_home: None,
            previews: Previews::new(context.egui_ctx.clone(), preview_cache),
            update: None,
            update_check: None,
        }
    }

    pub fn open_initial_directories(
        &mut self,
        ctx: &egui::Context,
        locations: [Option<PathBuf>; 2],
    ) {
        let Some((cwd, default_right)) = self.startup_defaults.take() else {
            return;
        };
        let configured_home = std::mem::replace(&mut self.engine.home, cwd.clone());
        for (side, path) in Side::ALL.into_iter().zip(locations) {
            if let Some(path) = path {
                self.engine.request_initial_directory(side, path);
            }
        }
        let right_stamp = default_right.then(|| self.engine.tab(Side::Right).navigation_stamp());
        let read = engine::read_pool::ReadPool::new(1, 1, probes::repaint(ctx));
        read.submit((), move |_| {
            configured_home
                .canonicalize()
                .ok()
                .filter(|path| path.is_dir())
                .unwrap_or(cwd)
        });
        self.startup_home = Some(StartupHome {
            read: Some(read),
            right_stamp,
        });
    }

    fn poll_startup_home(&mut self) {
        let Some(startup) = &mut self.startup_home else {
            return;
        };
        if let Some(read) = &startup.read {
            let Some(((), home)) = read.drain().pop() else {
                return;
            };
            self.engine.home = home;
            startup.read = None;
        }
        let right = self.engine.tab(Side::Right);
        if startup.right_stamp == Some(right.navigation_stamp()) {
            if right.pending_navigation().is_some() {
                return;
            }
            if right.base() != self.engine.home {
                self.engine
                    .apply_initial_directory(Side::Right, self.engine.home.clone());
            }
        }
        self.startup_home = None;
    }

    fn poll_dialog_reads(&mut self, ctx: &egui::Context) {
        for (_, apply) in self.dialog_reads.drain() {
            apply(self, ctx);
        }
    }

    fn cancel_dialog_read(&mut self) {
        self.dialog_revision += 1;
        self.dialog_reads.retain(|()| false);
    }

    fn read_for_tab<R: Send + 'static>(
        &self,
        side: Side,
        read: impl FnOnce() -> R + Send + 'static,
        apply: impl FnOnce(&mut Self, &egui::Context, R) + Send + 'static,
    ) {
        let revision = self.dialog_revision;
        let origin = self.engine.tab(side).navigation_stamp();
        self.dialog_reads.submit((), move |_| {
            let result = read();
            Box::new(move |app, ctx| {
                if app.dialog_revision == revision
                    && app.engine.tab(side).navigation_stamp() == origin
                {
                    apply(app, ctx, result);
                }
            })
        });
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Command(command) => {
                if let Command::Step(side, ..) | Command::Find(side, ..) = command {
                    self.interface.focus_scroll = Some(side);
                    ctx.request_repaint();
                }
                self.engine.dispatch(command);
            }
            Action::Tool(side, tool) => self.invoke(side, tool, ctx),
            Action::Open(side, entry, background) => {
                self.engine.keep_provisional_focus(side);
                if entry.archive.is_some() && !entry.is_dir() {
                    self.interface.quick = Some(side);
                } else if entry.is_dir() || entry.kind == engine::Kind::Archive {
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
                if let Some(archive) = destination
                    .ancestors()
                    .find(|path| path.is_file() && engine::is_archive(path))
                {
                    if moving {
                        self.notice("Use Copy to add files to an archive");
                    } else {
                        let folder = destination
                            .strip_prefix(archive)
                            .unwrap_or_else(|_| Path::new(""));
                        self.dialog = Some(Dialog::pack(side, paths, archive, folder));
                    }
                } else {
                    self.dialog =
                        Some(Dialog::transfer(side, paths, destination, moving).with_target(side));
                }
            }
            Action::Path(side, path) => self.engine.dispatch(Command::Navigate {
                side,
                path,
                new_tab: false,
                background: false,
            }),
            Action::ShowQueue => self.interface.queue = true,
            Action::ToggleQuick(side) => {
                self.interface.quick = if self.interface.quick == Some(side) {
                    None
                } else {
                    self.engine.keep_provisional_focus(side);
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
            Action::CheckForUpdates => self.check_for_updates(ctx),
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
        toolbar.menu_button("File", |ui| dialogs::file_menu(ui, side, actions));
        toolbar.menu_button("Select", |ui| dialogs::selection_menu(ui, side, actions));
        toolbar.menu_button("View", |ui| self.view_menu(ui, side, actions));
        toolbar.menu_button("Tools", |ui| {
            let bottom_margin = egui::Frame::menu(ui.style()).total_margin().bottom;
            let available_height =
                ui.ctx().content_rect().bottom() - ui.cursor().top() - bottom_margin;
            egui::ScrollArea::vertical()
                .max_height(available_height.max(1.))
                .show(ui, |ui| dialogs::tools_menu(ui, side, actions));
        });
        toolbar.menu_button("Help", |ui| {
            if self.desktop.supports_updates() && ui.button("Check for Updates…").clicked() {
                actions.push(Action::CheckForUpdates);
                ui.close();
            }
            if ui.button("Keyboard shortcuts · F1").clicked() {
                self.interface.show_help = true;
                ui.close();
            }
            if ui.button("About DeshCmd").clicked() {
                self.interface.about = Some(about::About::new());
                ui.close();
            }
        });
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
            .selectable_label(self.engine.settings.show_hidden, "Hidden files · Ctrl+.")
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
                        "Bookmark here",
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
                    panes::tree(ui, side, &root, current, 0, &mut self.tree_folders, actions);
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

    fn operation_target(&self, path: &Path, preferred: Side) -> Side {
        [preferred, preferred.other()]
            .into_iter()
            .filter_map(|side| {
                let location = self.engine.tab(side).location();
                let location = match location {
                    engine::Location::Archive { file, .. } if path == file => file.clone(),
                    _ => location.display_path(),
                };
                path.starts_with(&location)
                    .then(|| (side, location.components().count()))
            })
            .reduce(|best, candidate| {
                if candidate.1 > best.1 {
                    candidate
                } else {
                    best
                }
            })
            .map_or(preferred, |(side, _)| side)
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
                ui.spacing_mut().item_spacing.y = 3.;
                ui.spacing_mut().interact_size.y = 18.;
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
                        if let Some((done, total)) = job.progress.overall() {
                            let shown = if job.state == JobState::Done {
                                total
                            } else {
                                done
                            };
                            ui.add(operation_progress(shown, total, 85.))
                                .on_hover_text("Overall operation progress");
                        }
                        if let Some(start) = job.started {
                            let elapsed = job
                                .finished_at
                                .unwrap_or_else(Instant::now)
                                .saturating_duration_since(start);
                            ui.label(mono(clock(elapsed.as_secs()), 10., MUTED))
                                .on_hover_text("Elapsed time");
                            if job.state == JobState::Running
                                && let Some((done, total)) = job.progress.overall()
                                && done > 0
                            {
                                let rate = done as f64 / elapsed.as_secs_f64().max(0.1);
                                if job.progress.total.is_some_and(|bytes| bytes > 0) {
                                    ui.label(mono(
                                        format!("{}/s", metric(rate as u64)),
                                        10.,
                                        MUTED,
                                    ));
                                }
                                let eta = ((total - done) as f64 / rate).ceil() as u64;
                                ui.label(mono(clock(eta), 10., MUTED))
                                    .on_hover_text("Estimated time remaining");
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
                        if !job.paths.items.is_empty()
                            || job.paths.destination.is_some()
                            || !job.message.is_empty()
                            || !job.warnings.is_empty()
                        {
                            let details = ui.small_button("Details").on_hover_ui(|ui| {
                                if job.message.is_empty() && job.warnings.is_empty() {
                                    ui.label("Show operation details");
                                }
                                if !job.message.is_empty() {
                                    ui.label(&job.message);
                                }
                                for warning in &job.warnings {
                                    ui.label(warning);
                                }
                            });
                            if details.clicked() {
                                actions.push(Action::Tool(
                                    self.engine.settings.active,
                                    Tool::JobDetails(job.id),
                                ));
                            }
                        }
                        if let Some(result) = job.message.lines().next() {
                            ui.add(egui::Label::new(label(result, 12., SUB)).truncate())
                                .on_hover_text(result);
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
                    queue_paths(ui, &job.paths);
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
            let copy_paths = input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { key, modifiers, pressed: true, .. }
                        if *key == COPY_PATH_SHORTCUT.logical_key
                            && modifiers.matches_exact(COPY_PATH_SHORTCUT.modifiers)
                )
            });
            (
                input
                    .consume_key(egui::Modifiers::COMMAND, Key::C)
                    .then_some(if copy_paths {
                        Tool::CopyPaths
                    } else {
                        Tool::CopyClipboard
                    }),
                input.consume_key(egui::Modifiers::COMMAND, Key::X),
                input.consume_key(egui::Modifiers::COMMAND, Key::V),
            )
        });
        if !(copy.is_some() || cut || paste) {
            return;
        }
        let text_context = self.text_field_focused(ctx)
            || self.dialog.is_some()
            || self.find_files.open
            || self.update.is_some()
            || self.preferences.is_some()
            || self.interface.show_help
            || self.interface.about.is_some()
            || self.engine.jobs.iter().any(|job| job.conflict.is_some());
        if text_context {
            let event = if copy.is_some() {
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
            let tool = if let Some(tool) = copy {
                tool
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
            || self.find_files.open
            || self.update.is_some()
            || self.preferences.is_some()
            || self.interface.show_help
            || self.interface.about.is_some()
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
        if exact(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::F)
            || exact(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::F)
        {
            actions.push(Action::Tool(side, Tool::FindFiles));
            return;
        }
        if exact(egui::Modifiers::CTRL, Key::F) || exact(egui::Modifiers::COMMAND, Key::F) {
            if self.interface.quick == Some(side.other()) {
                self.interface.quick = None;
            }
            self.interface.panes[side.index()].focus_filter();
            return;
        }
        if ctx.memory(|memory| memory.has_focus(panes::filter_id(side))) && just(Key::Escape) {
            self.interface.panes[side.index()].close_filter();
            ctx.memory_mut(|memory| memory.surrender_focus(panes::filter_id(side)));
            actions.push(Action::Command(Command::Filter(
                side,
                engine::MatchOptions::filename(""),
            )));
            return;
        }
        if editing {
            return;
        }

        if just(Key::Tab) {
            actions.push(Action::Command(Command::Focus(side.other())));
        }
        let pane = self.engine.pane(side);
        let tab_count = pane.tabs.len();
        for (modifiers, offset) in [
            (egui::Modifiers::CTRL, 1),
            (
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                tab_count - 1,
            ),
        ] {
            if exact(modifiers, Key::Tab) && tab_count > 1 {
                let index = (pane.active + offset) % tab_count;
                actions.push(Action::Command(Command::Tab(side, index)));
            }
        }
        let initial_press = |modifiers, key| {
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
        if initial_press(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::T) {
            let index = self.engine.pane(side).active;
            actions.push(Action::Command(Command::SendTab(side, index, false)));
        }
        if command(Key::T) {
            actions.push(Action::Command(Command::NewTab(side)));
        }
        if initial_press(egui::Modifiers::CTRL, Key::ArrowUp) {
            actions.push(Action::Command(Command::OpenFocusedDirectoryInTab {
                source: side,
                destination: side,
            }));
        }
        if initial_press(egui::Modifiers::CTRL, Key::ArrowRight) && side == Side::Left {
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
            if self.engine.tab(side).pending_navigation().is_some() {
                actions.push(Action::Command(Command::CancelNavigation(side)));
                return;
            }
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
        if initial_press(egui::Modifiers::CTRL, Key::Period) {
            actions.push(Action::Command(Command::ToggleHidden));
        }
        if initial_press(egui::Modifiers::SHIFT, Key::F4) {
            actions.push(Action::Tool(side, Tool::NewFile));
        }
        if shift(Key::F5) {
            actions.push(Action::Tool(side, Tool::CopyHere));
        }
        for (modifiers, key, tool) in [
            (egui::Modifiers::SHIFT, Key::Delete, Tool::Delete),
            (egui::Modifiers::NONE, Key::Delete, Tool::Trash),
            (egui::Modifiers::NONE, Key::F8, Tool::Trash),
        ] {
            if initial_press(modifiers, key) {
                actions.push(Action::Tool(side, tool));
            }
        }
        for (key, tool) in [
            (Key::F2, Tool::Rename),
            (Key::F5, Tool::Copy),
            (Key::F6, Tool::Move),
            (Key::F7, Tool::NewFolder),
        ] {
            if just(key) {
                actions.push(Action::Tool(side, tool));
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let copy_path = ctx.format_shortcut(&COPY_PATH_SHORTCUT);
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
                            ("Ctrl + Tab", "Switch to next tab"),
                            ("Ctrl + Shift + Tab", "Switch to previous tab"),
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
                            ("Ctrl + F", "Filter the active panel"),
                            ("Ctrl + Shift + F", "Find files"),
                            ("Ctrl + R", "Refresh the active panel"),
                            ("Ctrl + .", "Toggle hidden files"),
                            ("F2", "Rename"),
                            ("Ctrl + Q / F3", "Toggle Quick View"),
                            ("Shift + F4", "Create text file"),
                            ("F5 / F6", "Copy or move to opposite panel"),
                            ("Shift + F5", "Copy in same folder"),
                            ("F7 / F8", "Create folder or move to Trash"),
                            ("F9", "Open terminal in active folder"),
                            ("Shift + Delete", "Delete permanently"),
                            ("Ctrl + C / X / V", "Copy, cut, or paste files"),
                            (&copy_path, "Copy full paths"),
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
        } else if self.interface.ui_font == UiFont::DejaVuSansMono {
            176.
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

        let source = self.interface.quick.unwrap_or(self.engine.settings.active);
        let source_tab = self.engine.tab(source);
        let page = self.interface.panes[source.index()].preview_page(source_tab);
        self.previews
            .update(source_tab, self.interface.quick.is_some(), page);
        for (side, rect) in Side::ALL.into_iter().zip([panel1, panel2]) {
            let quick = self.interface.quick.filter(|source| source.other() == side);
            let cursor_restored = quick.is_none() && self.engine.take_cursor_restored(side);
            let pane = self.engine.pane(side);
            let root = pane.tab().base();
            let space = self
                .spaces
                .request(root.to_owned(), |path| engine::space(path))
                .as_deref()
                .copied();
            let column_set = if self.interface.alternate_columns[side.index()] {
                &self.engine.settings.alternate_columns
            } else {
                &self.engine.settings.columns
            };
            let pending_jobs = if self.interface.queue && queue_height > queue_gap {
                Vec::new()
            } else {
                self.engine
                    .jobs
                    .iter()
                    .filter(|job| job.target == side && !job.state.finished())
                    .collect::<Vec<_>>()
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
                double_click_new_tab: self.engine.settings.double_click_new_tab,
                show_file_tooltips: !self.interface.hide_file_tooltips,
                reveal_cursor: self.interface.focus_scroll == Some(side) || cursor_restored,
                history_preview: self.engine.history_preview(side),
                pending_jobs: &pending_jobs,
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
        self.previews.finish_frame();
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
                destination: self.engine.tab(side).location().display_path(),
                moving: false,
            });
        }
        #[cfg(not(target_os = "macos"))]
        window::resize_edges(ui, root);
    }
}

fn queue_paths(ui: &mut Ui, paths: &engine::JobPaths) {
    let item = paths.items.first();
    let destination = paths.destination.as_deref();
    if item.is_none() && destination.is_none() {
        return;
    }
    ui.horizontal(|ui| {
        let path_label = |ui: &mut Ui, path: &Path, more: usize, width: f32| {
            let mut summary = engine::display_name(path);
            if more > 0 {
                summary.push_str(&format!(" +{more}"));
            }
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                summary.push_str(&format!(" · {}", parent.display()));
            }
            let width = width.max(0.);
            ui.allocate_ui_with_layout(
                vec2(width, 17.),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_width(width);
                    ui.add(egui::Label::new(label(summary, 12., SUB)).truncate())
                        .on_hover_text(path.display().to_string());
                },
            );
        };
        if let Some(item) = item {
            let width = ui.available_width();
            let width = if destination.is_some() {
                (width - 28.) * 0.5
            } else {
                width
            };
            path_label(ui, item, paths.items.len() - 1, width);
        }
        if let Some(destination) = destination {
            if item.is_some() {
                ui.label(label("To", 12., MUTED));
            }
            path_label(ui, destination, 0, ui.available_width());
        }
    });
}

fn clock(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn operation_progress(done: u64, total: u64, width: f32) -> egui::ProgressBar {
    egui::ProgressBar::new(done as f32 / total as f32)
        .desired_width(width)
        .show_percentage()
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.engine.poll();
        if let Some(delay) = self.engine.listing_wake_after() {
            ctx.request_repaint_after(delay);
        }
        self.poll_startup_home();
        self.poll_update();
        self.record_notice();
        self.places_cache.poll();
        let read_places = self.read_places;
        if let Some(places) = self.places_cache.request((), move |()| Some(read_places())) {
            self.places = places;
        }
        self.spaces.poll();
        self.tree_folders.poll();
        self.poll_dialog_reads(ctx);
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
        self.tree_folders
            .begin_render_pass(ctx.cumulative_pass_nr());
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
        ctx.input(|input| {
            if input.pointer.delta() != egui::Vec2::ZERO || input.pointer.any_pressed() {
                self.interface.hide_file_tooltips = false;
            }
            if input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: Key::ArrowDown
                            | Key::ArrowUp
                            | Key::ArrowLeft
                            | Key::ArrowRight
                            | Key::PageUp
                            | Key::PageDown
                            | Key::Home
                            | Key::End
                            | Key::Tab
                            | Key::Enter
                            | Key::Backspace,
                        pressed: true,
                        ..
                    }
                )
            }) {
                self.interface.hide_file_tooltips = true;
            }
        });
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
            self.show_find_files(&ctx);
            self.dialogs(&ctx);
        }
        if self.interface.show_help {
            self.shortcuts(&ctx);
        }
        if let Some(about) = &mut self.interface.about
            && !about.show(&ctx)
        {
            self.interface.about = None;
        }
        self.update_dialog(&ctx);
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
        eframe::set_value(storage, "find_templates", &self.find_files.templates);
        eframe::set_value(
            storage,
            "show_inline_filter",
            &self.interface.show_inline_filter,
        );
        eframe::set_value(storage, "show_places", &self.interface.show_places);
        eframe::set_value(storage, "show_bottom_bar", &self.interface.show_bottom_bar);
        eframe::set_value(storage, "ui_font", &self.interface.ui_font);
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

#[cfg(all(test, feature = "perf"))]
mod performance;

#[cfg(test)]
mod tests;
