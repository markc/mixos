// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `comp.motion.run`: one tick of a motion. The timing lives in the `motion`
//! crate; this module moves the agent pointer or the window to where the
//! motion is, and reports whether it has arrived. The engine
//! (`services/compd/src/comp.rs`) owns the runs and their replies, and calls
//! these once per tick.

use comp_model::ControlReply;
use motion::{Glide, Status, Timeline};
use world::state::Loop;

use crate::{control, input};

/// What one tick of a motion found.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// The motion is under way and has further to go.
    Moving,
    /// The motion is at its last point; the run is finished.
    Arrived,
    /// The seat or the window refused the move. The run ends and its caller
    /// gets this refusal.
    Refused(ControlReply),
}

/// Move the agent pointer to where `glide` is at `now` seconds, on `output`
/// (or the default output when `None`).
pub fn pointer_step(lp: &mut Loop, glide: &mut Glide, output: Option<&str>, now: f64) -> Step {
    let (point, status) = glide.sample(now);
    if let Err(refusal) = input::glide_step(lp, output, point.x, point.y) {
        return Step::Refused(refusal);
    }
    arrived_or_moving(status)
}

/// Move window `id` (at `generation`) to where its motion is at `now`
/// seconds. `from` is where it started, `to` the target with `None` fields
/// unchanged, and `timeline` gives the eased progress.
pub fn window_step(
    lp: &mut Loop,
    id: u64,
    generation: u64,
    from: [f64; 4],
    to: [Option<f64>; 4],
    timeline: &mut Timeline,
    now: f64,
) -> Step {
    let progress = timeline.progress(now);
    let mut rect = [0.0; 4];
    for (i, slot) in rect.iter_mut().enumerate() {
        let end = to[i].unwrap_or(from[i]);
        *slot = from[i] + (end - from[i]) * progress;
    }
    if let Err(refusal) = control::window_glide_step(lp, id, generation, rect) {
        return Step::Refused(refusal);
    }
    arrived_or_moving(timeline.status_at(now))
}

fn arrived_or_moving(status: Status) -> Step {
    match status {
        Status::Running => Step::Moving,
        Status::Done => Step::Arrived,
    }
}
