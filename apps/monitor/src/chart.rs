// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The replaceable large-chart renderer. The engine supplies the same rings
//! used by painted sparklines; plotting types never enter that data path.
use egui::{Ui, Vec2};
use egui_plot::{
    AxisHints, GridMark, HoverPosition, ItemId, ItemIdSet, Line, Plot, PlotMemory, PlotResponse,
};
use std::{cell::Cell, collections::HashMap};
use toolkit::graph::{self, Hue};

pub struct Series<'a> {
    pub name: &'a str,
    /// A compact legend label; the full name remains in hover and tooltips.
    pub legend_label: Option<&'a str>,
    pub hue: Hue,
    pub values: &'a [f64],
}

pub struct Chart<'a> {
    pub series: &'a [Series<'a>],
    pub samples: usize,
    pub interval_s: f64,
    pub scale: Scale,
    pub format: fn(f64) -> String,
}

/// Range semantics belong to the caller, independently of its unit formatter.
#[derive(Clone, Copy)]
pub enum Scale {
    Percent,
    /// A real capacity; the last tick must equal this value exactly.
    Capacity(f64),
    /// Cover this peak rate in bytes/s, rounded up in binary display units.
    Rate(f64),
    /// Event rates and gauges use decimal counts, with caller-owned units.
    Count(f64),
}

#[derive(Clone, Copy, Debug)]
struct YTicks {
    step: f64,
    intervals: usize,
}

impl YTicks {
    fn for_scale(scale: Scale) -> Self {
        if let Scale::Capacity(capacity) = scale {
            let capacity = if capacity.is_finite() {
                capacity.max(1.0)
            } else {
                1.0
            };
            return Self {
                step: capacity / 4.0,
                intervals: 4,
            };
        }
        let peak = match scale {
            Scale::Rate(peak) | Scale::Count(peak) => peak,
            _ => {
                return Self {
                    step: 25.0,
                    intervals: 4,
                };
            }
        };
        let peak = if peak.is_finite() { peak.max(1.0) } else { 1.0 };
        // Round in the same B/KiB/MiB/... units as the formatter, so the
        // labels read 10/20/30/40 GiB rather than 9.3/18.6/27.9 GiB.
        let mut unit = 1.0;
        for _ in 0..5 {
            if matches!(scale, Scale::Count(_)) || peak / unit < 1024.0 {
                break;
            }
            unit *= 1024.0;
        }
        let target = (peak / unit / 4.0).max(1.0 / unit);
        let magnitude = 10f64.powf(target.log10().floor());
        let factor = [1.0, 2.0, 2.5, 5.0, 10.0]
            .into_iter()
            .find(|factor| factor * magnitude >= target)
            .unwrap_or(10.0);
        let step = (factor * magnitude * unit).max(1.0);
        let intervals = (peak / step).ceil().clamp(2.0, 4.0) as usize;
        Self { step, intervals }
    }

    fn max(self) -> f64 {
        self.step * self.intervals as f64
    }

    fn marks(self) -> Vec<GridMark> {
        (0..=self.intervals)
            .map(|i| GridMark {
                value: self.step * i as f64,
                step_size: self.step,
            })
            .collect()
    }
}

/// Passive zoom state, discarded on a node change and retained per resource
/// while moving between Performance sections on the same node.
#[derive(Clone, Default)]
struct Charts {
    node: Option<String>,
    ranges: HashMap<egui::Id, TimeRange>,
}

fn state_id() -> egui::Id {
    egui::Id::unique("monitor-chart-ranges")
}

