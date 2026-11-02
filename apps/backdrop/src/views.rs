// SPDX-License-Identifier: MIT OR Apache-2.0
//! Live views use Bus snapshots only. The wallpaper engine remains beside them.
mod history;
pub mod host;
mod reader;
mod train;
mod train_gpu;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
pub use train::Train;

pub trait View {
    fn subscriptions(&self) -> &'static [&'static str];
    fn on_event(&mut self, snapshot: Arc<Snapshot>, visible: bool, now: Instant);
    /// Pure drawing description. Only the host has access to egui or the GPU.
    fn draw(&mut self, visible: bool, now: Instant) -> Option<Frame>;
    fn wants_frame(&self, visible: bool, now: Instant) -> Option<Duration>;
}

#[derive(Clone, PartialEq)]
pub struct Snapshot {
    pub worker_minute: u64,
    pub alerts: std::collections::BTreeMap<String, Value>,
    pub queue: Value,
    pub workers: Value,
    pub error: Option<String>,
    pub history: Value,
    pub gate: Value,
    pub hours: u16,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            worker_minute: 0,
            alerts: Default::default(),
            queue: Value::Null,
            workers: Value::Null,
            error: None,
            history: Value::Null,
            gate: Value::Null,
            hours: 24,
        }
    }
}
impl Snapshot {
    pub fn alert(&mut self, topic: &str, body: &Value) {
        if topic == "pipeline.alerts" {
            if let Some(alerts) = body["alerts"].as_array() {
                self.alerts.clear();
                for alert in alerts {
                    let mut alert = alert.clone();
                    alert["at"] = body["at"].clone();
                    self.alert("pipeline.alert", &alert);
                }
            }
            return;
        }
        let Some(check) = body["check"].as_str().filter(|s| !s.is_empty()) else {
            return;
        };
        if !body["at"].is_number() && !body["at"].is_string() {
            return;
        }
        if self.alerts.get(check).is_some_and(|old| {
            old["at"]
                .as_f64()
                .zip(body["at"].as_f64())
                .is_some_and(|(a, b)| a > b)
        }) {
            return;
        }
        if topic == "pipeline.clear" {
            self.alerts.remove(check);
        } else if body["text"].is_string() {
            self.alerts.insert(check.into(), body.clone());
        }
    }
    pub fn gating_batch(&self) -> Option<&str> {
        self.queue["batches"]
            .as_array()?
            .iter()
            .find(|b| b["state"] == "gating")?["batch"]
            .as_str()
    }
    pub fn current_gate(&self) -> Option<&Value> {
        (self.gate["batch"].as_str() == Some(self.gating_batch()?)).then_some(&self.gate)
    }
    pub fn gate_progress(&self) -> Option<f32> {
        let gate = self.current_gate()?;
        let stages = gate["stages"]
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.)?;
        Some((gate["stage"].as_f64()?.max(0.) / stages).clamp(0., 1.) as f32)
    }
}
pub struct Frame {
    pub snapshot: Arc<Snapshot>,
    pub scene: Arc<Vec<train::Block>>,
    pub progress: f32,
}

