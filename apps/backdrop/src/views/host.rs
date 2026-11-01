// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host is the only live-view repaint source. Views receive data, a clock
//! and visibility; neither egui Context nor Ui can cross the View boundary.
use super::{Frame, Snapshot, View};
use crate::program::Budget;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MIN_FRAME: Duration = Duration::from_millis(100);
const IDLE: Duration = Duration::from_secs(2);
thread_local! {
    static STRINGS:toolkit::Strings = toolkit::Strings::new(include_str!("../../i18n/en/backdrop.ftl"));
}

pub struct Host<V> {
    pub view: V,
    pub active: bool,
    visible: bool,
    snapshot: Arc<Snapshot>,
    last_event: Option<Instant>,
    bootstrap: bool,
    ctx: Option<egui::Context>,
    gpu: super::train_gpu::Cache,
}
impl<V: View> Host<V> {
    pub fn is_visible(&self) -> bool {
        self.active && self.visible
    }
    pub fn new(view: V, ctx: Option<egui::Context>) -> Self {
        Self {
            view,
            active: true,
            visible: false,
            snapshot: Arc::default(),
            last_event: None,
            bootstrap: false,
            ctx,
            gpu: Default::default(),
        }
    }
    pub fn event(&mut self, snapshot: Arc<Snapshot>, force: bool, now: Instant) {
        if force && self.active && !self.visible { self.bootstrap = true; }
        if force || !Arc::ptr_eq(&snapshot, &self.snapshot) {
            self.snapshot = snapshot.clone();
            self.last_event = Some(now);
            self.view.on_event(snapshot, self.visible, now);
            self.schedule(now);
        }
    }
    pub fn visibility(&mut self, visible: bool, now: Instant) {
        if self.visible != visible {
            self.visible = visible;
            self.bootstrap = false;
            if visible {
                self.last_event = Some(now);
            }
            self.view.on_event(self.snapshot.clone(), visible, now);
            self.schedule(now);
        }
    }
    /// Authoritative policy: even a view that asks for frames forever loses
    /// its animation lease two seconds after the latest data/visibility event.
    pub fn delay(&self, now: Instant) -> Option<Duration> {
        if !self.active
            || !self.visible
            || self
                .last_event
                .is_none_or(|t| now.saturating_duration_since(t) >= IDLE)
        {
            return None;
        }
        self.view
            .wants_frame(self.visible, now)
            .map(|d| d.max(MIN_FRAME))
    }
    fn schedule(&self, now: Instant) {
        if let (Some(ctx), Some(delay)) = (&self.ctx, self.repaint_delay(now)) {
            ctx.request_repaint_after(delay);
        }
    }
    pub fn repaint_delay(&self, now: Instant) -> Option<Duration> {
        self.delay(now).or_else(||(self.active && self.bootstrap).then_some(MIN_FRAME))
    }
    pub fn probe_drawn(&mut self) { self.bootstrap = false; }
    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        budget: &Arc<Budget>,
        renderer_2d: bool,
        now: Instant,
    ) {
        if let Some(frame) = self.view.draw(self.visible, now) {
            self.paint(ui, budget, renderer_2d, &frame);
        }
        self.schedule(now);
    }
    pub fn release_gpu(&mut self) {
        self.gpu = Default::default();
    }
    fn paint(&mut self, ui: &mut egui::Ui, budget: &Arc<Budget>, renderer_2d: bool, frame: &Frame) {
        STRINGS.with(|strings|self.paint_with_strings(ui,budget,renderer_2d,frame,strings));
    }
    fn paint_with_strings(&mut self, ui: &mut egui::Ui, budget: &Arc<Budget>, renderer_2d: bool, frame:&Frame, strings:&toolkit::Strings) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0., ui.visuals().panel_fill);
        let fallback = renderer_2d || !self.gpu.draw(ui, &frame.scene, frame.progress, budget);
        if fallback {
            self.release_gpu();
        }
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.heading(strings.get("train-title"));
            ui.label(strings.with(
                "train-header",
                &[
                    ("sha", frame.snapshot.queue["main"].as_str().unwrap_or("?")),
                    (
                        "state",
                        &strings.get(if frame.snapshot.queue.is_null() {
                            "train-unknown"
                        } else if frame.snapshot.queue["paused"].is_null() {
                            "train-running"
                        } else {
                            "train-paused"
                        }),
                    ),
                ],
            ));
            ui.label(strings.get("train-track"));
            if let Some(gate) = frame.snapshot.current_gate() {
                let label = strings.with(
                    "train-gate-stage",
                    &[
                        ("stage", &gate["stage"].to_string()),
                        ("stages", &gate["stages"].to_string()),
                        ("last", gate["last"].as_str().unwrap_or("?")),
                    ],
                );
                let colour = if gate["failing"] == true {
                    ui.visuals().error_fg_color
                } else {
                    ui.visuals().selection.bg_fill
                };
                if let Some(progress) = frame.snapshot.gate_progress() {
                    ui.add(egui::ProgressBar::new(progress).fill(colour));
                }
                if gate["failing"] == true {
                    ui.colored_label(colour, label);
                } else {
                    ui.label(label);
                }
            } else if frame.snapshot.gating_batch().is_some() {
                ui.label(strings.get("train-stage-unavailable"));
            }
            if let Some(error) = &frame.snapshot.error {
                ui.label(error);
            }
        });
        if fallback {
            ui.label(strings.get("train-fallback"));
            egui_plot::Plot::new("train-2d")
                .height((rect.height() - 300.).max(80.))
                .allow_zoom(false)
                .allow_drag(false)
                .allow_scroll(false)
                .show(ui, |plot| {
                    for block in frame.scene.iter().filter(|b| !b.label.is_empty()) {
                        plot.points(
                            egui_plot::Points::new(
                                &block.label,
                                vec![[block.position[0] as f64, block.position[2] as f64]],
                            )
                            .radius(6.),
                        );
                        plot.text(egui_plot::Text::new(
                            &block.label,
                            egui_plot::PlotPoint::new(
                                block.position[0] as f64,
                                block.position[2] as f64 + 0.3,
                            ),
                            &block.label,
                        ));
                    }
                });
        }
        // Allocate an overlay at the bottom, without giving a view a Ui.
        ui.add_space((ui.available_height() - 190.).max(0.));
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.label(strings.with(
                "train-history",
                &[("hours", &frame.snapshot.hours.to_string())],
            ));
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            let series = super::history::series(&frame.snapshot.history, frame.snapshot.hours, now);
            if !frame.snapshot.history["landed"].is_array() || !frame.snapshot.history["batches"].is_array() {
                ui.label(strings.get("train-history-unavailable"));
            }
            egui_plot::Plot::new(("train-history", frame.snapshot.hours))
                .height(140.)
                .include_x(-f64::from(frame.snapshot.hours))
                .include_x(0.)
                .allow_zoom(false)
                .allow_drag(false)
                .allow_scroll(false)
                .show(ui, |plot| {
                    plot.line(egui_plot::Line::new(
                        strings.get("train-landings"),
                        series.landings,
                    ));
                    plot.points(
                        egui_plot::Points::new(strings.get("train-gates"), series.gate_minutes)
                            .radius(3.),
                    );
                });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Greedy;
    impl View for Greedy {
        fn subscriptions(&self) -> &'static [&'static str] {
            &[]
        }
        fn on_event(&mut self, _: Arc<Snapshot>, _: bool, _: Instant) {}
        fn draw(&mut self, _: bool, _: Instant) -> Option<Frame> {
            None
        }
        fn wants_frame(&self, _: bool, _: Instant) -> Option<Duration> {
            Some(Duration::ZERO)
        }
    }
    #[test]
    fn host_clamps_and_revokes_idle_animation_without_a_gpu() {
        let now = Instant::now();
        let mut host = Host::new(Greedy, None);
        assert_eq!(host.delay(now), None);
        host.visibility(true,now);
        host.event(Arc::default(), true, now);
        assert_eq!(host.delay(now), Some(MIN_FRAME));
        assert_eq!(host.delay(now + IDLE), None);
    }
    #[test]
    fn hidden_host_never_requests_frames_even_on_data_events() {
        let now = Instant::now();
        let mut host = Host::new(Greedy, None);
        host.visibility(false, now);
        host.event(Arc::default(), true, now);
        assert_eq!(host.delay(now), None);
        assert_eq!(host.delay(now + Duration::from_secs(1)), None);
        host.visibility(true, now + IDLE);
        assert_eq!(host.delay(now + IDLE), Some(MIN_FRAME));
        host.active = false;
        assert_eq!(host.delay(now + IDLE), None);
    }
    #[test]
    fn outputs_coalesce_independently_over_the_same_snapshot() {
        let now = Instant::now();
        let snapshot = Arc::new(Snapshot {
            queue: serde_json::json!({"queued":[{"branch":"work/a"}]}),
            ..Default::default()
        });
        let mut a = Host::new(super::super::Train::default(), None);
        a.visibility(true,now);
        let mut b = Host::new(super::super::Train::default(), None);
        b.visibility(false, now);
        a.event(snapshot.clone(), false, now);
        b.event(snapshot.clone(), false, now);
        let latest = Arc::new(Snapshot {
            queue: serde_json::json!({"queued":[{"branch":"work/b"}]}),
            ..Default::default()
        });
        b.event(latest.clone(), false, now);
        assert!(a.view.draw(true, now).is_some());
        assert!(b.view.draw(false, now).is_none());
        assert!(b.view.cached_scene_is_empty());
        b.visibility(true, now);
        let frame = b.view.draw(true, now).unwrap();
        assert!(Arc::ptr_eq(&frame.snapshot, &latest));
        assert!(frame.scene.iter().any(|b| b.label == "work/b"));
        assert!(Arc::ptr_eq(&a.snapshot, &snapshot));
    }
    #[test]
    fn train_finishes_its_animation_and_host_has_no_idle_timer() {
        let now = Instant::now();
        let mut host = Host::new(super::super::Train::default(), None);
        host.visibility(true,now);
        host.event(Arc::default(), true, now);
        host.view.draw(true, now).unwrap();
        assert_eq!(host.delay(now), Some(MIN_FRAME));
        let end = now + Duration::from_millis(600);
        assert_eq!(host.view.draw(true, end).unwrap().progress, 1.);
        assert_eq!(host.delay(end), None);
        assert_eq!(host.delay(now + IDLE), None);
    }
    #[test]
    fn visibility_is_unconfirmed_at_creation_and_catalogue_is_cached() {
        let host=Host::new(Greedy,None);
        assert!(!host.is_visible());
        let a=STRINGS.with(|s|s as *const toolkit::Strings);
        let b=STRINGS.with(|s|s as *const toolkit::Strings);
        assert_eq!(a,b);
    }
}
