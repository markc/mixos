// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! On-demand software presentation. No timer remains armed on an idle output.

use std::{cell::RefCell, rc::Rc, time::{Duration, Instant}};
use smithay::backend::renderer::{Bind, damage::OutputDamageTracker, gles::{GlesRenderer, GlesTexture}};
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::reexports::calloop::timer::{Timer, TimeoutAction};
use world::state::Loop;

const INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 60);

pub(crate) struct Context {
    output: Output,
    renderer: GlesRenderer,
    targets: [GlesTexture; 2],
    next: usize,
    tracker: OutputDamageTracker,
    last: Option<Instant>,
    armed: bool,
}

impl Context {
    pub(crate) fn new(output: Output, renderer: GlesRenderer, targets: [GlesTexture; 2], tracker: OutputDamageTracker) -> Self {
        Self { output, renderer, targets, next: 0, tracker, last: None, armed: false }
    }
}

pub(crate) fn arm(state: &mut Loop, context: &Rc<RefCell<Context>>) {
    let mut ctx = context.borrow_mut();
    let key = world::state::state::output_key(&ctx.output);
    if ctx.armed || !state.state.redraw.needs(&key) { return; }
    let at = ctx.last.map_or_else(Instant::now, |last| (last + INTERVAL).max(Instant::now()));
    let context = Rc::clone(context);
    state.loop_handle.insert_source(Timer::from_deadline(at), move |_, _, state| {
        {
            let mut ctx = context.borrow_mut();
            ctx.armed = false;
            draw(state, &mut ctx);
        }
        arm(state, &context);
        TimeoutAction::Drop
    }).expect("headless: frame timer");
    ctx.armed = true;
}

fn draw(state: &mut Loop, ctx: &mut Context) {
    let key = world::state::state::output_key(&ctx.output);
    if !state.state.redraw.needs(&key) { return; }
    ctx.last = Some(Instant::now());
    state.state.redraw.rendering(&key);
    state.state.redraw.begin_render();
    state.inner.render_output = Some(key.clone());
    let mut visible = Vec::new();
    let result = compose(state, ctx, &mut visible);
    match result {
        Ok(true) => {
            state.state.redraw.frame(&key, false);
            present(state, &ctx.output, &visible);
            model::stats::registry::base::present(&key);
            slots::sdnotify::ready_once("first frame presented (headless)");
        }
        Ok(false) => {
            state.state.redraw.frame(&key, true);
            // Empty frames owe callbacks too. This draw was already paced by
            // the one-shot timer, so no unbounded commit/callback loop arises.
            send_frames(state, &ctx.output, &visible);
        }
        Err(error) => {
            warn!("headless: frame failed: {error}");
            ctx.tracker = OutputDamageTracker::from_output(&ctx.output);
            state.state.redraw.request_for(protocols::redraw::schedule::schedule::RedrawReason::Rescue);
        }
    }
    frames::draw::present::callbacks::callbacks::housekeeping(state);
    state.inner.render_output = None;
    state.state.redraw.end_render();
    if let Some(at) = graphics::bridge::publish::wake::wake::take_deadline() {
        let handle = state.loop_handle.clone();
        state.state.redraw.request_at(&handle, at, protocols::redraw::schedule::schedule::RedrawReason::Ui, |s| &mut s.state.redraw);
    }
}

fn compose(state: &mut Loop, ctx: &mut Context, visible: &mut Vec<Window>) -> Result<bool, Box<dyn std::error::Error>> {
    use frames::draw::plan::frame::frame::{FramePass, plan};
    if !plan(&state.inner.status).has_pass(FramePass::Scene) { return Ok(false); }
    let size = ctx.output.current_mode().expect("headless mode").size;
    let prepared = frames::scene::scene::prepare(state, &mut ctx.renderer, size);
    let scene = frames::scene::scene::scene(state, &mut ctx.renderer, size, prepared);
    *visible = scene.visible_window;
    // Track content changes independently of the alternating targets. This
    // age-1 query is only a scene diff; it never assumes preserved pixels in
    // either texture. An unchanged scene submits nothing.
    if ctx.tracker.damage_output(1, &scene.Element)?.0.is_none() {
        return Ok(false);
    }
    let mut target = ctx.renderer.bind(&mut ctx.targets[ctx.next])?;
    // No buffer-age assumptions: each target is repainted completely.
    let mut repaint = OutputDamageTracker::from_output(&ctx.output);
    let result = repaint.render_output(&mut ctx.renderer, &mut target, 0, &scene.Element, [0.1, 0.1, 0.1, 1.0])?;
    let damaged = result.damage.is_some_and(|d| !d.is_empty());
    result.sync.wait()?;
    ctx.next ^= 1;
    // TODO(Phase 2): screencopy and Bus capture read this completed target;
    // cursor inclusion is selected for each capture, not globally.
    Ok(damaged)
}

fn present(state: &mut Loop, output: &Output, visible: &[Window]) {
    use frames::draw::present::{callbacks::callbacks, software::software};
    let mut feedback = callbacks::collect_feedback(output, visible, None);
    software::presented_now(&mut feedback, output);
    world::comp::presentation::frame_queued(state, output, visible);
    let now: Duration = smithay::utils::Clock::<smithay::utils::Monotonic>::new().now().into();
    world::comp::presentation::presented(state, output, now, None, software::software_present_kind().bits());
    send_frames(state, output, visible);
}

fn send_frames(state: &mut Loop, output: &Output, visible: &[Window]) {
    use frames::draw::present::callbacks::callbacks;
    callbacks::send_window_frames(state, output, visible);
    callbacks::send_layer_frames(state, output);
    frames::draw::present::cursor::cursor::send_frames(state, output);
}
