// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The fixed tables of the COSMIC theme model: the two built-in 32-colour
//! palettes, the accent swatch row, the density and roundness presets with
//! their read-back rules, and the frosted-glass alpha map with its slider.
//!
//! Pure data, matching the clean-room theme-model spec (§3.1, §3.2, §5.1,
//! §5.2) value for value. The derivation that consumes the palettes is
//! [`crate::derive`].

use crate::{Contrast, Mode, SrgbColour};

/// An opaque colour from its `0xRRGGBB` value.
const fn rgb(hex: u32) -> SrgbColour {
    SrgbColour {
        red: ((hex >> 16) & 0xff) as f64 / 255.0,
        green: ((hex >> 8) & 0xff) as f64 / 255.0,
        blue: (hex & 0xff) as f64 / 255.0,
        alpha: 1.0,
    }
}

/// Which built-in palette a theme was derived from: its mode and whether it
/// is the high-contrast variant. The high-contrast variants carry the same
/// 32 colours as their normal twins; only the derivation's alphas and the
/// accent-text step differ.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaletteKind {
    Dark,
    Light,
    HighContrastDark,
    HighContrastLight,
}

impl PaletteKind {
    pub const fn of(mode: Mode, contrast: Contrast) -> Self {
        match (mode, contrast) {
            (Mode::Dark, Contrast::Normal) => Self::Dark,
            (Mode::Light, Contrast::Normal) => Self::Light,
            (Mode::Dark, Contrast::High) => Self::HighContrastDark,
            (Mode::Light, Contrast::High) => Self::HighContrastLight,
        }
    }

    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark | Self::HighContrastDark)
    }

    pub const fn is_high_contrast(self) -> bool {
        matches!(self, Self::HighContrastDark | Self::HighContrastLight)
    }
}

/// One built-in palette: 32 named opaque sRGB colours and its kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BasePalette {
    pub kind: PaletteKind,
    pub bright_red: SrgbColour,
    pub bright_green: SrgbColour,
    pub bright_orange: SrgbColour,
    /// The default window background.
    pub gray_1: SrgbColour,
    pub gray_2: SrgbColour,
    /// The eleven-step neutral ramp: step k has OKLab lightness k/10 in the
    /// dark palette, and the light palette holds the same steps reversed.
    pub neutral: [SrgbColour; 11],
    /// The default accent.
    pub accent_blue: SrgbColour,
    pub accent_indigo: SrgbColour,
    pub accent_purple: SrgbColour,
    pub accent_pink: SrgbColour,
    pub accent_red: SrgbColour,
    pub accent_orange: SrgbColour,
    pub accent_yellow: SrgbColour,
    pub accent_green: SrgbColour,
    pub accent_warm_grey: SrgbColour,
    pub ext_warm_grey: SrgbColour,
    pub ext_orange: SrgbColour,
    pub ext_yellow: SrgbColour,
    pub ext_blue: SrgbColour,
    pub ext_purple: SrgbColour,
    pub ext_pink: SrgbColour,
    pub ext_indigo: SrgbColour,
}

/// The names of the nine accent swatches, in the Appearance page's order.
pub const SWATCH_NAMES: [&str; 9] = [
    "blue",
    "indigo",
    "purple",
    "pink",
    "red",
    "orange",
    "yellow",
    "green",
    "warm_grey",
];

const DARK_NEUTRALS: [SrgbColour; 11] = [
    rgb(0x000000),
    rgb(0x030303),
    rgb(0x161616),
    rgb(0x2E2E2E),
    rgb(0x484848),
    rgb(0x636363),
    rgb(0x808080),
    rgb(0x9E9E9E),
    rgb(0xBEBEBE),
    rgb(0xDEDEDE),
    rgb(0xFFFFFF),
];

const fn reversed(steps: [SrgbColour; 11]) -> [SrgbColour; 11] {
    let mut out = steps;
    let mut i = 0;
    while i < 11 {
        out[i] = steps[10 - i];
        i += 1;
    }
    out
}

