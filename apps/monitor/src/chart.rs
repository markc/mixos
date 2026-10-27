// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The replaceable large-chart renderer. The engine supplies the same rings
//! used by painted sparklines; plotting types never enter that data path.
use egui::{Ui, Vec2};
use egui_plot::{HoverPosition, Legend, Line, Plot};
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
    pub max: f64,
    pub format: fn(f64) -> String,
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
    let interval_s = if chart.interval_s.is_finite() && chart.interval_s > 0.0 {
        chart.interval_s
    } else {
        1.0
    };
    let duration = chart.samples.saturating_sub(1).max(1) as f64 * interval_s;
    let mut plot = Plot::new(id)
        .width(size.x)
        .height(size.y)
        .default_x_bounds(-duration, 0.0)
        .default_y_bounds(0.0, chart.max.max(1.0))
        .allow_drag([true, false])
        .allow_zoom([true, false])
        .allow_boxed_zoom(false)
        .allow_scroll(false)
        .allow_double_click_reset(true)
        .show_grid([false, true])
        .show_y(false)
        .x_axis_label(crate::label("graph-seconds-ago"))
        .x_axis_formatter(|mark, _| format!("{:.0}", -mark.value))
        .y_axis_formatter(|mark, _| (chart.format)(mark.value))
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
        plot_ui.set_plot_bounds_y(0.0..=chart.max.max(1.0));
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
            if scroll != 0.0 {
                plot_ui.zoom_bounds_around_hovered(Vec2::new((scroll * 0.01).exp(), 1.0));
            }
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
    });
}

#[cfg(test)]
mod tests {
    use super::*;
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
            .build_ui_state(
                |ui, state: &mut (Rect, [f64; 2], [f64; 2])| {
                    let id = ui.make_persistent_id("test-chart");
                    show(
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
                            max: 100.0,
                            format: vitals::format::percent,
                        },
                    );
                    let memory = egui_plot::PlotMemory::load(ui.ctx(), id).unwrap();
                    *state = (
                        *memory.transform().frame(),
                        memory.bounds().min(),
                        memory.bounds().max(),
                    );
                },
                (Rect::NOTHING, [0.0; 2], [0.0; 2]),
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
        // Two stationary clicks inside the plot use egui_plot's reset path.
        for _ in 0..2 {
            h.event(button(end, true));
            h.step();
            h.event(button(end, false));
            h.step();
        }
        h.run();
        assert!((h.state().1[0] - initial.1[0]).abs() < 0.01);
        assert!((h.state().2[0] - initial.2[0]).abs() < 0.01);
    }
}
