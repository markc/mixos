// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Motion maths for compd (plan `_plan/2026-10-13-motion.md`, phase 1).
//!
//! Everything here is a pure function of time or distance. Nothing reads a
//! clock, a display or a GPU, so every rule can be pinned by a test. The
//! compositor samples these at each presented frame.
//!
//! * [`bezier`]: CSS-style cubic-bezier timing (`x` in [0,1], `y` may overshoot).
//! * [`minjerk`]: minimum-jerk position profile for human-like travel.
//! * [`spring`]: closed-form damped spring, for retargeting mid-flight.
//! * [`route`]: centripetal Catmull-Rom through waypoints, sampled by arc length.
//! * [`fitts`]: movement time from distance and target width.
//! * [`hermite`]: cubic Hermite segment with given end velocities, so a
//!   retarget keeps velocity continuous.
//! * [`timeline`]: one motion as a clamped, monotone function of elapsed time.

mod bezier;
mod fitts;
mod hermite;
mod minjerk;
mod route;
mod spring;
mod timeline;

pub use bezier::{BezierError, CubicBezier};
pub use fitts::fitts_ms;
pub use hermite::Hermite;
pub use minjerk::min_jerk;
pub use route::{Point, Route, RouteError};
pub use spring::{Spring, SpringError, SpringState};
pub use timeline::{Easing, Status, Timeline};
