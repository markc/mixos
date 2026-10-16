// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Movement time from Fitts's law, Shannon form: `T = a + b·log2(D/W + 1)`.
//! `a` and `b` are the caller's constants. The plan treats the published
//! ranges as unverified, so they are parameters, not hard-coded.

/// Movement time in milliseconds for distance `distance` to a target of width
/// `width`. Returns `a_ms` for zero distance, and is clamped to be finite.
/// Non-positive or non-finite widths give `a_ms` as well: no target size, no
/// index of difficulty.
pub fn fitts_ms(distance: f64, width: f64, a_ms: f64, b_ms_per_bit: f64) -> f64 {
    if !distance.is_finite() || distance <= 0.0 || !width.is_finite() || width <= 0.0 {
        return a_ms.max(0.0);
    }
    let id = (distance / width + 1.0).log2();
    (a_ms + b_ms_per_bit * id).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_distance_is_the_fixed_cost() {
        assert_eq!(fitts_ms(0.0, 20.0, 50.0, 120.0), 50.0);
    }

    #[test]
    fn longer_moves_take_longer() {
        let short = fitts_ms(100.0, 20.0, 50.0, 120.0);
        let long = fitts_ms(1000.0, 20.0, 50.0, 120.0);
        assert!(long > short);
    }

    #[test]
    fn smaller_targets_take_longer() {
        let big = fitts_ms(400.0, 80.0, 50.0, 120.0);
        let small = fitts_ms(400.0, 8.0, 50.0, 120.0);
        assert!(small > big);
    }

    #[test]
    fn degenerate_width_falls_back_to_fixed_cost() {
        assert_eq!(fitts_ms(300.0, 0.0, 50.0, 120.0), 50.0);
        assert_eq!(fitts_ms(300.0, f64::NAN, 50.0, 120.0), 50.0);
    }

    #[test]
    fn matches_the_formula() {
        // D/W = 3, log2(4) = 2, so T = 50 + 120*2 = 290.
        assert!((fitts_ms(300.0, 100.0, 50.0, 120.0) - 290.0).abs() < 1e-9);
    }
}
