// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Minimum-jerk position profile (Flash and Hogan, 1985). Zero velocity and
//! zero acceleration at both ends, peak speed 1.875 times the average at
//! the midpoint. Used for human-like pointer travel.

/// Progress at normalised time `u`, clamped to [0,1]:
/// `s(u) = 10u^3 - 15u^4 + 6u^5`.
pub fn min_jerk(u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    let u3 = u * u * u;
    u3 * (10.0 - 15.0 * u + 6.0 * u * u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_and_midpoint() {
        assert_eq!(min_jerk(0.0), 0.0);
        assert_eq!(min_jerk(1.0), 1.0);
        assert!((min_jerk(0.5) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn velocity_is_zero_at_both_ends() {
        let h = 1e-6;
        let v0 = (min_jerk(h) - min_jerk(0.0)) / h;
        let v1 = (min_jerk(1.0) - min_jerk(1.0 - h)) / h;
        assert!(v0.abs() < 1e-4, "start velocity {v0}");
        assert!(v1.abs() < 1e-4, "end velocity {v1}");
    }

    #[test]
    fn peak_speed_is_one_point_875_times_average() {
        let h = 1e-5;
        let peak = (0..=2000)
            .map(|i| {
                let u = i as f64 / 2000.0;
                (min_jerk((u + h).min(1.0)) - min_jerk((u - h).max(0.0))) / (2.0 * h)
            })
            .fold(f64::MIN, f64::max);
        assert!((peak - 1.875).abs() < 1e-3, "peak {peak}");
    }

    #[test]
    fn monotone() {
        let mut last = 0.0;
        for i in 0..=1000 {
            let y = min_jerk(i as f64 / 1000.0);
            assert!(y + 1e-12 >= last);
            last = y;
        }
    }
}
