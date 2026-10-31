// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The derivation's primitives (spec §4.1): `Ramp`, `Idx`, `Surface`,
//! `Small`, `Text`, the non-standard `Over` and WCAG contrast. Colours are
//! gamma-encoded sRGB with straight alpha; lightness is OKLab, converted
//! through the crate's linear-light OKLCh helpers.

use crate::colour_model::derivation::{gamut_map_oklch, linear_srgb_to_oklch};
use crate::colour_model::{linear_to_srgb, relative_luminance, srgb_to_linear};
use crate::{LinearRgba, SrgbColour, contrast_ratio};

pub(super) const TRANSPARENT: SrgbColour = SrgbColour {
    red: 0.0,
    green: 0.0,
    blue: 0.0,
    alpha: 0.0,
};
pub(super) const BLACK: SrgbColour = SrgbColour {
    red: 0.0,
    green: 0.0,
    blue: 0.0,
    alpha: 1.0,
};
pub(super) const WHITE: SrgbColour = SrgbColour {
    red: 1.0,
    green: 1.0,
    blue: 1.0,
    alpha: 1.0,
};

/// The bisection budget for bringing a ramp step back into gamut.
const GAMUT_ITERATIONS: usize = 64;
/// `Small`'s CIE LCh chroma cap: 3% of the 0..128 chroma scale.
const SMALL_CHROMA_CAP: f64 = 0.03 * 128.0;

pub(super) fn with_alpha(colour: SrgbColour, alpha: f64) -> SrgbColour {
    SrgbColour { alpha, ..colour }
}

fn to_linear(colour: SrgbColour) -> LinearRgba {
    LinearRgba {
        red: srgb_to_linear(colour.red),
        green: srgb_to_linear(colour.green),
        blue: srgb_to_linear(colour.blue),
        alpha: colour.alpha,
    }
}

