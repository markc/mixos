// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runtime's contract without a GPU: when passes run, how messages flow,
//! what the program sees of size, scale and input.

use std::sync::{Arc, Mutex};

use super::program::UiProgram;
use super::runtime::UiRuntime;
use super::wake::RuntimeWake;

#[derive(Default)]
struct Probe {
    passes: u32,
    applied: Vec<u32>,
    emit_once: Option<u32>,
    seen: Arc<Mutex<Seen>>,
}

#[derive(Default, Clone, Debug)]
struct Seen {
    pixels_per_point: f32,
    screen: egui::Vec2,
    hover: Option<egui::Pos2>,
}

impl UiProgram for Probe {
    type Message = u32;

    fn show(&mut self, ui: &mut egui::Ui, emit: &mut Vec<u32>) {
        let ctx = ui.ctx().clone();
        self.passes += 1;
        let mut seen = self.seen.lock().unwrap();
        seen.pixels_per_point = ctx.pixels_per_point();
        seen.screen = ctx.content_rect().size();
        seen.hover = ctx.input(|input| input.pointer.hover_pos());
        if let Some(message) = self.emit_once.take() {
            emit.push(message);
        }
    }

    fn update(&mut self, message: u32) {
        self.applied.push(message);
    }

    fn process(&self, message: &u32) -> Vec<u32> {
        // 10 derives 11.
        if *message == 10 { vec![11] } else { Vec::new() }
    }

    fn setup(&mut self, _ctx: &egui::Context) {}
}

fn runtime(program: Probe) -> UiRuntime<Probe> {
    UiRuntime::build(
        program,
        (400, 200),
        2.0,
        None,
        RuntimeWake::with_waker(|| {}),
    )
}

#[test]
fn the_first_tick_draws_and_an_idle_one_does_not() {
    let mut rt = runtime(Probe::default());
    assert!(rt.is_dirty(), "a new instance owes its first frame");
    assert!(rt.tick());
    let after_first = rt.program.passes;
    assert!(after_first >= 1);
    rt.acknowledge_frame();
    // egui may ask for a settling pass after the first; drain those.
    for _ in 0..4 {
        rt.tick();
        rt.acknowledge_frame();
    }
    let settled = rt.program.passes;
    assert!(!rt.tick(), "idle: no paint wanted");
    assert_eq!(rt.program.passes, settled, "idle: no pass");
    assert!(!rt.is_dirty());
}

#[test]
fn the_program_sees_points_at_the_runtime_scale() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut rt = runtime(Probe {
        seen: seen.clone(),
        ..Probe::default()
    });
    rt.tick();
    let first = seen.lock().unwrap().clone();
    assert_eq!(first.pixels_per_point, 2.0);
    assert_eq!(first.screen, egui::vec2(200.0, 100.0));
    rt.resize((600, 300), 1.5);
    rt.tick();
    let second = seen.lock().unwrap().clone();
    assert_eq!(second.pixels_per_point, 1.5);
    assert_eq!(second.screen, egui::vec2(400.0, 200.0));
}

#[test]
fn pointer_input_runs_a_pass_that_sees_it() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut rt = runtime(Probe {
        seen: seen.clone(),
        ..Probe::default()
    });
    rt.tick();
    rt.acknowledge_frame();
    rt.queue_event(egui::Event::PointerMoved(egui::pos2(12.0, 34.0)));
    assert!(rt.has_queued());
    assert!(rt.tick());
    assert_eq!(seen.lock().unwrap().hover, Some(egui::pos2(12.0, 34.0)));
    rt.pointer_leave();
    rt.tick();
    assert_eq!(seen.lock().unwrap().hover, None);
}

/// A button filling the surface that counts its clicks.
#[derive(Default)]
struct Clicker {
    clicks: u32,
}

impl UiProgram for Clicker {
    type Message = ();

    fn show(&mut self, ui: &mut egui::Ui, emit: &mut Vec<()>) {
        let rect = ui.max_rect();
        if ui.put(rect, egui::Button::new("go")).clicked() {
            emit.push(());
        }
    }

    fn update(&mut self, _message: ()) {
        self.clicks += 1;
    }

    fn setup(&mut self, _ctx: &egui::Context) {}
}

/// A press and its release queued before one pass (a fast tap, or an
/// injected click) still click: the release waits for the next pass.
#[test]
fn a_press_and_release_queued_together_still_click() {
    let mut rt = UiRuntime::build(
        Clicker::default(),
        (200, 100),
        1.0,
        None,
        RuntimeWake::with_waker(|| {}),
    );
    rt.tick();
    rt.queue_input(crate::input::Input::PointerMoved { x: 50.0, y: 50.0 });
    rt.tick();
    rt.queue_input(crate::input::Input::Button {
        code: 0x110,
        pressed: true,
    });
    rt.queue_input(crate::input::Input::Button {
        code: 0x110,
        pressed: false,
    });
    for _ in 0..4 {
        rt.tick();
    }
    assert_eq!(rt.program.clicks, 1);
    assert!(!rt.has_queued(), "the held-back release ran");
}

