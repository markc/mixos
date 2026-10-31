// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Every row of the theme-model spec's §4.5 worked numbers, plus the
//! fresh 1.10.0 dark background write.

use super::primitives::{contrast, idx, lightness, ramp};
use super::*;
use crate::cosmic_tables::{DARK_PALETTE, LIGHT_PALETTE, PaletteKind, glass_alpha_map};

/// `#RRGGBB` for an opaque colour, `#RRGGBBAA` otherwise: the spec's form.
fn hex(colour: SrgbColour) -> String {
    let full = colour.to_hex();
    match full.strip_suffix("FF") {
        Some(opaque) => opaque.to_owned(),
        None => full,
    }
}

fn colour(text: &str) -> SrgbColour {
    SrgbColour::from_hex(text).unwrap()
}

fn dark_default() -> DerivedTheme {
    derive(
        &Seeds::default(),
        Mode::Dark,
        Contrast::Normal,
        &FrostedGlass::default(),
    )
}

fn light_default() -> DerivedTheme {
    derive(
        &Seeds::default(),
        Mode::Light,
        Contrast::Normal,
        &FrostedGlass::default(),
    )
}

fn grey_accent() -> DerivedTheme {
    let seeds = Seeds {
        accent: Some(colour("#7F7F7F")),
        ..Seeds::default()
    };
    derive(
        &seeds,
        Mode::Dark,
        Contrast::Normal,
        &FrostedGlass::default(),
    )
}

#[test]
fn hex_round_trip_and_refusals() {
    assert_eq!(colour("#1b1b1bc2").to_hex(), "#1B1B1BC2");
    assert_eq!(colour("#63D0DF").to_hex(), "#63D0DFFF");
    for bad in ["", "1B1B1B", "#1B1B1", "#1B1B1BC", "#GGGGGG", "#1B1B1B1B1B"] {
        assert_eq!(SrgbColour::from_hex(bad), None, "{bad}");
    }
}

#[test]
fn fresh_1_10_dark_background_component() {
    let c = dark_default().background.component;
    assert_eq!(hex(c.base), "#2E2E2E");
    assert_eq!(hex(c.hover), "#434343");
    assert_eq!(hex(c.pressed), "#585858");
    assert_eq!(hex(c.selected), "#434343");
    assert_eq!(hex(c.divider), "#C0C0C033");
    assert_eq!(hex(c.on), "#C0C0C0");
    assert_eq!(hex(c.disabled), "#2E2E2E80");
}

#[test]
fn dark_default_indices_and_controls() {
    assert_eq!(idx(DARK_PALETTE.gray_1), 22);
    let controls = ramp::<11>(DARK_PALETTE.neutral[2]);
    for (k, step) in controls.iter().enumerate() {
        assert_eq!(hex(*step), hex(DARK_PALETTE.neutral[k]), "controls[{k}]");
    }
}

#[test]
fn dark_default_surfaces() {
    let t = dark_default();
    assert_eq!(hex(t.background.component.base), "#2E2E2E");
    assert_eq!(hex(t.primary.base), "#272727");
    assert_eq!(hex(t.secondary.base), "#343434");
    assert_eq!(hex(t.primary.component.base), "#363636");
    assert_eq!(hex(t.secondary.component.base), "#3B3B3B");
}

#[test]
fn dark_default_texts() {
    let t = dark_default();
    assert_eq!(hex(t.background.on), "#E7E7E7");
    assert_eq!(hex(t.background.component.on), "#C0C0C0");
    assert_eq!(hex(t.secondary.on), "#C7C7C7");
    // Container alpha 0.76154 pushes 24 extra steps: 27 + 94 and 27 + 74
    // both leave the ramp, so the ramp's end.
    assert_eq!(hex(t.primary.on), "#FFFFFF");
}

#[test]
fn dark_default_overlays_and_buttons() {
    let t = dark_default();
    assert_eq!(hex(t.background.divider), "#444444");
    assert_eq!(hex(t.background.component.hover), "#434343");
    assert_eq!(hex(t.background.component.pressed), "#585858");
    assert_eq!(hex(t.button.base), "#9E9E9E40");
    assert_eq!(hex(t.button.hover), "#63636366");
    assert_eq!(hex(t.button.pressed), "#2B2B2B9F");
    assert_eq!(hex(t.accent.hover), "#63BAC6");
    assert_eq!(hex(t.accent.on), "#000000");
    assert_eq!(t.accent_text, None);
    assert!(contrast(t.primary.base, t.accent.base) >= 4.0);
    assert!((contrast(t.primary.base, t.accent.base) - 8.3).abs() < 0.1);
    assert_eq!(idx(t.accent.base), 80);
    assert_eq!(hex(t.accent_button.on), "#030303");
    assert_eq!(hex(t.warning_button.divider), "#FFFFFF");
    assert_eq!(hex(t.background.small_widget), "#27272740");
    assert_eq!(hex(t.shade), "#00000052");
}

