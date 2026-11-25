use std::f32::consts::{FRAC_PI_2, TAU};

use eframe::egui;
use egui::{
    Align2, Color32 as C, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    Style, Ui, Vec2, pos2, vec2,
};

pub const BASE: C = C::from_rgb(12, 15, 19);
pub const RAIL: C = C::from_rgb(16, 20, 25);
pub const SURFACE: C = C::from_rgb(19, 24, 30);
pub const RAISED: C = C::from_rgb(26, 32, 40);
pub const HOVER: C = C::from_rgb(35, 43, 52);
pub const LINE: C = C::from_rgb(45, 54, 64);
pub const TEXT: C = C::from_rgb(232, 238, 243);
pub const SUB: C = C::from_rgb(165, 176, 189);
pub const MUTED: C = C::from_rgb(123, 138, 154);
pub const ACCENT: C = C::from_rgb(170, 235, 129);
pub const ACCENT_DARK: C = C::from_rgb(36, 61, 48);
pub const BLUE: C = C::from_rgb(124, 178, 245);
pub const AMBER: C = C::from_rgb(234, 187, 116);
pub const RED: C = C::from_rgb(247, 145, 143);

pub fn install(context: &egui::Context) {
    let mut style = Style {
        visuals: egui::Visuals::dark(),
        ..Style::default()
    };
    let visual = &mut style.visuals;
    visual.override_text_color = Some(TEXT);
    visual.panel_fill = BASE;
    visual.window_fill = SURFACE;
    visual.extreme_bg_color = BASE;
    visual.faint_bg_color = RAIL;
    visual.window_stroke = Stroke::new(1., LINE);
    visual.window_corner_radius = CornerRadius::same(12);
    visual.selection.bg_fill = ACCENT_DARK;
    visual.selection.stroke = Stroke::new(1., ACCENT);
    visual.widgets.inactive.bg_fill = RAISED;
    visual.widgets.inactive.weak_bg_fill = RAISED;
    visual.widgets.inactive.bg_stroke = Stroke::new(1., LINE);
    visual.widgets.hovered.bg_fill = HOVER;
    visual.widgets.hovered.weak_bg_fill = HOVER;
    visual.widgets.hovered.bg_stroke = Stroke::new(1., MUTED);
    visual.widgets.active.bg_fill = ACCENT_DARK;
    visual.widgets.active.weak_bg_fill = ACCENT_DARK;
    visual.widgets.active.bg_stroke = Stroke::new(1., ACCENT);
    visual.error_fg_color = RED;
    visual.warn_fg_color = AMBER;
    visual.hyperlink_color = BLUE;
    style.spacing.item_spacing = vec2(8., 8.);
    style.spacing.button_padding = vec2(10., 5.);
    style.spacing.interact_size.y = 27.;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(14.));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(13.));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(11.));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(12.));
    context.set_theme(egui::ThemePreference::Dark);
    context.set_style_of(egui::Theme::Dark, style);
}

pub fn label(text: impl Into<String>, size: f32, color: C) -> RichText {
    RichText::new(text).size(size).color(color)
}
pub fn mono(text: impl Into<String>, size: f32, color: C) -> RichText {
    label(text, size, color).monospace()
}

#[derive(Clone, Copy)]
pub enum Icon {
    Brand,
    Folder,
    File,
    Image,
    Audio,
    Archive,
    Code,
    Video,
    Home,
    Drive,
    Star,
    Tree,
    Back,
    Next,
    Up,
    Plus,
    Close,
    #[cfg(not(target_os = "macos"))]
    Minimize,
    #[cfg(not(target_os = "macos"))]
    Maximize,
    #[cfg(not(target_os = "macos"))]
    Restore,
    Refresh,
    Search,
    Stack,
    Columns,
    Sidebar,
    Grid,
    List,
    Swap,
    Copy,
    Move,
    Trash,
    Eye,
    Queue,
    More,
    Settings,
    Link,
    Pin,
    Chevron,
    Download,
}

