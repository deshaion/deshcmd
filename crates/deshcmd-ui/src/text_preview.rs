use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::SystemTime;

use deshcmd_engine::{Entry, TextPreviewReader, TextPreviewWindow};
use eframe::egui::{self, FontId, Rect, Sense, Ui, pos2, vec2};

use crate::{child, theme::*};

const FONT_SIZE: f32 = 12.;

pub(crate) struct LargeTextView {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    requests: mpsc::Sender<u64>,
    results: mpsc::Receiver<io::Result<TextPreviewWindow>>,
    loading: bool,
    requested: Option<u64>,
    failed: bool,
    window: Option<TextPreviewWindow>,
    offset: u64,
    wheel_remainder: f32,
    thumb_grab: f32,
}

impl LargeTextView {
    pub fn new(entry: &Entry, ctx: egui::Context) -> Self {
        let (requests, read) = mpsc::channel();
        let (write, results) = mpsc::channel();
        let path = entry.path.clone();
        let len = entry.size;
        std::thread::spawn(move || {
            let mut reader = None;
            while let Ok(block) = read.recv() {
                let result = (|| {
                    if reader.is_none() {
                        reader = Some(TextPreviewReader::open(&path, len)?);
                    }
                    reader.as_mut().unwrap().read_window(block)
                })();
                let failed = result.is_err();
                if write.send(result).is_err() {
                    break;
                }
                ctx.request_repaint();
                if failed {
                    break;
                }
            }
        });
        Self {
            path: entry.path.clone(),
            modified: entry.modified,
            len,
            requests,
            results,
            loading: false,
            requested: None,
            failed: false,
            window: None,
            offset: 0,
            wheel_remainder: 0.,
            thumb_grab: 0.,
        }
    }

    pub fn matches(&self, entry: &Entry) -> bool {
        self.path == entry.path && self.modified == entry.modified && self.len == entry.size
    }

    fn request_window(&mut self, offset: u64) {
        let block = TextPreviewReader::block(offset);
        if !self.loading && self.requested != Some(block) {
            if self.requests.send(block).is_err() {
                self.failed = true;
                return;
            }
            self.requested = Some(block);
            self.loading = true;
        }
    }

    pub fn show(&mut self, ui: &mut Ui) {
        if self.loading {
            match self.results.try_recv() {
                Ok(Ok(window)) => {
                    self.window = Some(window);
                    self.loading = false;
                }
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => self.failed = true,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.failed {
            ui.label(label("Could not read the file in Quick View.", 13., SUB));
            return;
        }

        let rect = ui.available_rect_before_wrap();
        if rect.width() <= 14. || rect.height() <= 0. {
            return;
        }
        let bar = Rect::from_min_max(pos2(rect.right() - 10., rect.top()), rect.max);
        let viewport = Rect::from_min_max(rect.min, pos2(bar.left() - 4., rect.bottom()));
        let row_height = ui.fonts_mut(|fonts| fonts.row_height(&FontId::monospace(FONT_SIZE)));
        let visible = ((viewport.height() - 12.) / row_height).floor().max(1.) as usize;
        let offset = self.offset;
        // Keep the document position in bytes. egui only lays out a screenful,
        // so neither scroll precision nor layout depends on the full file size.
        let mut top = None;
        let mut at_end = false;
        if let Some(window) = &self.window
            && window.bytes.contains(&offset)
            && !window.rows.is_empty()
        {
            let row = window
                .rows
                .partition_point(|(start, _)| *start <= offset)
                .saturating_sub(1);
            let has_end = window.bytes.end == self.len;
            let max_top = window
                .rows
                .len()
                .saturating_sub(if has_end { visible } else { 1 });
            let mut row = row.min(max_top);
            if ui.rect_contains_pointer(rect) {
                let wheel = ui.input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta.y));
                self.wheel_remainder -= wheel / row_height;
                let steps = self.wheel_remainder.trunc();
                self.wheel_remainder -= steps;
                row = row.saturating_add_signed(steps as isize).min(max_top);
            }
            top = Some(row);
            self.offset = window.rows[row].0;
            at_end = has_end && row == max_top && self.offset > 0;
        }

        let jump = self.scrollbar(ui, bar, at_end);
        if let Some(offset) = jump {
            self.offset = offset;
            self.wheel_remainder = 0.;
            ui.ctx().request_repaint();
        }
        // A jump into the current window can be rendered on the next frame.
        // Requesting a neighboring window also preloads the next and previous blocks.
        self.request_window(self.offset);
        let mut contents = child(
            ui,
            ui.scope_id().with("large text rows"),
            viewport,
            egui::Layout::top_down(egui::Align::Min),
        );
        if let (Some(window), Some(top)) = (&self.window, top) {
            let bottom = (top + visible).min(window.rows.len());
            let text = window.rows[top..bottom]
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            egui::ScrollArea::horizontal()
                .id_salt(&self.path)
                .auto_shrink([false, false])
                .show(&mut contents, |ui| {
                    ui.add(
                        egui::Label::new(mono(text, FONT_SIZE, TEXT))
                            .extend()
                            .selectable(true),
                    );
                });
        } else {
            contents.spinner();
        }
    }

    fn scrollbar(&mut self, ui: &mut Ui, bar: Rect, at_end: bool) -> Option<u64> {
        let response = ui.interact(
            bar,
            ui.scope_id().with("large text scrollbar"),
            Sense::click_and_drag(),
        );
        let thumb_height = 30_f32.min(bar.height());
        let travel = (bar.height() - thumb_height).max(1.);
        let fraction = if at_end {
            1.
        } else {
            (self.offset as f64 / (self.len - 1) as f64) as f32
        };
        let thumb = Rect::from_min_size(
            pos2(bar.left(), bar.top() + fraction * travel),
            vec2(bar.width(), thumb_height),
        );
        if response.drag_started()
            && let Some(press) = ui.input(|input| input.pointer.press_origin())
        {
            self.thumb_grab = if thumb.contains(press) {
                press.y - thumb.top()
            } else {
                thumb_height / 2.
            };
        }
        ui.painter().rect_filled(bar, 4., LINE);
        ui.painter().rect_filled(
            thumb,
            4.,
            if response.hovered() || response.dragged() {
                SUB
            } else {
                MUTED
            },
        );
        if (response.drag_started()
            || response.drag_stopped()
            || response.dragged() && ui.input(|input| input.pointer.delta().y != 0.)
            || response.clicked()
                && !response
                    .interact_pointer_pos()
                    .is_some_and(|p| thumb.contains(p)))
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let grab = if response.clicked() {
                thumb_height / 2.
            } else {
                self.thumb_grab
            };
            let fraction = ((pointer.y - bar.top() - grab) / travel).clamp(0., 1.);
            return Some((f64::from(fraction) * (self.len - 1) as f64).round() as u64);
        }
        None
    }
}