pub struct State {
    pub refresh: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    pub selected: String,
    pub train_service: String,
    pub buildd_service: String,
    pub snapshot: Arc<Snapshot>,
    pub hosts: std::collections::BTreeMap<u64, Arc<Mutex<host::Host<Train>>>>,
    pub renderer_2d: bool,
    pub hours: u16,
    pub history_generation: u64,
}
impl Default for State {
    fn default() -> Self {
        Self {
            refresh: Default::default(),
            selected: "wallpaper".into(),
            train_service: "train".into(),
            buildd_service: "buildd".into(),
            snapshot: Arc::default(),
            hosts: Default::default(),
            renderer_2d: false,
            hours: 24,
            history_generation: 0,
        }
    }
}
impl State {
    pub fn command(&mut self, command: &str, body: &Value) -> Result<(), String> {
        let object = body.as_object().ok_or("expected an object")?;
        match command {
            "backdrop.view.set" => {
                if object
                    .keys()
                    .any(|k| !["view", "args"].contains(&k.as_str()))
                {
                    return Err("unknown field".into());
                }
                let view = body["view"].as_str().ok_or("view is required")?;
                if !["wallpaper", "train"].contains(&view) {
                    return Err("unknown view".into());
                }
                let mut train = "train".to_owned();
                let mut buildd = "buildd".to_owned();
                let mut hours = self.hours;
                if let Some(args) = object.get("args") {
                    let args = args.as_object().ok_or("args must be an object")?;
                    for (key, value) in args {
                        if key == "zoom" {
                            hours = match value.as_str() {
                                Some("1h") => 1,
                                Some("24h") => 24,
                                Some("7d") => 168,
                                _ => return Err("zoom must be 1h, 24h or 7d".into()),
                            };
                            continue;
                        }
                        if key == "renderer" {
                            if !matches!(value.as_str(), Some("2d" | "3d")) {
                                return Err("renderer must be 2d or 3d".into());
                            }
                            continue;
                        }
                        let target = value
                            .as_str()
                            .filter(|s| !s.is_empty() && s.len() <= 256)
                            .ok_or("invalid service name")?;
                        match key.as_str() {
                            "train_service" => train = target.into(),
                            "buildd_service" => buildd = target.into(),
                            _ => return Err("unknown view argument".into()),
                        }
                    }
                }
                if train != self.train_service
                    || buildd != self.buildd_service
                    || hours != self.hours
                {
                    self.snapshot = Arc::new(Snapshot {
                        hours,
                        ..Default::default()
                    });
                }
                self.hours = hours;
                if let Some(renderer) = body["args"]["renderer"].as_str() {
                    self.renderer_2d = renderer == "2d";
                }
                self.train_service = train;
                self.buildd_service = buildd;
                self.selected = view.into();
            }
            "backdrop.view.cycle" => {
                if object.len() != 1 || !matches!(body["dir"].as_str(), Some("next" | "prev")) {
                    return Err("dir must be next or prev".into());
                }
                self.selected = if self.selected == "train" {
                    "wallpaper"
                } else {
                    "train"
                }
                .into();
            }
            _ => return Err("unknown view command".into()),
        }
        Ok(())
    }
}

/// Parameterised commands share the engine registry used by macros and Bus.
pub fn registry() -> toolkit::action::EngineRegistry<State> {
    use toolkit::action::{Class, EngineAction, EngineRegistry, Error, ParamSpec};
    let mut registry = EngineRegistry::new();
    for (id, label, run) in [
        (
            "backdrop.view.set",
            "backdrop-view-set",
            set as fn(&mut State, &Value) -> Result<Value, Error>,
        ),
        (
            "backdrop.view.cycle",
            "backdrop-view-cycle",
            cycle as fn(&mut State, &Value) -> Result<Value, Error>,
        ),
    ] {
        registry.add(EngineAction {
            id,
            label,
            menu: None,
            submenu: None,
            group: 0,
            shortcut: None,
            icon: None,
            enabled: |_| Ok(()),
            run,
            class: Class::Intent,
            params: Some(ParamSpec {
                type_name: "backdrop::ViewRequest",
                schema: || {
                    serde_json::json!({"type":"object"})
                        .try_into()
                        .expect("object schema")
                },
            }),
        });
    }
    registry
}
fn set(state: &mut State, params: &Value) -> Result<Value, toolkit::action::Error> {
    execute(state, "backdrop.view.set", params)
}
fn cycle(state: &mut State, params: &Value) -> Result<Value, toolkit::action::Error> {
    execute(state, "backdrop.view.cycle", params)
}
fn execute(
    state: &mut State,
    id: &'static str,
    params: &Value,
) -> Result<Value, toolkit::action::Error> {
    state
        .command(id, params)
        .map_err(|msg| toolkit::action::Error::BadParams { id, msg })?;
    Ok(serde_json::json!({"view":state.selected}))
}

pub fn refresh(shared: &std::sync::Mutex<crate::program::Shared>) {
    let signal = crate::program::lock(shared).live.refresh.clone();
    *crate::program::lock(&signal.0) = true;
    signal.1.notify_one();
}

