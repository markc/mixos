// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The derived scheme `mixos` and the colour axes: how seeds become the 31
//! chrome roles, and how an authored scheme's roles become seeds again.
//!
//! A derived scheme is authored as seeds (the theming ADR's A3): the accent
//! axis and the four tints ([`crate::Tints`]) feed [`crate::derive::derive`],
//! and [`ROLE_MAP`] reads the 31 roles off the derived theme. The compiler
//! enters them as authored primitives, so they render exactly, and sets the
//! semantic palette from the same theme ([`SEMANTIC_MAP`]) so the pairs and
//! the button table agree with the chrome.
//!
//! Under an authored scheme the tints have no slot, and a chosen accent
//! replaces the four accent roles only ([`apply_accent`]): `accent` is the
//! colour, `accent_text` follows the derivation's step 14 against the card,
//! `accent_border` is the accent, and `accent_soft` is the accent at 0.2.
//! The scheme's semantic pairs and focus ring keep their own accent, so
//! no choice can break their contrast contracts.
//!
//! [`seeds_of`] is the inverse used to export an authored scheme to COSMIC
//! and for the first tint under an authored scheme (F4): window background
//! from `chrome`, container background from `card`, the scheme's hue
//! ([`accent_seed`]), text tint from `text`, control tint from `field`.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use crate::derive::primitives::{over, with_alpha};
use crate::derive::{Component, Container, DerivedTheme, FrostedGlass, Seeds, derive};
use crate::family::chrome::ResolvedChrome;
use crate::source::{ColourSpace, DesignV1Source, OklchSource};
use crate::{AccentChoice, Colour8, DesignContext, ResolvedDictionary, SrgbColour, Tints};

/// One of the derived theme's three opaque containers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layer {
    Background,
    Primary,
    Secondary,
}

/// A container's own colours.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerPart {
    Base,
    On,
    Divider,
}

/// The theme's components: each container's own, and the button family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Family {
    Of(Layer),
    Accent,
    AccentButton,
    Button,
    Destructive,
    DestructiveButton,
    IconButton,
    Success,
    Warning,
}

/// A component's colours that a role reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Part {
    Base,
    Hover,
    Pressed,
    Selected,
    Divider,
    On,
    OnDisabled,
}

/// Where a role's colour comes from in a [`DerivedTheme`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RoleSource {
    Container(Layer, ContainerPart),
    Component(Family, Part),
    /// `accent_text`, or the accent when it already reads (spec §4.7).
    AccentText,
    /// The theme's shade.
    Shade,
    /// The accent at this alpha.
    AccentAt(f64),
    /// Black at this alpha.
    BlackAt(f64),
    /// The text-rank guard (Fable's R5): of `background.on` and
    /// `background.component.on`, the one with the higher contrast against
    /// `background.base` (`lower: false`) or the lower (`lower: true`), at
    /// `alpha`, so `text` is never dimmer than `text_dim` whatever the tints.
    TextRank {
        lower: bool,
        alpha: f64,
    },
    /// The first composited over the second, with the reference's `Over`.
    Over(&'static RoleSource, &'static RoleSource),
}

use ContainerPart as C;
use Family as F;
use Layer::{Background as Bg, Primary as Pr, Secondary as Se};
use Part as P;
use RoleSource::{
    AccentAt, AccentText, BlackAt, Component as Comp, Container as Cont, Over, Shade, TextRank,
};

const FIELD: RoleSource = Over(&Comp(F::Button, P::Base), &Cont(Pr, C::Base));

