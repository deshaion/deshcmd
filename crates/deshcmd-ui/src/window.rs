use eframe::egui::{Context, Id, PointerButton, Rect, Sense, Ui, ViewportCommand};
#[cfg(not(target_os = "macos"))]
use eframe::egui::{CursorIcon, viewport::ResizeDirection};

#[cfg(not(target_os = "macos"))]
use crate::theme::{Icon, icon_button};

pub(crate) const HEADER_HEIGHT: f32 = 44.;

#[cfg(not(target_os = "macos"))]
pub(crate) fn controls(ui: &mut Ui) -> bool {
    let close = icon_button(ui, Icon::Close, "Close window", true).clicked();
    let enlarged = ui.ctx().input(|input| {
        let viewport = input.viewport();
        viewport.maximized == Some(true) || viewport.fullscreen == Some(true)
    });
    let (icon, label) = if enlarged {
        (Icon::Restore, "Restore window")
    } else {
        (Icon::Maximize, "Maximize window")
    };
    if icon_button(ui, icon, label, true).clicked() {
        toggle_maximize(ui.ctx());
    }
    if icon_button(ui, Icon::Minimize, "Minimize window", true).clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
    close
}

fn toggle_maximize(ctx: &Context) {
    let command = ctx.input(|input| {
        let viewport = input.viewport();
        if viewport.fullscreen == Some(true) {
            ViewportCommand::Fullscreen(false)
        } else {
            ViewportCommand::Maximized(viewport.maximized != Some(true))
        }
    });
    ctx.send_viewport_cmd(command);
}

pub(crate) fn drag_region(ui: &mut Ui, rect: Rect) {
    if !rect.is_positive() {
        return;
    }
    let response = ui.interact(rect, Id::unique("window-titlebar"), Sense::click_and_drag());
    if response.double_clicked_by(PointerButton::Primary) {
        toggle_maximize(ui.ctx());
    } else if response.drag_started_by(PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn resize_edges(ui: &Ui, rect: Rect) {
    if !ui.rect_contains_pointer(rect) {
        return;
    }
    let (pos, pressed) = ui.input(|input| {
        let viewport = input.viewport();
        let pos = if viewport.maximized == Some(true) || viewport.fullscreen == Some(true) {
            None
        } else {
            input.pointer.interact_pos()
        };
        (pos, input.pointer.button_pressed(PointerButton::Primary))
    });
    let Some(pos) = pos else {
        return;
    };
    let edge = |point: f32, start: f32, end: f32| {
        if point < start + 5. {
            -1
        } else if point > end - 5. {
            1
        } else {
            0
        }
    };
    use CursorIcon as Cursor;
    use ResizeDirection as Direction;
    let (direction, cursor) = match (
        edge(pos.x, rect.left(), rect.right()),
        edge(pos.y, rect.top(), rect.bottom()),
    ) {
        (-1, -1) => (Direction::NorthWest, Cursor::ResizeNwSe),
        (1, 1) => (Direction::SouthEast, Cursor::ResizeNwSe),
        (1, -1) => (Direction::NorthEast, Cursor::ResizeNeSw),
        (-1, 1) => (Direction::SouthWest, Cursor::ResizeNeSw),
        (0, -1) => (Direction::North, Cursor::ResizeVertical),
        (0, 1) => (Direction::South, Cursor::ResizeVertical),
        (-1, 0) => (Direction::West, Cursor::ResizeHorizontal),
        (1, 0) => (Direction::East, Cursor::ResizeHorizontal),
        _ => return,
    };
    ui.ctx().set_cursor_icon(cursor);
    if pressed {
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}
