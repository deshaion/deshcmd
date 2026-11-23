use std::path::{Path, PathBuf};

use deshcmd_engine::{
    self as engine, Column, Command, Entry, Location, Pane, Select, Side, Tab, TabLock, View,
};
use eframe::egui;
use egui::{
    Align, Align2, Color32, FontId, Id, Layout, Rect, Response, Sense, Stroke, Ui, pos2, vec2,
};

use crate::dialogs::{self, Tool};
use crate::{Action, CachedPreview, Previews, child, theme::*};

// Tabs, navigation, and room for the details header and one row.
pub(crate) const MIN_HEIGHT: f32 = 142.;

#[derive(Default)]
pub(crate) struct PanelUi {
    tab_id: u64,
    base: PathBuf,
    path_draft: String,
    filter: String,
    editing_path: bool,
    request_path: bool,
    active_tab_in_strip: Option<(usize, u64, String, f32)>,
}
pub(crate) struct PaneScreen<'a> {
    pub pane: &'a Pane,
    pub side: Side,
    pub active: bool,
    pub home: &'a Path,
    pub space: Option<(u64, u64)>,
    pub columns: &'a [Column],
    pub reveal_cursor: bool,
    pub preview: Option<(Side, Option<&'a Entry>)>,
}

#[derive(Clone)]
struct Drag(Vec<PathBuf>);

pub(crate) fn filter_id(side: Side) -> Id {
    Id::unique(("filter", side.index()))
}
pub(crate) fn path_id(side: Side) -> Id {
    Id::unique(("path", side.index()))
}

impl PanelUi {
    pub fn set_filter_text(&mut self, text: &str) {
        self.filter = text.to_owned();
    }

    pub fn start_path(&mut self) {
        self.editing_path = true;
        self.request_path = true;
    }