pub fn set_node(ctx: &egui::Context, node: Option<&str>) {
    ctx.data_mut(|data| {
        let charts = data.get_temp_mut_or_default::<Charts>(state_id());
        if charts.node.as_deref() != node {
            charts.node = node.map(str::to_owned);
            charts.ranges.clear();
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TimeRange {
    left: f64,
    right: f64,
}

impl TimeRange {
    fn full(window: f64) -> Self {
        Self {
            left: -window,
            right: 0.0,
        }
    }
    fn span(self) -> f64 {
        self.right - self.left
    }
    fn clamped(self, window: f64, minimum: f64) -> Self {
        if !self.left.is_finite() || !self.right.is_finite() {
            return Self::full(window);
        }
        let span = self.span().clamp(minimum.min(window), window);
        let right = self.right.clamp(-window + span, 0.0);
        Self {
            left: (right - span).max(-window),
            right,
        }
    }
    fn zoom(self, factor: f64, pivot: f64, window: f64, minimum: f64) -> Self {
        if !factor.is_finite() || factor <= 0.0 || factor == 1.0 {
            return self;
        }
        let fraction = ((pivot - self.left) / self.span()).clamp(0.0, 1.0);
        let span = (self.span() / factor).clamp(minimum.min(window), window);
        Self {
            left: pivot - fraction * span,
            right: pivot + (1.0 - fraction) * span,
        }
        .clamped(window, minimum)
    }
}

fn tick_label(format: fn(f64) -> String, value: f64) -> String {
    let text = format(value);
    let split = text
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(text.len());
    let number = &text[..split];
    let number = if number.contains('.') {
        number.trim_end_matches('0').trim_end_matches('.')
    } else {
        number
    };
    let unit = text[split..].trim();
    if unit.is_empty() {
        number.to_owned()
    } else {
        format!("{number} {unit}")
    }
}

fn time_tick(value: f64) -> String {
    if value >= 0.0 {
        "0".into()
    } else {
        format!("{:.0}", -value)
    }
}

fn readout_at(chart: &Chart<'_>, x: f64, interval_s: f64, hidden: &ItemIdSet) -> Option<String> {
    if !x.is_finite() || x > 0.0 {
        return None;
    }
    let steps = (-x / interval_s).round() as usize;
    let rows: Vec<_> = chart
        .series
        .iter()
        .filter_map(|series| {
            let n = series.values.len().min(chart.samples);
            if steps < n && !hidden.contains(&ItemId::new(series.name)) {
                let value = series.values[series.values.len() - 1 - steps];
                if !value.is_finite() {
                    return None;
                }
                Some(readout(
                    series.name,
                    value,
                    steps as f64 * interval_s,
                    chart.format,
                ))
            } else {
                None
            }
        })
        .collect();
    (!rows.is_empty()).then(|| rows.join("\n"))
}

fn annotated_readout(
    chart: &Chart<'_>,
    x: f64,
    interval_s: f64,
    hidden: &ItemIdSet,
    annotation: Option<&dyn Fn(usize) -> Option<String>>,
) -> Option<String> {
    let mut text = readout_at(chart, x, interval_s, hidden)?;
    if let Some(extra) = annotation.and_then(|f| f((-x / interval_s).round() as usize)) {
        text.push('\n');
        text.push_str(&extra);
    }
    Some(text)
}

pub fn readout(name: &str, value: f64, age_s: f64, format: fn(f64) -> String) -> String {
    let value = format(value);
    let age = crate::label_with("graph-age", &[("age", &format!("{:.0}", age_s.max(0.0)))]);
    if name.is_empty() {
        format!("{value} · {age}")
    } else {
        format!("{name}: {value} · {age}")
    }
}

fn points(values: &[f64], samples: usize, interval_s: f64) -> Vec<[f64; 2]> {
    let n = values.len().min(samples);
    values[values.len() - n..]
        .iter()
        .enumerate()
        .filter(|(_, value)| value.is_finite())
        .map(|(i, value)| [-((n - 1 - i) as f64) * interval_s, *value])
        .collect()
}

pub fn show(ui: &mut Ui, id: impl egui::AsIdSalt, size: Vec2, chart: Chart<'_>) {
    let _ = render(ui, id, size, chart);
}

pub fn show_titled(
    ui: &mut Ui,
    id: impl egui::AsIdSalt,
    title: &str,
    size: Vec2,
    chart: Chart<'_>,
) {
    let _ = render_custom(ui, id, size, chart, Some(title), None);
}

/// A caller-owned derived readout at the same sample as the plotted data.
/// Keeps ratios and resource semantics out of the chart renderer.
pub fn show_annotated(
    ui: &mut Ui,
    id: impl egui::AsIdSalt,
    title: &str,
    size: Vec2,
    chart: Chart<'_>,
    annotation: &dyn Fn(usize) -> Option<String>,
) {
    let _ = render_custom(ui, id, size, chart, Some(title), Some(annotation));
}

// Return the actual response to renderer tests; do not reconstruct egui_plot's
// private memory key from its id salt. Plotting types stay inside this module.
fn render(ui: &mut Ui, id: impl egui::AsIdSalt, size: Vec2, chart: Chart<'_>) -> PlotResponse<()> {
    render_custom(ui, id, size, chart, None, None)
}

/// Fit entries alongside the title first, then wrap only overflowing entries.
/// Each row is right-aligned; no legend row is taken from the plot's height.
fn header_rows(width: f32, title: f32, entries: &[f32], gap: f32) -> Vec<Vec<usize>> {
    let mut rows = vec![Vec::new()];
    let mut remaining = (width - title - gap).max(0.0);
    for (i, entry) in entries.iter().enumerate() {
        let spacing = if rows.last().unwrap().is_empty() {
            0.0
        } else {
            gap
        };
        if *entry + spacing > remaining && (!rows.last().unwrap().is_empty() || rows.len() == 1) {
            rows.push(Vec::new());
            remaining = width;
        }
        let spacing = if rows.last().unwrap().is_empty() {
            0.0
        } else {
            gap
        };
        rows.last_mut().unwrap().push(i);
        remaining -= entry + spacing;
    }
    rows
}

/// One measured title header. Swatch and text have separate rectangles,
/// rather than relying on whitespace surviving a widget's text layout.
fn title_line(
    ui: &mut Ui,
    plot_id: egui::Id,
    title: &str,
    width: f32,
    series: &[Series<'_>],
    hidden: &mut ItemIdSet,
) {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let height = ui.text_style_height(&egui::TextStyle::Small);
    let swatch = height * 0.4;
    let gap = ui.spacing().item_spacing.x * 0.5;
    let title =
        ui.painter()
            .layout_no_wrap(title.to_owned(), font.clone(), ui.visuals().text_color());
    let entries: Vec<_> = series
        .iter()
        .filter(|_| series.len() > 1)
        .map(|s| {
            let colour = if hidden.contains(&ItemId::new(s.name)) {
                ui.visuals().weak_text_color()
            } else {
                graph::tint(ui, s.hue)
            };
            (
                ui.painter().layout_no_wrap(
                    s.legend_label.unwrap_or(s.name).to_owned(),
                    font.clone(),
                    colour,
                ),
                colour,
            )
        })
        .collect();
    let widths: Vec<_> = entries
        .iter()
        .map(|(g, _)| swatch + gap + g.size().x)
        .collect();
    let rows = header_rows(width, title.size().x, &widths, gap);
    let total_height =
        rows.len() as f32 * height + (rows.len() - 1) as f32 * ui.spacing().item_spacing.y;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, total_height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let title_rect = egui::Rect::from_min_size(rect.min, title.size()).intersect(rect);
    ui.interact(title_rect, plot_id.with("title"), egui::Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, title.text()));
    painter.galley(rect.min, title, ui.visuals().text_color());
    for (row_index, row) in rows.iter().enumerate() {
        let row_width =
            row.iter().map(|&i| widths[i]).sum::<f32>() + row.len().saturating_sub(1) as f32 * gap;
        let mut x = rect.right() - row_width;
        let y = rect.top() + row_index as f32 * (height + ui.spacing().item_spacing.y);
        for &i in row {
            let entry = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(widths[i], height));
            let visible = !hidden.contains(&ItemId::new(series[i].name));
            let response = ui.interact(
                entry.intersect(rect),
                plot_id.with(series[i].name),
                egui::Sense::click(),
            );
            if response.hovered() || response.has_focus() {
                painter.rect_filled(entry, 0.0, ui.visuals().widgets.hovered.bg_fill);
            }
            let centre = egui::pos2(x + swatch * 0.5, entry.center().y);
            painter.circle_filled(centre, swatch * 0.5, entries[i].1);
            painter.galley(
                egui::pos2(x + swatch + gap, y),
                entries[i].0.clone(),
                entries[i].1,
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Button, true, visible, series[i].name)
            });
            let clicked = response.clicked();
            response.on_hover_text(series[i].name);
            if clicked {
                let id = ItemId::new(series[i].name);
                if visible {
                    hidden.insert(id);
                } else {
                    hidden.remove(&id);
                }
            }
            x += widths[i] + gap;
        }
    }
}

fn render_custom(
    ui: &mut Ui,
    id: impl egui::AsIdSalt,
    size: Vec2,
    chart: Chart<'_>,
    title: Option<&str>,
    annotation: Option<&dyn Fn(usize) -> Option<String>>,
) -> PlotResponse<()> {
    let interval_s = if chart.interval_s.is_finite() && chart.interval_s > 0.0 {
        chart.interval_s
    } else {
        1.0
    };
    let window = chart.samples.saturating_sub(1).max(1) as f64 * interval_s;
    let minimum = (4.0 * interval_s).min(window);
    let ticks = YTicks::for_scale(chart.scale);
    let text_height = ui.text_style_height(&egui::TextStyle::Body);
    // Top labels extend above the plot. This padding is part of the supplied
    // height, not an extra row that pushes the rest of the pane down.
    let plot_id = ui.make_persistent_id(id);
    let mut hidden = PlotMemory::load(ui.ctx(), plot_id)
        .map(|memory| memory.hidden_items)
        .unwrap_or_default();
    if let Some(title) = title {
        title_line(ui, plot_id, title, size.x, chart.series, &mut hidden);
    }
    let padding = text_height * 0.5 + ui.spacing().item_spacing.y;
    ui.add_space(padding);
    if let Some(mut memory) = PlotMemory::load(ui.ctx(), plot_id) {
        memory.hidden_items = hidden.clone();
        memory.store(ui.ctx(), plot_id);
    }
    let mut range = ui
        .ctx()
        .data_mut(|data| {
            data.get_temp_mut_or_default::<Charts>(state_id())
                .ranges
                .get(&plot_id)
                .copied()
                .unwrap_or(TimeRange::full(window))
        })
        .clamped(window, minimum);
    let zoomed = range.span() < window - f64::EPSILON * window;
    let pointer_time = Cell::new(None);
    let y_axis = AxisHints::new_y()
        .formatter(|mark, _| tick_label(chart.format, mark.value))
        .tick_label_color(ui.visuals().text_color())
        .label_spacing(0.0..=text_height);
    let plot = Plot::new(plot_id)
        .id(plot_id)
        .width(size.x)
        .height((size.y - padding).max(text_height * 2.0))
        .default_x_bounds(-window, 0.0)
        .default_y_bounds(0.0, ticks.max())
        // egui_plot applies its own navigation after the build callback. Own
        // every input path here so the final clamp precedes axes and painting;
        // no native pan, axis drag, pinch or wheel can bypass the time limits.
        .allow_drag(false)
        .allow_zoom(false)
        .allow_axis_zoom_drag(false)
        .allow_boxed_zoom(false)
        .allow_scroll(false)
        .allow_double_click_reset(false)
        .sense(if zoomed {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::click()
        })
        .show_grid([false, true])
        .show_y(false)
        .x_axis_formatter(|mark, _| time_tick(mark.value))
        .custom_y_axes(vec![y_axis])
        .y_grid_spacer(move |_| ticks.marks())
        .label_formatter(|hover| {
            let position = match hover {
                HoverPosition::NearDataPoint { position, .. }
                | HoverPosition::Elsewhere { position } => position,
            };
            let x = pointer_time.get().unwrap_or(position.x);
            annotated_readout(&chart, x, interval_s, &hidden, annotation)
        });
    let colours: Vec<_> = chart
        .series
        .iter()
        .map(|s| graph::tint(ui, s.hue))
        .collect();
    let output = plot.show(ui, |plot_ui| {
        let response = plot_ui.response();
        let rect = response.rect;
        let reset = response.double_clicked();
        if zoomed && response.dragged_by(egui::PointerButton::Primary) && rect.width() > 0.0 {
            let shift = -f64::from(response.drag_delta().x / rect.width()) * range.span();
            range.left += shift;
            range.right += shift;
        }
        if response.contains_pointer()
            && let Some(pointer) = plot_ui.ctx().input(|input| input.pointer.hover_pos())
        {
            let fraction = f64::from((pointer.x - rect.left()) / rect.width().max(1.0));
            let pivot = range.left + fraction * range.span();
            let factor = plot_ui.ctx().input_mut(|input| {
                let wheel = input.smooth_scroll_delta;
                input.smooth_scroll_delta = Vec2::ZERO;
                f64::from(input.zoom_delta_2d().x) * (f64::from(wheel.x + wheel.y) * 0.01).exp()
            });
            range = range.zoom(factor, pivot, window, minimum);
        }
        if reset {
            range = TimeRange::full(window);
        }
        // Apply after *all* navigation, including the native egui pinch/Ctrl
        // zoom delta. Native plot navigation stays disabled above.
        range = range.clamped(window, minimum);
        pointer_time.set(plot_ui.response().hover_pos().map(|pointer| {
            range.left + f64::from((pointer.x - rect.left()) / rect.width().max(1.0)) * range.span()
        }));
        plot_ui.set_plot_bounds_x(range.left..=range.right);
        plot_ui.set_plot_bounds_y(0.0..=ticks.max());
        for (series, colour) in chart.series.iter().zip(colours) {
            if hidden.contains(&ItemId::new(series.name)) {
                continue;
            }
            plot_ui.line(
                Line::new(
                    series.name,
                    points(series.values, chart.samples, interval_s),
                )
                .color(colour)
                .width(1.5),
            );
        }
    });
    if let Some(mut memory) = PlotMemory::load(ui.ctx(), plot_id) {
        memory.hidden_items = hidden;
        memory.store(ui.ctx(), plot_id);
    }
    ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Charts>(state_id())
            .ranges
            .insert(plot_id, range);
    });
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_legends_stay_above_the_plot_and_toggle_lines_and_readouts() {
        use egui_kittest::{Harness, kittest::Queryable};
        let mut h = Harness::builder()
            .with_size(egui::vec2(380.0, 220.0))
            .build_ui_state(
                |ui, state: &mut (egui::Rect, ItemIdSet, String)| {
                    let series = [
                        Series {
                            legend_label: None,
                            name: "Active opens",
                            hue: Hue::Series(1),
                            values: &[1.0, 2.0],
                        },
                        Series {
                            legend_label: None,
                            name: "Passive opens",
                            hue: Hue::Series(2),
                            values: &[3.0, 4.0],
                        },
                        Series {
                            legend_label: None,
                            name: "Sent resets",
                            hue: Hue::Series(3),
                            values: &[5.0, 6.0],
                        },
                        Series {
                            legend_label: None,
                            name: "Established resets",
                            hue: Hue::Series(4),
                            values: &[7.0, 8.0],
                        },
                    ];
                    let id = ui.make_persistent_id("legend-test");
                    let chart = Chart {
                        series: &series,
                        samples: 60,
                        interval_s: 1.0,
                        scale: Scale::Count(10.0),
                        format: vitals::format::percent,
                    };
                    let output = render_custom(
                        ui,
                        "legend-test",
                        egui::vec2(360.0, 150.0),
                        chart,
                        Some("Rates"),
                        None,
                    );
                    state.0 = *output.transform.frame();
                    state.1 = PlotMemory::load(ui.ctx(), id).unwrap().hidden_items;
                    state.2 = readout_at(
                        &Chart {
                            series: &series,
                            samples: 60,
                            interval_s: 1.0,
                            scale: Scale::Count(10.0),
                            format: vitals::format::percent,
                        },
                        0.0,
                        1.0,
                        &state.1,
                    )
                    .unwrap();
                },
                (egui::Rect::NOTHING, ItemIdSet::default(), String::new()),
            );
        h.run();
        for name in [
            "Active opens",
            "Passive opens",
            "Sent resets",
            "Established resets",
        ] {
            assert!(
                h.get_by_label(name).rect().bottom() <= h.state().0.top(),
                "{name} overlaps the data"
            );
        }
        h.get_by_label("Active opens").click();
        h.run();
        assert!(h.state().1.contains(&ItemId::new("Active opens")));
        assert!(!h.state().2.contains("Active opens"));
        assert!(h.state().2.contains("Passive opens"));
        h.get_by_label("Active opens").click();
        h.run();
        assert!(!h.state().1.contains(&ItemId::new("Active opens")));
        assert!(h.state().2.contains("Active opens"));
    }

    #[test]
    fn title_legends_keep_labelled_y_ticks_plot_height_and_clear_space_for_swatches() {
        use egui::{Rect, Shape};
        use egui_kittest::{Harness, kittest::Queryable};
        fn shapes(shape: &Shape, text: &mut Vec<(String, Rect)>, circles: &mut Vec<Rect>) {
            match shape {
                Shape::Vec(list) => {
                    for shape in list {
                        shapes(shape, text, circles);
                    }
                }
                Shape::Text(t) => text.push((
                    t.galley.text().to_owned(),
                    Rect::from_min_size(t.pos, t.galley.size()),
                )),
                Shape::Circle(c) => circles.push(Rect::from_center_size(
                    c.center,
                    Vec2::splat(c.radius * 2.0),
                )),
                _ => {}
            }
        }
        for theme in [
            toolkit::Theme::embedded(),
            toolkit::Theme::for_context(design::DesignContext::revision_one()),
        ] {
            let mut plain_height: Option<f32> = None;
            for with_legend in [false, true] {
                let theme = theme.clone();
                let mut h = Harness::builder()
                    .with_context_setup(move |ctx| toolkit::install(ctx, &theme))
                    .with_size(egui::vec2(400.0, 280.0))
                    .build_ui_state(
                        move |ui, frame| {
                            let series: Vec<_> = [
                                ("Active opens", "Active"),
                                ("Passive opens", "Passive"),
                                ("Sent resets", "Sent"),
                                ("Established resets", "Estab."),
                            ]
                            .into_iter()
                            .take(if with_legend { 4 } else { 1 })
                            .enumerate()
                            .map(|(i, (name, short))| Series {
                                name,
                                legend_label: Some(short),
                                hue: Hue::Series(i + 1),
                                values: &[10.0, 20.0],
                            })
                            .collect();
                            let output = render_custom(
                                ui,
                                "height-test",
                                egui::vec2(360.0, 130.0),
                                Chart {
                                    series: &series,
                                    samples: 60,
                                    interval_s: 1.0,
                                    scale: Scale::Percent,
                                    format: vitals::format::percent,
                                },
                                Some("Rates"),
                                None,
                            );
                            *frame = *output.transform.frame();
                        },
                        Rect::NOTHING,
                    );
                h.run();
                let mut text = Vec::new();
                let mut circles = Vec::new();
                for shape in &h.output().shapes {
                    shapes(&shape.shape, &mut text, &mut circles);
                }
                for label in ["0 %", "25 %", "50 %", "75 %", "100 %"] {
                    assert!(
                        text.iter().any(|(s, _)| s == label),
                        "missing {label} with legend={with_legend}"
                    );
                }
                if let Some(height) = plain_height {
                    assert!(
                        (h.state().height() - height).abs() < 0.5,
                        "a title legend must not shrink the plot"
                    );
                } else {
                    plain_height = Some(h.state().height());
                }
                if with_legend {
                    let title = text.iter().find(|(s, _)| s == "Rates").unwrap().1;
                    for (name, short) in [
                        ("Active opens", "Active"),
                        ("Passive opens", "Passive"),
                        ("Sent resets", "Sent"),
                        ("Established resets", "Estab."),
                    ] {
                        let hit = h.get_by_label(name).rect();
                        let label = text.iter().find(|(s, _)| s == short).unwrap().1;
                        let swatch = circles.iter().find(|r| hit.contains(r.center())).unwrap();
                        assert!(swatch.right() < label.left(), "swatch overlaps {name}");
                        assert!(
                            (label.top() - title.top()).abs() < 0.5,
                            "legend belongs on the title line"
                        );
                        assert!(label.bottom() < h.state().top());
                    }
                }
            }
        }
    }

    #[test]
    fn tcp_counts_use_decimal_ticks_and_derived_hover_uses_the_same_sample() {
        let ticks = YTicks::for_scale(Scale::Count(1500.0));
        assert_eq!(ticks.step, 500.0);
        assert_eq!(ticks.max(), 1500.0);
        let chart = Chart {
            series: &[Series {
                legend_label: None,
                name: "Retransmits",
                hue: Hue::Warning,
                values: &[2.0, 10.0],
            }],
            samples: 60,
            interval_s: 2.0,
            scale: Scale::Count(10.0),
            format: vitals::format::percent,
        };
        let ratio = |steps| {
            let r = [10.0, 2.0][steps];
            let out = [100.0, 200.0][steps];
            Some(format!(
                "{}%",
                vitals::model::retransmit_percent(r, out).unwrap()
            ))
        };
        let hidden = ItemIdSet::default();
        assert!(
            annotated_readout(&chart, 0.0, 2.0, &hidden, Some(&ratio))
                .unwrap()
                .ends_with("10%")
        );
        assert!(
            annotated_readout(&chart, -2.0, 2.0, &hidden, Some(&ratio))
                .unwrap()
                .ends_with("1%")
        );
        assert!(annotated_readout(&chart, 0.1, 2.0, &hidden, Some(&ratio)).is_none());
    }

    #[test]
    fn time_ranges_are_bounded_and_wheel_zoom_preserves_the_pointer_time() {
        let full = TimeRange::full(60.0);
        let zoomed = full.zoom(2.0, -30.0, 60.0, 4.0);
        assert_eq!(
            zoomed,
            TimeRange {
                left: -45.0,
                right: -15.0
            }
        );
        assert_eq!((zoomed.left + zoomed.right) * 0.5, -30.0);
        for range in [
            TimeRange {
                left: -10.0,
                right: 20.0,
            },
            TimeRange {
                left: -100.0,
                right: -80.0,
            },
            TimeRange {
                left: -1e20,
                right: 1e20,
            },
            zoomed.zoom(1e20, -30.0, 60.0, 4.0),
            zoomed.zoom(1e-20, -30.0, 60.0, 4.0),
        ] {
            let range = range.clamped(60.0, 4.0);
            assert!(range.left >= -60.0 && range.right <= 0.0);
            assert!((4.0..=60.0).contains(&range.span()));
        }
        assert_eq!(time_tick(1.0), "0");
        assert_eq!(time_tick(-0.0), "0");
    }

    #[test]
    fn capacity_ticks_and_whole_unit_labels_keep_the_real_capacity() {
        let ticks = YTicks::for_scale(Scale::Capacity(32.0 * 1024f64.powi(3)));
        assert_eq!(ticks.max(), 32.0 * 1024f64.powi(3));
        let labels: Vec<_> = ticks
            .marks()
            .iter()
            .map(|mark| tick_label(vitals::format::bytes, mark.value))
            .collect();
        assert_eq!(labels, ["0 B", "8 GiB", "16 GiB", "24 GiB", "32 GiB"]);
        assert_eq!(tick_label(vitals::format::percent, 0.0), "0 %");
        assert_eq!(tick_label(vitals::format::percent, 100.0), "100 %");
        assert_eq!(tick_label(vitals::format::percent, 12.5), "12.5 %");
    }

    #[test]
    fn readouts_skip_hidden_series_and_future_positions() {
        let chart = Chart {
            series: &[
                Series {
                    legend_label: None,
                    name: "Receive",
                    hue: Hue::Accent,
                    values: &[1.0, 2.0],
                },
                Series {
                    legend_label: None,
                    name: "Send",
                    hue: Hue::AccentPlus,
                    values: &[3.0, 4.0],
                },
            ],
            samples: 60,
            interval_s: 1.0,
            scale: Scale::Percent,
            format: vitals::format::percent,
        };
        let mut hidden = ItemIdSet::default();
        hidden.insert(ItemId::new("Send"));
        let text = readout_at(&chart, 0.0, 1.0, &hidden).unwrap();
        assert!(text.contains("Receive"));
        assert!(!text.contains("Send"));
        assert!(readout_at(&chart, 0.01, 1.0, &hidden).is_none());
        hidden.insert(ItemId::new("Receive"));
        assert!(readout_at(&chart, 0.0, 1.0, &hidden).is_none());
    }

    #[test]
    fn node_changes_reset_zoom_but_resources_within_a_node_keep_their_ranges() {
        let ctx = egui::Context::default();
        let cpu = egui::Id::unique("test-cpu");
        let net = egui::Id::unique("test-net");
        let range = TimeRange {
            left: -20.0,
            right: -10.0,
        };
        set_node(&ctx, Some("alpha"));
        ctx.data_mut(|data| {
            let charts = data.get_temp_mut_or_default::<Charts>(state_id());
            charts.ranges.insert(cpu, range);
            charts.ranges.insert(net, TimeRange::full(60.0));
        });
        set_node(&ctx, Some("alpha"));
        ctx.data_mut(|data| {
            let charts = data.get_temp_mut_or_default::<Charts>(state_id());
            assert_eq!(charts.ranges[&cpu], range);
            assert_eq!(charts.ranges[&net], TimeRange::full(60.0));
        });
        set_node(&ctx, Some("beta"));
        set_node(&ctx, Some("alpha"));
        ctx.data_mut(|data| {
            assert!(
                data.get_temp_mut_or_default::<Charts>(state_id())
                    .ranges
                    .is_empty()
            )
        });
    }

    #[test]
    fn percentage_ticks_are_fixed_and_byte_ticks_cover_the_range_with_few_marks() {
        let percent = YTicks::for_scale(Scale::Percent);
        assert_eq!(
            percent
                .marks()
                .iter()
                .map(|mark| mark.value)
                .collect::<Vec<_>>(),
            [0.0, 25.0, 50.0, 75.0, 100.0]
        );
        for peak in [
            0.0,
            1.0,
            31.0,
            999.0,
            1024.0,
            5e6,
            32.0 * 1024f64.powi(3),
            u64::MAX as f64,
        ] {
            let ticks = YTicks::for_scale(Scale::Rate(peak));
            let marks = ticks.marks();
            assert!((3..=5).contains(&marks.len()));
            assert!(ticks.max() >= peak);
            assert_eq!(marks.first().unwrap().value, 0.0);
            assert_eq!(marks.last().unwrap().value, ticks.max());
        }
        let memory = YTicks::for_scale(Scale::Rate(32.0 * 1024f64.powi(3)));
        assert_eq!(memory.step, 10.0 * 1024f64.powi(3));
        assert_eq!(memory.max(), 40.0 * 1024f64.powi(3));
    }

    #[test]
    fn short_charts_paint_every_y_tick_label_in_the_resource_unit() {
        use egui::Shape;
        use egui_kittest::Harness;
        fn collect_text(shape: &Shape, labels: &mut Vec<(String, egui::Rect)>) {
            match shape {
                Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect_text(shape, labels);
                    }
                }
                Shape::Text(text) => labels.push((
                    text.galley.text().to_owned(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => {}
            }
        }
        type Formatter = fn(f64) -> String;
        for (scale, format, expected) in [
            (
                Scale::Percent,
                vitals::format::percent as Formatter,
                vec!["0 %", "25 %", "50 %", "75 %", "100 %"],
            ),
            (
                Scale::Capacity(32.0 * 1024f64.powi(3)),
                vitals::format::bytes as Formatter,
                vec!["0 B", "8 GiB", "16 GiB", "24 GiB", "32 GiB"],
            ),
            (
                Scale::Rate(5e6),
                vitals::format::rate as Formatter,
                vec!["0 B/s", "2 MiB/s", "4 MiB/s", "6 MiB/s"],
            ),
        ] {
            let mut h = Harness::builder()
                .with_context_setup(|ctx| toolkit::install(ctx, &toolkit::Theme::embedded()))
                .with_size(egui::vec2(640.0, 200.0))
                .build_ui(move |ui| {
                    ui.small("Subtitle");
                    show(
                        ui,
                        "short-chart",
                        egui::vec2(600.0, 120.0),
                        Chart {
                            series: &[Series {
                                legend_label: None,
                                name: "sample",
                                hue: Hue::Accent,
                                values: &[0.0; 60],
                            }],
                            samples: 60,
                            interval_s: 1.0,
                            scale,
                            format,
                        },
                    );
                });
            h.run();
            let mut labels = Vec::new();
            for shape in &h.output().shapes {
                collect_text(&shape.shape, &mut labels);
            }
            let subtitle = labels
                .iter()
                .find(|(label, _)| label == "Subtitle")
                .unwrap()
                .1;
            for expected in expected {
                assert!(
                    labels.iter().any(|(label, _)| label == expected),
                    "missing y tick {expected}: {labels:?}"
                );
                let tick = labels
                    .iter()
                    .find(|(label, _)| label == expected)
                    .unwrap()
                    .1;
                assert!(
                    tick.top() > subtitle.bottom(),
                    "y tick {expected} touches subtitle"
                );
            }
        }
    }

    #[test]
    fn time_runs_to_zero_and_partial_histories_keep_the_sample_interval() {
        assert_eq!(
            points(&[1.0, 2.0, 3.0], 60, 2.0),
            [[-4.0, 1.0], [-2.0, 2.0], [0.0, 3.0]]
        );
        assert_eq!(points(&[1.0, 2.0, 3.0], 2, 1.0), [[-1.0, 2.0], [0.0, 3.0]]);
        assert!(points(&[], 60, 1.0).is_empty());
    }
    #[test]
    fn readout_uses_monitor_units_and_age_catalogue() {
        assert!(readout("CPU", 12.5, 12.0, vitals::format::percent).contains("12 s ago"));
        assert!(readout("CPU", 12.5, 12.0, vitals::format::percent).contains("12.5%"));
        assert!(readout("", 1024.0, 0.0, vitals::format::rate).contains("/s"));
    }

    #[test]
    fn wheel_zoom_and_drag_change_only_time_and_double_click_resets() {
        use egui::{Event, Modifiers, PointerButton, Pos2, Rect, TouchPhase, vec2};
        use egui_kittest::Harness;
        let mut h = Harness::builder()
            .with_size(vec2(640.0, 320.0))
            // The default 250 ms step puts two press/release pairs 500 ms
            // apart, outside egui's double-click window.
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(120)
            .build_ui_state(
                |ui, state: &mut (Rect, [f64; 2], [f64; 2], bool)| {
                    let plot = render(
                        ui,
                        "test-chart",
                        vec2(600.0, 260.0),
                        Chart {
                            series: &[Series {
                                legend_label: None,
                                name: "CPU",
                                hue: Hue::Accent,
                                values: &[20.0; 60],
                            }],
                            samples: 60,
                            interval_s: 1.0,
                            scale: Scale::Percent,
                            format: vitals::format::percent,
                        },
                    );
                    *state = (
                        *plot.transform.frame(),
                        plot.transform.bounds().min(),
                        plot.transform.bounds().max(),
                        plot.response.double_clicked(),
                    );
                },
                (Rect::NOTHING, [0.0; 2], [0.0; 2], false),
            );
        h.run();
        let initial = *h.state();
        let at = initial.0.center();
        h.hover_at(at);
        h.event(Event::MouseWheel {
            source: egui::MouseWheelSource::Unknown,
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 80.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        h.run_steps(30);
        let zoomed = *h.state();
        assert!(zoomed.2[0] - zoomed.1[0] < initial.2[0] - initial.1[0]);
        assert!(
            ((zoomed.1[0] + zoomed.2[0]) - (initial.1[0] + initial.2[0])).abs() < 0.01,
            "wheel zoom must preserve the time under the centred pointer"
        );
        assert_eq!((zoomed.1[1], zoomed.2[1]), (0.0, 100.0));
        // Pinch supplies the native egui zoom delta to the same bounded path.
        h.event(Event::Zoom(1.25));
        h.step();
        assert!(h.state().2[0] - h.state().1[0] < zoomed.2[0] - zoomed.1[0]);
        assert_eq!((h.state().1[1], h.state().2[1]), (0.0, 100.0));
        let zoomed = *h.state();
        let button = |pos: Pos2, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        h.event(button(at, true));
        h.step();
        let end = at + vec2(50.0, 30.0);
        h.hover_at(end);
        h.step();
        h.event(button(end, false));
        h.run();
        assert!((h.state().1[0] - zoomed.1[0]).abs() > 0.1);
        assert_eq!((h.state().1[1], h.state().2[1]), (0.0, 100.0));
        // Two stationary clicks inside the plot reset the time range.
        let mut saw_reset = false;
        for _ in 0..2 {
            h.event(button(end, true));
            h.step();
            h.event(button(end, false));
            h.step();
            saw_reset |= h.state().3;
        }
        h.run();
        assert!(saw_reset, "the fixture must deliver a plot double-click");
        assert!((h.state().1[0] - initial.1[0]).abs() < 0.01);
        assert!((h.state().2[0] - initial.2[0]).abs() < 0.01);
        assert_eq!((h.state().1[1], h.state().2[1]), (0.0, 100.0));
        h.event(Event::Zoom(1e8));
        h.step();
        assert!((h.state().2[0] - h.state().1[0] - 4.0).abs() < 0.01);
        for delta in [5000.0, -5000.0] {
            let start = h.state().0.center();
            h.hover_at(start);
            h.event(button(start, true));
            h.step();
            let end = start + vec2(delta, 0.0);
            h.hover_at(end);
            h.step();
            assert!(h.state().1[0] >= -59.0 && h.state().2[0] <= 0.0);
            h.event(button(end, false));
            h.step();
        }
        h.hover_at(h.state().0.center());
        h.event(Event::Zoom(1e-8));
        h.step();
        assert_eq!((h.state().1[0], h.state().2[0]), (-59.0, 0.0));
    }

    #[test]
    fn full_window_chart_drags_scroll_the_enclosing_page() {
        use egui::{Event, Modifiers, PointerButton, Rect, TouchPhase, vec2};
        use egui_kittest::Harness;
        let mut h = Harness::builder()
            .with_size(vec2(640.0, 240.0))
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(120)
            .build_ui_state(
                |ui, state: &mut (Rect, f32, bool)| {
                    let scroll = egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            let plot = render(
                                ui,
                                "scrolling-chart",
                                vec2(580.0, 120.0),
                                Chart {
                                    series: &[Series {
                                        legend_label: None,
                                        name: "CPU",
                                        hue: Hue::Accent,
                                        values: &[20.0; 60],
                                    }],
                                    samples: 60,
                                    interval_s: 1.0,
                                    scale: Scale::Percent,
                                    format: vitals::format::percent,
                                },
                            );
                            state.0 = *plot.transform.frame();
                            state.2 = plot.response.dragged();
                            ui.add_space(400.0);
                        });
                    state.1 = scroll.state.clamped_offset().y;
                },
                (Rect::NOTHING, 0.0, false),
            );
        h.run();
        let start = h.state().0.center();
        // The real page uses egui's default OnTouch drag scrolling. Report
        // both touch and its primary pointer events, as native backends do.
        let touch = |phase, pos| Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(1),
            phase,
            pos,
            force: None,
        };
        h.hover_at(start);
        h.event(touch(TouchPhase::Start, start));
        h.event(Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        h.step();
        h.hover_at(start - vec2(0.0, 35.0));
        h.event(touch(TouchPhase::Move, start - vec2(0.0, 35.0)));
        h.step();
        h.hover_at(start - vec2(0.0, 50.0));
        h.event(touch(TouchPhase::Move, start - vec2(0.0, 50.0)));
        h.step();
        assert!(
            !h.state().2,
            "a full-window chart must not capture the drag"
        );
        assert!(
            h.state().1 > 0.0,
            "the enclosing page must receive the drag"
        );
        h.event(Event::PointerButton {
            pos: start - vec2(0.0, 50.0),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        h.event(touch(TouchPhase::End, start - vec2(0.0, 50.0)));
        h.step();
    }
}
