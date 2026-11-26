use std::f32::consts::TAU;
use std::ops::Range;

use eframe::egui;
use egui::{
    Align, Align2, Context, Layout, Painter, Pos2, Rect, Sense, Shape, Stroke, UiBuilder, Vec2,
    vec2,
};

use crate::theme::{ACCENT, ACCENT_DARK, SUB, SURFACE, TEXT, label};

const MESSAGES: &[&str] = &[
    "YOU GOT THIS",
    "KEEP GOING",
    "YOU CAN DO IT",
    "ONE STEP AT A TIME",
    "GOOD THINGS AHEAD",
];

pub(crate) struct About {
    center: Vec2,
    velocity: Vec2,
    phase: f32,
    angle: f32,
    last_frame: Option<f64>,
    hover_origin: Option<Pos2>,
    reveal: f32,
    message: Lettering,
    rope: Vec<Pos2>,
}

impl About {
    pub(crate) fn new() -> Self {
        let direction = fastrand::f32() * TAU;
        Self {
            center: Vec2::ZERO,
            velocity: Vec2::angled(direction) * 42.,
            phase: fastrand::f32() * TAU,
            angle: fastrand::f32() * TAU,
            last_frame: None,
            hover_origin: None,
            reveal: 0.,
            message: Lettering::new(MESSAGES[fastrand::usize(..MESSAGES.len())]),
            rope: Vec::new(),
        }
    }