    pub fn show(
        &mut self,
        root: &mut Ui,
        rect: Rect,
        screen: PaneScreen<'_>,
        previews: &mut Previews,
        actions: &mut Vec<Action>,
    ) {
        let PaneScreen {
            pane,
            side,
            active,
            home,
            space,
            columns,
            reveal_cursor,
            preview,
        } = screen;
        let tab = pane.tab();
        root.painter().rect(
            rect,
            10.,
            SURFACE,
            Stroke::new(
                1.,
                if active {
                    Color32::from_rgb(92, 134, 96)
                } else {
                    LINE
                },
            ),
            egui::StrokeKind::Inside,
        );
        if let Some((source, entry)) = preview {
            show_preview(root, rect, side, source, entry, previews, actions);
            return;
        }
        if self.tab_id != tab.id || self.base != tab.base() {
            self.tab_id = tab.id;
            self.base = tab.base().to_path_buf();
            self.path_draft = tab.base().display().to_string();
            self.filter.clone_from(&tab.filter.text);
        }
        let short = rect.height() < 225.;
        let tabs = Rect::from_min_max(
            rect.min + vec2(9., 5.),
            pos2(rect.right() - 8., rect.top() + 38.),
        );
        self.tabs(root, tabs, pane, side, active, actions);

        let nav = Rect::from_min_max(
            pos2(rect.left() + 8., tabs.bottom() + 2.),
            pos2(rect.right() - 8., tabs.bottom() + 34.),
        );
        self.navigation(root, nav, tab, side, home, actions);
        let mut body_top = nav.bottom() + 4.;
        if !short {
            let tools = Rect::from_min_max(
                pos2(rect.left() + 12., body_top),
                pos2(rect.right() - 11., body_top + 33.),
            );
            self.tools(root, tools, tab, side, actions);
            body_top = tools.bottom() + 4.;
        }
        let footer_height = 31.;
        let show_status = rect.height() >= MIN_HEIGHT + footer_height;
        let body_bottom = if show_status {
            rect.bottom() - footer_height
        } else {
            rect.bottom() - 1.
        };
        let body = Rect::from_min_max(
            pos2(rect.left() + 2., body_top.min(body_bottom)),
            pos2(rect.right() - 2., body_bottom),
        );
        let drop_area = root.interact(
            rect,
            Id::unique(("drop-pane", side.index())),
            Sense::hover(),
        );
        if let Some(payload) = drop_area.dnd_hover_payload::<Drag>()
            && !payload
                .0
                .iter()
                .all(|path| path.parent() == Some(tab.base()))
        {
            root.painter().rect_stroke(
                rect.shrink(3.),
                8.,
                Stroke::new(1.5, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        let mut contents = child(
            root,
            Id::unique(("file-list", side.index(), tab.id)),
            body,
            Layout::top_down(Align::Min),
        );
        contents.spacing_mut().item_spacing = vec2(0., 0.);
        match tab.saved.view {
            View::Gallery => gallery(&mut contents, tab, side, previews, actions),
            View::Compact => rows(
                &mut contents,
                tab,
                side,
                &[Column::Name],
                reveal_cursor,
                false,
                actions,
            ),
            View::Details => rows(
                &mut contents,
                tab,
                side,
                columns,
                reveal_cursor,
                true,
                actions,
            ),
        }
        if let Some(payload) = drop_area.dnd_release_payload::<Drag>() {
            let moving = root.input(|input| input.modifiers.shift);
            if !payload
                .0
                .iter()
                .all(|path| path.parent() == Some(tab.base()))
            {
                actions.push(Action::Drop {
                    side,
                    paths: payload.0.clone(),
                    destination: tab.base().to_path_buf(),
                    moving,
                });
            }
        }
        if root.input(|input| input.pointer.button_clicked(egui::PointerButton::Primary))
            && root
                .ctx()
                .pointer_latest_pos()
                .is_some_and(|pos| rect.contains(pos))
            && !active
        {
            actions.insert(0, Action::Command(Command::Focus(side)));
        }
        if show_status {
            let footer = Rect::from_min_max(
                pos2(rect.left() + 1., body_bottom),
                rect.right_bottom() - vec2(1., 1.),
            );
            panel_status(root, footer, side, tab, space);
        }
    }

    fn tabs(
        &mut self,
        root: &mut Ui,
        rect: Rect,
        pane: &Pane,
        side: Side,
        active: bool,
        actions: &mut Vec<Action>,
    ) {
        let mut ui = child(
            root,
            Id::unique(("tabs", side.index())),
            rect,
            Layout::left_to_right(Align::Center),
        );
        ui.spacing_mut().item_spacing.x = 4.;
        let add = Rect::from_center_size(pos2(rect.right() - 15., rect.center().y), vec2(29., 30.));
        let mut button = child(
            &mut ui,
            Id::unique("new-tab"),
            add,
            Layout::left_to_right(Align::Center),
        );
        if icon_button(&mut button, Icon::Plus, "New tab · Ctrl+T", true).clicked() {
            actions.push(Action::Command(Command::NewTab(side)));
        }
        let area = Rect::from_min_max(rect.min, pos2(add.left() - 5., rect.bottom()));
        let mut list = child(
            &mut ui,
            Id::unique("strip"),
            area,
            Layout::left_to_right(Align::Center),
        );
        let active_tab = pane.tab();
        let active_label = (pane.active, active_tab.id, active_tab.title(), area.width());
        let reveal_active = self.active_tab_in_strip.as_ref() != Some(&active_label);
        self.active_tab_in_strip = Some(active_label);
        egui::ScrollArea::horizontal()
            .max_width(area.width())
            .show(&mut list, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.;
                    let mut centers = Vec::with_capacity(pane.tabs.len());
                    let mut reorder = None;
                    for (index, tab) in pane.tabs.iter().enumerate() {
                        let current = index == pane.active;
                        let title = tab.title();
                        let ink = if current && active {
                            ACCENT
                        } else if current {
                            TEXT
                        } else {
                            SUB
                        };
                        let label_left = 27.;
                        let label_right =
                            if pane.tabs.len() > 1 || tab.saved.lock != TabLock::Normal {
                                22.
                            } else {
                                10.
                            };
                        let padding = label_left + label_right;
                        let mut job = egui::text::LayoutJob::simple_singleline(
                            title.clone(),
                            FontId::proportional(13.),
                            ink,
                        );
                        job.wrap.max_width = (area.width().floor() - padding).max(0.);
                        job.wrap.max_rows = 1;
                        job.wrap.break_anywhere = true;
                        let galley = ui.painter().layout_job(job);
                        let elided = galley.elided;
                        let width = galley.size().x.ceil() + padding;
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(width, 29.), Sense::click_and_drag());
                        centers.push(rect.center().x);
                        if current && reveal_active {
                            response.scroll_to_me(None);
                        }
                        if current || response.hovered() {
                            ui.painter().rect_filled(
                                rect,
                                6.,
                                if current && active {
                                    ACCENT_DARK
                                } else {
                                    RAISED
                                },
                            );
                        }
                        Icon::Folder.draw(ui, pos2(rect.left() + 14., rect.center().y), 14., ink);
                        ui.painter().galley(
                            pos2(
                                rect.left() + label_left,
                                rect.center().y - galley.size().y / 2.,
                            ),
                            galley,
                            ink,
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                true,
                                format!("{} tab", title),
                            )
                        });
                        let close = Rect::from_center_size(
                            pos2(rect.right() - 11., rect.center().y),
                            vec2(17., 20.),
                        );
                        let close_response = ui.interact(
                            close,
                            ui.scope_id().with(("close", tab.id)),
                            Sense::click(),
                        );
                        if tab.saved.lock != TabLock::Normal {
                            Icon::Link.draw(
                                ui,
                                close.center(),
                                10.,
                                if close_response.hovered() {
                                    ACCENT
                                } else {
                                    MUTED
                                },
                            );
                        } else if pane.tabs.len() > 1
                            && (current || response.hovered() || close_response.hovered())
                        {
                            Icon::Close.draw(
                                ui,
                                close.center(),
                                10.,
                                if close_response.hovered() { RED } else { MUTED },
                            );
                        }
                        if pane.tabs.len() > 1
                            && (close_response.clicked() || response.middle_clicked())
                        {
                            actions.push(Action::Command(Command::CloseTab(side, index)));
                        } else if response.clicked() {
                            actions.push(Action::Command(Command::Tab(side, index)));
                        }
                        if response.drag_stopped()
                            && let Some(pos) = ui.input(|input| input.pointer.latest_pos())
                            && area.contains(pos)
                        {
                            reorder = Some((index, pos.x));
                        }
                        if let Some(payload) = response.dnd_release_payload::<Drag>() {
                            let moving = ui.input(|input| input.modifiers.shift);
                            actions.push(Action::Drop {
                                side,
                                paths: payload.0.clone(),
                                destination: tab.base().to_path_buf(),
                                moving,
                            });
                        }
                        response
                            .on_hover_ui(|ui| {
                                if elided {
                                    ui.label(&title);
                                }
                                ui.label(tab.base().display().to_string());
                            })
                            .context_menu(|ui| {
                                if ui.button("Rename tab").clicked() {
                                    actions.push(Action::Tool(side, Tool::RenameTab(index)));
                                    ui.close();
                                }
                                ui.menu_button("Tab lock", |ui| {
                                    for (label, lock) in [
                                        ("Normal", TabLock::Normal),
                                        ("Lock path", TabLock::Locked),
                                        ("Reset when leaving tab", TabLock::Reset),
                                        ("Open folders in new tabs", TabLock::NewTab),
                                    ] {
                                        if ui
                                            .selectable_label(tab.saved.lock == lock, label)
                                            .clicked()
                                        {
                                            actions.push(Action::Command(Command::LockTab(
                                                side, index, lock,
                                            )));
                                            ui.close();
                                        }
                                    }
                                });
                                if ui.button("Move tab to other panel").clicked() {
                                    actions
                                        .push(Action::Command(Command::SendTab(side, index, true)));
                                    ui.close();
                                }
                                if ui.button("Copy tab to other panel").clicked() {
                                    actions.push(Action::Command(Command::SendTab(
                                        side, index, false,
                                    )));
                                    ui.close();
                                }
                                if ui.button("Close duplicate tabs").clicked() {
                                    actions.push(Action::Command(Command::CloseDuplicates(side)));
                                    ui.close();
                                }
                                if ui
                                    .add_enabled(
                                        pane.tabs.len() > 1,
                                        egui::Button::new("Close tab"),
                                    )
                                    .clicked()
                                {
                                    actions.push(Action::Command(Command::CloseTab(side, index)));
                                    ui.close();
                                }
                            });
                    }
                    if let Some((from, x)) = reorder {
                        let to = centers
                            .iter()
                            .enumerate()
                            .filter(|&(index, center)| index != from && *center < x)
                            .count();
                        if to != from {
                            actions.push(Action::Command(Command::ReorderTab(side, from, to)));
                        }
                    }
                });
            });
    }

    fn navigation(
        &mut self,
        root: &mut Ui,
        rect: Rect,
        tab: &Tab,
        side: Side,
        home: &Path,
        actions: &mut Vec<Action>,
    ) {
        let mut ui = child(
            root,
            Id::unique(("nav", side.index())),
            rect,
            Layout::left_to_right(Align::Center),
        );
        ui.spacing_mut().item_spacing.x = 1.;
        let back = icon_button(&mut ui, Icon::Back, "Back · Alt+Left", tab.can_back());
        if back.clicked() {
            actions.push(Action::Command(Command::Back(side)));
        }
        back.context_menu(|ui| {
            ui.label(label("Directory history", 12., MUTED));
            egui::ScrollArea::vertical()
                .max_height(250.)
                .show(ui, |ui| {
                    for (index, location) in tab.history.iter().enumerate().rev() {
                        if ui.button(location.base().display().to_string()).clicked() {
                            actions.push(Action::Command(Command::History(side, index)));
                            ui.close();
                        }
                    }
                });
        });
        if icon_button(
            &mut ui,
            Icon::Next,
            "Forward · Alt+Right",
            tab.can_forward(),
        )
        .clicked()
        {
            actions.push(Action::Command(Command::Forward(side)));
        }
        if icon_button(
            &mut ui,
            Icon::Up,
            "Parent folder · Alt+Up",
            tab.base().parent().is_some() || !matches!(tab.location(), Location::Directory(_)),
        )
        .clicked()
        {
            actions.push(Action::Command(Command::Parent(side)));
        }
        ui.add_space(4.);
        let width = (ui.available_width() - 32.).max(40.);
        let (path_rect, path_response) = ui.allocate_exact_size(vec2(width, 29.), Sense::click());
        ui.painter().rect(
            path_rect,
            6.,
            BASE,
            Stroke::new(1., LINE),
            egui::StrokeKind::Inside,
        );
        let path_ui_rect = path_rect.shrink2(vec2(5., 1.));
        let mut path_ui = child(
            &mut ui,
            Id::unique("breadcrumb"),
            path_ui_rect,
            Layout::left_to_right(Align::Center),
        );
        path_ui.spacing_mut().item_spacing.x = 1.;
        if self.editing_path {
            let edit = path_ui.add_sized(
                path_ui_rect.size(),
                egui::TextEdit::singleline(&mut self.path_draft)
                    .id(path_id(side))
                    .frame(egui::Frame::NONE)
                    .font(egui::TextStyle::Monospace),
            );
            if self.request_path {
                edit.request_focus();
                self.request_path = false;
            }
            if (edit.has_focus() || edit.lost_focus())
                && root.input(|input| input.key_pressed(egui::Key::Enter))
            {
                if !self.path_draft.trim().is_empty() {
                    actions.push(Action::Path(side, PathBuf::from(self.path_draft.trim())));
                }
                self.editing_path = false;
            }
            if root.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.path_draft = tab.base().display().to_string();
                self.editing_path = false;
            }
        } else {
            let crumbs = crumbs(tab.base(), home);
            let label_width = |text: &str| {
                path_ui
                    .painter()
                    .layout_no_wrap(text.to_owned(), FontId::proportional(12.), TEXT)
                    .size()
                    .x
                    + 19.
            };
            let mut total = 0.;
            let mut start = crumbs.len();
            for (text, _) in crumbs.iter().rev() {
                total += label_width(text);
                if total > width - 8. && start < crumbs.len() {
                    break;
                }
                start -= 1;
            }
            if start != 0
                && width > 108.
                && path_ui
                    .add(egui::Button::new(label("…", 13., MUTED)).frame(false))
                    .on_hover_text("Edit the full path")
                    .clicked()
            {
                self.start_path();
            }
            for (idx, (text, path)) in crumbs.iter().enumerate().skip(start) {
                if idx > start {
                    let (separator, _) =
                        path_ui.allocate_exact_size(vec2(11., 22.), Sense::hover());
                    Icon::Chevron.draw(&path_ui, separator.center(), 10., MUTED);
                }
                if path_ui
                    .add(
                        egui::Button::new(label(
                            text,
                            12.,
                            if idx == crumbs.len() - 1 { TEXT } else { SUB },
                        ))
                        .frame(false),
                    )
                    .clicked()
                {
                    if idx == crumbs.len() - 1 {
                        self.start_path();
                    } else {
                        actions.push(Action::Path(side, path.clone()));
                    }
                }
            }
            if path_response.clicked() {
                self.start_path();
            }
            path_response.on_hover_text(tab.base().display().to_string());
        }
        if icon_button(&mut ui, Icon::More, "Location and view options", true).clicked() {
            self.start_path();
        }
    }

    fn tools(
        &mut self,
        root: &mut Ui,
        rect: Rect,
        tab: &Tab,
        side: Side,
        actions: &mut Vec<Action>,
    ) {
        let mut ui = child(
            root,
            Id::unique(("panel-tools", side.index())),
            rect,
            Layout::left_to_right(Align::Center),
        );
        let width = (rect.width() - 107.).clamp(60., 450.);
        let (filter_rect, _) = ui.allocate_exact_size(vec2(width, 28.), Sense::hover());
        ui.painter().rect(
            filter_rect,
            6.,
            RAIL,
            Stroke::new(1., LINE),
            egui::StrokeKind::Inside,
        );
        Icon::Search.draw(
            &ui,
            pos2(filter_rect.left() + 13., filter_rect.center().y),
            13.,
            MUTED,
        );
        let field = Rect::from_min_max(
            filter_rect.min + vec2(22., 1.),
            filter_rect.max - vec2(if self.filter.is_empty() { 5. } else { 23. }, 1.),
        );
        let mut filter = child(
            &mut ui,
            Id::unique("filter-field"),
            field,
            Layout::left_to_right(Align::Center),
        );
        let edit = filter.add_sized(
            field.size(),
            egui::TextEdit::singleline(&mut self.filter)
                .id(filter_id(side))
                .hint_text("Filter by name or mask")
                .frame(egui::Frame::NONE),
        );
        if edit.changed() {
            let mut criteria = tab.filter.clone();
            criteria.text.clone_from(&self.filter);
            actions.push(Action::Command(Command::Filter(side, criteria)));
        }
        if !self.filter.is_empty() {
            let clear = Rect::from_center_size(
                pos2(filter_rect.right() - 12., filter_rect.center().y),
                vec2(18., 25.),
            );
            let response = ui.interact(clear, ui.scope_id().with("clear-filter"), Sense::click());
            Icon::Close.draw(
                &ui,
                clear.center(),
                10.,
                if response.hovered() { ACCENT } else { MUTED },
            );
            if response.clicked() {
                self.filter.clear();
                let mut criteria = tab.filter.clone();
                criteria.text.clear();
                actions.push(Action::Command(Command::Filter(side, criteria)));
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 1.;
            for (icon, title, view) in [
                (Icon::Grid, "Gallery", View::Gallery),
                (Icon::List, "Compact list", View::Compact),
                (Icon::Columns, "Details", View::Details),
            ] {
                if action(ui, icon, "", title, true, tab.saved.view == view).clicked() {
                    actions.push(Action::Command(Command::View(side, view)));
                }
            }
        });
    }
}