#[test]
fn dark_default_transparent_background() {
    let t = dark_default();
    assert_eq!(hex(t.transparent_background.base), "#1B1B1BC2");
    assert_eq!(hex(t.transparent_background.component.hover), "#424242C8");
}

#[test]
fn dark_grey_accent_takes_the_contrast_path() {
    let t = grey_accent();
    assert!((contrast(t.primary.base, colour("#7F7F7F")) - 3.73).abs() < 0.01);
    let accent_text = t.accent_text.expect("grey accent is under 4:1");
    assert_eq!(hex(accent_text), "#B6B6B6");
    assert!((accent_text.red - 0.715_587_4).abs() < 5e-4);
    assert!((contrast(t.primary.base, accent_text) - 7.4).abs() < 0.05);
    assert_eq!(hex(t.text_button.on), "#B6B6B6");
    assert_eq!(hex(t.link_button.on), "#B6B6B6");
    assert_eq!(idx(colour("#7F7F7F")), 60);
    assert_eq!(hex(t.accent_button.on), "#030303");
}

#[test]
fn light_default_surfaces() {
    let t = light_default();
    assert_eq!(idx(LIGHT_PALETTE.gray_1), 88);
    assert_eq!(hex(t.background.component.base), "#F5F5F5");
    assert_eq!(hex(t.primary.base), "#EBEBEB");
    assert_eq!(hex(t.secondary.base), "#FCFCFC");
    // #EBEBEB re-indexes to 94, not 93: 94 ≥ 91 steps down 6 to 88.
    assert_eq!(idx(t.primary.base), 94);
    assert_eq!(hex(t.primary.component.base), "#DADADA");
    assert_eq!(idx(t.secondary.base), 99);
    assert_eq!(hex(t.secondary.component.base), "#F5F5F5");
}

#[test]
fn light_default_texts() {
    let t = light_default();
    assert_eq!(hex(t.background.on), "#121212");
    assert_eq!(hex(t.background.component.on), "#272727");
}

#[test]
fn light_primary_texts_take_the_container_alpha() {
    let t = light_default();
    // Step 12: 24 extra steps, 94 − 94 = 0. The pre-1.10 shipped #202020
    // came from a generation with container alpha 1.
    assert_eq!(hex(t.primary.on), "#000000");
    // Idx(#DADADA) = 89; 89 − 94 < 0, so 89 − 74 = 15.
    assert_eq!(idx(t.primary.component.base), 89);
    assert_eq!(hex(t.primary.component.on), "#0B0B0B");
}

#[test]
fn light_default_overlays_and_buttons() {
    let t = light_default();
    assert_eq!(hex(t.background.divider), "#B0B0B0");
    assert_eq!(hex(t.button.base), "#2E2E2E40");
    assert_eq!(hex(t.button.pressed), "#6868689F");
    assert_eq!(hex(t.accent.on), "#FFFFFF");
    assert_eq!(t.accent_text, None);
    assert!((contrast(t.background.base, t.accent.base) - 6.2).abs() < 0.1);
    assert_eq!(idx(t.accent.base), 40);
    assert_eq!(hex(t.accent_button.on), "#E1E1E1");
    assert_eq!(hex(t.accent_button.divider), "#DEDEDE");
    assert_eq!(hex(t.background.component.hover), "#F6F6F6");
    // Normative (ruling R8): step 9's pressed overlay is chosen from bgIdx
    // 88 < 91, so black at 0.2.
    assert_eq!(hex(t.background.component.pressed), "#C4C4C4");
    assert_eq!(hex(t.transparent_background.component.hover), "#EFEFEFC8");
    assert_eq!(hex(t.shade), "#00000014");
}

#[test]
fn high_contrast_changes_only_alphas_and_the_accent_step() {
    let normal = grey_accent();
    let seeds = Seeds {
        accent: Some(colour("#7F7F7F")),
        ..Seeds::default()
    };
    let hc = derive(&seeds, Mode::Dark, Contrast::High, &FrostedGlass::default());
    assert!(hc.is_high_contrast && !normal.is_high_contrast);
    assert_eq!(
        hc.background.component.base,
        normal.background.component.base
    );
    assert_eq!(hc.background.component.divider.alpha, 0.65);
    let on = hc.background.on;
    assert_eq!(
        hc.background.divider,
        over(with_alpha(on, 0.5), hc.background.base)
    );
    // The accent-text search steps 60: 27 + 60 = 87.
    let expected = ramp::<100>(colour("#7F7F7F"))[87];
    assert_eq!(hc.accent_text, Some(expected));
}