impl Icon {
    pub fn draw(self, ui: &Ui, center: Pos2, size: f32, color: C) {
        let painter = ui.painter();
        let point = |x: f32, y: f32| center + vec2(x - 8., y - 8.) * (size / 16.);
        let stroke = Stroke::new((size / 11.).clamp(1.05, 2.5), color);
        let line = |segments: &[(f32, f32)]| {
            painter.add(egui::Shape::line(
                segments.iter().map(|&(x, y)| point(x, y)).collect(),
                stroke,
            ));
        };
        let rect = |x0, y0, x1, y1| {
            painter.rect_stroke(
                Rect::from_min_max(point(x0, y0), point(x1, y1)),
                1.5,
                stroke,
                StrokeKind::Inside,
            )
        };
        match self {
            Self::Brand => {
                painter.rect_filled(
                    Rect::from_min_max(point(1., 2.), point(7., 14.)),
                    1.5,
                    color,
                );
                painter.rect_filled(
                    Rect::from_min_max(point(9., 4.), point(15., 12.)),
                    1.5,
                    color,
                );
            }
            Self::Folder => {
                line(&[
                    (1., 13.),
                    (1., 4.),
                    (6., 4.),
                    (8., 6.),
                    (15., 6.),
                    (15., 13.),
                    (1., 13.),
                ]);
            }
            Self::File => {
                line(&[
                    (4., 1.),
                    (10., 1.),
                    (13., 4.),
                    (13., 15.),
                    (4., 15.),
                    (4., 1.),
                ]);
                line(&[(10., 1.), (10., 5.), (13., 5.)]);
                line(&[(6., 9.), (11., 9.)]);
                line(&[(6., 12.), (10., 12.)]);
            }
            Self::Image => {
                rect(1., 2., 15., 14.);
                painter.circle_filled(point(5., 6.), size / 12., color);
                line(&[(3., 12.), (7., 8.), (9., 10.), (11., 7.), (14., 11.)]);
            }
            Self::Audio => {
                line(&[(5., 12.), (5., 4.), (12., 2.), (12., 10.)]);
                painter.circle_filled(point(3.75, 12.5), size / 9., color);
                painter.circle_filled(point(10.75, 10.5), size / 9., color);
            }
            Self::Archive => {
                rect(2., 4., 14., 14.);
                rect(1., 2., 15., 5.);
                line(&[(6., 8.), (10., 8.)]);
            }
            Self::Code => {
                line(&[(5., 4.), (1., 8.), (5., 12.)]);
                line(&[(11., 4.), (15., 8.), (11., 12.)]);
                line(&[(9., 3.), (7., 13.)]);
            }
            Self::Video => {
                rect(1., 3., 15., 13.);
                line(&[(6., 5.), (11., 8.), (6., 11.), (6., 5.)]);
            }
            Self::Home => {
                line(&[(1., 7.), (8., 1.), (15., 7.)]);
                line(&[
                    (3., 6.),
                    (3., 14.),
                    (6., 14.),
                    (6., 10.),
                    (10., 10.),
                    (10., 14.),
                    (13., 14.),
                    (13., 6.),
                ]);
            }
            Self::Drive => {
                rect(1., 6., 15., 13.);
                line(&[(2., 6.), (4., 3.), (12., 3.), (14., 6.)]);
                painter.circle_filled(point(12., 10.), size / 14., color);
            }
            Self::Star => {
                line(&[
                    (8., 1.),
                    (10., 6.),
                    (15., 6.),
                    (11., 9.),
                    (13., 14.),
                    (8., 11.),
                    (3., 14.),
                    (5., 9.),
                    (1., 6.),
                    (6., 6.),
                    (8., 1.),
                ]);
            }
            Self::Tree => {
                line(&[(3., 2.), (3., 13.), (8., 13.)]);
                line(&[(3., 5.), (8., 5.)]);
                rect(8., 3., 14., 7.);
                rect(8., 11., 14., 15.);
            }
            Self::Back => {
                line(&[(8., 3.), (3., 8.), (8., 13.)]);
                line(&[(3., 8.), (14., 8.)]);
            }
            Self::Next => {
                line(&[(8., 3.), (13., 8.), (8., 13.)]);
                line(&[(2., 8.), (13., 8.)]);
            }
            Self::Up => {
                line(&[(3., 8.), (8., 3.), (13., 8.)]);
                line(&[(8., 3.), (8., 14.)]);
            }
            Self::Plus => {
                line(&[(3., 8.), (13., 8.)]);
                line(&[(8., 3.), (8., 13.)]);
            }
            Self::Close => {
                line(&[(4., 4.), (12., 12.)]);
                line(&[(12., 4.), (4., 12.)]);
            }
            #[cfg(not(target_os = "macos"))]
            Self::Minimize => line(&[(3., 8.), (13., 8.)]),
            #[cfg(not(target_os = "macos"))]
            Self::Maximize => {
                rect(3., 3., 13., 13.);
            }
            #[cfg(not(target_os = "macos"))]
            Self::Restore => {
                line(&[(6., 4.), (6., 2.), (14., 2.), (14., 10.), (12., 10.)]);
                rect(2., 5., 11., 14.);
            }
            Self::Refresh => {
                line(&[
                    (13., 6.),
                    (11., 3.),
                    (7., 2.),
                    (3., 5.),
                    (2., 9.),
                    (5., 13.),
                    (10., 14.),
                    (14., 10.),
                ]);
                line(&[(13., 2.), (13., 6.), (9., 6.)]);
            }
            Self::Search => {
                painter.circle_stroke(point(7., 7.), size * 0.27, stroke);
                line(&[(10., 10.), (14., 14.)]);
            }
            Self::Stack => {
                rect(1., 2., 15., 14.);
                line(&[(2., 8.), (14., 8.)]);
            }
            Self::Sidebar => {
                rect(1., 2., 15., 14.);
                line(&[(6., 2.), (6., 14.)]);
            }
            Self::Columns => {
                rect(1., 2., 15., 14.);
                line(&[(8., 3.), (8., 13.)]);
            }
            Self::Grid => {
                for (x, y) in [(2., 2.), (9., 2.), (2., 9.), (9., 9.)] {
                    rect(x, y, x + 5., y + 5.);
                }
            }
            Self::List => {
                for y in [4., 8., 12.] {
                    line(&[(5., y), (14., y)]);
                    painter.circle_filled(point(2., y), size / 15., color);
                }
            }
            Self::Swap => {
                line(&[(2., 5.), (14., 5.), (11., 2.)]);
                line(&[(14., 11.), (2., 11.), (5., 14.)]);
            }
            Self::Copy => {
                rect(5., 5., 14., 14.);
                line(&[(2., 10.), (2., 2.), (10., 2.)]);
            }
            Self::Move => {
                line(&[
                    (1., 5.),
                    (6., 5.),
                    (8., 7.),
                    (14., 7.),
                    (14., 13.),
                    (1., 13.),
                    (1., 5.),
                ]);
                line(&[(7., 3.), (14., 3.), (12., 1.)]);
            }
            Self::Trash => {
                line(&[(2., 4.), (14., 4.)]);
                line(&[(4., 4.), (5., 14.), (11., 14.), (12., 4.)]);
                line(&[(6., 4.), (6., 2.), (10., 2.), (10., 4.)]);
            }
            Self::Eye => {
                line(&[
                    (1., 8.),
                    (4., 4.),
                    (8., 3.),
                    (12., 4.),
                    (15., 8.),
                    (12., 12.),
                    (8., 13.),
                    (4., 12.),
                    (1., 8.),
                ]);
                painter.circle_stroke(point(8., 8.), size / 7., stroke);
            }
            Self::Queue => {
                for y in [4., 8., 12.] {
                    line(&[(6., y), (14., y)]);
                    painter.circle_filled(point(2.5, y), size / 13., color);
                }
            }
            Self::More => {
                for x in [3., 8., 13.] {
                    painter.circle_filled(point(x, 8.), size / 12., color);
                }
            }
            Self::Settings => {
                let tooth_profile = [(-0.27, 5.3), (-0.17, 7.2), (0.17, 7.2), (0.27, 5.3)];
                let outline = (0..6)
                    .flat_map(|tooth| {
                        let angle = tooth as f32 * TAU / 6. - FRAC_PI_2;
                        tooth_profile.map(|(offset, radius)| {
                            let (y, x) = (angle + offset).sin_cos();
                            point(8. + x * radius, 8. + y * radius)
                        })
                    })
                    .collect();
                painter.add(egui::Shape::closed_line(outline, stroke));
                painter.circle_stroke(center, size * 0.15, stroke);
            }
            Self::Link => {
                line(&[
                    (7., 4.),
                    (10., 1.),
                    (13., 1.),
                    (15., 3.),
                    (15., 6.),
                    (12., 9.),
                ]);
                line(&[
                    (4., 7.),
                    (1., 10.),
                    (1., 13.),
                    (3., 15.),
                    (6., 15.),
                    (9., 12.),
                ]);
                line(&[(5., 11.), (11., 5.)]);
            }
            Self::Pin => {
                line(&[
                    (5., 2.),
                    (11., 2.),
                    (10., 7.),
                    (13., 10.),
                    (3., 10.),
                    (6., 7.),
                    (5., 2.),
                ]);
                line(&[(8., 10.), (8., 15.)]);
            }
            Self::Chevron => {
                line(&[(6., 4.), (10., 8.), (6., 12.)]);
            }
            Self::Download => {
                line(&[(8., 2.), (8., 10.)]);
                line(&[(4., 7.), (8., 11.), (12., 7.)]);
                line(&[(2., 12.), (2., 14.), (14., 14.), (14., 12.)]);
            }
        }
    }
}

