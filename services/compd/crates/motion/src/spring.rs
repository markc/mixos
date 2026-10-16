// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Damped spring in closed form. The compositor evaluates it from the
//! retarget instant, never by Euler stepping, so the result does not depend
//! on frame rate. Position is measured from the target: `x = pos - target`.

use std::fmt;

/// Why a spring was refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpringError {
    /// Stiffness must be positive and finite.
    Stiffness,
    /// Damping ratio must be non-negative and finite.
    Damping,
}

impl fmt::Display for SpringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stiffness => write!(f, "spring stiffness must be positive and finite"),
            Self::Damping => write!(f, "spring damping ratio must be non-negative and finite"),
        }
    }
}

impl std::error::Error for SpringError {}

/// Offset from the target and velocity at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringState {
    pub x: f64,
    pub v: f64,
}

/// A spring with natural frequency `omega` (rad/s) and damping ratio `zeta`.
/// `zeta == 1` is critically damped (no overshoot); `zeta < 1` oscillates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    omega: f64,
    zeta: f64,
}

impl Spring {
    /// Build from stiffness `k` and mass `m` (`omega = sqrt(k/m)`).
    pub fn new(stiffness: f64, mass: f64, zeta: f64) -> Result<Self, SpringError> {
        if !(stiffness.is_finite() && stiffness > 0.0 && mass.is_finite() && mass > 0.0) {
            return Err(SpringError::Stiffness);
        }
        if !(zeta.is_finite() && zeta >= 0.0) {
            return Err(SpringError::Damping);
        }
        Ok(Self {
            omega: (stiffness / mass).sqrt(),
            zeta,
        })
    }

    /// State `t` seconds after `start`, given state `start` at `t = 0`.
    pub fn evaluate(&self, start: SpringState, t: f64) -> SpringState {
        let t = t.max(0.0);
        let (w, z) = (self.omega, self.zeta);
        if (z - 1.0).abs() < 1e-9 {
            // Critically damped: x(t) = (x0 + (v0 + w x0) t) e^{-wt}.
            let e = (-w * t).exp();
            let c = start.v + w * start.x;
            SpringState {
                x: (start.x + c * t) * e,
                v: (start.v - w * c * t) * e,
            }
        } else if z < 1.0 {
            // Underdamped: x(t) = e^{-zwt}(x0 cos wd t + B sin wd t).
            let wd = w * (1.0 - z * z).sqrt();
            let e = (-z * w * t).exp();
            let b = (start.v + z * w * start.x) / wd;
            let (s, c) = (wd * t).sin_cos();
            SpringState {
                x: e * (start.x * c + b * s),
                v: e * ((b * wd - z * w * start.x) * c - (start.x * wd + z * w * b) * s),
            }
        } else {
            // Overdamped: two real roots.
            let r = w * (z * z - 1.0).sqrt();
            let (r1, r2) = (-z * w + r, -z * w - r);
            // Solve A + B = x0, A r1 + B r2 = v0.
            let b = (start.v - r1 * start.x) / (r2 - r1);
            let a = start.x - b;
            SpringState {
                x: a * (r1 * t).exp() + b * (r2 * t).exp(),
                v: a * r1 * (r1 * t).exp() + b * r2 * (r2 * t).exp(),
            }
        }
    }

    /// True once both offset and velocity are inside `eps` (pixels, or
    /// whatever unit `start` used).
    pub fn settled(s: SpringState, eps: f64) -> bool {
        s.x.abs() < eps && s.v.abs() < eps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn starts_at_its_initial_state() {
        for z in [0.3, 1.0, 2.0] {
            let sp = Spring::new(900.0, 1.0, z).unwrap();
            let s0 = SpringState { x: 40.0, v: -12.0 };
            let s = sp.evaluate(s0, 0.0);
            assert!(close(s.x, 40.0, 1e-9) && close(s.v, -12.0, 1e-9), "z={z}");
        }
    }

    #[test]
    fn velocity_matches_numeric_derivative() {
        for z in [0.4, 1.0, 1.6] {
            let sp = Spring::new(800.0, 1.0, z).unwrap();
            let s0 = SpringState { x: 100.0, v: 30.0 };
            let t = 0.07;
            let h = 1e-7;
            let num = (sp.evaluate(s0, t + h).x - sp.evaluate(s0, t - h).x) / (2.0 * h);
            let ana = sp.evaluate(s0, t).v;
            assert!(close(num, ana, 1e-3), "z={z}: {num} vs {ana}");
        }
    }

    #[test]
    fn critical_damping_never_crosses_the_target() {
        let sp = Spring::new(900.0, 1.0, 1.0).unwrap();
        let s0 = SpringState { x: 200.0, v: 0.0 };
        let mut t = 0.0;
        while t < 2.0 {
            assert!(sp.evaluate(s0, t).x >= -1e-9, "crossed at t={t}");
            t += 0.001;
        }
    }

    #[test]
    fn underdamped_spring_overshoots_and_settles() {
        let sp = Spring::new(900.0, 1.0, 0.3).unwrap();
        let s0 = SpringState { x: 200.0, v: 0.0 };
        let min = (0..2000)
            .map(|i| sp.evaluate(s0, i as f64 * 0.001).x)
            .fold(f64::MAX, f64::min);
        assert!(min < -5.0, "expected overshoot, min {min}");
        assert!(Spring::settled(sp.evaluate(s0, 5.0), 0.01));
    }

    #[test]
    fn overdamped_settles_without_crossing() {
        let sp = Spring::new(900.0, 1.0, 2.0).unwrap();
        let s0 = SpringState { x: 200.0, v: 0.0 };
        let mut t = 0.0;
        while t < 3.0 {
            assert!(sp.evaluate(s0, t).x >= -1e-6, "crossed at t={t}");
            t += 0.001;
        }
        assert!(Spring::settled(sp.evaluate(s0, 5.0), 0.01));
    }

    #[test]
    fn invalid_parameters_are_refused() {
        assert_eq!(Spring::new(0.0, 1.0, 1.0), Err(SpringError::Stiffness));
        assert_eq!(Spring::new(10.0, -1.0, 1.0), Err(SpringError::Stiffness));
        assert_eq!(Spring::new(10.0, 1.0, -0.5), Err(SpringError::Damping));
    }
}