/// The 31 chrome roles of the derived scheme, in [`crate::family::chrome::ROLES`]
/// order, as Fable signed them off (2026-10-31): the design's A3 rows, plus
/// a recessed `canvas` (the theme's shade over the background, as every
/// authored scheme sits its desktop below its windows), `canvas_dot` on the
/// separator colour, `row_selected` as the accent tint (the reference's
/// neutral `selected` equals `hover`), a modal `scrim` at the authored
/// schemes' strength, and the text-rank guard.
pub const ROLE_MAP: [(&str, RoleSource); 31] = [
    // Surfaces.
    ("chrome", Cont(Bg, C::Base)),
    ("canvas", Over(&Shade, &Cont(Bg, C::Base))),
    ("canvas_dot", Cont(Bg, C::Divider)),
    ("dock", Cont(Bg, C::Base)),
    ("card", Cont(Pr, C::Base)),
    ("card_border", Cont(Pr, C::Divider)),
    ("tab_strip", Cont(Bg, C::Base)),
    // Controls and their states.
    ("field", FIELD),
    ("field_border", Over(&Comp(F::Button, P::Divider), &FIELD)),
    ("hover", Comp(F::Of(Bg), P::Hover)),
    ("pressed", Comp(F::Of(Bg), P::Pressed)),
    ("row_selected", AccentAt(ACCENT_SOFT_ALPHA)),
    // Text and icons.
    (
        "text",
        TextRank {
            lower: false,
            alpha: 1.0,
        },
    ),
    (
        "text_dim",
        TextRank {
            lower: true,
            alpha: 1.0,
        },
    ),
    (
        "text_faint",
        TextRank {
            lower: true,
            alpha: TEXT_FAINT_ALPHA,
        },
    ),
    ("icon", Comp(F::IconButton, P::On)),
    // Accent and selection.
    ("accent", Comp(F::Accent, P::Base)),
    ("accent_soft", AccentAt(ACCENT_SOFT_ALPHA)),
    ("accent_border", Comp(F::Accent, P::Base)),
    ("accent_text", AccentText),
    ("menu_highlight", Comp(F::Of(Pr), P::Hover)),
    ("menu_highlight_text", Comp(F::Of(Pr), P::On)),
    // Lines, depth and overlays.
    ("separator", Cont(Bg, C::Divider)),
    ("shadow", Shade),
    ("scrim", BlackAt(SCRIM_ALPHA)),
    // Actions and status.
    ("primary_bg", Comp(F::AccentButton, P::Base)),
    ("primary_text", Comp(F::AccentButton, P::On)),
    ("danger", Comp(F::Destructive, P::Base)),
    ("warning", Comp(F::Warning, P::Base)),
    ("caption_close", Comp(F::DestructiveButton, P::Base)),
    ("caption_close_text", Comp(F::DestructiveButton, P::On)),
];

/// The semantic palette of the derived scheme, so pairs, rings and the
/// button table are drawn from the same theme as the chrome. `elevated` is
/// the secondary container, which clears the pairs' 1.25:1 floor over
/// the background in both modes. `palette.accent.default`, which is also
/// the focus ring, is entered beside these as [`ring_safe_accent`].
pub const SEMANTIC_MAP: [(&str, RoleSource); 12] = [
    ("palette.background.1", Cont(Bg, C::Base)),
    ("palette.background.2", Cont(Pr, C::Base)),
    ("palette.background.3", Comp(F::Of(Pr), P::Base)),
    ("palette.background.muted", Comp(F::Of(Bg), P::Base)),
    ("palette.background.elevated", Cont(Se, C::Base)),
    ("palette.foreground.default", Cont(Bg, C::On)),
    ("palette.foreground.muted", Comp(F::Of(Bg), P::On)),
    ("palette.foreground.quiet", Comp(F::Of(Bg), P::On)),
    ("palette.accent.hover", Comp(F::Accent, P::Hover)),
    ("status.success", Comp(F::Success, P::Base)),
    ("status.warning", Comp(F::Warning, P::Base)),
    ("status.danger", Comp(F::Destructive, P::Base)),
];

/// `accent_soft`'s alpha: the derivation's hover-overlay alpha. It fills
/// selections under body text, so it stays a tint, as studio authors it.
pub const ACCENT_SOFT_ALPHA: f64 = 0.2;

/// The modal veil's alpha, in both modes: the authored schemes veil at
/// 0.40 to 0.55, and the reference's shade (0.08 in light) is a shadow, not
/// a veil. No exporter reads `scrim`.
pub const SCRIM_ALPHA: f64 = 0.43;

/// `text_faint`'s alpha: the reference's `on_disabled` (`on` at 0.65), laid
/// over the ranked dim text.
pub const TEXT_FAINT_ALPHA: f64 = 0.65;

