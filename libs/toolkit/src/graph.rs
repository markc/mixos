// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Small histories drawn with the newest sample at the right edge.
//! Painted directly so grids of small histories stay cheap.
//!
//! Colours come from the theme's visuals, never constants (AGENTS.md §4).
use egui::{Color32, Mesh, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2};

/// Theme-relative hues, shared by painted graphs and application charts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hue {
    Accent,
    AccentPlus,
    Warning,
    AccentMinus,
    /// Further theme-relative colours for arbitrary multi-series histories.
    Series(usize),
}

/// Timeline and caller-owned readout wording. Values are oldest first, with
/// the newest at age zero. `samples` is the full window capacity, so partial
/// histories keep their spacing. No formatter is called until hover.
#[derive(Clone, Copy)]
pub struct Readout<'a> {
    pub samples: usize,
    pub interval_s: f64,
    /// Series index, value and age in seconds; include units and localised age.
    pub format: &'a dyn Fn(usize, f64, u64) -> String,
}

/// Nearest sample's age in steps, clamped to this history's populated span.
fn nearest(rect: Rect, x: f32, n: usize, samples: usize) -> Option<usize> {
    if n == 0 || samples == 0 || rect.width() <= 0.0 {
        return None;
    }
    let steps = ((rect.right() - x) / rect.width() * (samples.max(2) - 1) as f32)
        .round()
        .max(0.0) as usize;
    Some(steps.min(n.min(samples) - 1))
}

fn readout_rows(series: &[(&[f64], Hue)], steps: usize, readout: Readout<'_>) -> Vec<String> {
    series
        .iter()
        .enumerate()
        .filter_map(|(i, (values, _))| {
            let n = values.len().min(readout.samples);
            if steps < n {
                Some((readout.format)(
                    i,
                    values[values.len() - 1 - steps],
                    (steps as f64 * readout.interval_s).max(0.0).round() as u64,
                ))
            } else {
                None
            }
        })
        .collect()
}

