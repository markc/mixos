// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Geometry after the compile: the roundness and density axes laid over a
//! resolved style (the theming ADR's R5 step 2a). One function, [`apply`],
//! shared by the toolkit and compd's decor, so both draw the same lengths.
//!
//! **Roundness** (A4) sets the six COSMIC radius tokens from [`RADIUS_MAP`]
//! and overrides the legacy radii from them ([`round`]); `square`
//! is 0 on all six, on every desk (F2), where the reference's own Square is
//! 2. The style keeps every other trait.
//!
//! **Density** (A5, F3) has two mechanisms under one step. The legacy
//! layout tokens keep the toolkit's multiplier model with the steps 0.85,
//! 1.0 and 1.2 ([`legacy_lengths`]), so `comfortable` is today's look pixel
//! for pixel. The new `spacing_*` and `header_padding_*` tokens take the
//! reference's table per step, for COSMIC-shaped surfaces. The multiplier is
//! a migration mechanism: no consumer reads the density as a number.
//!
//! Text scale is not geometry: it is accessibility (R5 step 7) and stays
//! with the toolkit.

pub use crate::cosmic_tables::{Density, Roundness};
use crate::family::style::ResolvedStyle;

/// The two geometry axes. The default is `round`, `comfortable`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Geometry {
    pub roundness: Roundness,
    pub density: Density,
}

impl Roundness {
    /// The settings names, `appearance.roundness`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Round => "round",
            Self::SlightlyRound => "slightly_round",
            Self::Square => "square",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.name() == name)
    }

    /// The six radius tokens MixOS draws for this preset, by [`RADIUS_MAP`]:
    /// `[radius_0, radius_xs, radius_s, radius_m, radius_l, radius_xl]`.
    pub const fn radii(self) -> [u8; 6] {
        let column = self as usize;
        let mut out = [0; 6];
        let mut i = 0;
        while i < 6 {
            out[i] = RADIUS_MAP[i].1[column];
            i += 1;
        }
        out
    }
}

impl Density {
    /// The settings names, `appearance.density`. COSMIC stores
    /// `comfortable` as `Standard`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Standard => "comfortable",
            Self::Spacious => "spacious",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.name() == name)
    }

    /// The legacy tokens' multiplier: today's named steps. Transitional:
    /// settings' `CommonUi.density` carries it to the toolkit's
    /// `ui_scale::tokens` until that moves onto [`apply`] (key space v2, S8a).
    pub const fn multiplier(self) -> f64 {
        match self {
            Self::Compact => 0.85,
            Self::Standard => 1.0,
            Self::Spacious => 1.2,
        }
    }
}

/// The A4 radius table: each COSMIC radius token per preset, in
/// [`Roundness::ALL`] order (round, slightly round, square).
pub const RADIUS_MAP: [(&str, [u8; 3]); 6] = [
    ("radius_0", [0, 0, 0]),
    ("radius_xs", [4, 2, 0]),
    ("radius_s", [8, 8, 0]),
    ("radius_m", [16, 8, 0]),
    ("radius_l", [32, 8, 0]),
    ("radius_xl", [160, 8, 0]),
];

/// `style` with both axes applied. Apply it once, to the compiled style:
/// the density multiplies.
pub fn apply(style: ResolvedStyle, geometry: &Geometry) -> ResolvedStyle {
    spacing(
        legacy_lengths(round(style, geometry.roundness), geometry.density),
        geometry.density,
    )
}

/// The roundness overlay alone. The legacy radii take the COSMIC token of
/// the widgets they draw (spec 2 §5.2): `radius_sm` ← `radius_xs`,
/// `radius` ← `radius_s`, `radius_lg` ← `radius_m` (cards), `field_radius`
/// and `menu_highlight_radius` ← `radius_s` (text input, menu items), and
/// `radius_pill` ← `radius_xl` (pill buttons).
pub fn round(mut style: ResolvedStyle, roundness: Roundness) -> ResolvedStyle {
    let [r0, xs, s, m, l, xl] = roundness.radii();
    style.radius_0 = r0;
    style.radius_xs = xs;
    style.radius_s = s;
    style.radius_m = m;
    style.radius_l = l;
    style.radius_xl = xl;
    style.radius_sm = xs;
    style.radius = s;
    style.radius_lg = m;
    style.field_radius = s;
    style.menu_highlight_radius = s;
    style.radius_pill = xl;
    style
}