impl RoleSource {
    /// The colour this source names in `theme`.
    pub fn pick(&self, theme: &DerivedTheme) -> SrgbColour {
        let container = |layer: Layer| -> &Container {
            match layer {
                Bg => &theme.background,
                Pr => &theme.primary,
                Se => &theme.secondary,
            }
        };
        match *self {
            Cont(layer, part) => {
                let c = container(layer);
                match part {
                    C::Base => c.base,
                    C::On => c.on,
                    C::Divider => c.divider,
                }
            }
            Comp(family, part) => {
                let c: &Component = match family {
                    F::Of(layer) => &container(layer).component,
                    F::Accent => &theme.accent,
                    F::AccentButton => &theme.accent_button,
                    F::Button => &theme.button,
                    F::Destructive => &theme.destructive,
                    F::DestructiveButton => &theme.destructive_button,
                    F::IconButton => &theme.icon_button,
                    F::Success => &theme.success,
                    F::Warning => &theme.warning,
                };
                match part {
                    P::Base => c.base,
                    P::Hover => c.hover,
                    P::Pressed => c.pressed,
                    P::Selected => c.selected,
                    P::Divider => c.divider,
                    P::On => c.on,
                    P::OnDisabled => c.on_disabled,
                }
            }
            AccentText => theme.accent_text_colour(),
            Shade => theme.shade,
            AccentAt(alpha) => with_alpha(theme.accent.base, alpha),
            BlackAt(alpha) => SrgbColour {
                red: 0.0,
                green: 0.0,
                blue: 0.0,
                alpha,
            },
            TextRank { lower, alpha } => {
                let base = theme.background.base;
                let on = theme.background.on;
                let component = theme.background.component.on;
                let reads = |text: SrgbColour| crate::derive::primitives::contrast(text, base);
                let (higher, dimmer) = if reads(on) >= reads(component) {
                    (on, component)
                } else {
                    (component, on)
                };
                with_alpha(if lower { dimmer } else { higher }, alpha)
            }
            Over(top, under) => over(top.pick(theme), under.pick(theme)),
        }
    }

    /// The source as the reference names it, e.g. `background.component.hover`.
    pub fn describe(&self) -> String {
        let layer = |layer: Layer| match layer {
            Bg => "background",
            Pr => "primary",
            Se => "secondary",
        };
        let part = |part: Part| match part {
            P::Base => "base",
            P::Hover => "hover",
            P::Pressed => "pressed",
            P::Selected => "selected",
            P::Divider => "divider",
            P::On => "on",
            P::OnDisabled => "on_disabled",
        };
        match *self {
            Cont(l, p) => format!(
                "{}.{}",
                layer(l),
                match p {
                    C::Base => "base",
                    C::On => "on",
                    C::Divider => "divider",
                }
            ),
            Comp(family, p) => {
                let family = match family {
                    F::Of(l) => format!("{}.component", layer(l)),
                    F::Accent => "accent".into(),
                    F::AccentButton => "accent_button".into(),
                    F::Button => "button".into(),
                    F::Destructive => "destructive".into(),
                    F::DestructiveButton => "destructive_button".into(),
                    F::IconButton => "icon_button".into(),
                    F::Success => "success".into(),
                    F::Warning => "warning".into(),
                };
                format!("{family}.{}", part(p))
            }
            AccentText => "accent_text (else accent.base)".into(),
            Shade => "shade".into(),
            AccentAt(alpha) => format!("accent.base @ {alpha}"),
            BlackAt(alpha) => format!("black @ {alpha}"),
            TextRank { lower, alpha } => {
                let rank = if lower { "lower" } else { "higher" };
                let at = if alpha < 1.0 {
                    format!(" @ {alpha}")
                } else {
                    String::new()
                };
                format!(
                    "{rank}-contrast of background.on, background.component.on on background.base{at}"
                )
            }
            Over(top, under) => format!("{} over {}", top.describe(), under.describe()),
        }
    }
}

/// The derivation's seeds for `context`: its accent and tints.
pub fn seeds_for(context: &DesignContext) -> Seeds {
    let colour = |seed: Option<Colour8>| seed.map(Colour8::srgb);
    Seeds {
        accent: context.accent.resolve(context.mode),
        bg_color: colour(context.tints.window_bg),
        primary_container_bg: colour(context.tints.container_bg),
        neutral_tint: colour(context.tints.control_tint.map(Colour8::opaque)),
        text_tint: colour(context.tints.text_tint.map(Colour8::opaque)),
        ..Seeds::default()
    }
}

/// Which theme the derived scheme's semantic palette is read from. The
/// roles are always the context's own; when its accent or tints break a
/// pair's contract, the compiler steps down this list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Semantics {
    /// The context's seeds.
    Seeds,
    /// The context's tints with the palette's accent.
    PaletteAccent,
    /// The palette alone, which always compiles.
    Palette,
}

impl Semantics {
    pub const ALL: [Self; 3] = [Self::Seeds, Self::PaletteAccent, Self::Palette];

    pub const fn describe(self) -> &'static str {
        match self {
            Self::Seeds => "the chosen seeds",
            Self::PaletteAccent => "the tints with the palette's accent",
            Self::Palette => "the palette's own theme",
        }
    }

    fn context(self, context: &DesignContext) -> DesignContext {
        match self {
            Self::Seeds => context.clone(),
            Self::PaletteAccent => DesignContext {
                accent: AccentChoice::Auto,
                ..context.clone()
            },
            Self::Palette => DesignContext {
                accent: AccentChoice::Auto,
                tints: Tints::default(),
                ..context.clone()
            },
        }
    }
}

