// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A glide: one point travelling a route over a timeline. Easing acts on
//! distance travelled (the plan's rule), so speed stays smooth through
//! waypoints. Pure: the caller passes the time and gets a point back.

use crate::route::{Point, Route};
use crate::timeline::{Easing, Status, Timeline};

/// A route with a timeline. `sample` gives the point and whether the glide
/// has finished at that time.
#[derive(Clone, Debug, PartialEq)]
pub struct Glide {
    route: Route,
    timeline: Timeline,
}

impl Glide {
    /// A glide along `route`, starting at `start` seconds and lasting
    /// `duration` seconds (clamped by [`Timeline::new`]).
    pub fn new(route: Route, easing: Easing, start: f64, duration: f64) -> Self {
        Self {
            route,
            timeline: Timeline::new(start, duration, easing),
        }
    }

    /// The point and status at time `now` (seconds, same clock as `start`).
    pub fn sample(&mut self, now: f64) -> (Point, Status) {
        let p = self.timeline.progress(now);
        let point = self.route.at_progress(p);
        (point, self.timeline.status_at(now))
    }

    /// The time at which the glide ends.
    pub fn end(&self) -> f64 {
        self.timeline.end()
    }

    /// The final point of the route.
    pub fn target(&self) -> Point {
        self.route.at_progress(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bezier::CubicBezier;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn runs_from_first_to_last_waypoint_and_reports_done() {
        let route = Route::new(&[p(0.0, 0.0), p(100.0, 0.0), p(100.0, 100.0)]).unwrap();
        let mut g = Glide::new(route, Easing::Linear, 5.0, 2.0);
        let (start, s0) = g.sample(5.0);
        assert_eq!(s0, Status::Running);
        assert!(start.x.abs() < 1e-9 && start.y.abs() < 1e-9);
        let (end, s1) = g.sample(7.0);
        assert_eq!(s1, Status::Done);
        assert!((end.x - 100.0).abs() < 1e-6 && (end.y - 100.0).abs() < 1e-6);
        assert_eq!(g.target(), end);
    }

    #[test]
    fn constant_speed_on_a_straight_line_with_linear_easing() {
        let route = Route::new(&[p(0.0, 0.0), p(300.0, 0.0)]).unwrap();
        let mut g = Glide::new(route, Easing::Linear, 0.0, 1.0);
        let (mid, _) = g.sample(0.5);
        assert!((mid.x - 150.0).abs() < 1e-6);
    }

    #[test]
    fn ease_in_out_is_slow_at_the_start() {
        let route = Route::new(&[p(0.0, 0.0), p(300.0, 0.0)]).unwrap();
        let mut g = Glide::new(
            route,
            Easing::Bezier(CubicBezier::EASE_IN_OUT),
            0.0,
            1.0,
        );
        let (early, _) = g.sample(0.1);
        assert!(early.x < 30.0, "slow start, got {}", early.x);
    }

    #[test]
    fn zero_duration_lands_on_the_end_at_once() {
        let route = Route::new(&[p(0.0, 0.0), p(40.0, 0.0)]).unwrap();
        let mut g = Glide::new(route, Easing::Linear, 1.0, 0.0);
        let (point, status) = g.sample(1.0);
        assert_eq!(status, Status::Done);
        assert!((point.x - 40.0).abs() < 1e-9);
    }
}
