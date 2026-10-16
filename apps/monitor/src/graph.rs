// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The graphs: a minute of readings drawn right to left, newest at the
//! right edge, as Task Manager draws them. Painted directly: a 64-core grid
//! is 64 small graphs a frame, which a plotting widget per graph would make
//! heavy.
//!
//! Colours come from the theme's visuals, never constants (AGENTS.md §4).
use egui::{Color32, Mesh, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2};
use vitals::history::WINDOW;

/// What a graph shows; each resource keeps one colour everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cpu,
    Memory,
    Disk,
    Net,
}

/// The resource's line colour, from the installed theme.
pub fn tint(ui: &Ui, kind: Kind) -> Color32 {
    let palette = toolkit::chrome::Chrome::of(ui.ctx()).palette;
    let hue = match kind {
        Kind::Cpu => palette.accent,
        // A third of the way round the colour wheel from the accent, each
        // way: distinct from it in every scheme, and still the theme's.
        Kind::Memory => turned(palette.accent, 1.0 / 3.0),
        Kind::Disk => palette.warning,
        Kind::Net => turned(palette.accent, -1.0 / 3.0),
    };
    legible(hue, ui.visuals().dark_mode)
}

/// `colour` with enough contrast to read as a line on the graph's
/// background: deep on a light theme, bright on a dark one. A light
/// scheme's accent is often a pale highlight that would wash out.
fn legible(colour: Color32, dark: bool) -> Color32 {
    let mut hsva = egui::ecolor::Hsva::from(colour);
    if dark {
        hsva.v = hsva.v.max(0.75);
    } else {
        hsva.v = hsva.v.min(0.55);
        hsva.s = hsva.s.max(0.55);
    }
    hsva.into()
}

/// `colour` with its hue turned by `turns` of the wheel.
fn turned(colour: Color32, turns: f32) -> Color32 {
    let mut hsva = egui::ecolor::Hsva::from(colour);
    hsva.h = (hsva.h + turns).rem_euclid(1.0);
    hsva.into()
}

/// The x of reading `i` of `n`, the newest at the right edge.
fn x_of(rect: Rect, i: usize, n: usize) -> f32 {
    let step = rect.width() / (WINDOW.max(2) - 1) as f32;
    rect.right() - (n - 1 - i) as f32 * step
}

fn y_of(rect: Rect, value: f64, max: f64) -> f32 {
    let f = if max > 0.0 { (value / max).clamp(0.0, 1.0) } else { 0.0 };
    rect.bottom() - f as f32 * rect.height()
}

fn points(rect: Rect, values: &[f64], max: f64) -> Vec<Pos2> {
    let n = values.len().min(WINDOW);
    let values = &values[values.len() - n..];
    values
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(x_of(rect, i, n), y_of(rect, *v, max)))
        .collect()
}

/// The shaded area under `line`, down to the rect's bottom edge.
fn area(rect: Rect, line: &[Pos2], fill: Color32) -> Shape {
    let mut mesh = Mesh::default();
    for pair in line.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(a, fill);
        mesh.colored_vertex(b, fill);
        mesh.colored_vertex(pos2(b.x, rect.bottom()), fill);
        mesh.colored_vertex(pos2(a.x, rect.bottom()), fill);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    Shape::mesh(mesh)
}

fn frame(ui: &Ui, rect: Rect, grid: bool) {
    let v = ui.visuals();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, v.extreme_bg_color);
    if grid {
        let line = Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color.gamma_multiply(0.5));
        for i in 1..4 {
            let y = rect.top() + rect.height() * i as f32 / 4.0;
            painter.hline(rect.x_range(), y, line);
        }
        for i in 1..6 {
            let x = rect.left() + rect.width() * i as f32 / 6.0;
            painter.vline(x, rect.y_range(), line);
        }
    }
    painter.rect_stroke(
        rect,
        0.0,
        v.widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
}

/// One series as a shaded line on a grid, scaled to `max`.
pub fn line(ui: &mut Ui, size: Vec2, values: &[f64], max: f64, kind: Kind) -> Response {
    lines(ui, size, &[(values, kind)], max)
}

/// Several series on one grid, the first shaded, the rest lines only (a
/// disk's reads and writes, an interface's receive and send).
pub fn lines(ui: &mut Ui, size: Vec2, series: &[(&[f64], Kind)], max: f64) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    frame(ui, rect, size.y > 40.0);
    let painter = ui.painter_at(rect.shrink(1.0));
    for (i, (values, kind)) in series.iter().enumerate() {
        let colour = tint(ui, *kind);
        let line = points(rect.shrink(1.0), values, max);
        if line.len() < 2 {
            continue;
        }
        if i == 0 {
            painter.add(area(rect.shrink(1.0), &line, colour.gamma_multiply(0.22)));
            painter.add(Shape::line(line, Stroke::new(1.5, colour)));
        } else {
            painter.add(Shape::dashed_line(&line, Stroke::new(1.25, colour), 4.0, 3.0));
        }
    }
    response
}

/// A horizontal meter: `fraction` of `width` filled.
pub fn meter(ui: &mut Ui, width: f32, fraction: f64, kind: Kind) -> Response {
    let height = ui.text_style_height(&egui::TextStyle::Body) * 0.8;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, v.extreme_bg_color);
        let mut filled = rect;
        filled.set_width(rect.width() * fraction.clamp(0.0, 1.0) as f32);
        painter.rect_filled(filled, 2.0, tint(ui, kind).gamma_multiply(0.7));
        painter.rect_stroke(rect, 2.0, v.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_reading_is_at_the_right_edge() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(590.0, 100.0));
        let p = points(rect, &[0.0, 50.0, 100.0], 100.0);
        assert_eq!(p.len(), 3);
        assert_eq!(p[2], pos2(590.0, 0.0));
        assert_eq!(p[1], pos2(580.0, 50.0));
        assert_eq!(p[0].y, 100.0);
    }

    #[test]
    fn values_past_the_scale_are_held_at_the_top() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(10.0, 10.0));
        assert_eq!(y_of(rect, 200.0, 100.0), 0.0);
        assert_eq!(y_of(rect, 5.0, 0.0), 10.0);
    }
}
