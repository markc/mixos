// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A route through waypoints: a centripetal Catmull-Rom curve (Yuksel, Schaefer
//! and Keyser, 2011), sampled into a polyline and indexed by arc length. Easing
//! is applied to distance travelled, so speed stays smooth through waypoints.
//! The curve passes through every waypoint and has no cusps within a segment.

use std::fmt;

/// A point in the same units as the caller's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Why a route was refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RouteError {
    /// Fewer than two waypoints.
    TooFew,
    /// A coordinate is NaN or infinite.
    NotFinite,
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFew => write!(f, "a route needs at least two waypoints"),
            Self::NotFinite => write!(f, "route coordinates must be finite"),
        }
    }
}

impl std::error::Error for RouteError {}

/// A sampled route with its cumulative arc length.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    points: Vec<Point>,
    /// `cum[i]` is the length from `points[0]` to `points[i]`.
    cum: Vec<f64>,
}

/// Samples per segment. Enough that the polyline error is well under a pixel
/// for route spans up to a few thousand pixels.
const SAMPLES_PER_SEGMENT: usize = 128;

impl Route {
    /// Build a route through `waypoints`, in order. Consecutive duplicate
    /// points are allowed; they add no length.
    pub fn new(waypoints: &[Point]) -> Result<Self, RouteError> {
        if waypoints.len() < 2 {
            return Err(RouteError::TooFew);
        }
        if waypoints
            .iter()
            .any(|p| !p.x.is_finite() || !p.y.is_finite())
        {
            return Err(RouteError::NotFinite);
        }
        // Duplicate the first and last points so the curve reaches the ends.
        let n = waypoints.len();
        let mut ext = Vec::with_capacity(n + 2);
        ext.push(waypoints[0]);
        ext.extend_from_slice(waypoints);
        ext.push(waypoints[n - 1]);

        let mut points = Vec::with_capacity((n - 1) * SAMPLES_PER_SEGMENT + 1);
        for seg in 0..n - 1 {
            let (p0, p1, p2, p3) = (ext[seg], ext[seg + 1], ext[seg + 2], ext[seg + 3]);
            for k in 0..SAMPLES_PER_SEGMENT {
                let u = k as f64 / SAMPLES_PER_SEGMENT as f64;
                points.push(catmull_rom(p0, p1, p2, p3, u));
            }
        }
        points.push(waypoints[n - 1]);

        let mut cum = Vec::with_capacity(points.len());
        cum.push(0.0);
        for w in points.windows(2) {
            let last = *cum.last().unwrap_or(&0.0);
            cum.push(last + dist(w[0], w[1]));
        }
        Ok(Self { points, cum })
    }

    /// Total length along the route.
    pub fn length(&self) -> f64 {
        *self.cum.last().unwrap_or(&0.0)
    }

    /// The point a distance `d` along the route. `d` is clamped to the route.
    pub fn at_distance(&self, d: f64) -> Point {
        let total = self.length();
        if total <= 0.0 {
            return self.points[0];
        }
        let d = d.clamp(0.0, total);
        // First index whose cumulative length reaches d.
        let i = match self.cum.binary_search_by(|c| c.total_cmp(&d)) {
            Ok(i) => return self.points[i],
            Err(i) => i.clamp(1, self.points.len() - 1),
        };
        let (c0, c1) = (self.cum[i - 1], self.cum[i]);
        let f = if c1 > c0 { (d - c0) / (c1 - c0) } else { 0.0 };
        lerp(self.points[i - 1], self.points[i], f)
    }

    /// The point at eased progress `s` in [0,1] of the route's length.
    pub fn at_progress(&self, s: f64) -> Point {
        self.at_distance(s.clamp(0.0, 1.0) * self.length())
    }
}

fn dist(a: Point, b: Point) -> f64 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

fn lerp(a: Point, b: Point, f: f64) -> Point {
    Point {
        x: a.x + (b.x - a.x) * f,
        y: a.y + (b.y - a.y) * f,
    }
}

