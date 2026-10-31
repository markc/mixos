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
//!   `derive(seeds_of(roles))`, the closest COSMIC can draw (`closest`).
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
/// `chrome`. A surface that is itself translucent is first composited over
/// `chrome`.
pub const FLAT_BASE: [(&str, &str); 6] = [
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