pub const DARK_PALETTE: BasePalette = BasePalette {
    kind: PaletteKind::Dark,
    bright_red: rgb(0xFFA09A),
    bright_green: rgb(0x5EDB8C),
    bright_orange: rgb(0xFFA37D),
    gray_1: rgb(0x1B1B1B),
    gray_2: rgb(0x262626),
    neutral: DARK_NEUTRALS,
    accent_blue: rgb(0x63D0DF),
    accent_indigo: rgb(0xA1C0EB),
    accent_purple: rgb(0xE79CFE),
    accent_pink: rgb(0xFF9CB1),
    accent_red: rgb(0xFDA1A0),
    accent_orange: rgb(0xFFAD00),
    accent_yellow: rgb(0xF7E062),
    accent_green: rgb(0x92CF9C),
    accent_warm_grey: rgb(0xCABAB4),
    ext_warm_grey: rgb(0x9B8E8A),
    ext_orange: rgb(0xFFAD00),
    ext_yellow: rgb(0xFEDB40),
    ext_blue: rgb(0x48B9C7),
    ext_purple: rgb(0xCF7DFF),
    ext_pink: rgb(0xF93A83),
    ext_indigo: rgb(0x3E88FF),
};

pub const LIGHT_PALETTE: BasePalette = BasePalette {
    kind: PaletteKind::Light,
    bright_red: rgb(0x890418),
    bright_green: rgb(0x00572C),
    bright_orange: rgb(0x792C00),
    gray_1: rgb(0xD7D7D7),
    gray_2: rgb(0xE4E4E4),
    neutral: reversed(DARK_NEUTRALS),
    accent_blue: rgb(0x00525A),
    accent_indigo: rgb(0x2E496D),
    accent_purple: rgb(0x68217C),
    accent_pink: rgb(0x86043A),
    accent_red: rgb(0x78292E),
    accent_orange: rgb(0x624000),
    accent_yellow: rgb(0x534800),
    accent_green: rgb(0x185529),
    accent_warm_grey: rgb(0x554742),
    ext_warm_grey: rgb(0x9B8E8A),
    ext_orange: rgb(0xFBB86C),
    ext_yellow: rgb(0xF7E062),
    ext_blue: rgb(0x6ACAD8),
    ext_purple: rgb(0xD58CFF),
    ext_pink: rgb(0xFF9CDD),
    ext_indigo: rgb(0x95C4FC),
};

impl BasePalette {
    /// The built-in palette for `mode`, tagged high contrast when asked.
    pub const fn builtin(mode: Mode, contrast: Contrast) -> Self {
        let mut palette = match mode {
            Mode::Dark => DARK_PALETTE,
            Mode::Light => LIGHT_PALETTE,
        };
        palette.kind = PaletteKind::of(mode, contrast);
        palette
    }

    /// The nine accent swatches in [`SWATCH_NAMES`] order.
    pub const fn swatches(&self) -> [SrgbColour; 9] {
        [
            self.accent_blue,
            self.accent_indigo,
            self.accent_purple,
            self.accent_pink,
            self.accent_red,
            self.accent_orange,
            self.accent_yellow,
            self.accent_green,
            self.accent_warm_grey,
        ]
    }

    /// The swatch named `name` (one of [`SWATCH_NAMES`]).
    pub fn swatch(&self, name: &str) -> Option<SrgbColour> {
        SWATCH_NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| self.swatches()[index])
    }
}

/// Interface density: three steps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Density {
    Compact,
    #[default]
    Standard,
    Spacious,
}

/// The ten spacing tokens, in logical px.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpacingTokens {
    pub space_none: u16,
    pub space_xxxs: u16,
    pub space_xxs: u16,
    pub space_xs: u16,
    pub space_s: u16,
    pub space_m: u16,
    pub space_l: u16,
    pub space_xl: u16,
    pub space_xxl: u16,
    pub space_xxxl: u16,
}

const fn spacing(values: [u16; 10]) -> SpacingTokens {
    SpacingTokens {
        space_none: values[0],
        space_xxxs: values[1],
        space_xxs: values[2],
        space_xs: values[3],
        space_s: values[4],
        space_m: values[5],
        space_l: values[6],
        space_xl: values[7],
        space_xxl: values[8],
        space_xxxl: values[9],
    }
}

impl Density {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Standard, Self::Spacious];

    /// The spacing table for this step.
    pub const fn spacing(self) -> SpacingTokens {
        match self {
            Self::Compact => spacing([0, 4, 4, 8, 8, 16, 24, 32, 48, 64]),
            Self::Standard => spacing([0, 4, 8, 12, 16, 24, 32, 48, 64, 128]),
            Self::Spacious => spacing([4, 8, 12, 16, 24, 32, 48, 64, 128, 160]),
        }
    }

    /// Reading a step back from a stored table: `space_m` 16 is compact, 24
    /// standard, anything else spacious.
    pub const fn from_spacing(spacing: &SpacingTokens) -> Self {
        match spacing.space_m {
            16 => Self::Compact,
            24 => Self::Standard,
            _ => Self::Spacious,
        }
    }

    /// The header bar's padding `[top, right, bottom, left]` in logical px
    /// for a client-decorated window. Standard and spacious share one.
    pub const fn header_padding(self, maximised: bool) -> [u16; 4] {
        match (self, maximised) {
            (Self::Compact, false) => [3, 7, 4, 7],
            (Self::Compact, true) => [4, 8, 4, 8],
            (_, false) => [7, 7, 8, 7],
            (_, true) => [8, 8, 8, 8],
        }
    }

    /// The panel's and dock's inter-applet spacing: the table's `space_none`.
    pub const fn panel_spacing(self) -> u16 {
        self.spacing().space_none
    }
}

