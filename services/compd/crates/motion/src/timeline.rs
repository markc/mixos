// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! One motion as a function of elapsed time. The caller supplies `now` in
//! seconds on any monotone clock. A timeline never goes backwards: if `now`
//! jitters below the last value it returned, the last value is kept.
//!
//! Completion is decided here, not by a frame. `status_at` reports `Done` as
//! soon as the end time is reached, whether or not any frame was presented.

use crate::bezier::CubicBezier;
use crate::minjerk::min_jerk;

/// The progress curve of a timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Easing {
    Linear,
    Bezier(CubicBezier),
    MinJerk,
}

impl Easing {
    fn apply(&self, u: f64) -> f64 {
        match self {
            Self::Linear => u.clamp(0.0, 1.0),
            Self::Bezier(c) => c.sample(u),
            Self::MinJerk => min_jerk(u),
        }
    }
}

/// Whether a motion is still running.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Running,
    Done,
}

/// A timeline that runs from `start` for `duration` seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeline {
    start: f64,
    duration: f64,
    easing: Easing,
    /// The latest time seen. A clock step back is ignored, so progress never
    /// runs backwards in time. Overshoot curves still return to their end value.
    latest: f64,
}

/// The longest any motion may run. Above this a call is refused, so a script
/// cannot wait on a motion indefinitely (see the VT decision in the plan).
pub const MAX_DURATION_S: f64 = 10.0;

impl Timeline {
    /// A timeline starting at `start` seconds lasting `duration` seconds.
    /// Non-finite or negative durations are treated as zero (an instant jump).
    /// Durations over [`MAX_DURATION_S`] are clamped to it.
    pub fn new(start: f64, duration: f64, easing: Easing) -> Self {
        let duration = if duration.is_finite() {
            duration.clamp(0.0, MAX_DURATION_S)
        } else {
            0.0
        };
        Self {
            start,
            duration,
            easing,
            latest: start,
        }
    }

    /// The time at which the motion ends.
    pub fn end(&self) -> f64 {
        self.start + self.duration
    }

    /// Eased progress at time `now`. Time is taken as the latest seen, so a
    /// clock step back gives the same value as before. Overshoot curves can
    /// exceed 1 in value and come back down, as their shape requires.
    pub fn progress(&mut self, now: f64) -> f64 {
        if now > self.latest {
            self.latest = now;
        }
        let raw = if self.duration <= 0.0 {
            1.0
        } else {
            (self.latest - self.start) / self.duration
        };
        self.easing.apply(raw.clamp(0.0, 1.0))
    }

    /// Whether the motion has reached its end at `now`.
    pub fn status_at(&self, now: f64) -> Status {
        if now >= self.end() {
            Status::Done
        } else {
            Status::Running
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_runs_from_zero_to_one() {
        let mut t = Timeline::new(10.0, 2.0, Easing::Linear);
        assert_eq!(t.progress(9.0), 0.0);
        assert!((t.progress(11.0) - 0.5).abs() < 1e-12);
        assert_eq!(t.progress(12.0), 1.0);
        assert_eq!(t.progress(99.0), 1.0);
    }

    #[test]
    fn status_flips_at_the_end_time_with_no_frames() {
        let t = Timeline::new(0.0, 0.4, Easing::MinJerk);
        assert_eq!(t.status_at(0.39), Status::Running);
        assert_eq!(t.status_at(0.4), Status::Done);
        assert_eq!(t.status_at(5.0), Status::Done);
    }

    #[test]
    fn jitter_never_moves_progress_backwards() {
        let mut t = Timeline::new(0.0, 1.0, Easing::Linear);
        let a = t.progress(0.6);
        let b = t.progress(0.5); // a clock step back
        assert_eq!(a, b);
    }

    #[test]
    fn overshoot_curve_comes_back_down() {
        // A bounce-back curve passes 1 and returns to 1 only at the end of time.
        let c = CubicBezier::new(0.05, 0.9, 0.1, 1.1).unwrap();
        let mut t = Timeline::new(0.0, 1.0, Easing::Bezier(c));
        let peak = (1..=99)
            .map(|i| {
                let v = t.progress(i as f64 / 100.0);
                v
            })
            .fold(f64::MIN, f64::max);
        assert!(peak > 1.0, "overshoot peak {peak}");
        assert!((t.progress(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zero_duration_is_an_instant_jump() {
        let mut t = Timeline::new(3.0, 0.0, Easing::Bezier(CubicBezier::EASE));
        assert_eq!(t.progress(3.0), 1.0);
        assert_eq!(t.status_at(3.0), Status::Done);
    }

    #[test]
    fn overlong_and_nan_durations_are_bounded() {
        let long = Timeline::new(0.0, 500.0, Easing::Linear);
        assert_eq!(long.end(), MAX_DURATION_S);
        let nan = Timeline::new(0.0, f64::NAN, Easing::Linear);
        assert_eq!(nan.end(), 0.0);
    }

    #[test]
    fn bezier_easing_is_used() {
        let mut t = Timeline::new(0.0, 1.0, Easing::Bezier(CubicBezier::EASE_IN));
        let p = t.progress(0.5);
        assert!(p < 0.5, "ease-in is slow at the start, got {p}");
    }
}