fn from_linear(rgb: [f64; 3], alpha: f64) -> SrgbColour {
    let encode = |channel: f64| linear_to_srgb(channel.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    SrgbColour {
        red: encode(rgb[0]),
        green: encode(rgb[1]),
        blue: encode(rgb[2]),
        alpha,
    }
}

/// OKLab lightness, 0 to 1.
pub(super) fn lightness(colour: SrgbColour) -> f64 {
    linear_srgb_to_oklch(to_linear(colour)).0
}

/// Ramp(c, N): N steps of c's OKLCh hue and chroma at lightness i/(N−1),
/// each gamut-mapped by chroma bisection and clamped. Every step keeps the
/// seed's alpha.
pub(super) fn ramp(seed: SrgbColour, steps: usize) -> Vec<SrgbColour> {
    let (_, chroma, hue) = linear_srgb_to_oklch(to_linear(seed));
    let last = (steps - 1) as f64;
    (0..steps)
        .map(|i| {
            let rgb = gamut_map_oklch(i as f64 / last, chroma, hue, GAMUT_ITERATIONS);
            from_linear(rgb, seed.alpha)
        })
        .collect()
}

/// Idx(c) = clamp(round(L × 100), 0, 99). The factor is 100 although a
/// 100-step ramp's step i has L = i/99, so re-indexing a ramp colour shifts
/// it up by one from step 50; the derivation depends on that shift.
pub(super) fn idx(colour: SrgbColour) -> usize {
    (lightness(colour) * 100.0).round().clamp(0.0, 99.0) as usize
}

/// The index `n` steps from `base`, up or down, if it stays in 0..=99.
fn step(base: usize, n: usize, up: bool) -> Option<usize> {
    if up {
        Some(base + n).filter(|index| *index <= 99)
    } else {
        base.checked_sub(n)
    }
}

/// Surface(b, n): the ramp step n away from base index b, lighter when the
/// theme is dark or b < 91, else darker; `fallback` off the ramp's ends.
pub(super) fn surface(
    base: usize,
    n: usize,
    ramp: &[SrgbColour],
    dark: bool,
    fallback: SrgbColour,
) -> SrgbColour {
    step(base, n, dark || base < 91).map_or(fallback, |index| ramp[index])
}

/// Small(b, 5): five neutral-ramp steps from b (up when b ≤ 40 or
/// 51 < b < 65, else down), with its CIE LCh chroma capped at 3.84.
pub(super) fn small(base: usize, neutral_ramp: &[SrgbColour], fallback: SrgbColour) -> SrgbColour {
    let up = base <= 40 || (51 < base && base < 65);
    let colour = step(base, 5, up).map_or(fallback, |index| neutral_ramp[index]);
    cap_cie_chroma(colour, SMALL_CHROMA_CAP)
}

/// Text(b): the ramp step 70 (else 50) steps from b towards the far end,
/// lighter on a dark base (b < 60) and darker on a light one, pushed
/// further by round((1 − alpha) × 100) steps for a translucent surface;
/// the ramp's end when both leave it. A text tint ramp replaces `ramp`.
/// The colour keeps the ramp's alpha; `alpha` only moves the index.
pub(super) fn text(
    base: usize,
    ramp: &[SrgbColour],
    fallback: SrgbColour,
    tint: Option<&[SrgbColour]>,
    alpha: f64,
) -> SrgbColour {
    let ramp = tint.unwrap_or(ramp);
    let extra = if alpha < 1.0 {
        ((1.0 - alpha) * 100.0).round() as usize
    } else {
        0
    };
    let dark = base < 60;
    let index = step(base, 70 + extra, dark)
        .or_else(|| step(base, 50 + extra, dark))
        .unwrap_or(if dark { 99 } else { 0 });
    ramp.get(index).copied().unwrap_or(fallback)
}

/// Over(a, b), "a over b" on gamma-encoded channels, exactly as the
/// reference computes it: out_α = a_α + b_α(1 − a_α) and
/// out_c = a_c·a_α + b_c·b_α(1 − a_α)/out_α. The first term is not divided
/// by out_α, so this is not the textbook operator; do not fix it, or the
/// derived values stop matching.
pub(super) fn over(a: SrgbColour, b: SrgbColour) -> SrgbColour {
    let alpha = a.alpha + b.alpha * (1.0 - a.alpha);
    let channel = |top: f64, bottom: f64| {
        let under = if alpha > 0.0 {
            bottom * b.alpha * (1.0 - a.alpha) / alpha
        } else {
            0.0
        };
        (top * a.alpha + under).clamp(0.0, 1.0)
    };
    SrgbColour {
        red: channel(a.red, b.red),
        green: channel(a.green, b.green),
        blue: channel(a.blue, b.blue),
        alpha: alpha.clamp(0.0, 1.0),
    }
}

/// WCAG 2.1 contrast of two colours' opaque channels.
pub(super) fn contrast(a: SrgbColour, b: SrgbColour) -> f64 {
    contrast_ratio(to_linear(a), to_linear(b))
}

/// WCAG 2.1 relative luminance of a colour's opaque channels.
pub(super) fn luminance(colour: SrgbColour) -> f64 {
    relative_luminance(to_linear(colour))
}

// CIE 1931 XYZ for linear sRGB under the D65 white point.
const D65: [f64; 3] = [0.950_47, 1.0, 1.088_83];
const LAB_EPSILON: f64 = 216.0 / 24_389.0;
const LAB_KAPPA: f64 = 24_389.0 / 27.0;

fn lab_f(t: f64) -> f64 {
    if t > LAB_EPSILON {
        t.cbrt()
    } else {
        (LAB_KAPPA * t + 16.0) / 116.0
    }
}

fn lab_f_inverse(f: f64) -> f64 {
    let cube = f * f * f;
    if cube > LAB_EPSILON {
        cube
    } else {
        (116.0 * f - 16.0) / LAB_KAPPA
    }
}

/// CIE Lab of `colour` as `(f(Y/Yn), a*, b*)`; L* is `116 f − 16`.
fn cie_lab(colour: SrgbColour) -> (f64, f64, f64) {
    let linear = to_linear(colour);
    let (r, g, b) = (linear.red, linear.green, linear.blue);
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = 0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b;
    let (fx, fy, fz) = (lab_f(x / D65[0]), lab_f(y / D65[1]), lab_f(z / D65[2]));
    (fy, 500.0 * (fx - fy), 200.0 * (fy - fz))
}

/// Caps `colour`'s CIE LCh chroma (0..128 scale) at `max`, keeping its CIE
/// lightness and hue, and clamps the result back into sRGB.
fn cap_cie_chroma(colour: SrgbColour, max: f64) -> SrgbColour {
    let (fy, a, b_star) = cie_lab(colour);
    let chroma = a.hypot(b_star);
    if chroma <= max {
        return colour;
    }
    let scale = max / chroma;
    let fx = fy + a * scale / 500.0;
    let fz = fy - b_star * scale / 200.0;
    let (x, y, z) = (
        lab_f_inverse(fx) * D65[0],
        lab_f_inverse(fy) * D65[1],
        lab_f_inverse(fz) * D65[2],
    );
    let rgb = [
        3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z,
        -0.969_266_0 * x + 1.876_010_8 * y + 0.041_556_0 * z,
        0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z,
    ];
    from_linear(rgb, colour.alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(byte: u8) -> SrgbColour {
        let v = f64::from(byte) / 255.0;
        SrgbColour {
            red: v,
            green: v,
            blue: v,
            alpha: 1.0,
        }
    }

    #[test]
    fn idx_reindexes_ramp_steps_up_by_one_from_fifty() {
        let ramp = ramp(grey(0x1B), 100);
        for (i, colour) in ramp.iter().enumerate() {
            let expected = if i < 50 { i } else { (i + 1).min(99) };
            assert_eq!(idx(*colour), expected, "step {i}");
        }
    }

    #[test]
    fn ramp_keeps_the_seed_alpha_and_spans_black_to_white() {
        let seed = with_alpha(grey(0x80), 0.4);
        let ramp = ramp(seed, 11);
        assert_eq!(ramp.len(), 11);
        assert!(ramp.iter().all(|step| step.alpha == 0.4));
        assert_eq!(ramp[0].to_srgba8()[..3], [0, 0, 0]);
        assert_eq!(ramp[10].to_srgba8()[..3], [255, 255, 255]);
    }

    #[test]
    fn ramp_of_a_saturated_seed_stays_in_gamut() {
        let seed = SrgbColour {
            red: 0.0,
            green: 0.0,
            blue: 1.0,
            alpha: 1.0,
        };
        for step in ramp(seed, 100) {
            for channel in [step.red, step.green, step.blue] {
                assert!((0.0..=1.0).contains(&channel));
            }
        }
    }

    #[test]
    fn surface_steps_lighter_below_91_even_in_light_mode() {
        let ramp = ramp(grey(0x80), 100);
        assert_eq!(surface(88, 8, &ramp, false, BLACK), ramp[96]);
        assert_eq!(surface(94, 6, &ramp, false, BLACK), ramp[88]);
        assert_eq!(surface(95, 8, &ramp, true, BLACK), BLACK);
        assert_eq!(surface(3, 5, &ramp, false, WHITE), ramp[8]);
    }

    #[test]
    fn small_direction_bands() {
        let ramp = ramp(BLACK, 100);
        assert_eq!(small(40, &ramp, WHITE), ramp[45]);
        assert_eq!(small(45, &ramp, WHITE), ramp[40]);
        assert_eq!(small(52, &ramp, WHITE), ramp[57]);
        assert_eq!(small(65, &ramp, WHITE), ramp[60]);
        assert_eq!(small(97, &ramp, WHITE), ramp[92]);
    }

    #[test]
    fn small_caps_a_tinted_step_at_cie_chroma_3_84() {
        let tint = SrgbColour {
            red: 0.9,
            green: 0.2,
            blue: 0.2,
            alpha: 1.0,
        };
        let ramp = ramp(tint, 100);
        let chroma = |colour| {
            let (_, a, b) = cie_lab(colour);
            f64::hypot(a, b)
        };
        assert!(chroma(ramp[27]) > SMALL_CHROMA_CAP);
        let capped = small(22, &ramp, BLACK);
        let capped_chroma = chroma(capped);
        assert!(
            (capped_chroma - SMALL_CHROMA_CAP).abs() < 1e-3,
            "capped chroma {capped_chroma}"
        );
        // Lightness is kept.
        assert!((cie_lab(capped).0 - cie_lab(ramp[27]).0).abs() < 1e-5);
        // A neutral step is untouched.
        let neutral = ramp_of_grey();
        assert_eq!(small(22, &neutral, BLACK), neutral[27]);
    }

    #[test]
    fn text_alpha_pushes_further_and_falls_back_to_the_end() {
        let ramp = ramp(grey(0x1B), 100);
        assert_eq!(text(22, &ramp, WHITE, None, 1.0), ramp[92]);
        assert_eq!(text(30, &ramp, WHITE, None, 1.0), ramp[80]);
        assert_eq!(text(27, &ramp, WHITE, None, 0.76154), ramp[99]);
        assert_eq!(text(88, &ramp, BLACK, None, 1.0), ramp[18]);
        assert_eq!(text(94, &ramp, BLACK, None, 0.76154), ramp[0]);
        assert_eq!(text(60, &ramp, BLACK, None, 1.0), ramp[10]);
        let tint = ramp_of_tint();
        assert_eq!(text(22, &ramp, WHITE, Some(&tint), 1.0), tint[92]);
    }

    fn ramp_of_grey() -> Vec<SrgbColour> {
        ramp(BLACK, 100)
    }

    fn ramp_of_tint() -> Vec<SrgbColour> {
        ramp(
            SrgbColour {
                red: 0.3,
                green: 0.5,
                blue: 0.9,
                alpha: 1.0,
            },
            100,
        )
    }

    #[test]
    fn over_is_the_reference_operator_not_the_textbook_one() {
        let hover = with_alpha(grey(0x63), 0.2);
        let button = with_alpha(grey(0x9E), 0.25);
        // The reference: #63636366. The textbook operator gives #818166.
        assert_eq!(over(hover, button).to_srgba8(), [0x63, 0x63, 0x63, 0x66]);
        // Opaque under: an ordinary lerp.
        let lerp = over(with_alpha(WHITE, 0.1), grey(0x2E));
        assert_eq!(lerp.to_srgba8(), [0x43, 0x43, 0x43, 0xFF]);
        assert_eq!(over(TRANSPARENT, TRANSPARENT), TRANSPARENT);
    }

    #[test]
    fn contrast_matches_the_worked_numbers() {
        let primary = grey(0x27);
        assert!((contrast(primary, grey(0x7F)) - 3.73).abs() < 0.01);
        assert!((contrast(primary, grey(0xB6)) - 7.4).abs() < 0.05);
        assert!(luminance(grey(0x27)) < luminance(grey(0x28)));
    }
}