/// The derived theme of `context` (its mode, contrast, accent and tints).
/// Frosted glass is not a compile axis: the compile derives with the
/// default glass, whose container alpha still reaches the primary texts.
pub fn derived_theme(context: &DesignContext) -> DerivedTheme {
    derive(
        &seeds_for(context),
        context.mode,
        context.contrast,
        &FrostedGlass::default(),
    )
}

/// `colour` as an authored OKLCH primitive. Every 8-bit sRGB value
/// round-trips through it.
fn oklch(colour: SrgbColour) -> OklchSource {
    let (l, c, h) = crate::colour_model::derivation::linear_srgb_to_oklch(colour.to_linear());
    OklchSource {
        color_space: ColourSpace::Oklch,
        l,
        c,
        h,
        alpha: colour.alpha,
    }
}

/// Before the colour compile: under `mixos`, the derived theme's roles and
/// semantic palette, entered as authored primitives; nothing under an
/// authored scheme. Returns each entered primitive with the rule that
/// produced it, for provenance.
pub(crate) fn complete_source(
    source: &mut DesignV1Source,
    authored: &mut BTreeSet<String>,
    context: &DesignContext,
    semantics: Semantics,
) -> BTreeMap<String, String> {
    let mut origins = BTreeMap::new();
    let mut enter = |name: String, colour: SrgbColour, rule: String| {
        source.primitives.colors.insert(name.clone(), oklch(colour));
        authored.insert(name.clone());
        origins.insert(name, rule);
    };
    let mode = context.mode.name();
    if context.scheme.is_derived() {
        let theme = derived_theme(context);
        for (role, from) in ROLE_MAP {
            enter(
                format!("chrome.{role}"),
                from.pick(&theme),
                format!("derive.mixos.{mode}.{}", from.describe()),
            );
        }
        let semantic = if semantics == Semantics::Seeds {
            theme
        } else {
            derived_theme(&semantics.context(context))
        };
        for (name, from) in SEMANTIC_MAP {
            enter(
                name.into(),
                from.pick(&semantic),
                format!("derive.mixos.{mode}.{}", from.describe()),
            );
        }
        enter(
            "palette.accent.default".into(),
            ring_safe_accent(&semantic),
            format!("derive.mixos.{mode}.accent.base, ring-safe"),
        );
        // As every light scheme: hierarchy text on the quiet foreground.
        if context.mode == crate::Mode::Light {
            source.semantics.pairs.insert(
                "muted".into(),
                crate::source::PairSource::authored(
                    "palette.background.muted",
                    "palette.foreground.quiet",
                    None,
                ),
            );
        }
    }
    origins
}

/// The accent as the semantic `palette.accent.default`, which is also the
/// focus ring: the accent itself when it clears the compiler's non-text
/// floor against the three pairs the ring sits next to (`muted`, `secondary`
/// on the card, and `accent`, which `control_pair` derives from the ring
/// itself), else the accent walked in OKLab lightness, nearest first, until
/// it does. Any accent then compiles; the chrome roles keep the exact colour.
pub fn ring_safe_accent(theme: &DerivedTheme) -> SrgbColour {
    use crate::colour_model::NON_TEXT_CONTRAST;
    use crate::colour_model::derivation::{
        control_pair_surface, gamut_map_oklch, linear_srgb_to_oklch, non_text_contrast_ratio,
    };
    let surface = |colour: SrgbColour| colour.to_linear().rendered();
    let muted = surface(theme.background.component.base);
    let card = surface(theme.primary.base);
    let bg3 = surface(theme.primary.component.base);
    let bg1 = surface(theme.background.base);
    let accent = theme.accent.base;
    let clears = |ring: crate::LinearRgba| {
        let ring = ring.rendered();
        let Ok(pair) = control_pair_surface(bg3, ring, bg1) else {
            return false;
        };
        [muted, card, pair.rendered()]
            .into_iter()
            .all(|s| non_text_contrast_ratio(ring, s) >= NON_TEXT_CONTRAST)
    };
    let (l, c, h) = linear_srgb_to_oklch(accent.to_linear());
    // Nearest first, either way: tints can put dark surfaces in a light
    // mode and light ones in a dark mode.
    (0..=100)
        .flat_map(|k| [l + 0.01 * f64::from(k), l - 0.01 * f64::from(k)])
        .filter(|lightness| (0.0..=1.0).contains(lightness))
        .map(|lightness| rgb(gamut_map_oklch(lightness, c, h, 64)))
        .find(|candidate| clears(*candidate))
        .map_or(accent, SrgbColour::from_linear)
}