/// A server-decorated window's header padding, whatever the density.
pub const SSD_HEADER_PADDING: [u16; 4] = [2, 8, 2, 8];

/// Roundness: three presets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Roundness {
    #[default]
    Round,
    SlightlyRound,
    Square,
}

/// The six radius tokens, each four corners (top-left, top-right,
/// bottom-right, bottom-left), in logical px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadiusTokens {
    pub radius_0: [f64; 4],
    pub radius_xs: [f64; 4],
    pub radius_s: [f64; 4],
    pub radius_m: [f64; 4],
    pub radius_l: [f64; 4],
    pub radius_xl: [f64; 4],
}

const fn radii(values: [f64; 6]) -> RadiusTokens {
    RadiusTokens {
        radius_0: [values[0]; 4],
        radius_xs: [values[1]; 4],
        radius_s: [values[2]; 4],
        radius_m: [values[3]; 4],
        radius_l: [values[4]; 4],
        radius_xl: [values[5]; 4],
    }
}

impl Roundness {
    pub const ALL: [Self; 3] = [Self::Round, Self::SlightlyRound, Self::Square];

    /// The preset's radius tokens as the reference ships them (its Square
    /// is 2 px, not 0).
    pub const fn corner_radii(self) -> RadiusTokens {
        match self {
            Self::Round => radii([0.0, 4.0, 8.0, 16.0, 32.0, 160.0]),
            Self::SlightlyRound => radii([0.0, 2.0, 8.0, 8.0, 8.0, 8.0]),
            Self::Square => radii([0.0, 2.0, 2.0, 2.0, 2.0, 2.0]),
        }
    }

    /// Reading a preset back from stored radii: `radius_m` 16 is round, 8
    /// slightly round, anything else square (so all-zero radii read back
    /// as square too). The top-left corner decides.
    pub fn from_radii(radii: &RadiusTokens) -> Self {
        let m = radii.radius_m[0];
        if m == 16.0 {
            Self::Round
        } else if m == 8.0 {
            Self::SlightlyRound
        } else {
            Self::Square
        }
    }

    /// The dock's padding after a roundness change.
    pub const fn dock_padding(self) -> u16 {
        match self {
            Self::Round | Self::SlightlyRound => 4,
            Self::Square => 0,
        }
    }
}

/// The panel's and dock's corner radius after a roundness change: the
/// `radius_xl` corners when the panel has a gap from the screen edge; else
/// `radius_xl` capped at 12 when it does not extend to the edges; else 0.
pub fn panel_radius(radii: &RadiusTokens, gap: bool, extend: bool) -> f64 {
    let xl = radii.radius_xl[0];
    if gap {
        xl
    } else if !extend {
        xl.min(12.0)
    } else {
        0.0
    }
}

/// Frosted-glass blur strength, the "Frost thickness" slider's 14 steps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BlurStrength {
    ExtremelyLow,
    ExtremelyLow2,
    VeryLow,
    VeryLow2,
    Low,
    Low2,
    #[default]
    Medium,
    Medium2,
    High,
    High2,
    VeryHigh,
    VeryHigh2,
    ExtremelyHigh,
    ExtremelyHigh2,
}

impl BlurStrength {
    pub const ALL: [Self; 14] = [
        Self::ExtremelyLow,
        Self::ExtremelyLow2,
        Self::VeryLow,
        Self::VeryLow2,
        Self::Low,
        Self::Low2,
        Self::Medium,
        Self::Medium2,
        Self::High,
        Self::High2,
        Self::VeryHigh,
        Self::VeryHigh2,
        Self::ExtremelyHigh,
        Self::ExtremelyHigh2,
    ];

    /// The slider position, 0 to 13.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The step at slider position `index`, if it is 0 to 13.
    pub const fn from_index(index: usize) -> Option<Self> {
        if index < 14 {
            Some(Self::ALL[index])
        } else {
            None
        }
    }
}

/// One container alpha per blur strength, indexed by
/// [`BlurStrength::index`].
pub type AlphaMap = [f64; 14];

