// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The answer of `settings.palette`: for one resolved look and each mode,
//! everything an exporter writes, so exporters name colours and compute
//! nothing (the decision's ruling 1).
//!
//! Per mode:
//!
//! - `roles`: the 31 effective chrome roles, as egui draws them;
//! - `flat`: every role with its translucency composited over the surface
//!   it is drawn on ([`FLAT_BASE`]), for Qt's palette and the KDE scheme,
//!   which take opaque colours;
//! - `accent`: the resolved accent, and `swatches`: the nine swatches of
//!   that mode;
//! - `cosmic`: the COSMIC-model values, the builder seeds and the derived
//!   theme with every container and component, plus the spacing and the
//!   radii of the geometry. Under the derived scheme they are the
//!   derivation itself (`parity: exact`); under an authored scheme they are
//!   `derive(seeds_of(roles))`, the closest COSMIC can draw (`closest`);
//! - `fonts`: the interface and monospace fonts MixOS draws ([`Fonts`]),
//!   with the session's font choices over the design's (the caller applies
//!   them with [`Fonts::with_choices`]);
//! - `toolkit`: from the same theme as `cosmic`, libadwaita's named colours
//!   (`gtk`) and the three qt5ct/qt6ct palette groups (`qt`), for the GTK
//!   and Qt exporters.
//!
//! Colours serialise as `#RRGGBBAA`. Pure data.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::cosmic_tables::{BasePalette, RadiusTokens, SWATCH_NAMES, SpacingTokens};
use crate::derive::primitives::over;
use crate::derive::{DerivedTheme, FrostedGlass, Seeds, derive};
use crate::geometry::Geometry;
use crate::{
    DesignCompileResult, DesignContext, DesignDiagnostic, DesignSourceDocument, Mode,
    ResolvedDictionary, SrgbColour, compile_design,
};

/// Both modes of one look.
#[derive(Clone, Debug, Serialize)]
pub struct Palette {
    pub dark: ModePalette,
    pub light: ModePalette,
}

/// One mode's palette.
#[derive(Clone, Debug, Serialize)]
pub struct ModePalette {
    pub scheme: &'static str,
    pub mode: &'static str,
    pub contrast: &'static str,
    /// `exact` under the derived scheme, `closest` under an authored one.
    pub parity: &'static str,
    pub roles: BTreeMap<String, SrgbColour>,
    pub flat: BTreeMap<String, SrgbColour>,
    pub accent: SrgbColour,
    pub swatches: BTreeMap<&'static str, SrgbColour>,
    pub cosmic: Cosmic,
    /// The interface and monospace fonts MixOS draws, so an exporter
    /// writes the same ones.
    pub fonts: Fonts,
    /// What the GTK and Qt exporters write, from `cosmic.theme`.
    pub toolkit: crate::derive::toolkit::Toolkit,
}

/// The interface and monospace fonts of one look.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Fonts {
    pub ui: Font,
    pub mono: Font,
}

/// One font: its family, size in logical pixels and CSS weight.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Font {
    pub family: String,
    pub size_px: f64,
    pub weight: u16,
}

/// The faces the toolkit bundles and draws whatever family a design's type
/// records name: a session chooses another family through its settings
/// ([`Fonts::with_choices`]), never through the design.
pub const UI_FAMILY: &str = "Inter";
pub const MONO_FAMILY: &str = "JetBrains Mono";

/// The body weight of a style with fixed faces (the chrome's Inter
/// Regular).
const FIXED_FACES_WEIGHT: u16 = 400;

impl Fonts {
    /// The fonts of a compiled design: the bundled families, at the sizes
    /// and weights of its `ui` and `mono` type roles (a style with fixed
    /// faces draws its body at Regular).
    pub fn of(design: &crate::UnstampedResolvedDesign) -> Self {
        use crate::family::style::Faces;
        use crate::{TypographyRole, active_typography};
        let typography = design.typography();
        let ui = active_typography(Some(typography), TypographyRole::Ui);
        let mono = active_typography(Some(typography), TypographyRole::Mono);
        let fixed = design
            .dictionary()
            .style
            .is_some_and(|style| style.faces == Faces::Fixed);
        Self {
            ui: Font {
                family: UI_FAMILY.into(),
                size_px: ui.font_size,
                weight: if fixed { FIXED_FACES_WEIGHT } else { ui.weight },
            },
            mono: Font {
                family: MONO_FAMILY.into(),
                size_px: mono.font_size,
                weight: mono.weight,
            },
        }
    }

    /// These fonts with a session's choices over them; `None` keeps the
    /// design's.
    pub fn with_choices(
        mut self,
        family: Option<&str>,
        size_px: Option<f64>,
        weight: Option<u16>,
        mono_family: Option<&str>,
    ) -> Self {
        fn named(name: Option<&str>) -> Option<&str> {
            name.map(str::trim).filter(|n| !n.is_empty())
        }
        if let Some(family) = named(family) {
            self.ui.family = family.into();
        }
        if let Some(size) = size_px {
            self.ui.size_px = size;
        }
        if let Some(weight) = weight {
            self.ui.weight = weight;
        }
        if let Some(family) = named(mono_family) {
            self.mono.family = family.into();
        }
        self
    }
}

