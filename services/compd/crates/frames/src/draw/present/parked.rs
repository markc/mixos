// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Frame callbacks while compd cannot present: the session is paused (its VT is
//! in the background), the panel is powered down, or no output is live.
//!
//! Every presenting path answers frame callbacks after a flip. With no flip,
//! nothing answers them, and a client that paces on callbacks stops. A client
//! started while the VT is hidden commits one buffer and its UI loop never runs
//! again, so anything it does in that loop (control commands, screenshots, its
//! own GPU-start watchdog) stalls until the VT comes back — and a watchdog that
//! times out reads the stall as a crashed GPU start.
//!
//! A parked desktop is treated as fully occluded: every surface still waiting on
//! a callback gets one at most every [`PARKED_TRICKLE`], the rate the occluded
//! trickle gives a covered window. The timer is armed from the main loop pass
//! that every client request wakes (not from the skipped frame: a VT switch
//! that leaves pipes in flight swallows the redraw pings that would reach it),
//! fires once, and re-arms only while something still waits. It never
//! schedules a redraw, so an idle parked desktop costs nothing.
//!
//! One second per frame is too slow for work that needs a few frames in a row:
//! a capture settles, draws and collects over about five frames, which took
//! 5–6 s parked. Demand after a quiet spell or an idle timer gets a short burst:
//! the next [`BURST_FRAMES`] sends come [`BURST_INTERVAL`] apart, then 1 Hz
//! resumes. Refills are at least [`BURST_MIN_SPACING`] apart, bounding clients
//! that toggle idle/busy. Continuous animation never restarts demand or goes
//! quiet, so it never refills the burst.

use crate::draw::present::callbacks::{OCCLUDED_TRICKLE, surface_awaits_frame, tree_awaits_frame};
use smithay::desktop::layer_map_for_output;
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::output::Output;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::seat::WaylandFocus;
use std::time::{Duration, Instant};
use world::state::Loop;

/// One callback per waiting surface at most this often while frames are parked.
pub const PARKED_TRICKLE: Duration = OCCLUDED_TRICKLE;

/// Fast sends a burst allows.
pub const BURST_FRAMES: u32 = 8;
/// The spacing of burst sends (about one 60 Hz frame).
pub const BURST_INTERVAL: Duration = Duration::from_millis(16);
/// How long the trickle must have sent nothing before the burst refills.
pub const BURST_QUIET: Duration = Duration::from_secs(2);
/// Minimum spacing between refills, including when demand restarts.
pub const BURST_MIN_SPACING: Duration = Duration::from_millis(250);