/// The reference's default alpha map, literal for literal. Nominally
/// 0.90 − k × 0.3/13, but entry 10 is 0.66023 (the formula gives 0.66923)
/// and entries 3 and 5 are truncated rather than rounded. Reproduced as is.
pub const DEFAULT_ALPHA_MAP: AlphaMap = [
    0.90, 0.87692, 0.85385, 0.83076, 0.80769, 0.78461, 0.76154, 0.73846, 0.71538, 0.69231, 0.66023,
    0.64615, 0.62308, 0.60,
];

/// The default map's top and bottom entries, which bound the slider maths.
const ALPHA_TOP: f64 = 0.90;
const ALPHA_BOTTOM: f64 = 0.60;

/// The "Glass opacity" slider: `g` in 0..=1, 0.5 the default, offsets every
/// default entry by one amount. Below 0.5 the offset reaches −0.60 at 0;
/// above it, +0.10 at 1. Entries are not clamped here (only at blend time).
pub fn glass_alpha_map(g: f64) -> AlphaMap {
    let offset = if g < 0.5 {
        (g - 0.5) / 0.5 * ALPHA_BOTTOM
    } else {
        (g - 0.5) / 0.5 * (1.0 - ALPHA_TOP)
    };
    DEFAULT_ALPHA_MAP.map(|alpha| alpha + offset)
}

/// Reading the slider back from a stored map, the exact inverse of
/// [`glass_alpha_map`]: the first entry (lowest blur strength, default
/// 0.90) above 0.90 gives `0.5 + 0.5 × (e0 − 0.90) / 0.10`; else the last
/// entry (highest strength, default 0.60) below 0.60 gives
/// `0.5 − 0.5 × (0.60 − e13) / 0.60`; else 0.5.
pub fn glass_from_alpha_map(map: &AlphaMap) -> f64 {
    let first = map[0];
    let last = map[13];
    if first > ALPHA_TOP {
        0.5 + 0.5 * (first - ALPHA_TOP) / (1.0 - ALPHA_TOP)
    } else if last < ALPHA_BOTTOM {
        0.5 - 0.5 * (ALPHA_BOTTOM - last) / ALPHA_BOTTOM
    } else {
        0.5
    }
}