/// The density's new tokens alone: the reference's spacing table and the
/// client-decorated header padding for `density`.
pub fn spacing(mut style: ResolvedStyle, density: Density) -> ResolvedStyle {
    let table = density.spacing();
    for (token, value) in [
        (&mut style.spacing_none, table.space_none),
        (&mut style.spacing_xxxs, table.space_xxxs),
        (&mut style.spacing_xxs, table.space_xxs),
        (&mut style.spacing_xs, table.space_xs),
        (&mut style.spacing_s, table.space_s),
        (&mut style.spacing_m, table.space_m),
        (&mut style.spacing_l, table.space_l),
        (&mut style.spacing_xl, table.space_xl),
        (&mut style.spacing_xxl, table.space_xxl),
        (&mut style.spacing_xxxl, table.space_xxxl),
    ] {
        *token = f64::from(value);
    }
    let [top, right, bottom, left] = density.header_padding(false);
    style.header_padding_top = f64::from(top);
    style.header_padding_right = f64::from(right);
    style.header_padding_bottom = f64::from(bottom);
    style.header_padding_left = f64::from(left);
    style
}

/// The legacy layout tokens at `density`'s multiplier, exactly as the
/// toolkit's `ui_scale` scales them today (the same tokens, the same
/// margin rounding), so `comfortable` changes nothing.
pub fn legacy_lengths(mut style: ResolvedStyle, density: Density) -> ResolvedStyle {
    let density = density.multiplier();
    for length in [
        &mut style.push_height,
        &mut style.dock_margin,
        &mut style.options_bar,
        &mut style.status_bar,
        &mut style.tool_bar,
        &mut style.tool_button,
        &mut style.tool_margin,
        &mut style.rail,
        &mut style.rail_button,
        &mut style.title_bar_height,
        &mut style.menu_title_height,
        &mut style.menu_row_height,
        &mut style.menu_row_padding_x,
        &mut style.menu_row_padding_y,
        &mut style.bar_row_gap,
        &mut style.bars_bar_inset,
        &mut style.button_padding_x,
        &mut style.button_padding_y,
        &mut style.button_push_padding,
        &mut style.caption_gap,
        &mut style.caption_width,
        &mut style.dialog_above_buttons,
        &mut style.dialog_above_rule,
        &mut style.dialog_below_rule,
        &mut style.dialog_button_gap,
        &mut style.divider_slot,
        &mut style.edge_gap,
        &mut style.field_height,
        &mut style.field_padding_x,
        &mut style.field_padding_y,
        &mut style.icon_button,
        &mut style.interact,
        &mut style.item_spacing_x,
        &mut style.item_spacing_y,
        &mut style.link_gap,
        &mut style.mark_gap,
        &mut style.md_code_padding_x,
        &mut style.md_code_padding_y,
        &mut style.md_gap,
        &mut style.md_gap_after_heading,
        &mut style.md_gap_before_heading,
        &mut style.md_gutter,
        &mut style.md_item_gap,
        &mut style.md_quote_indent,
        &mut style.menu_margin,
        &mut style.menu_separator_height,
        &mut style.menu_title_padding_x,
        &mut style.menu_title_padding_y,
        &mut style.panel_card_after,
        &mut style.panel_card_margin_bottom,
        &mut style.panel_card_margin_collapsed,
        &mut style.panel_card_margin_left,
        &mut style.panel_card_margin_right,
        &mut style.panel_card_margin_top,
        &mut style.panel_header,
        &mut style.panel_header_after,
        &mut style.panel_ellipsis_gap,
        &mut style.panel_ellipsis,
        &mut style.panel_pill_gap,
        &mut style.panel_pill_pad,
        &mut style.panel_strip,
        &mut style.search_lead,
        &mut style.shortcut_gap,
        &mut style.slider_height,
        &mut style.slider_row_gap,
        &mut style.space_xs,
        &mut style.space_sm,
        &mut style.space_md,
        &mut style.space_lg,
        &mut style.space_xl,
        &mut style.submenu_gap,
        &mut style.tabs_pro_pad,
        &mut style.tabs_pro_title_left,
        &mut style.tabs_marker_gap,
        &mut style.tabs_strip,
        &mut style.tabs_studio_gap,
        &mut style.tabs_studio_margin_bottom,
        &mut style.tabs_studio_margin_left,
        &mut style.tabs_studio_margin_right,
        &mut style.tabs_studio_margin_top,
        &mut style.tabs_studio_meta_gap,
        &mut style.tabs_studio_pad,
        &mut style.title_bar_margin,
        &mut style.title_gap,
        &mut style.toggle_label_gap,
        &mut style.value_unit_gap,
        &mut style.window_margin,
        &mut style.bar_padding_y,
    ] {
        *length *= density;
    }
    // egui margins are whole signed bytes: round and saturate them as the
    // toolkit does, so a scaled margin cannot wrap.
    if density != 1.0 {
        for margin in [
            &mut style.menu_margin,
            &mut style.window_margin,
            &mut style.panel_card_margin_bottom,
            &mut style.panel_card_margin_collapsed,
            &mut style.panel_card_margin_left,
            &mut style.panel_card_margin_right,
            &mut style.panel_card_margin_top,
            &mut style.tabs_studio_margin_bottom,
            &mut style.tabs_studio_margin_left,
            &mut style.tabs_studio_margin_right,
            &mut style.tabs_studio_margin_top,
        ] {
            *margin = margin.round().clamp(0.0, f64::from(i8::MAX));
        }
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cosmic_tables::Roundness as R;
    use crate::{
        DesignContext, EMBEDDED_DEFAULT_SOURCE, Style, compile_design, parse_design_source,
    };

    fn styles() -> Vec<(Style, ResolvedStyle)> {
        let document = parse_design_source(
            crate::SourceIdentity::new("embedded"),
            EMBEDDED_DEFAULT_SOURCE,
        )
        .unwrap();
        Style::ALL
            .into_iter()
            .map(|style| {
                let context = DesignContext {
                    style: Some(style),
                    ..DesignContext::default()
                };
                let design = match compile_design(&document, context) {
                    crate::DesignCompileResult::Success(success) => success.candidate,
                    crate::DesignCompileResult::Fatal(failure) => {
                        panic!("{:?}", failure.diagnostics)
                    }
                };
                (style, design.dictionary().style.unwrap())
            })
            .collect()
    }

    #[test]
    fn comfortable_legacy_lengths_are_the_identity() {
        for (_, style) in styles() {
            assert_eq!(legacy_lengths(style, Density::Standard), style);
        }
    }

    #[test]
    fn steps_scale_the_toolkits_tokens_and_round_margins() {
        for (_, style) in styles() {
            let compact = legacy_lengths(style, Density::Compact);
            let spacious = legacy_lengths(style, Density::Spacious);
            assert_eq!(compact.field_height, style.field_height * 0.85);
            assert_eq!(spacious.title_bar_height, style.title_bar_height * 1.2);
            assert_eq!(spacious.window_margin, (style.window_margin * 1.2).round());
            // Untouched by density: text sizes, radii, the new tokens.
            assert_eq!(compact.body_size, style.body_size);
            assert_eq!(compact.radius, style.radius);
            assert_eq!(compact.spacing_m, style.spacing_m);
        }
    }

    #[test]
    fn the_radius_table_is_a4_with_square_zero() {
        assert_eq!(R::Round.radii(), [0, 4, 8, 16, 32, 160]);
        assert_eq!(R::SlightlyRound.radii(), [0, 2, 8, 8, 8, 8]);
        assert_eq!(R::Square.radii(), [0; 6]);
        // Round and slightly round are the reference's presets; square is
        // MixOS's 0 where the reference ships 2.
        for preset in [R::Round, R::SlightlyRound] {
            let reference = preset.corner_radii();
            let ours = preset.radii().map(f64::from);
            assert_eq!(
                [
                    reference.radius_0[0],
                    reference.radius_xs[0],
                    reference.radius_s[0],
                    reference.radius_m[0],
                    reference.radius_l[0],
                    reference.radius_xl[0],
                ],
                ours
            );
        }
        // Reading the written radii back gives the preset again.
        for preset in R::ALL {
            let [r0, xs, s, m, l, xl] = preset.radii().map(|r| [f64::from(r); 4]);
            let radii = crate::cosmic_tables::RadiusTokens {
                radius_0: r0,
                radius_xs: xs,
                radius_s: s,
                radius_m: m,
                radius_l: l,
                radius_xl: xl,
            };
            assert_eq!(R::from_radii(&radii), preset);
        }
    }

    #[test]
    fn roundness_overrides_the_legacy_radii_and_keeps_the_rest() {
        for (_, style) in styles() {
            for preset in R::ALL {
                let rounded = round(style, preset);
                let [_, xs, s, m, _, xl] = preset.radii();
                assert_eq!(
                    (
                        rounded.radius_sm,
                        rounded.radius,
                        rounded.radius_lg,
                        rounded.field_radius,
                        rounded.menu_highlight_radius,
                        rounded.radius_pill
                    ),
                    (xs, s, m, s, s, xl)
                );
                assert_eq!(rounded.square_radius, style.square_radius);
                assert_eq!(rounded.toggle_box_radius, style.toggle_box_radius);
                assert_eq!(rounded.widgets, style.widgets);
            }
        }
    }

    #[test]
    fn each_style_authors_its_nearest_preset() {
        // F2: studio, adwaita round; breeze, pro slightly round; classic,
        // plain square. Applying the style's own preset changes no radius
        // token of the new scale.
        for (style, resolved) in styles() {
            let preset = match style {
                Style::Studio | Style::Adwaita => R::Round,
                Style::Breeze | Style::Pro => R::SlightlyRound,
                Style::Classic | Style::Plain => R::Square,
            };
            let rounded = round(resolved, preset);
            assert_eq!(
                (
                    rounded.radius_0,
                    rounded.radius_xs,
                    rounded.radius_s,
                    rounded.radius_m,
                    rounded.radius_l,
                    rounded.radius_xl,
                    rounded.radius_pill
                ),
                (
                    resolved.radius_0,
                    resolved.radius_xs,
                    resolved.radius_s,
                    resolved.radius_m,
                    resolved.radius_l,
                    resolved.radius_xl,
                    resolved.radius_pill
                ),
                "{style:?}"
            );
        }
    }

    #[test]
    fn spacing_follows_the_reference_table_per_step() {
        let (_, style) = styles().remove(0);
        assert_eq!(spacing(style, Density::Standard), style);
        let compact = spacing(style, Density::Compact);
        assert_eq!(
            [
                compact.spacing_none,
                compact.spacing_xxxs,
                compact.spacing_xxs,
                compact.spacing_xs,
                compact.spacing_s,
                compact.spacing_m,
                compact.spacing_l,
                compact.spacing_xl,
                compact.spacing_xxl,
                compact.spacing_xxxl
            ],
            [0.0, 4.0, 4.0, 8.0, 8.0, 16.0, 24.0, 32.0, 48.0, 64.0]
        );
        assert_eq!(
            [
                compact.header_padding_top,
                compact.header_padding_right,
                compact.header_padding_bottom,
                compact.header_padding_left
            ],
            [3.0, 7.0, 4.0, 7.0]
        );
        let spacious = spacing(style, Density::Spacious);
        assert_eq!(spacious.spacing_none, 4.0);
        assert_eq!(spacious.spacing_xxxl, 160.0);
        assert_eq!(spacious.header_padding_bottom, 8.0);
    }

    #[test]
    fn the_default_geometry_on_studio_moves_only_radii() {
        let (_, studio) = styles()
            .into_iter()
            .find(|(s, _)| *s == Style::Studio)
            .unwrap();
        let applied = apply(studio, &Geometry::default());
        let mut expected = studio;
        expected.radius_sm = 4;
        expected.radius = 8;
        expected.radius_lg = 16;
        expected.field_radius = 8;
        expected.menu_highlight_radius = 8;
        assert_eq!(applied, expected);
    }

    #[test]
    fn names_round_trip() {
        for preset in R::ALL {
            assert_eq!(Roundness::from_name(preset.name()), Some(preset));
        }
        for step in Density::ALL {
            assert_eq!(Density::from_name(step.name()), Some(step));
        }
        assert_eq!(Density::from_name("standard"), None);
    }
}