fn crumbs(path: &Path, home: &Path) -> Vec<(String, PathBuf)> {
    if let Ok(suffix) = path.strip_prefix(home) {
        let mut result = vec![("Home".into(), home.to_path_buf())];
        let mut current = home.to_path_buf();
        for component in suffix.components() {
            current.push(component);
            result.push((
                component.as_os_str().to_string_lossy().into_owned(),
                current.clone(),
            ));
        }
        return result;
    }
    let mut current = PathBuf::new();
    let mut result = Vec::new();
    for component in path.components() {
        if matches!(component, std::path::Component::Prefix(_)) {
            current.push(component);
            continue;
        }
        current.push(component);
        let text = if matches!(component, std::path::Component::RootDir) {
            current.display().to_string()
        } else {
            component.as_os_str().to_string_lossy().into_owned()
        };
        result.push((text, current.clone()));
    }
    result
}

fn rows(
    ui: &mut Ui,
    tab: &Tab,
    side: Side,
    columns: &[Column],
    reveal: bool,
    details: bool,
    actions: &mut Vec<Action>,
) {
    let width = ui.available_width();
    let columns = column_widths(columns, width);
    if details {
        let (header, _) = ui.allocate_exact_size(vec2(width, 28.), Sense::hover());
        ui.painter().rect_filled(header, 0., RAISED);
        ui.painter()
            .hline(header.x_range(), header.bottom(), Stroke::new(1., LINE));
        let mut x = header.left();
        for &(column, width) in &columns {
            let rect = Rect::from_min_size(pos2(x, header.top()), vec2(width, header.height()));
            let response = ui
                .interact(
                    rect,
                    ui.scope_id().with(("column", column.label())),
                    Sense::click(),
                )
                .on_hover_text("Click to sort · Shift-click to add to sort");
            if response.clicked() {
                actions.push(Action::Command(Command::Sort(
                    side,
                    column,
                    ui.input(|input| input.modifiers.shift),
                )));
            }
            let sort = tab.saved.sort.iter().find(|item| item.column == column);
            let ink = if sort.is_some() || response.hovered() {
                ACCENT
            } else {
                MUTED
            };
            let name = ui
                .painter()
                .with_clip_rect(rect.intersect(ui.clip_rect()))
                .text(
                    pos2(rect.left() + 13., rect.center().y),
                    Align2::LEFT_CENTER,
                    column.label(),
                    FontId::proportional(11.5),
                    ink,
                );
            if let Some(sort) = sort
                && name.right() + 12. < rect.right()
            {
                let center = pos2(name.right() + 8., rect.center().y);
                let rise = if sort.descending { -2.5 } else { 2.5 };
                ui.painter().add(egui::Shape::line(
                    vec![
                        center + vec2(-2.8, rise),
                        center - vec2(0., rise),
                        center + vec2(2.8, rise),
                    ],
                    Stroke::new(1.1, ink),
                ));
            }
            x += width;
        }
    }
    if tab.visible.is_empty() {
        empty(ui, tab, side, actions);
        return;
    }
    let h = if details { 36. } else { 29. };
    let target = if reveal {
        tab.cursor
            .as_ref()
            .and_then(|cursor| tab.visible_entries().position(|row| row.path == *cursor))
    } else {
        None
    };
    let mut area = egui::ScrollArea::vertical()
        .id_salt("rows")
        .auto_shrink([false, false]);
    if let Some(index) = target {
        let id = ui.make_persistent_id(egui::Id::unique("rows"));
        let offset = egui::scroll_area::State::load(ui.ctx(), id)
            .map(|state| state.clamped_offset().y)
            .unwrap_or_default();
        let y = index as f32 * h;
        let height = ui.available_height();
        if y < offset {
            area = area.vertical_scroll_offset(y);
        }
        if y + h > offset + height {
            area = area.vertical_scroll_offset((y + h - height).max(0.));
        }
    }
    area.show_rows(ui, h, tab.visible.len(), |ui, range| {
        for index in range {
            let entry = &tab.entries[tab.visible[index]];
            let (rect, response) = ui.allocate_exact_size(vec2(width, h), Sense::click_and_drag());
            row_interaction(ui, &response, entry, tab, side, actions);
            let marked = tab.selected.contains(&entry.path);
            let focused = tab.cursor.as_ref() == Some(&entry.path);
            let background = if marked {
                ACCENT_DARK
            } else if response.hovered() {
                HOVER
            } else if index % 2 == 1 {
                Color32::from_rgb(20, 26, 32)
            } else {
                SURFACE
            };
            ui.painter().rect_filled(rect, 0., background);
            if marked {
                ui.painter().vline(
                    rect.left() + 1.,
                    rect.top() + 6. ..=rect.bottom() - 6.,
                    Stroke::new(2., ACCENT),
                );
            } else if focused {
                ui.painter().rect_stroke(
                    rect.shrink2(vec2(3., 2.)),
                    4.,
                    Stroke::new(1., LINE),
                    egui::StrokeKind::Inside,
                );
            }
            ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 0.5,
                Stroke::new(1., Color32::from_rgb(28, 35, 43)),
            );
            let (icon, ink) = icon_for(entry.kind);
            let mut x = rect.left();
            for &(column, width) in &columns {
                let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(width, h));
                let painter = ui
                    .painter()
                    .with_clip_rect(cell.shrink2(vec2(5., 0.)).intersect(ui.clip_rect()));
                if column == Column::Name {
                    let icon_rect =
                        Rect::from_center_size(pos2(x + 23., rect.center().y), vec2(25., 25.));
                    if details {
                        ui.painter()
                            .rect_filled(icon_rect, 6., ink.gamma_multiply(0.13));
                    }
                    icon.draw(ui, icon_rect.center(), 15., ink);
                    if entry.link {
                        Icon::Link.draw(ui, pos2(x + 33., rect.center().y + 7.), 8., SUB);
                    }
                    painter.text(
                        pos2(x + 41., rect.center().y),
                        Align2::LEFT_CENTER,
                        &entry.name,
                        FontId::proportional(13.),
                        if marked {
                            TEXT
                        } else if entry.hidden {
                            SUB
                        } else {
                            TEXT
                        },
                    );
                } else {
                    painter.text(
                        pos2(x + 13., rect.center().y),
                        Align2::LEFT_CENTER,
                        value(entry, column),
                        FontId::proportional(12.),
                        if marked { TEXT } else { SUB },
                    );
                }
                x += width;
            }
            folder_drop(ui, &response, entry, side, actions);
        }
    });
}