/// The builder's default gaps `(outer, inner)` in logical px.
pub const DEFAULT_GAPS: (u16, u16) = (0, 8);
/// The builder's default active window hint width in logical px.
pub const DEFAULT_ACTIVE_HINT: u16 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(colour: SrgbColour) -> String {
        let [r, g, b, _] = colour.to_srgba8();
        format!("#{r:02X}{g:02X}{b:02X}")
    }

    #[test]
    fn light_neutrals_are_the_dark_neutrals_reversed() {
        assert_eq!(hex(LIGHT_PALETTE.neutral[0]), "#FFFFFF");
        assert_eq!(hex(LIGHT_PALETTE.neutral[2]), "#BEBEBE");
        assert_eq!(hex(LIGHT_PALETTE.neutral[10]), "#000000");
        for k in 0..11 {
            assert_eq!(LIGHT_PALETTE.neutral[k], DARK_PALETTE.neutral[10 - k]);
        }
    }

    #[test]
    fn builtin_palette_carries_its_tag() {
        let hc = BasePalette::builtin(Mode::Light, Contrast::High);
        assert_eq!(hc.kind, PaletteKind::HighContrastLight);
        assert!(!hc.kind.is_dark() && hc.kind.is_high_contrast());
        assert_eq!(hc.gray_1, LIGHT_PALETTE.gray_1);
        assert_eq!(hex(DARK_PALETTE.swatch("warm_grey").unwrap()), "#CABAB4");
        assert_eq!(hex(LIGHT_PALETTE.swatch("blue").unwrap()), "#00525A");
        assert_eq!(DARK_PALETTE.swatch("teal"), None);
    }

    #[test]
    fn density_tables_and_read_back() {
        assert_eq!(
            Density::Compact.spacing(),
            spacing([0, 4, 4, 8, 8, 16, 24, 32, 48, 64])
        );
        assert_eq!(Density::Standard.spacing().space_xxxl, 128);
        assert_eq!(Density::Spacious.spacing().space_none, 4);
        for density in Density::ALL {
            assert_eq!(Density::from_spacing(&density.spacing()), density);
        }
        let mut odd = Density::Standard.spacing();
        odd.space_m = 20;
        assert_eq!(Density::from_spacing(&odd), Density::Spacious);
        assert_eq!(Density::Compact.header_padding(false), [3, 7, 4, 7]);
        assert_eq!(Density::Compact.header_padding(true), [4, 8, 4, 8]);
        assert_eq!(
            Density::Spacious.header_padding(false),
            Density::Standard.header_padding(false)
        );
        assert_eq!(Density::Standard.header_padding(true), [8, 8, 8, 8]);
        assert_eq!(Density::ALL.map(Density::panel_spacing), [0, 0, 4]);
    }

    #[test]
    fn roundness_tables_and_read_back() {
        let round = Roundness::Round.corner_radii();
        assert_eq!(round.radius_xs, [4.0; 4]);
        assert_eq!(round.radius_xl, [160.0; 4]);
        assert_eq!(Roundness::SlightlyRound.corner_radii().radius_xs, [2.0; 4]);
        assert_eq!(Roundness::Square.corner_radii().radius_s, [2.0; 4]);
        for roundness in Roundness::ALL {
            assert_eq!(Roundness::from_radii(&roundness.corner_radii()), roundness);
            assert_eq!(roundness.corner_radii().radius_0, [0.0; 4]);
        }
        // MixOS writes 0 on every token for square; it still reads back.
        assert_eq!(Roundness::from_radii(&radii([0.0; 6])), Roundness::Square);
        assert_eq!(panel_radius(&round, true, false), 160.0);
        assert_eq!(panel_radius(&round, false, false), 12.0);
        assert_eq!(panel_radius(&round, false, true), 0.0);
        assert_eq!(
            panel_radius(&Roundness::Square.corner_radii(), false, false),
            2.0
        );
        assert_eq!(Roundness::ALL.map(Roundness::dock_padding), [4, 4, 0]);
    }

    #[test]
    fn alpha_map_literals_are_the_reference_constants() {
        assert_eq!(DEFAULT_ALPHA_MAP[BlurStrength::Medium.index()], 0.76154);
        assert_eq!(DEFAULT_ALPHA_MAP[10], 0.66023);
        assert_eq!(DEFAULT_ALPHA_MAP[3], 0.83076);
        assert_eq!(DEFAULT_ALPHA_MAP[5], 0.78461);
        assert_eq!(BlurStrength::default(), BlurStrength::Medium);
        for (index, strength) in BlurStrength::ALL.into_iter().enumerate() {
            assert_eq!(strength.index(), index);
            assert_eq!(BlurStrength::from_index(index), Some(strength));
        }
        assert_eq!(BlurStrength::from_index(14), None);
    }

    fn assert_map(actual: AlphaMap, expected: AlphaMap) {
        for (a, e) in actual.into_iter().zip(expected) {
            assert!((a - e).abs() < 1e-12, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn glass_slider_writes_the_offset_map() {
        assert_map(glass_alpha_map(0.5), DEFAULT_ALPHA_MAP);
        assert_map(glass_alpha_map(0.0), DEFAULT_ALPHA_MAP.map(|a| a - 0.60));
        assert_map(glass_alpha_map(1.0), DEFAULT_ALPHA_MAP.map(|a| a + 0.10));
        assert!((glass_alpha_map(1.0)[0] - 1.0).abs() < 1e-12);
        assert_map(glass_alpha_map(0.75), DEFAULT_ALPHA_MAP.map(|a| a + 0.05));
        assert_map(glass_alpha_map(0.25), DEFAULT_ALPHA_MAP.map(|a| a - 0.30));
    }

    #[test]
    fn glass_read_back_inverts_the_slider_for_every_g() {
        assert_eq!(glass_from_alpha_map(&DEFAULT_ALPHA_MAP), 0.5);
        for step in 0..=100 {
            let g = f64::from(step) / 100.0;
            let back = glass_from_alpha_map(&glass_alpha_map(g));
            assert!((back - g).abs() < 1e-9, "g {g} read back as {back}");
        }
    }

    #[test]
    fn glass_read_back_tests_the_first_and_last_entries() {
        // Only e0 and e13 decide, not the map's extremes elsewhere.
        let mut map = DEFAULT_ALPHA_MAP;
        map[0] = 0.95;
        assert!((glass_from_alpha_map(&map) - 0.75).abs() < 1e-12);
        let mut map = DEFAULT_ALPHA_MAP;
        map[13] = 0.30;
        assert!((glass_from_alpha_map(&map) - 0.25).abs() < 1e-12);
        let mut map = DEFAULT_ALPHA_MAP;
        map[5] = 0.99;
        map[8] = 0.10;
        assert_eq!(glass_from_alpha_map(&map), 0.5);
    }
}
