// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Cubic-bezier timing with CSS semantics. The curve runs from (0,0) to
//! (1,1); `x1` and `x2` must lie in [0,1] so time never runs backwards, and
//! `y1` and `y2` may leave [0,1] to overshoot.

use std::fmt;

/// Why a timing curve was refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BezierError {
    /// `x1` or `x2` is outside [0,1], so the curve is not a function of time.
    XOutOfRange,
    /// A coordinate is NaN or infinite.
    NotFinite,
}

impl fmt::Display for BezierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::XOutOfRange => write!(f, "bezier x1 and x2 must lie in [0,1]"),
            Self::NotFinite => write!(f, "bezier coordinates must be finite"),
        }
    }
}

impl std::error::Error for BezierError {}

/// A timing curve `(x1, y1, x2, y2)`, as in CSS `cubic-bezier()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

impl CubicBezier {
    /// The CSS `ease` curve.
    pub const EASE: Self = Self::new_unchecked(0.25, 0.1, 0.25, 1.0);
    /// The CSS `ease-in` curve.
    pub const EASE_IN: Self = Self::new_unchecked(0.42, 0.0, 1.0, 1.0);
    /// The CSS `ease-out` curve.
    pub const EASE_OUT: Self = Self::new_unchecked(0.0, 0.0, 0.58, 1.0);
    /// The CSS `ease-in-out` curve.
    pub const EASE_IN_OUT: Self = Self::new_unchecked(0.42, 0.0, 0.58, 1.0);

    /// Validate and build a curve.
    pub fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Result<Self, BezierError> {
        if ![x1, y1, x2, y2].iter().all(|v| v.is_finite()) {
            return Err(BezierError::NotFinite);
        }
        if !(0.0..=1.0).contains(&x1) || !(0.0..=1.0).contains(&x2) {
            return Err(BezierError::XOutOfRange);
        }
        Ok(Self::new_unchecked(x1, y1, x2, y2))
    }

    const fn new_unchecked(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self { x1, y1, x2, y2 }
    }

    /// The progress `y` at normalised time `u`. `u` is clamped to [0,1].
    ///
    /// Solves `x(s) = u` for the curve parameter `s` with Newton steps,
    /// falling back to bisection where the slope is flat, then reads `y(s)`.
    pub fn sample(&self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        if u == 0.0 || u == 1.0 {
            return u;
        }
        let s = self.solve_s(u);
        bez(self.y1, self.y2, s)
    }

    fn solve_s(&self, u: f64) -> f64 {
        let mut s = u;
        for _ in 0..8 {
            let x = bez(self.x1, self.x2, s) - u;
            if x.abs() < 1e-9 {
                return s;
            }
            let d = bez_d(self.x1, self.x2, s);
            if d.abs() < 1e-6 {
                break;
            }
            s = (s - x / d).clamp(0.0, 1.0);
        }
        // Bisection: x(s) is monotone because x1 and x2 are in [0,1].
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if bez(self.x1, self.x2, mid) < u {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }
}

/// One coordinate of a cubic bezier with end points 0 and 1.
fn bez(p1: f64, p2: f64, s: f64) -> f64 {
    let k = 1.0 - s;
    3.0 * k * k * s * p1 + 3.0 * k * s * s * p2 + s * s * s
}

/// Derivative of [`bez`] with respect to `s`.
fn bez_d(p1: f64, p2: f64, s: f64) -> f64 {
    let k = 1.0 - s;
    3.0 * k * k * p1 + 6.0 * k * s * (p2 - p1) + 3.0 * s * s * (1.0 - p2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_exact() {
        for c in [CubicBezier::EASE, CubicBezier::EASE_IN, CubicBezier::EASE_OUT] {
            assert_eq!(c.sample(0.0), 0.0);
            assert_eq!(c.sample(1.0), 1.0);
        }
    }

    #[test]
    fn linear_curve_is_identity() {
        let c = CubicBezier::new(1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0).unwrap();
        for i in 0..=20 {
            let u = i as f64 / 20.0;
            assert!((c.sample(u) - u).abs() < 1e-6, "u={u}");
        }
    }

    #[test]
    fn valid_curves_are_monotone_in_time() {
        for c in [CubicBezier::EASE, CubicBezier::EASE_IN_OUT] {
            let mut last = 0.0;
            for i in 0..=400 {
                let y = c.sample(i as f64 / 400.0);
                assert!(y + 1e-9 >= last, "monotone at step {i}");
                last = y;
            }
        }
    }

    #[test]
    fn overshoot_is_allowed_in_y() {
        let c = CubicBezier::new(0.05, 0.9, 0.1, 1.1).unwrap();
        let peak = (0..=400)
            .map(|i| c.sample(i as f64 / 400.0))
            .fold(f64::MIN, f64::max);
        assert!(peak > 1.0, "overshoot peak {peak}");
    }

    #[test]
    fn x_outside_unit_interval_is_refused() {
        assert_eq!(
            CubicBezier::new(-0.1, 0.0, 0.5, 1.0),
            Err(BezierError::XOutOfRange)
        );
        assert_eq!(
            CubicBezier::new(0.2, 0.0, 1.5, 1.0),
            Err(BezierError::XOutOfRange)
        );
        assert_eq!(
            CubicBezier::new(f64::NAN, 0.0, 0.5, 1.0),
            Err(BezierError::NotFinite)
        );
    }

    #[test]
    fn input_is_clamped() {
        assert_eq!(CubicBezier::EASE.sample(-3.0), 0.0);
        assert_eq!(CubicBezier::EASE.sample(7.0), 1.0);
    }
}