fn rgb([red, green, blue]: [f64; 3]) -> crate::LinearRgba {
    crate::LinearRgba {
        red: red.clamp(0.0, 1.0),
        green: green.clamp(0.0, 1.0),
        blue: blue.clamp(0.0, 1.0),
        alpha: 1.0,
    }
}

/// After the chrome compile, under an authored scheme with a chosen accent:
/// the four accent roles from the axis, with the derivation's step-14 text
/// rule measured against the scheme's own `chrome` and `card`.
pub(crate) fn apply_accent(chrome: &mut ResolvedChrome, context: &DesignContext) {
    if context.scheme.is_derived() {
        return;
    }
    let Some(accent) = context.accent.resolve(context.mode) else {
        return;
    };
    let role = |name: &str| chrome.get(name).map(SrgbColour::from_linear);
    let bg = role("chrome").unwrap_or(accent);
    let card = role("card").unwrap_or(bg);
    let text = crate::derive::accent_text_for(accent, bg, card, context.mode, context.contrast)
        .unwrap_or(accent);
    for (name, colour) in [
        ("accent", accent),
        ("accent_soft", with_alpha(accent, ACCENT_SOFT_ALPHA)),
        ("accent_border", accent),
        ("accent_text", text),
    ] {
        chrome.set(name, colour.to_linear());
    }
}

/// The seeds that would draw `dictionary`'s roles in the derived model:
/// window background from `chrome`, container background from `card`, the
/// scheme's hue ([`accent_seed`]), text tint from `text`, control tint from
/// `field` (a ramp keeps only its hue and chroma), and the scheme's danger,
/// warning and success. `None` where the design authors no chrome family.
pub fn seeds_of(dictionary: &ResolvedDictionary) -> Option<Seeds> {
    let chrome = dictionary.chrome.as_ref()?;
    let role = |name: &str| chrome.get(name).map(SrgbColour::from_linear);
    let opaque = |colour: Option<SrgbColour>| colour.map(|c| with_alpha(c, 1.0));
    Some(Seeds {
        accent: accent_seed(chrome),
        bg_color: role("chrome"),
        primary_container_bg: role("card"),
        secondary_container_bg: None,
        neutral_tint: opaque(role("field")),
        text_tint: opaque(role("text")),
        success: opaque(
            dictionary
                .colours
                .primitives
                .get("status.success")
                .copied()
                .map(SrgbColour::from_linear),
        ),
        warning: opaque(role("warning")),
        destructive: opaque(role("danger")),
        window_hint: None,
    })
}

/// The hue a person recognises as the scheme (Fable's ruling on F4): the most
/// chromatic, in OKLCh, of its `accent`, `accent_border` and `primary_bg`,
/// each composited over `chrome` first (studio's border carries alpha). The
/// hue schemes' `accent` is their soft selection tint and their
/// `primary_bg` the hue itself; the chrome schemes keep their `accent`.
/// [`crate::resolved_accent_of`], the ring's accent, is a different question.
pub fn accent_seed(chrome: &ResolvedChrome) -> Option<SrgbColour> {
    let under = chrome.get("chrome").map(SrgbColour::from_linear);
    let chroma = |colour: SrgbColour| {
        crate::colour_model::derivation::linear_srgb_to_oklch(colour.to_linear()).1
    };
    ["accent", "accent_border", "primary_bg"]
        .into_iter()
        .filter_map(|role| chrome.get(role).map(SrgbColour::from_linear))
        .map(|colour| match under {
            Some(under) if colour.alpha < 1.0 => with_alpha(over(colour, under), 1.0),
            _ => with_alpha(colour, 1.0),
        })
        .fold(None, |best: Option<SrgbColour>, colour| match best {
            Some(best) if chroma(best) >= chroma(colour) => Some(best),
            _ => Some(colour),
        })
}

/// The settings the first tint under an authored scheme writes beside
/// `appearance.scheme = mixos` (F4): the accent and the four tints from
/// `seeds`, each quantised to the 8-bit wire form.
pub fn tint_batch(seeds: &Seeds) -> (AccentChoice, Tints) {
    let eight = |colour: Option<SrgbColour>| colour.map(|c| Colour8(c.to_srgba8()));
    let opaque = |colour: Option<SrgbColour>| eight(colour).map(Colour8::opaque);
    (
        opaque(seeds.accent).map_or(AccentChoice::Auto, AccentChoice::Custom),
        Tints {
            window_bg: eight(seeds.bg_color),
            container_bg: eight(seeds.primary_container_bg),
            text_tint: opaque(seeds.text_tint),
            control_tint: opaque(seeds.neutral_tint),
        },
    )
}