#[test]
fn a_frosted_flag_makes_texts_follow_the_container_alpha() {
    let frosted = FrostedGlass {
        panel: true,
        ..FrostedGlass::default()
    };
    assert_eq!(frosted.actual_alpha(), 0.76154);
    assert_eq!(FrostedGlass::default().actual_alpha(), 1.0);
    let maximised = FrostedGlass {
        maximized_apps: true,
        ..FrostedGlass::default()
    };
    assert_eq!(maximised.actual_alpha(), 1.0);
    let t = derive(&Seeds::default(), Mode::Dark, Contrast::Normal, &frosted);
    // 22 + 94 leaves the ramp, so 22 + 74 = 96 instead of 92 (#E7E7E7).
    assert_eq!(hex(t.background.on), "#F5F5F5");
}

#[test]
fn light_accent_text_falls_back_to_black() {
    // A pale accent on the light background: the 50-step walk from bgIdx 88
    // leaves the ramp, so the black fallback.
    let seeds = Seeds {
        accent: Some(colour("#C8C8C8")),
        ..Seeds::default()
    };
    let t = derive(
        &seeds,
        Mode::Light,
        Contrast::Normal,
        &FrostedGlass::default(),
    );
    assert_eq!(t.accent_text.map(hex), Some("#000000".to_owned()));
}

#[test]
fn link_button_on_disabled_is_its_on_at_half_over_its_base() {
    let t = dark_default();
    let on = t.link_button.on;
    assert_eq!(t.link_button.base, TRANSPARENT);
    // Over a transparent base the reference operator halves the channels.
    let expected = SrgbColour {
        red: on.red * 0.5,
        green: on.green * 0.5,
        blue: on.blue * 0.5,
        alpha: 0.5,
    };
    assert_eq!(t.link_button.on_disabled, expected);
}

#[test]
fn copied_through_inputs() {
    let tint = colour("#336699");
    let seeds = Seeds {
        neutral_tint: Some(tint),
        ..Seeds::default()
    };
    let t = derive(
        &seeds,
        Mode::Light,
        Contrast::Normal,
        &FrostedGlass::default(),
    );
    assert_eq!(t.control_tint, Some(tint));
    assert_eq!(t.text_tint, None);
    assert!(!t.is_dark);
    assert_eq!(t.palette.kind, PaletteKind::Light);
    assert_eq!(t.frosted, FrostedGlass::default());
}

fn dark_with(seeds: &Seeds, frosted: &FrostedGlass) -> DerivedTheme {
    derive(seeds, Mode::Dark, Contrast::Normal, frosted)
}

#[test]
fn transparent_primary_and_secondary_under_the_default_map() {
    let t = dark_default();
    // 0.76154 + 0.3 and + 0.6 both clamp to 1: opaque, as the primary.
    assert_eq!(hex(t.transparent_primary.base), "#272727");
    assert_eq!(
        t.transparent_primary.component.base,
        t.primary.component.base
    );
    assert_eq!(hex(t.transparent_secondary.base), "#343434");
    assert_eq!(hex(t.transparent_secondary.component.base), "#3B3B3B");
    // No hover or pressed overlay on the transparent primary.
    assert_eq!(
        t.transparent_primary.component.hover,
        t.transparent_primary.component.base
    );
}

#[test]
fn transparent_primary_and_secondary_at_the_lowest_glass() {
    let frosted = FrostedGlass {
        alpha_map: glass_alpha_map(0.0),
        ..FrostedGlass::default()
    };
    // Container alpha 0.76154 − 0.60 = 0.16154; no frosted flag, so texts
    // stay opaque-indexed except the primary's.
    let t = dark_with(&Seeds::default(), &frosted);
    assert_eq!(hex(t.transparent_background.base), "#1B1B1B29");
    assert_eq!(hex(t.transparent_primary.base), "#27272776");
    assert_eq!(hex(t.transparent_secondary.base), "#343434C2");
    assert_eq!(hex(t.transparent_secondary.component.base), "#3B3B3BC2");
    // Transparent primary texts use bgRamp: opaque, 27 + 70 = 97.
    let bg_ramp = ramp::<100>(DARK_PALETTE.gray_1);
    assert_eq!(t.transparent_primary.on, bg_ramp[97]);
    // Transparent secondary texts use its own ramp, so carry its alpha:
    // 32 + 70 ≥ 100 → 82.
    assert_eq!(hex(t.transparent_secondary.on), "#C7C7C7C2");
}

#[test]
fn text_tint_replaces_every_text_ramp() {
    let tint = colour("#3366CC");
    let seeds = Seeds {
        text_tint: Some(tint),
        ..Seeds::default()
    };
    let t = dark_with(&seeds, &FrostedGlass::default());
    let tinted = ramp::<100>(tint);
    assert_eq!(t.background.on, tinted[92]);
    assert_eq!(t.background.component.on, tinted[80]);
    assert_eq!(t.secondary.on, tinted[82]);
    assert_eq!(t.accent_button.on, tinted[10]);
    assert_eq!(t.text_tint, Some(tint));
}

