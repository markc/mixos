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
//! stops when its next tick finds nothing waiting. It never
//! schedules a redraw, so an idle parked desktop costs nothing.
//!
//! One second per frame is too slow for work that needs a few frames in a row:
//! a capture settles, draws and collects over about five frames, which took
//! 5–6 s parked. Demand returning more than [`DEMAND_IDLE`] after a send gets a burst:
//! the next [`BURST_FRAMES`] sends come [`BURST_INTERVAL`] apart, then 1 Hz
//! resumes. Refills are at least [`BURST_MIN_SPACING`] apart, bounding clients
//! that toggle idle/busy to at most 32 fast callbacks/s in the long run,
//! plus the trickle (a refill grants eight and is spaced by at least 250 ms).
//! Turnaround up to 250 ms stays continuous and gets only the initial burst,
//! then 1 Hz. Longer turnaround is indistinguishable from a new capture.

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
/// Maximum continuous client turnaround. Below a 300 ms capture gap, but
/// comfortably above a frame and ordinary 20–100 ms render turnaround.
pub const DEMAND_IDLE: Duration = Duration::from_millis(250);
/// Minimum spacing between refills, including when demand restarts.
pub const BURST_MIN_SPACING: Duration = Duration::from_millis(250);

thread_local! {
    static DEMAND: std::cell::RefCell<Demand> = std::cell::RefCell::new(Demand::default());
}

#[derive(Default)]
struct Demand {
    armed: bool,
    burst: u32,
    last_send: Option<Instant>,
    last_refill: Option<Instant>,
    next_send: Option<Instant>,
}

impl Demand {
    fn arm(&mut self, now: Instant, waiting: bool) -> Option<Duration> {
        if self.armed || !waiting {
            return None;
        }
        let period = next_period(
            self.last_send, now, &mut self.burst,
            self.last_send.is_none_or(|sent| now.duration_since(sent) > DEMAND_IDLE),
            &mut self.last_refill,
        );
        let deadline = send_deadline(self.last_send, now, period);
        self.next_send = Some(deadline);
        self.armed = true;
        Some(deadline.saturating_duration_since(now))
    }

    /// An empty probe drops the timer, preserving demand history for re-arm.
    fn probe(&mut self, waiting: bool) -> bool {
        if !waiting {
            self.armed = false;
        }
        self.armed
    }

    fn sent(&mut self, now: Instant) {
        self.last_send = Some(now);
        self.burst = self.burst.saturating_sub(1);
        self.next_send = Some(now + if self.burst > 0 { BURST_INTERVAL } else { PARKED_TRICKLE });
    }
}

