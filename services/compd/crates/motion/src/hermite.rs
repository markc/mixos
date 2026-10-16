// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Cubic Hermite segment over time. A retarget starts from the value and
//! velocity the motion had at the moment of the new call, so the path has no
//! kink. The segment runs `t` in [0, duration] with
//! `p(0) = p0`, `p'(0) = v0`, `p(T) = p1`, `p'(T) = v1`.

/// A one-dimensional Hermite segment. Velocities are per unit of `t`, the
/// same unit as `duration`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hermite {
    pub p0: f64,
    pub v0: f64,
    pub p1: f64,
    pub v1: f64,
    pub duration: f64,
}

impl Hermite {
    /// Position at time `t`, clamped to [0, duration].
    pub fn position(&self, t: f64) -> f64 {
        if self.duration <= 0.0 {
            return self.p1;
        }
        let s = (t / self.duration).clamp(0.0, 1.0);
        let h = self.duration;
        // Standard Hermite basis, with tangents scaled to the segment length.
        let s2 = s * s;
        let s3 = s2 * s;
        let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
        let h10 = s3 - 2.0 * s2 + s;
        let h01 = -2.0 * s3 + 3.0 * s2;
        let h11 = s3 - s2;
        h00 * self.p0 + h10 * h * self.v0 + h01 * self.p1 + h11 * h * self.v1
    }

    /// Velocity at time `t`, clamped to [0, duration].
    pub fn velocity(&self, t: f64) -> f64 {
        if self.duration <= 0.0 {
            return self.v1;
        }
        let s = (t / self.duration).clamp(0.0, 1.0);
        let h = self.duration;
        let s2 = s * s;
        let d00 = 6.0 * s2 - 6.0 * s;
        let d10 = 3.0 * s2 - 4.0 * s + 1.0;
        let d01 = -6.0 * s2 + 6.0 * s;
        let d11 = 3.0 * s2 - 2.0 * s;
        (d00 * self.p0 + d01 * self.p1) / h + d10 * self.v0 + d11 * self.v1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg() -> Hermite {
        Hermite {
            p0: 10.0,
            v0: 4.0,
            p1: 210.0,
            v1: 0.0,
            duration: 0.5,
        }
    }

    #[test]
    fn hits_both_ends_and_both_velocities() {
        let s = seg();
        assert!((s.position(0.0) - 10.0).abs() < 1e-9);
        assert!((s.position(0.5) - 210.0).abs() < 1e-9);
        assert!((s.velocity(0.0) - 4.0).abs() < 1e-9);
        assert!(s.velocity(0.5).abs() < 1e-9);
    }

    #[test]
    fn velocity_matches_numeric_derivative() {
        let s = seg();
        let h = 1e-7;
        for t in [0.05, 0.2, 0.4] {
            let num = (s.position(t + h) - s.position(t - h)) / (2.0 * h);
            assert!((num - s.velocity(t)).abs() < 1e-3, "t={t}");
        }
    }

    #[test]
    fn retarget_is_velocity_continuous() {
        // A motion moving at 300/s is retargeted; the new segment must start at
        // that velocity, so there is no kink at the join.
        let old_v = 300.0;
        let new = Hermite {
            p0: 500.0,
            v0: old_v,
            p1: 100.0,
            v1: 0.0,
            duration: 0.4,
        };
        assert!((new.velocity(0.0) - old_v).abs() < 1e-9);
    }

    #[test]
    fn zero_duration_jumps_to_the_end() {
        let s = Hermite {
            duration: 0.0,
            ..seg()
        };
        assert_eq!(s.position(0.0), 210.0);
    }
}