/// The COSMIC-model values of one mode.
#[derive(Clone, Debug, Serialize)]
pub struct Cosmic {
    /// The builder's colour inputs.
    pub builder: Seeds,
    /// The derived theme: containers, components, accent text, shade, the
    /// palette and the frosted inputs.
    pub theme: DerivedTheme,
    /// The density's spacing table.
    pub spacing: SpacingTokens,
    /// The roundness's radii, as MixOS draws them (`square` is 0).
    pub corner_radii: RadiusTokens,
}

/// The surface each translucent role is drawn on; every other role sits on
/// `chrome`. The selection fills sit on the card, where lists and selected
/// controls are drawn (the AA test for body text over `accent_soft`
/// measures there too). A surface that is itself translucent is flattened
/// first.
pub const FLAT_BASE: [(&str, &str); 8] = [
    ("accent_soft", "card"),
    ("row_selected", "card"),
    ("canvas_dot", "canvas"),
    ("card_border", "card"),
    ("field", "card"),
    ("field_border", "field"),
    ("menu_highlight", "card"),
    ("menu_highlight_text", "menu_highlight"),
];

/// The palette of `context`'s look in both modes (its own mode is ignored).
pub fn palette(
    document: &DesignSourceDocument,
    context: &DesignContext,
    geometry: &Geometry,
    frosted: &FrostedGlass,
) -> Result<Palette, Vec<DesignDiagnostic>> {
    let at = |mode: Mode| {
        mode_palette(
            document,
            &DesignContext {
                mode,
                ..context.clone()
            },
            geometry,
            frosted,
        )
    };
    Ok(Palette {
        dark: at(Mode::Dark)?,
        light: at(Mode::Light)?,
    })
}

/// The palette of `context` in its own mode.
pub fn mode_palette(
    document: &DesignSourceDocument,
    context: &DesignContext,
    geometry: &Geometry,
    frosted: &FrostedGlass,
) -> Result<ModePalette, Vec<DesignDiagnostic>> {
    let design = match compile_design(document, context.clone()) {
        DesignCompileResult::Success(success) => success.candidate,
        DesignCompileResult::Fatal(failure) => return Err(failure.diagnostics),
    };
    let fonts = Fonts::of(&design);
    let dictionary = design.dictionary();
    let roles = roles_of(dictionary);
    let builder = if context.scheme.is_derived() {
        crate::seeds::seeds_for(context)
    } else {
        crate::seeds::seeds_of(dictionary).unwrap_or_else(|| crate::seeds::seeds_for(context))
    };
    let theme = derive(&builder, context.mode, context.contrast, frosted);
    let accent = context
        .accent
        .resolve(context.mode)
        .or_else(|| crate::resolved_accent_of(dictionary))
        .unwrap_or(theme.accent.base);
    let swatches = BasePalette::builtin(context.mode, context.contrast).swatches();
    let [r0, xs, s, m, l, xl] = geometry.roundness.radii().map(|r| [f64::from(r); 4]);
    Ok(ModePalette {
        scheme: context.scheme.name(),
        mode: context.mode.name(),
        contrast: context.contrast.name(),
        parity: if context.scheme.is_derived() {
            "exact"
        } else {
            "closest"
        },
        flat: flat(&roles),
        roles,
        accent,
        swatches: SWATCH_NAMES.into_iter().zip(swatches).collect(),
        toolkit: crate::derive::toolkit::Toolkit::of(&theme),
        cosmic: Cosmic {
            builder,
            theme,
            spacing: geometry.density.spacing(),
            corner_radii: RadiusTokens {
                radius_0: r0,
                radius_xs: xs,
                radius_s: s,
                radius_m: m,
                radius_l: l,
                radius_xl: xl,
            },
        },
        fonts,
    })
}

/// The compiled chrome roles, as 8-bit sRGB.
pub fn roles_of(dictionary: &ResolvedDictionary) -> BTreeMap<String, SrgbColour> {
    dictionary
        .chrome
        .iter()
        .flat_map(|chrome| chrome.iter())
        .map(|(role, colour)| (role.to_owned(), SrgbColour::from_linear(colour)))
        .collect()
}

/// Every role, opaque: composited over the surface it is drawn on.
pub fn flat(roles: &BTreeMap<String, SrgbColour>) -> BTreeMap<String, SrgbColour> {
    roles
        .keys()
        .map(|role| (role.clone(), flatten(role, roles, 0)))
        .collect()
}

/// `role` over its surface, the surface flattened first (`field_border`
/// over `field` over `card`). The depth bounds a cycle in the table.
fn flatten(role: &str, roles: &BTreeMap<String, SrgbColour>, depth: usize) -> SrgbColour {
    let colour = roles[role];
    if colour.alpha >= 1.0 || depth > FLAT_BASE.len() {
        return colour;
    }
    let base = FLAT_BASE
        .iter()
        .find(|(name, _)| *name == role)
        .map_or("chrome", |(_, base)| base);
    if base == role || !roles.contains_key(base) {
        return colour;
    }
    over(colour, flatten(base, roles, depth + 1))
}

#[cfg(test)]
mod tests;
