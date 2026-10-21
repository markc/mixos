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
    if let Some(elapsed) = pipe
        .gpu_completion
        .as_ref()
        .and_then(|sample| sample.elapsed())
    {
        pipe.late_latch.observe(elapsed);
        pipe.gpu_completion = None;
    }
}

fn sync_reason(enabled: bool, tearing: bool, vrr_active: bool) -> Option<&'static str> {
    if !enabled {
        Some("kms_latch_disabled")
    } else if tearing {
        Some("kms_latch_tearing")
    } else if vrr_active {
        Some("kms_latch_vrr")
    } else {
        None
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
    // Read this pipe's committed CRTC state from the surface used by apply_vrr,
    // not the global preference or the surface's pending VRR request.
    let vrr_active = pipe.drm_output.as_ref().is_some_and(|output| {
        output.with_compositor(|comp| comp.surface().vrr_active())
    });
    let sync = sync_reason(
        model::environment::config::base::late_latch_enabled(),
        protocols::tearing::gate::gate::tearing() || pipe.last_tear,
        vrr_active,
    );
    let immediate = if sync.is_some() {
        sync
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
    } else if pipe.cap_interval.is_some() || protocols::tearing::gate::gate::governed() {
        // The global flag may describe a neighbouring output's last render.
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

#[cfg(test)]
mod tests {
    use super::*;
    use kms::scanout::timing::deadline::LateLatch;
    use model::environment::config::base::default_settings;
    use smithay::reexports::calloop::EventLoop;

    #[test]
    fn requested_vrr_with_fixed_refresh_still_defers_but_active_vrr_is_immediate() {
        let mut config = default_settings();
        config.vrr = true;
        let now = Instant::now();
        let period = Duration::from_millis(16);
        let margin = Duration::from_micros(config.late_latch_margin_us);
        let mut latch = LateLatch::default();
        latch.committed(now, margin, false);
        latch.flipped(Some(now), period, true);
        latch.flipped(Some(now), period, true);
        let plan = latch.plan(now, margin);

        assert!(config.vrr);
        // Unsupported, refused, off and not-yet-committed requests all have
        // vrr_active=false, independently for each pipe.
        assert_eq!(sync_reason(config.late_latch, false, false), None);
        let mut event_loop: EventLoop<'static, bool> = EventLoop::try_new().unwrap();
        let timer = DeadlineTimer::new(&event_loop.handle(), "fixed".into(), |fired, _| {
            *fired = true;
        })
        .unwrap();
        latch.begin(now, plan);
        assert!(timer.arm(plan.deadline.unwrap()));
        let mut fired = false;
        event_loop.dispatch(Some(Duration::ZERO), &mut fired).unwrap();
        assert!(!fired, "the frame waits for its deadline");
        assert_eq!(
            sync_reason(config.late_latch, false, true),
            Some("kms_latch_vrr")
        );
        // A sibling with active VRR does not change the fixed-refresh gate.
        assert_eq!(sync_reason(config.late_latch, false, false), None);
    }
}