    pub(crate) fn show(&mut self, ctx: &Context) -> bool {
        let mut open = true;
        egui::Window::new("About DeshCmd")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0., 0.])
            .show(ctx, |ui| {
                let available = ctx.content_rect().size() - vec2(36., 64.);
                let size = vec2(360., 164.).min(available).max(vec2(1., 1.));
                let (canvas, painter) = ui.allocate_painter(size, Sense::hover());
                let rect = canvas.rect;
                let (now, pointer) = ctx.input(|input| {
                    (
                        input.time,
                        input.pointer.hover_pos().filter(|_| input.focused),
                    )
                });
                let pointer = pointer.filter(|_| ui.rect_contains_pointer(rect));
                self.draw(&painter, rect, now, pointer);

                let mut text = ui.new_child(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_max(
                            rect.min + vec2(0., size.y * 0.29),
                            rect.max,
                        ))
                        .layout(Layout::top_down(Align::Center)),
                );
                text.spacing_mut().item_spacing.y = 3.;
                text.label(
                    label(format!("Version {}", env!("CARGO_PKG_VERSION")), 16., TEXT).strong(),
                );
                text.label(label("Dual-pane file manager", 14., SUB));
            });
        open
    }

    fn draw(&mut self, painter: &Painter, rect: Rect, now: f64, pointer: Option<Pos2>) {
        self.advance(rect, now, pointer);
        let eased = smoothstep(self.reveal);
        let edge = SURFACE.lerp_to_gamma(ACCENT_DARK, 0.85 + eased * 0.15);
        let highlight = SURFACE.lerp_to_gamma(ACCENT, 0.40 + eased * 0.50);
        if self.reveal == 0. {
            paint_strand(painter, &self.rope, edge, highlight, 0.);
            // Redraw the front at each crossing so the two strands stay distinct.
            for index in crossings(&self.rope) {
                let start = index.saturating_sub(3);
                let end = (index + 7).min(self.rope.len());
                paint_strand(painter, &self.rope[start..end], edge, highlight, 0.);
            }
        } else {
            // The bridges join the strokes in flight and fade as the letters form.
            let bridge_opacity = (1. - eased).powi(2);
            let mut previous_end = 1;
            for stroke in &self.message.strokes {
                if stroke.start > 0 && bridge_opacity > 0. {
                    paint_strand(
                        painter,
                        &self.rope[previous_end - 1..=stroke.start],
                        edge.gamma_multiply(bridge_opacity),
                        highlight.gamma_multiply(bridge_opacity),
                        eased,
                    );
                }
                paint_strand(painter, &self.rope[stroke.clone()], edge, highlight, eased);
                previous_end = stroke.end;
            }
        }
        if self.hover_origin.is_none() || self.reveal < 1. {
            painter.ctx().request_repaint();
        }
    }

    fn advance(&mut self, rect: Rect, now: f64, pointer: Option<Pos2>) {
        let dt = self
            .last_frame
            .replace(now)
            .map_or(0., |last| (now - last).clamp(0., 0.05) as f32);
        let scale = (rect.width() / 360.).min(rect.height() / 164.);
        let target_scale = (rect.width() * 0.86 / self.message.width).min(24. * scale);
        let target_center = rect.center() + vec2(0., rect.height() * 0.25);
        let target = |index: usize| target_center + self.message.points[index] * target_scale;

        // Check the last painted rope before advancing it. During the transition,
        // a stationary pointer keeps its hold as the rope moves out from under it.
        self.hover_origin = pointer.and_then(|pointer| {
            if let Some(origin) = self.hover_origin {
                let near_message = self.message.strokes.iter().any(|stroke| {
                    let points = stroke.clone().map(target);
                    near_path(pointer, points, 9.)
                });
                (pointer.distance(origin) <= 13. || near_message).then_some(origin)
            } else if self.reveal == 0. {
                near_path(pointer, self.rope.iter().copied(), 7.).then_some(pointer)
            } else {
                None
            }
        });
        if self.hover_origin.is_some() {
            self.reveal = (self.reveal + dt / 0.68).min(1.);
        } else {
            self.reveal = (self.reveal - dt / 0.50).max(0.);
        }

        if self.hover_origin.is_none() && self.reveal == 0. {
            self.phase += dt;
            self.angle += dt * (0.37 + 0.23 * (self.phase * 0.62).sin());
            self.velocity =
                Vec2::angled(self.velocity.angle() + dt * 0.40 * (self.phase * 0.93).sin()) * 42.;
            self.center += self.velocity * (dt * scale);
        }

        self.rope.clear();
        let mut bounds = Rect::NOTHING;
        let count = self.message.points.len();
        let rotation = egui::emath::Rot2::from_angle(self.angle);
        // The traveling curl grows past the point where its tangent doubles back.
        let curl = 15. + 9. * (self.phase * 1.13).sin();
        for index in 0..count {
            let s = 2. * index as f32 / (count - 1) as f32 - 1.;
            let turn = s * 4.8 - self.phase * 1.65;
            let offset = rotation
                * vec2(
                    43. * s + curl * turn.sin(),
                    0.86 * curl * turn.cos() + 10. * (s * 3.6 + self.phase * 1.12).sin(),
                )
                * scale;
            let point = Pos2::ZERO + offset;
            bounds.extend_with(point);
            self.rope.push(point);
        }
        let half_size = rect.size() * 0.5 - Vec2::splat(6. * scale);
        bounce(
            &mut self.center.x,
            &mut self.velocity.x,
            -half_size.x - bounds.min.x,
            half_size.x - bounds.max.x,
        );
        bounce(
            &mut self.center.y,
            &mut self.velocity.y,
            -half_size.y - bounds.min.y,
            half_size.y - bounds.max.y,
        );

        let eased = smoothstep(self.reveal);
        let origin = rect.center().to_vec2() + self.center;
        for (index, point) in self.rope.iter_mut().enumerate() {
            *point = (*point + origin).lerp(target(index), eased);
        }
    }
}

fn smoothstep(value: f32) -> f32 {
    value * value * (3. - 2. * value)
}

fn bounce(position: &mut f32, velocity: &mut f32, min: f32, max: f32) {
    if *position < min {
        *position = min;
        *velocity = velocity.abs();
    } else if *position > max {
        *position = max;
        *velocity = -velocity.abs();
    }
}

fn near_path(pointer: Pos2, points: impl Iterator<Item = Pos2> + Clone, radius: f32) -> bool {
    points.clone().zip(points.skip(1)).any(|(a, b)| {
        let segment = b - a;
        let t = ((pointer - a).dot(segment) / segment.length_sq().max(f32::EPSILON)).clamp(0., 1.);
        pointer.distance_sq(a + segment * t) <= radius * radius
    })
}

