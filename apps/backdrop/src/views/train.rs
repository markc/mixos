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
    snapshot: std::sync::Arc<super::Snapshot>,
    changed: Option<Instant>,
    pending: bool,
    settled: bool,
    scene: std::sync::Arc<Vec<Block>>,
}
pub fn rows<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map_or(&[], Vec::as_slice)
}
fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("?").into()
}
pub fn decoded(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| value.clone())
}
pub fn ellipsis(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit { value.into() }
    else { value.chars().take(limit.saturating_sub(1)).chain(['…']).collect() }
}
pub fn rejections(snapshot: &super::Snapshot) -> Vec<&Value> {
    let mut rejected: Vec<_> = rows(&snapshot.queue, "recent").iter().filter(|r| r["state"] == "rejected").collect();
    rejected.sort_by(|a,b| b["updated"].as_f64().unwrap_or(0.).total_cmp(&a["updated"].as_f64().unwrap_or(0.)));
    rejected
}
pub fn reason(row: &Value) -> String {
    let raw = row["reason"].as_str().unwrap_or("?");
    if raw.contains("conflict") { "conflict".into() }
    else if raw.contains("75") { "infra rc 75".into() }
    else if raw.contains("red") { "red: release gate".into() }
    else { ellipsis(raw, 40) }
}
impl Train {
    /// Pure snapshot → world-space scene. No GPU, file or Bus access.
    pub fn scene(&self) -> Vec<Block> {
        let mut out = vec![];
        let gate = self.snapshot.current_gate();
        let failing = gate.is_some_and(|g| g["failing"] == true);
        for (x, tone) in [(-5., 0), (0., 1), (5., 2)] {
            out.push(Block {
                position: [x, 0., 0.],
                size: [4.8, 0.12, 0.6],
                tone: if x == 0. && failing { 3 } else { tone },
                glow: 0.,
                label: if x == 0. {
                    gate.and_then(|g| g["last"].as_str()).unwrap_or("").into()
                } else {
                    String::new()
                },
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
            for (i, row) in rows(&self.snapshot.queue, key)
                .iter()
                .filter(|row| {
                    key != "recent" || row["state"] == "landed"
                })
                .take(12)
                .enumerate()
            {
                let rejected = row["state"] == "rejected";
                let progress = self.snapshot.gate_progress().unwrap_or(0.);
                out.push(Block {
                    position: [
                        x + i as f32 * 0.55 + if key == "batched" { progress * 3. } else { 0. },
                        0.35,
                        if rejected { 2. } else { z },
                    ],
                    size: [0.48, 0.6, 0.75],
                    tone: if rejected || (key == "batched" && failing) {
                        3
                    } else {
                        tone
                    },
                    glow: 0.,
                    label: text(row, "branch"),
                });
            }
        }
        for (i, row) in rejections(&self.snapshot).into_iter().take(3).enumerate() {
            out.push(Block { position: [4. + i as f32, 0.35, 2.], size: [0.48, 0.6, 0.75], tone: 3, glow: 0., label: text(row, "branch") });
        }
        let mut seen = std::collections::BTreeSet::new();
        for slot in rows(&self.snapshot.workers, "slots") {
            let name = text(slot, "worker");
            if !seen.insert(name.clone()) {
                continue;
            }
            let i = seen.len() - 1;
            if i >= 3 {
                break;
            }
            let jobs: Vec<_> = rows(&self.snapshot.workers, "running")
                .iter()
                .filter(|job| job["worker"] == slot["worker"]).collect();
            let busy = !jobs.is_empty();
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
            let mut label = if slot["light"] == 1 { "light".into() } else { name };
            for job in jobs {
                let spec = decoded(&job["spec"]);
                label.push_str(&format!("\n{} · {:.0} min", ellipsis(spec["label"].as_str().unwrap_or("?"), 28), (now - job["started"].as_f64().unwrap_or(now)).max(0.) / 60.));
            }
            let load = decoded(&slot["snapshot"])["load1"].as_f64().unwrap_or(0.) as f32;
            out.push(Block {
                position: [-4. + i as f32 * 4., 0.6, -3.],
                size: [2., 1.2, 1.2],
                tone: if busy { 1 } else { 0 },
                glow: (load / 16. + if busy { 0.5 } else { 0. }).clamp(0., 1.),
                label,
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
            "pipeline.alert",
            "pipeline.clear",
        ]
    }
    fn on_event(&mut self, snapshot: std::sync::Arc<super::Snapshot>, visible: bool, now: Instant) {
        self.snapshot = snapshot;
        self.pending = true;
        self.settled = false;
        if visible {
            self.changed = Some(now);
        } else {
            self.changed = None;
        }
        // No scene construction, layout, sample processing or GPU work here.
    }
    fn wants_frame(&self, visible: bool, _now: Instant) -> Option<Duration> {
        if visible && (self.pending || (self.changed.is_some() && !self.settled)) {
            Some(Duration::from_millis(100))
        } else {
            None
        }
    }
    fn draw(&mut self, visible: bool, now: Instant) -> Option<super::Frame> {
        if !visible {
            return None;
        }
        if self.pending {
            self.scene = std::sync::Arc::new(self.scene());
            self.pending = false;
        }
        self.settled = self
            .changed
            .is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_millis(600));
        Some(super::Frame {
            snapshot: self.snapshot.clone(),
            scene: self.scene.clone(),
            progress: self.changed.map_or(1., |t| {
                (now.saturating_duration_since(t).as_secs_f32() / 0.6).min(1.)
            }),
        })
    }
}
#[cfg(test)]
impl Train {
    pub fn cached_scene_is_empty(&self) -> bool {
        self.scene.is_empty()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn rejection_order_and_unicode_truncation_are_bounded() {
        let snapshot = super::super::Snapshot { queue: json!({"recent":[{"state":"rejected","branch":"old","updated":1},{"state":"landed","updated":9},{"state":"rejected","branch":"new","updated":4}]}), ..Default::default() };
        let rows = rejections(&snapshot);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["branch"], "new");
        assert_eq!(ellipsis("éééé", 3), "éé…");
    }
    #[test]
    fn recent_limit_counts_only_landings_and_rejections() {
        let mut rows = vec![json!({"state":"cancelled"}); 20];
        rows.push(json!({"state":"landed","branch":"work/landed"}));
        let mut train = Train::default();
        train.on_event(
            std::sync::Arc::new(super::super::Snapshot {
                queue: json!({"recent":rows}),
                ..Default::default()
            }),
            true,
            Instant::now(),
        );
        assert!(train.scene().iter().any(|b| b.label == "work/landed"));
    }
    #[test]
    fn layout_routes_rejections_and_bounds_progress_and_queue() {
        let mut train = Train::default();
        train.on_event(std::sync::Arc::new(super::super::Snapshot {queue:json!({"queued":[{"branch":"work/a"}], "batched":[{"branch":"work/b"}], "batches":[{"batch":"test","state":"gating"}],
            "recent":[{"branch":"work/c", "state":"rejected"}, {"branch":"work/d","state":"landed"}]}), gate:json!({"batch":"test","stage":9,"stages":3,"last":"lint","failing":true}), ..Default::default()}), true, Instant::now());
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
        assert_eq!(scene.iter().find(|b| b.label == "lint").unwrap().tone, 3);
    }
    #[test]
    fn worker_glow_accepts_encoded_snapshots() {
        let mut train = Train::default();
        train.on_event(std::sync::Arc::new(super::super::Snapshot {workers:json!({"slots":[{"worker":"heavy", "snapshot":"{\"load1\":8}"}], "running":[{"worker":"heavy"}]}), ..Default::default()}), true, Instant::now());
        assert_eq!(train.scene().last().unwrap().glow, 1.);
    }
}