thread_local! {
    /// A parked-trickle timer is armed (one at a time).
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Fast sends left in the current burst.
    static BURST: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// When the trickle last answered callbacks.
    static LAST_SEND: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    static LAST_REFILL: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    /// A timer was not re-armed because no surface awaited a callback.
    static IDLE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// The delay before the next send, refilling the burst after a quiet spell.
fn next_period(
    last_send: Option<Instant>,
    now: Instant,
    burst: &mut u32,
    restarted: bool,
    last_refill: &mut Option<Instant>,
) -> Duration {
    if (restarted || last_send.is_none_or(|sent| now.duration_since(sent) >= BURST_QUIET))
        && last_refill.is_none_or(|refill| now.duration_since(refill) >= BURST_MIN_SPACING)
    {
        *burst = BURST_FRAMES;
        *last_refill = Some(now);
    }
    if *burst > 0 {
        BURST_INTERVAL
    } else {
        PARKED_TRICKLE
    }
}

/// The output callbacks are attributed to: the active one, else any mapped one.
fn target_output(state: &Loop) -> Option<Output> {
    let space = &state.inner.space_state().state;
    let active = state.inner.active_output_key();
    space
        .outputs()
        .find(|o| world::state::output_key(o) == active)
        .or_else(|| space.outputs().next())
        .cloned()
}

/// The compositor-drawn surfaces (cursor, drag icon) that hold a callback.
fn drawn_surfaces(state: &Loop) -> Vec<WlSurface> {
    let mut out = Vec::new();
    if let smithay::input::pointer::CursorImageStatus::Surface(surface) =
        &state.state.seat.pointer_status
    {
        out.push(surface.clone());
    }
    if let Some(icon) = &state.state.dnd.icon {
        out.push(icon.clone());
    }
    out.retain(surface_awaits_frame);
    out
}

/// Whether any surface would get a callback from [`send`].
fn waiting(state: &Loop) -> bool {
    if !drawn_surfaces(state).is_empty() {
        return true;
    }
    // Under a session lock only the lock surface gets callbacks; on a parked
    // desktop it redraws on resume, so windows and layers wait with it.
    if world::comp::session_lock::active(state) {
        return false;
    }
    let space = &state.inner.space_state().state;
    space
        .elements()
        .any(|w| w.wl_surface().is_some_and(|s| tree_awaits_frame(&s)))
        || space.outputs().any(|o| {
            layer_map_for_output(o).layers().any(|l| {
                !world::comp::panels::concealed(l.wl_surface()) && tree_awaits_frame(l.wl_surface())
            })
        })
}

/// Answer every waiting callback once. The timer is the rate limit, so no
/// smithay throttle: a throttle equal to the period would drop alternate
/// sends to timer jitter.
fn send(state: &Loop) {
    let Some(output) = target_output(state) else {
        return;
    };
    let frame_time = state.inner.start_time.elapsed();
    for surface in drawn_surfaces(state) {
        send_frames_surface_tree(&surface, &output, frame_time, None, |_, _| {
            Some(output.clone())
        });
    }
    if world::comp::session_lock::active(state) {
        return;
    }
    let space = &state.inner.space_state().state;
    for window in space.elements() {
        window.send_frame(&output, frame_time, None, |_, _| Some(output.clone()));
    }
    for o in space.outputs() {
        for layer in layer_map_for_output(o).layers() {
            if world::comp::panels::concealed(layer.wl_surface()) {
                continue;
            }
            layer.send_frame(o, frame_time, None, |_, _| Some(o.clone()));
        }
    }
}

/// Arm the parked trickle if frames are parked and some surface waits on a
/// callback: a burst send after a quiet spell, else the 1 Hz trickle.
/// Called on every main-loop pass while parked. `parked` is the backend's own
/// answer (`native.render.execute.frames_parked`), read again when the timer
/// fires: once presentation resumes, the flip answers callbacks and the timer
/// drops without sending.
pub fn arm(state: &Loop, parked: fn(&Loop) -> bool) {
    if ARMED.get() || !parked(state) {
        return;
    }
    if !waiting(state) {
        IDLE.set(true);
        return;
    }
    let mut burst = BURST.get();
    let mut last_refill = LAST_REFILL.get();
    let period = next_period(LAST_SEND.get(), Instant::now(), &mut burst, IDLE.get(), &mut last_refill);
    LAST_REFILL.set(last_refill);
    BURST.set(burst);
    let armed = state.loop_handle.insert_source(
        Timer::from_duration(period),
        move |_, _, state: &mut Loop| {
            ARMED.set(false);
            if parked(state) {
                send(state);
                LAST_SEND.set(Some(Instant::now()));
                BURST.set(BURST.get().saturating_sub(1));
                let _ = state.inner.loader.display_handle.flush_clients();
                arm(state, parked);
            }
            TimeoutAction::Drop
        },
    );
    match armed {
        Ok(_) => {
            IDLE.set(false);
            ARMED.set(true);
        }
        Err(error) => warn!("parked frame trickle not armed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_demand_bursts() {
        let mut burst = 0;
        assert_eq!(
            next_period(None, Instant::now(), &mut burst, true, &mut None),
            BURST_INTERVAL
        );
        assert_eq!(burst, BURST_FRAMES);
    }

    #[test]
    fn spent_burst_trickles_until_quiet() {
        let sent = Instant::now();
        let mut burst = 0;
        assert_eq!(
            next_period(Some(sent), sent + PARKED_TRICKLE, &mut burst, false, &mut Some(sent)),
            PARKED_TRICKLE
        );
        assert_eq!(burst, 0);
        assert_eq!(
            next_period(Some(sent), sent + BURST_QUIET, &mut burst, false, &mut Some(sent)),
            BURST_INTERVAL
        );
        assert_eq!(burst, BURST_FRAMES);
    }

    #[test]
    fn burst_in_progress_is_not_refilled() {
        let sent = Instant::now();
        let mut burst = 3;
        assert_eq!(
            next_period(Some(sent), sent + BURST_INTERVAL, &mut burst, false, &mut Some(sent)),
            BURST_INTERVAL
        );
        assert_eq!(burst, 3);
    }

    #[test]
    fn restart_refills_only_after_minimum_spacing() {
        let start = Instant::now();
        let mut refill = Some(start);
        let mut burst = 0;
        assert_eq!(next_period(Some(start), start + BURST_MIN_SPACING - Duration::from_millis(1), &mut burst, true, &mut refill), PARKED_TRICKLE);
        assert_eq!(burst, 0);
        assert_eq!(next_period(Some(start), start + BURST_MIN_SPACING, &mut burst, true, &mut refill), BURST_INTERVAL);
        assert_eq!(burst, BURST_FRAMES);
        assert_eq!(refill, Some(start + BURST_MIN_SPACING));
    }

    #[test]
    fn continuous_demand_never_refills() {
        let start = Instant::now();
        let mut refill = Some(start);
        let mut burst = BURST_FRAMES;
        let mut sent = start;
        for _ in 0..100 {
            let period = next_period(Some(sent), sent, &mut burst, false, &mut refill);
            sent += period;
            burst = burst.saturating_sub(1);
        }
        assert_eq!(burst, 0);
        assert_eq!(refill, Some(start));
    }
}