fn crossings(points: &[Pos2]) -> impl Iterator<Item = usize> + '_ {
    const STEP: usize = 4;
    (0..points.len().saturating_sub(STEP))
        .step_by(STEP)
        .filter(move |&front| {
            let a = points[front];
            let ab = points[front + STEP] - a;
            (0..front.saturating_sub(STEP)).step_by(STEP).any(|back| {
                let c = points[back];
                let cd = points[back + STEP] - c;
                let determinant = ab.rot90().dot(cd);
                let t = (c - a).rot90().dot(cd) / determinant;
                let u = (c - a).rot90().dot(ab) / determinant;
                (0. ..1.).contains(&t) && (0. ..1.).contains(&u)
            })
        })
}

fn paint_strand(
    painter: &Painter,
    points: &[Pos2],
    edge: egui::Color32,
    highlight: egui::Color32,
    reveal: f32,
) {
    let outline_width = egui::lerp(5.5..=4., reveal);
    let highlight_width = egui::lerp(3.1..=2., reveal);
    painter.add(Shape::line(
        points.to_vec(),
        Stroke::new(outline_width, edge),
    ));
    painter.add(Shape::line(
        points.to_vec(),
        Stroke::new(highlight_width, highlight),
    ));
}

struct Lettering {
    points: Vec<Vec2>,
    strokes: Vec<Range<usize>>,
    width: f32,
}

impl Lettering {
    fn new(message: &str) -> Self {
        let mut points = Vec::new();
        let mut strokes = Vec::new();
        let mut x = 0.;
        for letter in message.chars() {
            if letter == ' ' {
                x += 0.46;
                continue;
            }
            for stroke in glyph(letter) {
                let translate = |(u, v)| vec2(x + u * 0.66, v - 0.5);
                let first = translate(stroke[0]);
                if let Some(&previous) = points.last() {
                    subdivide(&mut points, previous, first);
                    points.pop();
                }
                let start = points.len();
                points.push(first);
                for pair in stroke.windows(2) {
                    subdivide(&mut points, translate(pair[0]), translate(pair[1]));
                }
                strokes.push(start..points.len());
            }
            x += 0.86;
        }
        let width = x - 0.20;
        for point in &mut points {
            point.x -= width * 0.5;
        }
        Self {
            points,
            strokes,
            width,
        }
    }
}

fn subdivide(points: &mut Vec<Vec2>, start: Vec2, end: Vec2) {
    let steps = ((end - start).length() / 0.18).ceil().max(1.) as usize;
    for step in 1..=steps {
        points.push(egui::lerp(start..=end, step as f32 / steps as f32));
    }
}

