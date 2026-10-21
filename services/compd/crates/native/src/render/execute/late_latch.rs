// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native-only scheduling gate, shared by flip-driven and ping-driven renders.

use crate::context::render::render::OutputPipe;
use crate::wire::frame::deadline::DeadlineTimer;
use smithay::reexports::calloop::LoopHandle;
use std::time::{Duration, Instant};
use world::state::Loop;

fn fired(state: &mut Loop, key: &str) {
    state.state.redraw.release(key);
    state.state.redraw.wake();
}

pub(crate) fn cancel(pipe: &mut OutputPipe) {
    if let Some(timer) = &pipe.deadline_timer {
        timer.cancel();
    }
    pipe.late_latch.park();
    pipe.gpu_completion = None;
}

pub(crate) fn collect_gpu(pipe: &mut OutputPipe) {
    if let Some(elapsed) = pipe.gpu_completion.as_ref().and_then(|sample| sample.elapsed()) {
        pipe.late_latch.observe(elapsed);
        pipe.gpu_completion = None;
    }
}

/// Returns true while this pipe must wait. Once armed, all redraw reasons
/// coalesce until expiry, even if another pipe or a forced ping invokes execute.
pub(crate) fn defer(
    pipe: &mut OutputPipe,
    state: &mut Loop,
    handle: &LoopHandle<'static, Loop>,
    key: &str,
    trace_output: u64,
    trace_immediate: bool,
) -> bool {
    let now = Instant::now();
    if pipe.late_latch.armed.is_some_and(|at| now < at) {
        return true;
    }
    let config = model::environment::config::base::get();
    let margin = Duration::from_micros(config.late_latch_margin_us);
    let plan = pipe.late_latch.plan(now, margin);
    let immediate = if !model::environment::config::base::late_latch_enabled() {
        Some("kms_latch_disabled")
    } else if protocols::tearing::gate::gate::tearing() || pipe.last_tear {
        Some("kms_latch_tearing")
    } else if config.vrr {
        // Conservative even on a connector that refused VRR: a variable cadence
        // has no fixed next vblank to aim at.
        Some("kms_latch_vrr")
    } else if screencopy::active()
        || screencopy::file::pending(&pipe.output)
        || state
            .inner
            .kernel
            .get(&world::driver::capture::base::CAPTURE)
            .is_active()
        || super::diagnostics::enabled()
    {
        Some("kms_latch_capture")
    } else if protocols::tearing::gate::gate::governed() {
        Some("kms_latch_policy")
    } else if !pipe.late_latch.following_flip() {
        Some("kms_latch_idle")
    } else if pipe.mode.refresh <= 0 || plan.target.is_none() {
        Some("kms_latch_unpredictable")
    } else {
        None
    };
    if immediate.is_none() {
        pipe.late_latch.begin(now, plan);
    }
    // The flip handler only arms a wait. An immediate decision is recorded by
    // the executor when it actually renders, avoiding two decisions per frame.
    if !trace_immediate && (immediate.is_some() || plan.deadline.is_none()) {
        return false;
    }
    if let Some(reason) = immediate {
        ledger::redraw::trace_deadline(reason, trace_output, None, plan.budget);
        cancel(pipe);
        state.state.redraw.release(key);
        return false;
    }
    if let Some(at) = plan.deadline {
        if pipe.deadline_timer.is_none() {
            pipe.deadline_timer = DeadlineTimer::new(handle, key.to_owned(), fired);
        }
        if pipe
            .deadline_timer
            .as_ref()
            .is_some_and(|timer| timer.arm(at))
        {
            pipe.late_latch.armed = Some(at);
            state.state.redraw.defer(key);
            ledger::redraw::trace_deadline(
                "kms_latch_deferred",
                trace_output,
                plan.target.map(|t| t.vblank),
                plan.budget,
            );
            return true;
        }
        ledger::redraw::trace_deadline(
            "kms_latch_timer_failed",
            trace_output,
            plan.target.map(|t| t.vblank),
            plan.budget,
        );
        cancel(pipe);
    } else {
        ledger::redraw::trace_deadline(
            if pipe.late_latch.armed.is_some() {
                "kms_latch_deadline"
            } else {
                "kms_latch_clamped"
            },
            trace_output,
            plan.target.map(|t| t.vblank),
            plan.budget,
        );
        pipe.late_latch.armed = None;
    }
    state.state.redraw.release(key);
    false
}