pub fn icon_for(kind: deshcmd_engine::Kind) -> (Icon, C) {
    use deshcmd_engine::Kind;
    match kind {
        Kind::Folder => (Icon::Folder, AMBER),
        Kind::Image => (Icon::Image, C::from_rgb(183, 151, 247)),
        Kind::Audio => (Icon::Audio, C::from_rgb(242, 146, 175)),
        Kind::Video => (Icon::Video, C::from_rgb(240, 151, 158)),
        Kind::Archive => (Icon::Archive, C::from_rgb(237, 159, 105)),
        Kind::Code => (Icon::Code, ACCENT),
        Kind::Text => (Icon::File, BLUE),
        Kind::Other => (Icon::File, MUTED),
    }
}

pub fn icon_button(ui: &mut Ui, icon: Icon, title: &str, enabled: bool) -> egui::Response {
    custom_button(ui, icon, "", title, enabled, false, false, None)
}
pub fn action(
    ui: &mut Ui,
    icon: Icon,
    text: &str,
    title: &str,
    enabled: bool,
    primary: bool,
) -> egui::Response {
    custom_button(ui, icon, text, title, enabled, primary, false, None)
}
pub fn place(ui: &mut Ui, icon: Icon, text: &str, title: &str, active: bool) -> egui::Response {
    custom_button(
        ui,
        icon,
        text,
        title,
        true,
        false,
        active,
        Some(ui.available_width()),
    )
}