/// All live-view repaint requests go through an output host, never a reader or
/// view. Snapshot delivery while hidden only replaces the pending Arc.
pub fn notify(shared: &Mutex<crate::program::Shared>, force: bool) -> bool {
    let (active, snapshot, hosts) = {
        let state = crate::program::lock(shared);
        (
            state.live.selected == "train",
            state.live.snapshot.clone(),
            state.live.hosts.values().cloned().collect::<Vec<_>>(),
        )
    };
    for host in hosts {
        let mut host = crate::program::lock(&host);
        host.active = active;
        if active {
            host.event(snapshot.clone(), force, Instant::now());
        }
    }
    active
}

pub use reader::start_reader;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn alerts_update_and_clear_by_check() {
        let mut snapshot = Snapshot::default();
        snapshot.alert(
            "pipeline.alert",
            &json!({"check":"gate","text":"stalled","at":2}),
        );
        snapshot.alert(
            "pipeline.alert",
            &json!({"check":"gate","text":"old","at":1}),
        );
        assert_eq!(snapshot.alerts["gate"]["text"], "stalled");
        snapshot.alert(
            "pipeline.alert",
            &json!({"check":"gold","text":"behind","at":2}),
        );
        snapshot.alert("pipeline.clear", &json!({"check":"gate","at":3}));
        assert!(!snapshot.alerts.contains_key("gate"));
        assert_eq!(snapshot.alerts.len(), 1);
        snapshot.alert(
            "pipeline.alerts",
            &json!({"alerts":[{"check":"replayed","text":"already active"}],"at":4}),
        );
        assert_eq!(snapshot.alerts.len(), 1);
        assert!(snapshot.alerts.contains_key("replayed"));
        snapshot.alert("pipeline.alerts", &json!({"alerts":[],"at":5}));
        assert!(snapshot.alerts.is_empty());
    }
    #[test]
    fn selection_is_strict_and_failed_arguments_are_atomic() {
        let mut s = State::default();
        assert!(
            s.command(
                "backdrop.view.set",
                &json!({"view":"train","args":{"bad":"x"}})
            )
            .is_err()
        );
        assert_eq!(s.selected, "wallpaper");
        s.command("backdrop.view.cycle", &json!({"dir":"prev"}))
            .unwrap();
        assert_eq!(s.selected, "train");
        assert!(
            s.command("backdrop.view.cycle", &json!({"dir":"sideways"}))
                .is_err()
        );
    }
    #[test]
    fn zoom_arguments_map_to_owner_hours_and_reject_invalid_values_atomically() {
        let mut state = State::default();
        for (zoom, hours) in [("1h", 1), ("24h", 24), ("7d", 168)] {
            state
                .command(
                    "backdrop.view.set",
                    &json!({"view":"train","args":{"zoom":zoom}}),
                )
                .unwrap();
            assert_eq!(state.hours, hours);
            assert_eq!(state.snapshot.hours, hours);
        }
        assert!(
            state
                .command(
                    "backdrop.view.set",
                    &json!({"view":"train","args":{"zoom":"30d"}})
                )
                .is_err()
        );
        assert_eq!(state.hours, 168);
    }
    #[test]
    fn set_without_args_resets_custom_service_targets() {
        let mut s = State::default();
        s.command("backdrop.view.set",&json!({"view":"train","args":{"train_service":"train.alpha.bus","buildd_service":"buildd.alpha.bus"}})).unwrap();
        s.command("backdrop.view.set", &json!({"view":"train"}))
            .unwrap();
        assert_eq!((&*s.train_service, &*s.buildd_service), ("train", "buildd"));
    }
    #[test]
    fn gate_progress_requires_current_batch_and_a_nonzero_stage_count() {
        let mut snapshot = Snapshot {
            queue: json!({"batches":[{"batch":"new","state":"gating"}]}),
            gate: json!({"batch":"old","stage":1,"stages":2}),
            ..Default::default()
        };
        assert_eq!(snapshot.gate_progress(), None);
        snapshot.gate["batch"] = json!("new");
        assert_eq!(snapshot.gate_progress(), Some(0.5));
        snapshot.gate["stages"] = json!(0);
        assert_eq!(snapshot.gate_progress(), None);
    }
}
