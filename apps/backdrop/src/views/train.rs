// SPDX-License-Identifier: MIT OR Apache-2.0
use super::View;
use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Block {
    pub position: [f32; 3],
    pub size: [f32; 3],
    pub tone: usize,
    pub glow: f32,
    pub label: String,
}

#[derive(Default)]
pub struct Train {
    queue: Value,
    workers: Value,
    changed: Option<Instant>,
    // Live snapshots are a prototype stub, never presented as owner history.
    samples: Vec<(f64, f64, f64)>,
    pub error: Option<String>,
}
fn rows<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map_or(&[], Vec::as_slice)
}
fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("?").into()
}
fn decoded(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| value.clone())
}
impl Train {
    /// Pure snapshot → world-space scene. No GPU, file or Bus access.
    pub fn scene(&self) -> Vec<Block> {
        let mut out = vec![];
        for (x, tone) in [(-5., 0), (0., 1), (5., 2)] {
            out.push(Block {
                position: [x, 0., 0.],
                size: [4.8, 0.12, 0.6],
                tone,
                glow: 0.,
                label: String::new(),
            });
        }
        out.push(Block {
            position: [4., 0., 2.],
            size: [5., 0.12, 0.6],
            tone: 3,
            glow: 0.,
            label: String::new(),
        });
        for (key, x, z, tone) in [
            ("queued", -7., 0., 0),
            ("batched", -2., 0., 1),
            ("recent", 3., 0., 2),
        ] {
            for (i, row) in rows(&self.queue, key).iter().take(12).enumerate() {
                let rejected = row["state"] == "rejected";
                if key == "recent" && !rejected && row["state"] != "landed" {
                    continue;
                }
                let progress = self.queue["stage_progress"]
                    .as_f64()
                    .unwrap_or(0.)
                    .clamp(0., 1.) as f32;
                out.push(Block {
                    position: [
                        x + i as f32 * 0.55 + if key == "batched" { progress * 3. } else { 0. },
                        0.35,
                        if rejected { 2. } else { z },
                    ],
                    size: [0.48, 0.6, 0.75],
                    tone: if rejected { 3 } else { tone },
                    glow: 0.,
                    label: text(row, "branch"),
                });
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for slot in rows(&self.workers, "slots") {
            let name = text(slot, "worker");
            if !seen.insert(name.clone()) {
                continue;
            }
            let i = seen.len() - 1;
            if i >= 3 {
                break;
            }
            let busy = rows(&self.workers, "running")
                .iter()
                .any(|job| job["worker"] == slot["worker"]);
            let load = decoded(&slot["snapshot"])["load1"].as_f64().unwrap_or(0.) as f32;
            out.push(Block {
                position: [-4. + i as f32 * 4., 0.6, -3.],
                size: [2., 1.2, 1.2],
                tone: if busy { 1 } else { 0 },
                glow: (load / 16. + if busy { 0.5 } else { 0. }).clamp(0., 1.),
                label: if slot["light"] == 1 {
                    "light".into()
                } else {
                    name
                },
            });
        }
        out
    }
}
impl View for Train {
    fn subscriptions(&self) -> &'static [&'static str] {
        &[
            "train.landed",
            "train.rejected",
            "buildd.started",
            "buildd.done",
        ]
    }
    fn on_event(&mut self, topic: &str, data: Value, now: Instant) -> bool {
        let target = match topic {
            "train.queue" => &mut self.queue,
            "buildd.queue" => &mut self.workers,
            _ => return false,
        };
        if *target == data {
            return false;
        }
        *target = data;
        self.changed = Some(now);
        if topic == "train.queue" {
            let hour = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                / 3600.;
            let gates = rows(&self.queue, "batches").len() as f64;
            let landed = rows(&self.queue, "recent")
                .iter()
                .filter(|r| r["state"] == "landed")
                .count() as f64;
            self.samples.retain(|s| s.0 >= hour - 24.);
            if self.samples.len() >= 8640 {
                self.samples.remove(0);
            }
            self.samples.push((hour, gates, landed));
        }
        true
    }
    fn wants_frame(&self, now: Instant) -> Option<Duration> {
        self.changed
            .filter(|t| now.saturating_duration_since(*t) < Duration::from_millis(600))
            .map(|_| Duration::from_millis(100))
    }
    fn draw(&self, ui: &mut egui::Ui) {
        let strings = toolkit::Strings::new(include_str!("../../i18n/en/backdrop.ftl"));
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0., ui.visuals().panel_fill);
        let scene = self.scene();
        let progress = self
            .changed
            .map_or(1., |t| (t.elapsed().as_secs_f32() / 0.6).min(1.));
        super::train_gpu::draw(ui, &scene, progress);
        egui::Area::new(egui::Id::unique("train-header"))
            .fixed_pos(rect.min + egui::vec2(24., 24.))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.heading(strings.get("train-title"));
                    ui.label(strings.with(
                        "train-header",
                        &[
                            ("sha", &text(&self.queue, "main_sha")),
                            (
                                "state",
                                &strings.get(if self.queue.is_null() {
                                    "train-unknown"
                                } else if self.queue["paused"].is_null() {
                                    "train-running"
                                } else {
                                    "train-paused"
                                }),
                            ),
                        ],
                    ));
                    ui.label(strings.get("train-track"));
                    if self.queue["stage_progress"].is_null() {
                        ui.label(strings.get("train-stage-unavailable"));
                    }
                    if let Some(error) = &self.error {
                        ui.label(error);
                    }
                });
            });
        egui::Area::new(egui::Id::unique("train-chart"))
            .fixed_pos(egui::pos2(rect.left() + 24., rect.bottom() - 220.))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(strings.get("train-history-stub"));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64()
                        / 3600.;
                    egui_plot::Plot::new("train-live-samples")
                        .width(rect.width().min(640.) - 48.)
                        .height(140.)
                        .include_x(-24.)
                        .include_x(0.)
                        .allow_zoom(false)
                        .allow_drag(false)
                        .allow_scroll(false)
                        .show(ui, |plot| {
                            for (name, landing) in
                                [("train-gates", false), ("train-landings", true)]
                            {
                                let points: Vec<[f64; 2]> = self
                                    .samples
                                    .iter()
                                    .map(|s| [s.0 - now, if landing { s.2 } else { s.1 }])
                                    .collect();
                                plot.line(egui_plot::Line::new(strings.get(name), points));
                            }
                        });
                });
            });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn layout_routes_rejections_and_bounds_progress_and_queue() {
        let mut train = Train::default();
        train.on_event("train.queue", json!({"queued":[{"branch":"work/a"}], "batched":[{"branch":"work/b"}], "stage_progress":9,
            "recent":[{"branch":"work/c", "state":"rejected"}, {"branch":"work/d","state":"landed"}]}), Instant::now());
        let scene = train.scene();
        assert_eq!(
            scene.iter().find(|b| b.label == "work/c").unwrap().position[2],
            2.
        );
        assert_eq!(
            scene.iter().find(|b| b.label == "work/b").unwrap().position[0],
            1.
        );
        assert_eq!(scene.iter().find(|b| b.label == "work/d").unwrap().tone, 2);
    }
    #[test]
    fn worker_glow_accepts_encoded_snapshots() {
        let mut train = Train::default();
        train.on_event("buildd.queue", json!({"slots":[{"worker":"heavy", "snapshot":"{\"load1\":8}"}], "running":[{"worker":"heavy"}]}), Instant::now());
        assert_eq!(train.scene().last().unwrap().glow, 1.);
    }
}