/// Centripetal Catmull-Rom between `p1` and `p2` at `u` in [0,1), using the
/// Barry-Goldman pyramid with knot spacing `|Pi+1 - Pi|^0.5`.
fn catmull_rom(p0: Point, p1: Point, p2: Point, p3: Point, u: f64) -> Point {
    let knot = |a: Point, b: Point, t: f64| t + dist(a, b).sqrt();
    let t0 = 0.0;
    let t1 = knot(p0, p1, t0);
    let t2 = knot(p1, p2, t1);
    let t3 = knot(p2, p3, t2);
    let t = t1 + u * (t2 - t1);
    // A zero-length knot interval (duplicate points) collapses to the earlier point.
    let blend = |a: Point, b: Point, ta: f64, tb: f64| {
        if (tb - ta).abs() < 1e-12 {
            a
        } else {
            lerp(a, b, (t - ta) / (tb - ta))
        }
    };
    let a1 = blend(p0, p1, t0, t1);
    let a2 = blend(p1, p2, t1, t2);
    let a3 = blend(p2, p3, t2, t3);
    let b1 = blend(a1, a2, t0, t2);
    let b2 = blend(a2, a3, t1, t3);
    blend(b1, b2, t1, t2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn two_points_is_a_straight_line() {
        let r = Route::new(&[pt(0.0, 0.0), pt(300.0, 400.0)]).unwrap();
        assert!((r.length() - 500.0).abs() < 1e-6);
        let m = r.at_distance(250.0);
        assert!((m.x - 150.0).abs() < 1e-6 && (m.y - 200.0).abs() < 1e-6);
    }

    #[test]
    fn curve_passes_through_every_waypoint() {
        let w = [
            pt(0.0, 0.0),
            pt(100.0, 200.0),
            pt(300.0, 50.0),
            pt(400.0, 400.0),
        ];
        let r = Route::new(&w).unwrap();
        // Each waypoint lies on the sampled polyline (distance to nearest sample ~0).
        for want in &w {
            let near = r
                .points
                .iter()
                .map(|p| dist(*p, *want))
                .fold(f64::MAX, f64::min);
            assert!(near < 1e-6, "waypoint {want:?} missed by {near}");
        }
    }

    #[test]
    fn endpoints_are_exact() {
        let w = [pt(10.0, 10.0), pt(200.0, 50.0), pt(90.0, 300.0)];
        let r = Route::new(&w).unwrap();
        assert_eq!(r.at_distance(0.0), w[0]);
        let end = r.at_distance(r.length());
        assert!(dist(end, w[2]) < 1e-9);
    }

    #[test]
    fn constant_speed_by_arc_length() {
        let r = Route::new(&[pt(0.0, 0.0), pt(100.0, 300.0), pt(500.0, 100.0)]).unwrap();
        let total = r.length();
        // Equal distance steps give equal straight-line spacing between samples
        // (the route is sampled finely, so chord error is small).
        let n = 20;
        let mut prev = r.at_distance(0.0);
        let mut gaps = Vec::new();
        for i in 1..=n {
            let p = r.at_distance(total * i as f64 / n as f64);
            gaps.push(dist(prev, p));
            prev = p;
        }
        let mean = gaps.iter().sum::<f64>() / n as f64;
        // Chords cut across the bend at the corner, so allow a few percent.
        for g in gaps {
            assert!((g - mean).abs() / mean < 0.05, "gap {g} vs mean {mean}");
        }
    }

    #[test]
    fn duplicate_waypoints_add_no_length_and_do_not_panic() {
        let r = Route::new(&[pt(0.0, 0.0), pt(0.0, 0.0), pt(100.0, 0.0)]).unwrap();
        assert!((r.length() - 100.0).abs() < 1e-6);
        let p = r.at_progress(0.5);
        assert!((p.x - 50.0).abs() < 1e-6);
    }

    #[test]
    fn zero_length_route_stays_put() {
        let r = Route::new(&[pt(5.0, 5.0), pt(5.0, 5.0)]).unwrap();
        assert_eq!(r.length(), 0.0);
        assert_eq!(r.at_progress(0.7), pt(5.0, 5.0));
    }

    #[test]
    fn refuses_bad_input() {
        assert_eq!(Route::new(&[pt(0.0, 0.0)]), Err(RouteError::TooFew));
        assert_eq!(
            Route::new(&[pt(0.0, 0.0), pt(f64::NAN, 1.0)]),
            Err(RouteError::NotFinite)
        );
    }
}