#[test]
fn messages_apply_in_order_with_follow_ups_and_the_handler_sees_them() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut rt = runtime(Probe {
        emit_once: Some(10),
        ..Probe::default()
    });
    let sink = log.clone();
    rt.set_message_handler(move |message: &u32| sink.lock().unwrap().push(*message));
    rt.queue_message(1);
    rt.tick();
    // Queued 1 applies before the pass; the pass emits 10, which derives 11.
    assert_eq!(rt.program.applied, vec![1, 10, 11]);
    assert_eq!(*log.lock().unwrap(), vec![1, 10, 11]);
    assert!(rt.is_dirty(), "emitted messages owe a pass that draws them");
    let before = rt.program.passes;
    rt.tick();
    assert_eq!(rt.program.passes, before + 1);
}

/// An animation egui keeps asking frames for: a repaint every pass.
struct Spinner {
    passes: u32,
}

impl UiProgram for Spinner {
    type Message = ();

    fn show(&mut self, ui: &mut egui::Ui, _emit: &mut Vec<()>) {
        self.passes += 1;
        ui.ctx().request_repaint();
    }

    fn update(&mut self, _message: ()) {}

    fn setup(&mut self, _ctx: &egui::Context) {}
}

/// Outside a frame (compd parked) queued input still runs: a click queued
/// whole is drained by offscreen ticks and clicks once.
#[test]
fn offscreen_ticks_drain_a_queued_click() {
    let mut rt = UiRuntime::build(
        Clicker::default(),
        (200, 100),
        1.0,
        None,
        RuntimeWake::with_waker(|| {}),
    );
    rt.tick();
    rt.queue_input(crate::input::Input::PointerMoved { x: 50.0, y: 50.0 });
    rt.queue_input(crate::input::Input::Button { code: 0x110, pressed: true });
    rt.queue_input(crate::input::Input::Button { code: 0x110, pressed: false });
    let mut rounds = 0;
    while rt.has_input() && rounds < 8 {
        rt.tick_offscreen();
        rounds += 1;
    }
    assert_eq!(rt.program.clicks, 1);
    assert_eq!(rounds, 2, "the press pass, then the release pass");
}

/// A repaint request alone runs no offscreen pass, so an animating UI
/// cannot keep a parked compd busy.
#[test]
fn an_animation_runs_no_offscreen_pass() {
    let mut rt = UiRuntime::build(
        Spinner::default(),
        (200, 100),
        1.0,
        None,
        RuntimeWake::with_waker(|| {}),
    );
    rt.tick();
    let passes = rt.program.passes;
    assert!(rt.has_queued(), "it asked for a repaint");
    assert!(!rt.has_input());
    rt.tick_offscreen();
    assert_eq!(rt.program.passes, passes);
}

#[test]
fn a_hidden_animation_stops_asking_for_frames_until_it_is_shown() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let wakes = Arc::new(AtomicU32::new(0));
    let counter = wakes.clone();
    let wake = RuntimeWake::with_waker(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    });
    let mut rt = UiRuntime::build(Spinner { passes: 0 }, (100, 100), 1.0, None, wake);
    rt.tick();
    assert!(
        rt.has_queued(),
        "visible, the animation wants the next frame"
    );
    // Off screen: the registry acknowledges and suspends.
    rt.acknowledge_frame();
    rt.set_suspended(true);
    rt.tick();
    rt.acknowledge_frame();
    let woken = wakes.load(Ordering::Relaxed);
    let passes = rt.program.passes;
    for _ in 0..5 {
        rt.tick();
        rt.acknowledge_frame();
    }
    assert_eq!(
        wakes.load(Ordering::Relaxed),
        woken,
        "suspended: no loop wakeups"
    );
    assert_eq!(
        rt.program.passes, passes,
        "suspended: no passes without input"
    );
    assert!(!rt.is_dirty() && !rt.has_queued());
    // Input still runs a pass while hidden.
    rt.queue_input(crate::input::Input::PointerMoved { x: 1.0, y: 1.0 });
    rt.tick();
    assert_eq!(rt.program.passes, passes + 1);
    rt.acknowledge_frame();
    // Shown again: the missed repaint owes a pass.
    rt.set_suspended(false);
    assert!(rt.is_dirty());
}