fn row_interaction(
    ui: &Ui,
    response: &Response,
    entry: &Entry,
    tab: &Tab,
    side: Side,
    actions: &mut Vec<Action>,
) {
    let marked = tab.selected.contains(&entry.path);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::ListItem, true, marked, &entry.name)
    });
    let (ctrl, shift) = ui.input(|input| (input.modifiers.command, input.modifiers.shift));
    if response.double_clicked() || response.middle_clicked() && entry.is_dir() {
        actions.push(Action::Open(
            side,
            entry.clone(),
            ctrl || response.middle_clicked(),
        ));
    } else if response.clicked() || response.secondary_clicked() && !marked {
        actions.push(Action::Command(Command::Select(
            side,
            entry.path.clone(),
            if shift {
                Select::Extend
            } else if ctrl {
                Select::Toggle
            } else {
                Select::Replace
            },
        )));
    }
    if response.drag_started() {
        let paths = if marked {
            tab.operands()
        } else {
            vec![entry.path.clone()]
        };
        response.dnd_set_drag_payload(Drag(paths));
    }
    let mut hint = format!("{}\n{}", entry.path.display(), entry.kind.label());
    if !entry.is_dir() {
        hint.push_str(&format!(" · {}", metric(entry.size)));
    }
    if !entry.comment.is_empty() {
        hint.push_str(&format!("\n{}", entry.comment));
    }
    response.clone().on_hover_text(hint).context_menu(|ui| {
        if ui.button("Open").clicked() {
            actions.push(Action::Open(side, entry.clone(), false));
            ui.close();
        }
        if entry.is_dir() && ui.button("Open in background tab").clicked() {
            actions.push(Action::Open(side, entry.clone(), true));
            ui.close();
        }
        if ui.button("Quick View").clicked() {
            actions.push(Action::ToggleQuick(side));
            ui.close();
        }
        ui.separator();
        dialogs::entry_menu(ui, side, actions);
    });
}