// Single-line capitals in a unit box. Separate strokes allow for crossbars.
fn glyph(letter: char) -> &'static [&'static [(f32, f32)]] {
    match letter {
        'A' => &[
            &[(0., 1.), (0.5, 0.), (1., 1.)],
            &[(0.23, 0.56), (0.77, 0.56)],
        ],
        'C' => &[&[
            (1., 0.12),
            (0.76, 0.),
            (0.28, 0.),
            (0., 0.23),
            (0., 0.77),
            (0.28, 1.),
            (0.76, 1.),
            (1., 0.88),
        ]],
        'D' => &[&[
            (0., 1.),
            (0., 0.),
            (0.59, 0.),
            (0.89, 0.15),
            (1., 0.34),
            (1., 0.66),
            (0.89, 0.85),
            (0.59, 1.),
            (0., 1.),
        ]],
        'E' => &[
            &[(1., 0.), (0., 0.), (0., 1.), (1., 1.)],
            &[(0., 0.48), (0.81, 0.48)],
        ],
        'G' => &[&[
            (1., 0.13),
            (0.75, 0.),
            (0.28, 0.),
            (0., 0.23),
            (0., 0.77),
            (0.28, 1.),
            (0.74, 1.),
            (1., 0.79),
            (1., 0.54),
            (0.55, 0.54),
        ]],
        'H' => &[
            &[(0., 0.), (0., 1.)],
            &[(0., 0.5), (1., 0.5)],
            &[(1., 0.), (1., 1.)],
        ],
        'I' => &[
            &[(0.14, 0.), (0.86, 0.)],
            &[(0.5, 0.), (0.5, 1.)],
            &[(0.14, 1.), (0.86, 1.)],
        ],
        'K' => &[
            &[(0., 0.), (0., 1.)],
            &[(1., 0.), (0., 0.58)],
            &[(0.35, 0.39), (1., 1.)],
        ],
        'M' => &[&[(0., 1.), (0., 0.), (0.5, 0.61), (1., 0.), (1., 1.)]],
        'N' => &[&[(0., 1.), (0., 0.), (1., 1.), (1., 0.)]],
        'O' => &[&[
            (0.27, 0.),
            (0.73, 0.),
            (1., 0.22),
            (1., 0.78),
            (0.73, 1.),
            (0.27, 1.),
            (0., 0.78),
            (0., 0.22),
            (0.27, 0.),
        ]],
        'P' => &[&[
            (0., 1.),
            (0., 0.),
            (0.72, 0.),
            (1., 0.16),
            (1., 0.36),
            (0.72, 0.51),
            (0., 0.51),
        ]],
        'S' => &[&[
            (1., 0.13),
            (0.77, 0.),
            (0.23, 0.),
            (0., 0.17),
            (0., 0.35),
            (0.23, 0.48),
            (0.77, 0.52),
            (1., 0.65),
            (1., 0.83),
            (0.77, 1.),
            (0.23, 1.),
            (0., 0.87),
        ]],
        'T' => &[&[(0., 0.), (1., 0.)], &[(0.5, 0.), (0.5, 1.)]],
        'U' => &[&[
            (0., 0.),
            (0., 0.8),
            (0.25, 1.),
            (0.75, 1.),
            (1., 0.8),
            (1., 0.),
        ]],
        'Y' => &[
            &[(0., 0.), (0.5, 0.48), (1., 0.)],
            &[(0.5, 0.48), (0.5, 1.)],
        ],
        _ => unreachable!("Unsupported letter in an About message: {letter}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rope_reveals_the_same_message_until_the_pointer_leaves() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(360., 164.));
        for message in MESSAGES {
            let mut about = About::new();
            about.angle = 0.7;
            about.phase = 0.4;
            about.velocity = vec2(42., 0.);
            about.message = Lettering::new(message);
            about.advance(rect, 0., None);
            let center = about.center;
            let pointer = about.rope[about.rope.len() / 2];
            for frame in 1..=60 {
                about.advance(rect, f64::from(frame) / 60., Some(pointer));
            }
            assert_eq!(about.center, center);
            assert_eq!(about.reveal, 1.);
            let revealed = about.rope.clone();
            about.advance(rect, 1.1, Some(pointer));
            assert_eq!(about.rope, revealed);

            for frame in 67..=120 {
                about.advance(rect, f64::from(frame) / 60., None);
            }
            assert_eq!(about.reveal, 0.);
            assert_ne!(about.center, center);
        }
    }

    #[test]
    fn rope_curls_and_unwinds_while_bouncing_inside_the_window() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(360., 164.));
        let mut about = About::new();
        about.angle = 0.7;
        about.phase = 0.4;
        about.center.x = 175.;
        about.velocity = vec2(42., 0.);
        about.message = Lettering::new(MESSAGES[0]);
        about.advance(rect, 0., None);
        assert!(about.velocity.x < 0.);
        let mut crossed = false;
        let mut uncrossed_after_crossing = false;
        for frame in 1..=1800 {
            about.advance(rect, f64::from(frame) / 60., None);
            assert!(
                about
                    .rope
                    .iter()
                    .all(|&point| rect.shrink(5.).contains(point))
            );
            if frame % 6 == 0 {
                let crossing = crossings(&about.rope).next().is_some();
                uncrossed_after_crossing |= crossed && !crossing;
                crossed |= crossing;
            }
        }
        assert!(crossed && uncrossed_after_crossing);
    }
}
