// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `comp.motion.run`: one step of a pointer glide. The timing lives in the
//! `motion` crate; this module moves the agent pointer to where the glide is
//! and reports whether it has arrived. The engine (`services/compd/src/comp.rs`)
//! owns the runs and their replies, and calls [`step`] once per tick.

use comp_model::ControlReply;
use motion::{Glide, Status};
use world::state::Loop;

use crate::input;

/// What one tick of a glide found.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// The pointer is on the glide and it has further to go.
    Moving,
    /// The pointer is at the glide's last point; the run is finished.
    Arrived,
    /// The agent seat refused the move (grab, stale output, and so on). The
    /// run ends and its caller gets this refusal.
    Refused(ControlReply),
}

/// Move the agent pointer to where `glide` is at `now` seconds, on `output`
/// (or the default output when `None`).
pub fn step(lp: &mut Loop, glide: &mut Glide, output: Option<&str>, now: f64) -> Step {
    let (point, status) = glide.sample(now);
    if let Err(refusal) = input::glide_step(lp, output, point.x, point.y) {
        return Step::Refused(refusal);
    }
    match status {
        Status::Running => Step::Moving,
        Status::Done => Step::Arrived,
    }
}