fn folder_drop(ui: &Ui, response: &Response, entry: &Entry, side: Side, actions: &mut Vec<Action>) {
    if !entry.is_dir() {
        return;
    }
    if response.dnd_hover_payload::<Drag>().is_some() {
        ui.painter().rect_stroke(
            response.rect.shrink(2.),
            4.,
            Stroke::new(1., ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    if let Some(payload) = response.dnd_release_payload::<Drag>() {
        actions.push(Action::Drop {
            side,
            paths: payload.0.clone(),
            destination: entry.path.clone(),
            moving: ui.input(|input| input.modifiers.shift),
        });
    }
}

fn gallery(ui: &mut Ui, tab: &Tab, side: Side, previews: &mut Previews, actions: &mut Vec<Action>) {
    if tab.visible.is_empty() {
        empty(ui, tab, side, actions);
        return;
    }
    let outer_width = ui.available_width();
    let columns = ((outer_width - 10.) / 123.).floor().max(1.) as usize;
    let width = ((outer_width - 12.) / columns as f32).max(40.);
    let height = 112.;
    egui::ScrollArea::vertical()
        .id_salt("gallery")
        .auto_shrink([false, false])
        .show_rows(
            ui,
            height,
            tab.visible.len().div_ceil(columns),
            |ui, range| {
                for row in range {
                    ui.horizontal(|ui| {
                        ui.add_space(5.);
                        for index in row * columns..(row * columns + columns).min(tab.visible.len())
                        {
                            let entry = &tab.entries[tab.visible[index]];
                            let (outer, response) = ui
                                .allocate_exact_size(vec2(width, height), Sense::click_and_drag());
                            row_interaction(ui, &response, entry, tab, side, actions);
                            let rect = outer.shrink(4.);
                            let marked = tab.selected.contains(&entry.path);
                            let (icon, ink) = icon_for(entry.kind);
                            card(
                                ui,
                                rect,
                                if marked {
                                    ACCENT_DARK
                                } else if response.hovered() {
                                    HOVER
                                } else {
                                    RAIL
                                },
                            );
                            let image = Rect::from_min_max(
                                rect.min + vec2(4., 4.),
                                pos2(rect.right() - 4., rect.bottom() - 28.),
                            );
                            if entry.kind == engine::Kind::Image
                                && let Some(CachedPreview::Image(texture, size)) =
                                    previews.get(entry, 176)
                            {
                                ui.painter().image(
                                    texture.id(),
                                    Rect::from_center_size(
                                        image.center(),
                                        contained_size(*size, image.size() - vec2(4., 4.)),
                                    ),
                                    Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)),
                                    Color32::WHITE,
                                );
                            } else {
                                icon.draw(ui, image.center(), 32., ink);
                            }
                            ui.painter()
                                .with_clip_rect(
                                    rect.shrink2(vec2(6., 0.)).intersect(ui.clip_rect()),
                                )
                                .text(
                                    pos2(rect.center().x, rect.bottom() - 14.),
                                    Align2::CENTER_CENTER,
                                    &entry.name,
                                    FontId::proportional(12.),
                                    if marked { ACCENT } else { TEXT },
                                );
                            folder_drop(ui, &response, entry, side, actions);
                        }
                    });
                }
            },
        );
}

fn empty(ui: &mut Ui, tab: &Tab, side: Side, actions: &mut Vec<Action>) {
    let filtering = !tab.filter.text.is_empty();
    let empty_message = if filtering {
        "No matching files"
    } else {
        "This location is empty"
    };
    if ui.available_height() < 45. {
        ui.vertical_centered(|ui| {
            if let Some(message) = tab.error.as_deref() {
                ui.label(label("Folder unavailable", 13., TEXT))
                    .on_hover_text(message);
            } else if tab.loading && tab.entries.is_empty() {
                ui.label(label("Loading files…", 13., SUB));
            } else {
                ui.label(label(empty_message, 13., TEXT));
            }
        });
        return;
    }
    let offset = (ui.available_height() * 0.28).min(86.);
    ui.add_space(offset);
    ui.vertical_centered(|ui| {
        if let Some(message) = tab.error.as_deref() {
            ui.label(label("Folder unavailable", 15., TEXT).strong());
            ui.label(label(message, 12., SUB));
            if ui.small_button("Retry").clicked() {
                actions.push(Action::Command(Command::Refresh));
            }
        } else if tab.loading && tab.entries.is_empty() {
            ui.spinner();
            ui.label(label("Loading files…", 13., SUB));
        } else {
            let (rect, _) = ui.allocate_exact_size(vec2(40., 38.), Sense::hover());
            ui.painter().rect_filled(rect, 9., RAISED);
            if filtering {
                Icon::Search
            } else {
                Icon::Folder
            }
            .draw(ui, rect.center(), 21., if filtering { BLUE } else { AMBER });
            ui.add_space(7.);
            ui.label(label(empty_message, 15., TEXT).strong());
            ui.add_space(4.);
            if !filtering
                && matches!(tab.location(), Location::Directory(_))
                && ui.button("New folder").clicked()
            {
                actions.push(Action::Tool(side, Tool::NewFolder));
            }
        }
    });
}

fn column_widths(columns: &[Column], width: f32) -> Vec<(Column, f32)> {
    let mut chosen = vec![Column::Name];
    for column in columns {
        if *column != Column::Name && !chosen.contains(column) && chosen.len() < 5 {
            chosen.push(*column);
        }
    }
    let estimate = |column| match column {
        Column::Size => 79.,
        Column::Modified => 116.,
        Column::Kind => 87.,
        Column::Extension => 88.,
        Column::Attributes => 90.,
        Column::Path | Column::Comment => 140.,
        _ => 110.,
    };
    while chosen.len() > 1
        && chosen.iter().map(|column| estimate(*column)).sum::<f32>() > width + 30.
    {
        chosen.pop();
    }
    let used: f32 = chosen.iter().skip(1).map(|column| estimate(*column)).sum();
    chosen
        .into_iter()
        .map(|column| {
            (
                column,
                if column == Column::Name {
                    width - used
                } else {
                    estimate(column)
                },
            )
        })
        .collect()
}

pub(crate) fn value(entry: &Entry, column: Column) -> String {
    match column {
        Column::Name => entry.name.clone(),
        Column::Size => {
            if entry.is_dir() {
                "—".into()
            } else {
                metric(entry.size)
            }
        }
        Column::Modified => entry
            .modified
            .map(|date| {
                chrono::DateTime::<chrono::Local>::from(date)
                    .format("%d %b %Y")
                    .to_string()
            })
            .unwrap_or_else(|| "—".into()),
        Column::Kind => entry.kind.label().into(),
        Column::Extension => entry.extension.clone(),
        Column::Path => entry.path.display().to_string(),
        Column::Attributes => entry.attributes.clone(),
        Column::Comment => entry.comment.clone(),
    }
}

fn panel_status(root: &mut Ui, rect: Rect, side: Side, tab: &Tab, space: Option<(u64, u64)>) {
    root.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1., LINE));
    let mut ui = child(
        root,
        Id::unique(("panel-status", side.index())),
        rect.shrink2(vec2(12., 2.)),
        Layout::left_to_right(Align::Center),
    );
    let shown = tab.visible.len();
    let title = if tab.selected.is_empty() {
        format!("{shown} items")
    } else {
        format!(
            "{} selected · {}",
            tab.selected.len(),
            metric(tab.selected_bytes())
        )
    };
    ui.label(mono(
        title,
        11.,
        if tab.selected.is_empty() { SUB } else { ACCENT },
    ));
    if tab.skipped > 0 {
        ui.label(label(format!("{} unavailable", tab.skipped), 11., AMBER));
    }
    if !matches!(tab.location(), Location::Directory(_)) {
        ui.label(label(tab.title(), 11., BLUE));
    }
    if let Some((free, total)) = space {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.available_width() > 100. {
                ui.label(mono(format!("{} free", metric(free)), 11., MUTED))
                    .on_hover_text(format!("{} free of {}", metric(free), metric(total)));
            }
        });
    }
}

