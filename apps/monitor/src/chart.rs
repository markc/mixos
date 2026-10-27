// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The replaceable large-chart renderer. The engine supplies the same rings
//! used by painted sparklines; plotting types never enter that data path.
use egui::{Ui, Vec2};
use egui_plot::{AxisHints, GridMark, HoverPosition, Legend, Line, Plot, PlotResponse};
use toolkit::graph::{self, Hue};

pub struct Series<'a> {
    pub name: &'a str,
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
    /// Cover this many bytes (or bytes/s), rounded up in binary display units.
    Bytes(f64),
}

#[derive(Clone, Copy, Debug)]
struct YTicks {
    step: f64,
    intervals: usize,
}

impl YTicks {
    fn for_scale(scale: Scale) -> Self {
        let Scale::Bytes(peak) = scale else {
            return Self {
                step: 25.0,
                intervals: 4,
            };
        };
        let peak = if peak.is_finite() { peak.max(1.0) } else { 1.0 };
        // Round in the same B/KiB/MiB/... units as the formatter, so the
        // labels read 10/20/30/40 GiB rather than 9.3/18.6/27.9 GiB.
        let mut unit = 1.0;
        for _ in 0..5 {
            if peak / unit < 1024.0 {
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
        .map(|(i, value)| [-((n - 1 - i) as f64) * interval_s, *value])
        .collect()
}

pub fn show(ui: &mut Ui, id: impl egui::AsIdSalt, size: Vec2, chart: Chart<'_>) {
    let _ = render(ui, id, size, chart);
}

// Return the actual response to renderer tests; do not reconstruct egui_plot's
// private memory key from its id salt. Plotting types stay inside this module.
fn render(ui: &mut Ui, id: impl egui::AsIdSalt, size: Vec2, chart: Chart<'_>) -> PlotResponse<()> {
    let interval_s = if chart.interval_s.is_finite() && chart.interval_s > 0.0 {
        chart.interval_s
    } else {
        1.0
    };
    let duration = chart.samples.saturating_sub(1).max(1) as f64 * interval_s;
    let ticks = YTicks::for_scale(chart.scale);
    let text_height = ui.text_style_height(&egui::TextStyle::Body);
    // The top tick is centred on the plot edge and extends upwards by half
    // a line. Reserve that overhang plus the theme's normal gap below subtitles.
    ui.add_space(text_height * 0.5 + ui.spacing().item_spacing.y);
    let y_axis = AxisHints::new_y()
        .formatter(|mark, _| (chart.format)(mark.value))
        .tick_label_color(ui.visuals().text_color())
        // All of our marks are labelled. The default 20–30 pt fade hides
        // otherwise well-spaced text in the shorter Performance charts.
        .label_spacing(0.0..=text_height);
    let time_only = egui::Vec2b::new(true, false);
    let mut plot = Plot::new(id)
        .width(size.x)
        // Leave room for every tick and the existing x axis, including at
        // larger desktop text scales. Never squeeze five labels together.
        .height(size.y.max((ticks.intervals as f32 + 3.0) * text_height))
        .default_x_bounds(-duration, 0.0)
        .default_y_bounds(0.0, ticks.max())
        .allow_drag(time_only)
        .allow_zoom(time_only)
        .allow_axis_zoom_drag(time_only)
        .allow_boxed_zoom(false)
        .allow_scroll(time_only)
        // Reset x explicitly below; the native reset enables both auto bounds.
        .allow_double_click_reset(false)
        .show_grid([false, true])
        .show_y(false)
        .x_axis_label(crate::label("graph-seconds-ago"))
        .x_axis_formatter(|mark, _| format!("{:.0}", -mark.value))
        .custom_y_axes(vec![y_axis])
        .y_grid_spacer(move |_| ticks.marks())
        .label_formatter(|hover| {
            let position = match hover {
                HoverPosition::NearDataPoint { position, .. }
                | HoverPosition::Elsewhere { position } => position,
            };
            // Snap by time and show every series at that sample, even when
            // the pointer is between lines. Never imply interpolated readings.
            let steps = (-position.x / interval_s).round().max(0.0) as usize;
            let rows: Vec<_> = chart
                .series
                .iter()
                .filter_map(|series| {
                    let n = series.values.len().min(chart.samples);
                    if steps < n {
                        Some(readout(
                            series.name,
                            series.values[series.values.len() - 1 - steps],
                            steps as f64 * interval_s,
                            chart.format,
                        ))
                    } else {
                        None
                    }
                })
                .collect();
            (!rows.is_empty()).then(|| rows.join("\n"))
        });
    if chart.series.len() > 1 {
        plot = plot.legend(Legend::default());
    }
    let colours: Vec<_> = chart
        .series
        .iter()
        .map(|s| graph::tint(ui, s.hue))
        .collect();
    plot.show(ui, |plot_ui| {
        // The resource scale follows incoming data while x stays where the
        // person put it. Bounds changes never reset their time range.
        plot_ui.set_plot_bounds_y(0.0..=ticks.max());
        let reset_time = plot_ui.response().double_clicked();
        // egui_plot's native zoom handles pinch/Ctrl-wheel. Plain wheel also
        // zooms x, centred at the pointer; consume it before the pane scrolls.
        if plot_ui.response().hovered() {
            let scroll = plot_ui.ctx().input_mut(|input| {
                if input.modifiers.ctrl || input.modifiers.command {
                    return 0.0;
                }
                let delta = input.smooth_scroll_delta.y;
                input.smooth_scroll_delta.y = 0.0;
                delta
            });
            if !reset_time
                && scroll != 0.0
                && let Some(pointer) = plot_ui.pointer_coordinate()
            {
                // PlotUi::zoom_bounds modifies both axes and their auto-bound
                // state. Set just x, so wheel input cannot change y at all.
                let bounds = plot_ui.plot_bounds();
                let factor = f64::from((scroll * 0.01).exp());
                let left = pointer.x + (bounds.min()[0] - pointer.x) / factor;
                let right = pointer.x + (bounds.max()[0] - pointer.x) / factor;
                if left.is_finite() && right.is_finite() && left < right {
                    plot_ui.set_plot_bounds_x(left..=right);
                }
            }
        }
        if reset_time {
            // Apply after navigation so reset wins over wheel input in this pass.
            plot_ui.set_plot_bounds_x(-duration..=0.0);
        }
        for (series, colour) in chart.series.iter().zip(colours) {
            plot_ui.line(
                Line::new(
                    series.name,
                    points(series.values, chart.samples, interval_s),
                )
                .color(colour)
                .width(1.5),
            );
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let ticks = YTicks::for_scale(Scale::Bytes(peak));
            let marks = ticks.marks();
            assert!((3..=5).contains(&marks.len()));
            assert!(ticks.max() >= peak);
            assert_eq!(marks.first().unwrap().value, 0.0);
            assert_eq!(marks.last().unwrap().value, ticks.max());
        }
        let memory = YTicks::for_scale(Scale::Bytes(32.0 * 1024f64.powi(3)));
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
                vec!["0.0%", "25.0%", "50.0%", "75.0%", "100.0%"],
            ),
            (
                Scale::Bytes(32.0 * 1024f64.powi(3)),
                vitals::format::bytes as Formatter,
                vec!["0 B", "10.0 GiB", "20.0 GiB", "30.0 GiB", "40.0 GiB"],
            ),
            (
                Scale::Bytes(5e6),
                vitals::format::rate as Formatter,
                vec!["0 B/s", "2.0 MiB/s", "4.0 MiB/s", "6.0 MiB/s"],
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
        assert_eq!((zoomed.1[1], zoomed.2[1]), (0.0, 100.0));
        // Pinch uses egui_plot's native zoom path rather than our wheel path.
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
    }
}