#[allow(clippy::too_many_arguments)]
fn custom_button(
    ui: &mut Ui,
    icon: Icon,
    text: &str,
    title: &str,
    enabled: bool,
    primary: bool,
    selected: bool,
    width: Option<f32>,
) -> egui::Response {
    let font = FontId::proportional(13.);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, TEXT);
    let estimated = if text.is_empty() {
        30.
    } else {
        galley.size().x + 41.
    };
    let (rect, response) = ui.allocate_exact_size(
        vec2(width.unwrap_or(estimated), 30.),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            enabled,
            if title.is_empty() { text } else { title },
        )
    });
    if ui.is_rect_visible(rect) {
        let bg = if primary && enabled {
            if response.hovered() {
                C::from_rgb(194, 246, 165)
            } else {
                ACCENT
            }
        } else if selected {
            ACCENT_DARK
        } else if response.hovered() && enabled || response.has_focus() {
            HOVER
        } else {
            C::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 7., bg);
        if response.has_focus() {
            ui.painter()
                .rect_stroke(rect, 7., Stroke::new(1., ACCENT), StrokeKind::Inside);
        }
        let ink = if !enabled {
            MUTED.gamma_multiply(0.55)
        } else if primary {
            BASE
        } else if selected {
            ACCENT
        } else {
            SUB
        };
        icon.draw(
            ui,
            if text.is_empty() {
                rect.center()
            } else {
                pos2(rect.left() + 15., rect.center().y)
            },
            15.,
            ink,
        );
        if !text.is_empty() {
            ui.painter()
                .with_clip_rect(rect.shrink2(vec2(4., 1.)))
                .text(
                    pos2(rect.left() + 29., rect.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    FontId::proportional(13.),
                    if primary && enabled {
                        BASE
                    } else if !enabled || selected {
                        ink
                    } else {
                        TEXT
                    },
                );
        }
    }
    if title.is_empty() {
        response
    } else {
        response.on_hover_text(title)
    }
}

pub fn card(ui: &Ui, rect: Rect, fill: C) {
    ui.painter()
        .rect(rect, 10., fill, Stroke::new(1., LINE), StrokeKind::Inside);
}
pub fn metric(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["KB", "MB", "GB", "TB", "PB"] {
        if value < 1024. {
            break;
        }
        value /= 1024.;
        unit = next;
    }
    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}
pub fn contained_size(size: Vec2, bounds: Vec2) -> Vec2 {
    if size.x <= 0. || size.y <= 0. {
        return Vec2::ZERO;
    }
    size * (bounds.x / size.x).min(bounds.y / size.y).min(1.5)
}