fn show_preview(
    root: &mut Ui,
    rect: Rect,
    side: Side,
    source: Side,
    entry: Option<&Entry>,
    previews: &mut Previews,
    actions: &mut Vec<Action>,
) {
    let mut ui = child(
        root,
        Id::unique(("quickview", side.index())),
        rect.shrink2(vec2(13., 9.)),
        Layout::top_down(Align::Min),
    );
    ui.horizontal(|ui| {
        ui.label(label("Quick View", 14., TEXT).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, Icon::Close, "Close Quick View", true).clicked() {
                actions.push(Action::ToggleQuick(source));
            }
        });
    });
    ui.separator();
    let Some(entry) = entry else {
        ui.label(label("Select a file to preview.", 13., SUB));
        return;
    };
    ui.label(label(&entry.name, 17., TEXT).strong());
    ui.label(mono(
        format!(
            "{} · {}",
            entry.kind.label(),
            if entry.is_dir() {
                entry.attributes.clone()
            } else {
                metric(entry.size)
            }
        ),
        11.,
        SUB,
    ));
    if entry.is_dir() {
        if ui.button("Calculate folder size").clicked() {
            actions.push(Action::Tool(source, Tool::Statistics));
        }
    } else {
        match previews.get(entry, 720) {
            Some(CachedPreview::Image(texture, size)) => {
                let bounds = ui.available_size() - vec2(4., 10.);
                let target = contained_size(*size, bounds);
                ui.vertical_centered(|ui| {
                    ui.image((texture.id(), target));
                });
            }
            Some(CachedPreview::Text(text)) => {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(&mut ui, |ui| {
                        ui.add(egui::Label::new(mono(text, 12., TEXT)).selectable(true));
                    });
            }
            Some(CachedPreview::None) => {
                ui.label(label("Preview unavailable for this file.", 13., SUB));
                if ui.button("Open file").clicked() {
                    actions.push(Action::Open(source, entry.clone(), false));
                }
            }
            None => {
                ui.spinner();
            }
        }
    }
}