/// A line colour derived from the installed theme.
pub fn tint(ui: &Ui, kind: Hue) -> Color32 {
    let palette = crate::chrome::Chrome::of(ui.ctx()).palette;
    let hue = match kind {
        Hue::Accent => palette.accent,
        // A third of the way round the colour wheel from the accent, each
        // way: distinct from it in every scheme, and still the theme's.
        Hue::AccentPlus => turned(palette.accent, 1.0 / 3.0),
        Hue::Warning => palette.warning,
        Hue::AccentMinus => turned(palette.accent, -1.0 / 3.0),
        Hue::Series(index) => turned(palette.accent, index as f32 * 0.618_034),
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
fn x_of(rect: Rect, i: usize, n: usize, samples: usize) -> f32 {
    let step = rect.width() / (samples.max(2) - 1) as f32;
    rect.right() - (n - 1 - i) as f32 * step
}

fn y_of(rect: Rect, value: f64, max: f64) -> f32 {
    let f = if max > 0.0 {
        (value / max).clamp(0.0, 1.0)
    } else {
        0.0
    };
    rect.bottom() - f as f32 * rect.height()
}

fn points(rect: Rect, values: &[f64], max: f64, samples: usize) -> Vec<Pos2> {
    let n = values.len().min(samples);
    let values = &values[values.len() - n..];
    values
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(x_of(rect, i, n, samples), y_of(rect, *v, max)))
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
        let line = Stroke::new(
            1.0,
            v.widgets.noninteractive.bg_stroke.color.gamma_multiply(0.5),
        );
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
pub fn line(
    ui: &mut Ui,
    size: Vec2,
    values: &[f64],
    max: f64,
    hue: Hue,
    readout: Option<Readout<'_>>,
) -> Response {
    lines(ui, size, &[(values, hue)], max, readout)
}

/// Several series on one grid, the first shaded and the rest dashed lines.
/// Without a readout, sample spacing comes from the longest supplied slice.
pub fn lines(
    ui: &mut Ui,
    size: Vec2,
    series: &[(&[f64], Hue)],
    max: f64,
    readout: Option<Readout<'_>>,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    frame(ui, rect, size.y > 40.0);
    let samples = readout.map_or_else(
        || {
            series
                .iter()
                .map(|(values, _)| values.len())
                .max()
                .unwrap_or(0)
        },
        |readout| readout.samples,
    );
    let painter = ui.painter_at(rect.shrink(1.0));
    for (i, (values, kind)) in series.iter().enumerate() {
        let colour = tint(ui, *kind);
        let line = points(rect.shrink(1.0), values, max, samples);
        if line.len() < 2 {
            continue;
        }
        if i == 0 {
            painter.add(area(rect.shrink(1.0), &line, colour.gamma_multiply(0.22)));
            painter.add(Shape::line(line, Stroke::new(1.5, colour)));
        } else {
            painter.add(Shape::dashed_line(
                &line,
                Stroke::new(1.25, colour),
                4.0,
                3.0,
            ));
        }
    }
    if let Some(readout) = readout
        && let Some(pointer) = response.hover_pos()
    {
        let plot = rect.shrink(1.0);
        let n = series
            .iter()
            .map(|(values, _)| values.len())
            .max()
            .unwrap_or(0);
        if let Some(steps) = nearest(plot, pointer.x, n, readout.samples) {
            let x = x_of(plot, 0, steps + 1, readout.samples);
            painter.vline(
                x,
                plot.y_range(),
                Stroke::new(1.0, ui.visuals().weak_text_color()),
            );
            return response.on_hover_ui(|ui| {
                for row in readout_rows(series, steps, readout) {
                    ui.label(row);
                }
            });
        }
    }
    response
}

/// A horizontal meter: `fraction` of `width` filled.
pub fn meter(ui: &mut Ui, width: f32, fraction: f64, kind: Hue) -> Response {
    let height = ui.text_style_height(&egui::TextStyle::Body) * 0.8;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, v.extreme_bg_color);
        let mut filled = rect;
        filled.set_width(rect.width() * fraction.clamp(0.0, 1.0) as f32);
        painter.rect_filled(filled, 2.0, tint(ui, kind).gamma_multiply(0.7));
        painter.rect_stroke(
            rect,
            2.0,
            v.widgets.noninteractive.bg_stroke,
            egui::StrokeKind::Inside,
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_reading_is_at_the_right_edge() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(590.0, 100.0));
        let p = points(rect, &[0.0, 50.0, 100.0], 100.0, 60);
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

    #[test]
    fn hover_rounds_to_nearest_and_clamps_partial_empty_and_short_series() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(590.0, 100.0));
        assert_eq!(nearest(rect, 575.1, 60, 60), Some(1));
        assert_eq!(nearest(rect, 574.9, 60, 60), Some(2));
        assert_eq!(nearest(rect, -10.0, 3, 60), Some(2));
        assert_eq!(nearest(rect, 600.0, 3, 60), Some(0));
        assert_eq!(nearest(rect, 0.0, 0, 60), None);
        assert_eq!(nearest(rect, 0.0, 3, 0), None);
        let series = [
            (&[10.0, 20.0][..], Hue::Accent),
            (&[30.0][..], Hue::Warning),
        ];
        let format = |i, v, age| format!("{i}: {v:.0}% · {age} s ago");
        let readout = Readout {
            samples: 60,
            interval_s: 2.0,
            format: &format,
        };
        assert_eq!(
            readout_rows(&series, 0, readout),
            ["0: 20% · 0 s ago", "1: 30% · 0 s ago"]
        );
        assert_eq!(readout_rows(&series, 1, readout), ["0: 10% · 2 s ago"]);
    }

    #[test]
    fn hues_keep_the_original_resource_maths_in_light_and_dark_themes() {
        for dark in [false, true] {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let mut output = ctx.run_ui(Default::default(), |ui| {
                let p = crate::chrome::Chrome::of(ui.ctx()).palette;
                for (hue, base) in [
                    (Hue::Accent, p.accent),
                    (Hue::AccentPlus, turned(p.accent, 1.0 / 3.0)),
                    (Hue::AccentMinus, turned(p.accent, -1.0 / 3.0)),
                    (Hue::Warning, p.warning),
                ] {
                    assert_eq!(tint(ui, hue), legible(base, dark));
                }
                let mut colours: Vec<_> = (0..8)
                    .map(|i| tint(ui, Hue::Series(i)).to_array())
                    .collect();
                colours.sort_unstable();
                colours.dedup();
                assert_eq!(colours.len(), 8, "indexed hues extend the fixed palette");
                let source = egui::ecolor::Hsva::from(p.accent);
                let turned = egui::ecolor::Hsva::from(turned(p.accent, 1.0 / 3.0));
                assert!((turned.h - (source.h + 1.0 / 3.0).rem_euclid(1.0)).abs() < 0.01);
                let adjusted = egui::ecolor::Hsva::from(legible(p.accent, dark));
                if dark {
                    assert!(adjusted.v >= 0.74);
                } else {
                    assert!(adjusted.v <= 0.56 && adjusted.s >= 0.54);
                }
            });
            output.textures_delta.clear();
        }
    }

    #[test]
    fn a_painted_history_needs_no_readout_or_formatter() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let response = line(
                ui,
                Vec2::new(120.0, 40.0),
                &[1.0, 2.0, 3.0],
                3.0,
                Hue::Accent,
                None,
            );
            assert_eq!(response.rect.size(), Vec2::new(120.0, 40.0));
        });
        output.textures_delta.clear();
    }
}