#[test]
fn custom_container_backgrounds_get_their_own_ramps_and_keep_alpha() {
    let primary = colour("#404040");
    let secondary = colour("#50505080");
    let seeds = Seeds {
        primary_container_bg: Some(primary),
        secondary_container_bg: Some(secondary),
        ..Seeds::default()
    };
    let t = dark_with(&seeds, &FrostedGlass::default());
    assert_eq!(t.primary.base, primary);
    let p_idx = idx(primary);
    assert_eq!(t.primary.component.base, ramp::<100>(primary)[p_idx + 6]);
    assert_eq!(t.secondary.base, secondary);
    let s_idx = idx(secondary);
    assert_eq!(
        t.secondary.component.base,
        ramp::<100>(secondary)[s_idx + 3]
    );
    assert_eq!(t.secondary.component.base.alpha, secondary.alpha);
    // The transparent primary starts from the custom base too.
    assert_eq!(t.transparent_primary.base, with_alpha(primary, 1.0));
}

#[test]
fn opaque_seeds_are_forced_opaque_and_backgrounds_keep_alpha() {
    let half = |text: &str| with_alpha(colour(text), 0.5);
    let seeds = Seeds {
        accent: Some(half("#7F7F7F")),
        neutral_tint: Some(half("#336699")),
        text_tint: Some(half("#3366CC")),
        success: Some(half("#00FF00")),
        warning: Some(half("#FFFF00")),
        destructive: Some(half("#FF0000")),
        window_hint: Some(half("#FF00FF")),
        bg_color: Some(half("#1B1B1B")),
        ..Seeds::default()
    };
    let t = dark_with(&seeds, &FrostedGlass::default());
    for component in [t.accent, t.success, t.warning, t.destructive] {
        assert_eq!(component.base.alpha, 1.0);
    }
    assert_eq!(t.accent_button.on.alpha, 1.0);
    assert_eq!(t.secondary.on.alpha, 1.0);
    assert_eq!(t.control_tint.map(|c| c.alpha), Some(1.0));
    assert_eq!(t.window_hint, Some(colour("#FF00FF")));
    assert_eq!(t.background.base.alpha, 0.5);
}

#[test]
fn high_contrast_light_dividers() {
    let t = derive(
        &Seeds::default(),
        Mode::Light,
        Contrast::High,
        &FrostedGlass::default(),
    );
    assert_eq!(t.palette.kind, PaletteKind::HighContrastLight);
    assert!(t.is_high_contrast && !t.is_dark);
    for component in [t.background.component, t.primary.component, t.button] {
        assert_eq!(component.divider.alpha, 0.65);
    }
    for container in [t.background, t.primary, t.secondary] {
        assert_eq!(
            container.divider,
            over(with_alpha(container.on, 0.5), container.base)
        );
    }
}

#[test]
fn transparent_base_components_take_the_overlay_itself() {
    let t = dark_default();
    // controls[5] at 0.2 and controls[2] at 0.5, not composited.
    assert_eq!(hex(t.icon_button.hover), "#63636333");
    assert_eq!(hex(t.icon_button.pressed), "#16161680");
    assert_eq!(t.list_button.hover, TRANSPARENT);
    assert_eq!(hex(t.list_button.pressed), "#16161680");
}

#[test]
fn window_hint_is_copied_through() {
    let hint = colour("#FF8800");
    let seeds = Seeds {
        window_hint: Some(hint),
        ..Seeds::default()
    };
    assert_eq!(
        dark_with(&seeds, &FrostedGlass::default()).window_hint,
        Some(hint)
    );
    assert_eq!(dark_default().window_hint, None);
}

#[test]
fn consumer_accessors() {
    let t = dark_default();
    for k in 0..11 {
        assert_eq!(t.control(k), Some(DARK_PALETTE.neutral[k]));
    }
    assert_eq!(t.control(11), None);
    assert_eq!(t.accent_text_colour(), t.accent.base);
    assert_eq!(hex(grey_accent().accent_text_colour()), "#B6B6B6");
    assert_eq!(hex(t.small_widget_divider()), "#DEDEDE33");
    assert_eq!(t.window_header_background(), t.background.base);

    let tint = colour("#336699");
    let seeds = Seeds {
        neutral_tint: Some(tint),
        ..Seeds::default()
    };
    let tinted = dark_with(&seeds, &FrostedGlass::default());
    let control = tinted.control(5).unwrap();
    assert_ne!(control, DARK_PALETTE.neutral[5]);
    assert!((lightness(control) - lightness(DARK_PALETTE.neutral[5])).abs() < 1e-3);
    // Blue-tinted: blue is the strongest channel.
    assert!(control.blue > control.red);
}