pub(crate) fn tree(
    ui: &mut Ui,
    side: Side,
    path: &Path,
    current: &Path,
    depth: usize,
    actions: &mut Vec<Action>,
) {
    if depth > 15 {
        return;
    }
    let response = egui::CollapsingHeader::new(label(
        engine::display_name(path),
        12.,
        if path == current { ACCENT } else { SUB },
    ))
    .id_salt(path)
    .default_open(depth == 0 || current.starts_with(path))
    .show(ui, |ui| {
        if ui.small_button("Open folder").clicked() {
            actions.push(Action::Path(side, path.to_path_buf()));
        }
        let cache_id = Id::unique(("directory-tree", path));
        let folders = ui.data_mut(|data| {
            let cached: Option<(std::time::Instant, Vec<PathBuf>)> = data.get_temp(cache_id);
            if let Some((time, folders)) = cached
                && time.elapsed().as_secs() < 5
            {
                return folders;
            }
            let folders = engine::tree_folders(path).unwrap_or_default();
            data.insert_temp(cache_id, (std::time::Instant::now(), folders.clone()));
            folders
        });
        for folder in folders.iter().take(250) {
            tree(ui, side, folder, current, depth + 1, actions);
        }
        if folders.len() > 250 {
            ui.label(label("More folders in the file panel", 11., MUTED));
        }
    });
    if response.header_response.double_clicked() {
        actions.push(Action::Path(side, path.to_path_buf()));
    }
}