/// Delay before a send, refilling only on restarted demand with spacing.
fn next_period(
    last_send: Option<Instant>,
    now: Instant,
    burst: &mut u32,
    restarted: bool,
    last_refill: &mut Option<Instant>,
) -> Duration {
    if (restarted || last_send.is_none())
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

/// Preserve the rate limit across timer drop/re-arm during client rendering.
fn send_deadline(last_send: Option<Instant>, now: Instant, period: Duration) -> Instant {
    last_send.map_or(now + period, |sent| (sent + period).max(now))
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
/// callback: a burst after quiet or restarted demand, else the 1 Hz trickle.
/// Called on every main-loop pass while parked. `parked` is the backend's own
/// answer (`native.render.execute.frames_parked`), read again when the timer
/// fires: once presentation resumes, the flip answers callbacks and the timer
/// drops without sending.
pub fn arm(state: &Loop, parked: fn(&Loop) -> bool) {
    if DEMAND.with(|demand| demand.borrow().armed) || !parked(state) {
        return;
    }
    let Some(period) = DEMAND.with(|demand| demand.borrow_mut().arm(Instant::now(), waiting(state))) else {
        return;
    };
    let armed = state.loop_handle.insert_source(
        Timer::from_duration(period),
        move |_, _, state: &mut Loop| {
            if !DEMAND.with(|demand| demand.borrow_mut().probe(parked(state) && waiting(state))) {
                return TimeoutAction::Drop;
            }
            let now = Instant::now();
            let next_send = DEMAND.with(|demand| demand.borrow().next_send.expect("armed deadline"));
            if now < next_send {
                return TimeoutAction::ToDuration(next_send - now);
            }
            send(state);
            DEMAND.with(|demand| demand.borrow_mut().sent(Instant::now()));
            let _ = state.inner.loader.display_handle.flush_clients();
            // Do not check waiting immediately after draining callbacks: a
            // continuously animating client needs time to request its next one.
            // Drop the timer promptly when idle; a commit re-arms it without
            // treating ordinary render turnaround as restarted demand.
            TimeoutAction::ToDuration(BURST_INTERVAL)
        },
    );
    match armed {
        Ok(_) => {}
        Err(error) => {
            DEMAND.with(|demand| demand.borrow_mut().probe(false));
            warn!("parked frame trickle not armed: {error}");
        }
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
    fn spent_burst_does_not_refill_just_because_time_passed() {
        let sent = Instant::now();
        let mut burst = 0;
        assert_eq!(
            next_period(
                Some(sent),
                sent + PARKED_TRICKLE,
                &mut burst,
                false,
                &mut Some(sent)
            ),
            PARKED_TRICKLE
        );
        assert_eq!(burst, 0);
        assert_eq!(
            next_period(
                Some(sent),
                sent + Duration::from_secs(20),
                &mut burst,
                false,
                &mut Some(sent)
            ),
            PARKED_TRICKLE
        );
        assert_eq!(burst, 0);
    }

    #[test]
    fn burst_in_progress_is_not_refilled() {
        let sent = Instant::now();
        let mut burst = 3;
        assert_eq!(
            next_period(
                Some(sent),
                sent + BURST_INTERVAL,
                &mut burst,
                false,
                &mut Some(sent)
            ),
            BURST_INTERVAL
        );
        assert_eq!(burst, 3);
    }

    #[test]
    fn restart_refills_only_after_minimum_spacing() {
        let start = Instant::now();
        let mut refill = Some(start);
        let mut burst = 0;
        assert_eq!(
            next_period(
                Some(start),
                start + BURST_MIN_SPACING - Duration::from_millis(1),
                &mut burst,
                true,
                &mut refill
            ),
            PARKED_TRICKLE
        );
        assert_eq!(burst, 0);
        assert_eq!(
            next_period(
                Some(start),
                start + BURST_MIN_SPACING,
                &mut burst,
                true,
                &mut refill
            ),
            BURST_INTERVAL
        );
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

    // Model the production probe: callbacks drain, the 16 ms probe sees no
    // waiting callback and drops the timer, then the client's commit re-arms.
    fn client_cycle(turnaround: Duration, sends: usize) -> Vec<Duration> {
        let start = Instant::now();
        let mut demand = Demand::default();
        let mut now = start;
        let mut times = Vec::new();
        for _ in 0..sends {
            assert!(!demand.armed, "the idle probe dropped the previous timer");
            let period = demand.arm(now, true).expect("commit re-arms timer");
            assert_eq!(demand.arm(now, true), None, "no double-arm");
            now += period;
            assert!(demand.probe(true));
            times.push(now.duration_since(start));
            demand.sent(now);
            now += BURST_INTERVAL;
            assert!(turnaround > BURST_INTERVAL);
            assert!(!demand.probe(false));
            assert_eq!(demand.arm(now, false), None, "no idle timer");
            now += turnaround - BURST_INTERVAL;
        }
        times
    }

    #[test]
    fn rendering_clients_drop_and_rearm_without_refilling() {
        for ms in [20, 100, 250] {
            let times = client_cycle(Duration::from_millis(ms), 20);
            for pair in times[BURST_FRAMES as usize - 1..].windows(2) {
                assert_eq!(pair[1] - pair[0], PARKED_TRICKLE);
            }
        }
    }

    #[test]
    fn capture_gap_refills_after_probe_drop() {
        let start = Instant::now();
        let mut now = start;
        let mut demand = Demand::default();
        for _ in 0..3 {
            // No timer survived the idle probe between captures.
            let mut period = demand.arm(now, true).unwrap();
            assert!(period <= BURST_INTERVAL);
            assert_eq!(demand.burst, BURST_FRAMES);
            for _ in 0..BURST_FRAMES {
                now += period;
                assert!(demand.probe(true));
                demand.sent(now);
                assert!(!demand.probe(false));
                // A 20 ms render drops the 16 ms probe then re-arms.
                now += Duration::from_millis(20);
                if demand.burst > 0 {
                    period = demand.arm(now, true).unwrap();
                    assert!(period <= BURST_INTERVAL);
                }
            }
            now = demand.last_send.unwrap() + Duration::from_millis(300);
            assert!(now.duration_since(demand.last_send.unwrap()) > DEMAND_IDLE);
        }
    }
}
