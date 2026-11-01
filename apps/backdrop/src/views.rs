// SPDX-License-Identifier: MIT OR Apache-2.0
//! Live views use Bus snapshots only. The wallpaper engine remains beside them.
mod train;
mod train_gpu;
use serde_json::Value;
use std::time::{Duration, Instant};
pub use train::Train;

pub trait View {
    fn subscriptions(&self) -> &'static [&'static str];
    fn on_event(&mut self, topic: &str, data: Value, now: Instant) -> bool;
    /// Adds a wgpu paint callback for the scene and ordinary egui overlays.
    fn draw(&self, ui: &mut egui::Ui);
    fn wants_frame(&self, now: Instant) -> Option<Duration>;
}

pub struct State {
    pub refresh: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    pub selected: String,
    pub train_service: String,
    pub buildd_service: String,
    pub train: Train,
}
impl Default for State {
    fn default() -> Self {
        Self {
            refresh: Default::default(),
            selected: "wallpaper".into(),
            train_service: "train".into(),
            buildd_service: "buildd".into(),
            train: Train::default(),
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
                let mut train = self.train_service.clone();
                let mut buildd = self.buildd_service.clone();
                if let Some(args) = object.get("args") {
                    let args = args.as_object().ok_or("args must be an object")?;
                    for (key, value) in args {
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
                if train != self.train_service || buildd != self.buildd_service {
                    self.train = Train::default();
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

/// One read worker, never the UI/controller. Topic hints coalesce; fallback is
/// ten seconds. No private database reads and no repaint for identical data.
pub fn start_reader(
    handle: citizen::Handle,
    shared: std::sync::Arc<std::sync::Mutex<crate::program::Shared>>,
    wakers: crate::program::Wakers,
) -> Result<(), String> {
    let signal = crate::program::lock(&shared).live.refresh.clone();
    std::thread::Builder::new()
        .name("backdrop-train".into())
        .spawn(move || {
            loop {
                let pending = crate::program::lock(&signal.0);
                let (mut pending, _) = signal
                    .1
                    .wait_timeout_while(pending, Duration::from_secs(10), |p| !*p)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *pending = false;
                drop(pending);
                let (active, train, buildd) = {
                    let state = crate::program::lock(&shared);
                    (
                        state.live.selected == "train",
                        state.live.train_service.clone(),
                        state.live.buildd_service.clone(),
                    )
                };
                if !active {
                    continue;
                }
                let mut changed = false;
                let mut errors = Vec::new();
                for (target, verb) in [(&train, "train.queue"), (&buildd, "buildd.queue")] {
                    let reply = futures::executor::block_on(handle.call(
                        target,
                        verb,
                        serde_json::json!({}),
                    ));
                    let data = reply.and_then(|r| {
                        if r.rc == 0 {
                            serde_json::from_str(&r.body).map_err(|e| e.to_string())
                        } else {
                            Err(format!("{verb}: {}", r.error.unwrap_or(r.body)))
                        }
                    });
                    let mut state = crate::program::lock(&shared);
                    if state.live.train_service != train || state.live.buildd_service != buildd {
                        continue;
                    }
                    match data {
                        Ok(data) => {
                            changed |= state.live.train.on_event(verb, data, Instant::now());
                        }
                        Err(error) => {
                            errors.push(error);
                        }
                    }
                }
                {
                    let mut state = crate::program::lock(&shared);
                    if state.live.train_service == train && state.live.buildd_service == buildd {
                        let error = if errors.is_empty() {
                            None
                        } else {
                            Some(errors.join("; "))
                        };
                        changed |= state.live.train.error != error;
                        state.live.train.error = error;
                    }
                    changed &= state.live.selected == "train";
                }
                if changed {
                    for (_, ctx) in crate::program::lock(&wakers).iter() {
                        ctx.request_repaint_after(Duration::from_millis(100));
                    }
                }
                // Bound event bursts to the same low animation rate.
                std::thread::sleep(Duration::from_millis(100));
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
    fn view_contract_is_gpu_independent_and_idle_has_no_timer() {
        let mut train = Train::default();
        let view: &mut dyn View = &mut train;
        let now = Instant::now();
        assert_eq!(view.subscriptions().len(), 4);
        assert_eq!(view.wants_frame(now), None);
        assert!(view.on_event("train.queue", json!({"queued":[]}), now));
        assert_eq!(view.wants_frame(now), Some(Duration::from_millis(100)));
        assert_eq!(view.wants_frame(now + Duration::from_secs(1)), None);
        assert!(!view.on_event("train.queue", json!({"queued":[]}), now));
    }
}
