// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Resolve v2: the effective map from the authored [`Values`] (key space v2,
//! R5 as amended). Validation is the registry's ([`registry::validate`]);
//! the inputs are the [`Look`] view; the mode is resolved first, so no
//! context ever carries `auto`.
//!
//! The look (§3.2, §3.3, §4.3): each context compiles its scheme (`mixos`
//! as the derived scheme) with the accent and, under `mixos`, the four tints
//! ([`look_context`]). The accent it draws is sealed into its [`Effective`],
//! with the roundness and the authored `tune.*`, so the effective digest
//! covers the resolved accent, the resolved mode and the geometry inputs.
//! Geometry and tunes change only style tokens, which the read projection
//! never carries (ADR §2), so each follower lays them over its own compile:
//! `design::geometry::apply`, then the tunes (R5 steps 2a and 4).
//!
//! With the accent `auto`, no tints, `round` and `comfortable`, every
//! authored scheme resolves exactly as the format-4 path ([`crate::v1`])
//! does, its accent sealed rather than beside the map.
use super::{APPS, claims_app_axis, compile_context, error, parsed};
use crate::registry::{self, Values};
use crate::view::{Accent, Look, View};
use crate::{CommonUi, Diagnostic, Effective};
use std::collections::{BTreeMap, BTreeSet};

/// The effective mode of the authored `mode` (ruling F11, §3.3 A1):
///
/// | `appearance.mode` | `mode_override` | `daylight` | effective |
/// |---|---|---|---|
/// | `dark` / `light` | any | any | as authored |
/// | `auto` | `dark` / `light` | any | the override |
/// | `auto` | none | true | `light` |
/// | `auto` | none | false | `dark` |
/// | `auto` | none | none (no daylight citizen) | `dark` |
///
/// With the key that decided it, for provenance.
///
/// A name outside the registry's enums is refused, never read as dark:
/// validation already refuses it, so this is a second line, not a default.
pub fn effective_mode(
    authored: &str,
    state: &crate::view::State,
) -> Result<(Mode, &'static str), Diagnostic> {
    let named =
        |name: &str, path: &str| Mode::from_name(name).ok_or_else(|| error(path, "Unknown mode"));
    Ok(match authored {
        "auto" => match (state.mode_override.as_deref(), state.daylight) {
            (Some(name), _) => (
                named(name, "state.appearance.mode_override")?,
                "state.appearance.mode_override",
            ),
            (None, Some(true)) => (Mode::Light, "state.appearance.daylight"),
            (None, Some(false)) => (Mode::Dark, "state.appearance.daylight"),
            (None, None) => (Mode::Dark, "auto_without_daylight"),
        },
        name => (named(name, "appearance.mode")?, "profile"),
    })
}

/// A resolved mode: never `auto`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// The look inputs of one context, after the axes (R5 step 1).
struct Axes<'a> {
    look: &'a Look,
    scheme: String,
    mode: Mode,
    contrast: String,
    app: Option<&'a str>,
}

/// The design context for one context's axes (R5 step 2): the scheme, the
/// resolved mode, the accent and, under the derived scheme only, the tints.
/// An authored scheme has no slot for a tint (A3), so its contexts compile
/// without them and share one compile whatever the tints say.
fn look_context(axes: &Axes) -> Result<design::DesignContext, Diagnostic> {
    let appearance = &axes.look.appearance;
    let scheme = design::Scheme::from_name(&axes.scheme)
        .ok_or_else(|| error("appearance.scheme", "Unknown scheme"))?;
    let accent = match &appearance.accent {
        Accent::Auto => Some(design::AccentChoice::Auto),
        Accent::Swatch(text) | Accent::Colour(text) => design::AccentChoice::from_name(text),
    }
    .ok_or_else(|| error("appearance.accent", "Unknown accent"))?;
    let tint = |path: &str, value: &Option<String>| {
        value
            .as_deref()
            .map(|hex| design::Colour8::from_hex(hex).ok_or_else(|| error(path, "Unknown colour")))
            .transpose()
    };
    let tints = if scheme.is_derived() {
        design::Tints {
            window_bg: tint("appearance.window_bg", &appearance.window_bg)?,
            container_bg: tint("appearance.container_bg", &appearance.container_bg)?,
            text_tint: tint("appearance.text_tint", &appearance.text_tint)?,
            control_tint: tint("appearance.control_tint", &appearance.control_tint)?,
        }
    } else {
        design::Tints::default()
    };
    Ok(design::DesignContext {
        scheme,
        mode: design::Mode::from_name(axes.mode.name())
            .ok_or_else(|| error("appearance.mode", "Unknown mode"))?,
        contrast: design::Contrast::from_name(&axes.contrast)
            .ok_or_else(|| error("appearance.contrast", "Unknown contrast"))?,
        style: appearance
            .style
            .as_deref()
            .map(|name| {
                design::Style::from_name(name)
                    .ok_or_else(|| error("appearance.style", "Unknown style"))
            })
            .transpose()?,
        app: axes.app.map(str::to_owned),
        accent,
        tints,
    })
}

/// Every context's effective look, from `values` with the profile's pinned
/// `embedded_source`. The values are validated first: an invalid record is
/// refused, never resolved around.
pub fn resolve(
    values: &Values,
    embedded_source: &str,
) -> Result<BTreeMap<String, Effective>, Vec<Diagnostic>> {
    registry::validate(values)?;
    let look = Look::of(values);
    let appearance = &look.appearance;
    let (mode, mode_from) = effective_mode(&appearance.mode, &look.state).map_err(|e| vec![e])?;

    let source = appearance.source.as_deref().unwrap_or(embedded_source);
    let doc = parsed(source)?;
    let app_axis = claims_app_axis(&doc);
    let mut compiled = BTreeMap::new();
    let mut contexts = BTreeSet::from([String::from("desktop")]);
    contexts.extend(APPS.iter().map(|a| format!("app:{a}")));
    contexts.extend(look.apps.0.keys().map(|a| format!("app:{a}")));

    let mut result = BTreeMap::new();
    for context in contexts {
        let app = context.strip_prefix("app:");
        let overlay = app.and_then(|id| look.apps.0.get(id));
        let mut axes = Axes {
            look: &look,
            scheme: appearance.scheme.clone(),
            mode,
            contrast: appearance.contrast.clone(),
            app,
        };
        let mut text_scale = look.text_scale;
        let mut provenance = BTreeMap::from([
            ("appearance.scheme".to_owned(), "profile".to_owned()),
            ("appearance.mode".to_owned(), mode_from.to_owned()),
            ("appearance.contrast".to_owned(), "profile".to_owned()),
            ("accessibility.text_scale".to_owned(), "profile".to_owned()),
        ]);
        if let (Some(overlay), Some(app)) = (overlay, app) {
            let from = format!("apps.{app}");
            if let Some(scheme) = &overlay.scheme {
                axes.scheme = scheme.clone();
                provenance.insert("appearance.scheme".into(), from.clone());
            }
            if let Some(app_mode) = &overlay.mode {
                axes.mode = Mode::from_name(app_mode)
                    .ok_or_else(|| vec![error(&format!("{from}.mode"), "Unknown mode")])?;
                provenance.insert("appearance.mode".into(), from.clone());
            }
            if let Some(contrast) = &overlay.contrast {
                axes.contrast = contrast.clone();
                provenance.insert("appearance.contrast".into(), from.clone());
            }
            if let Some(scale) = overlay.text_scale {
                text_scale = scale;
                provenance.insert("accessibility.text_scale".into(), from);
            }
            // A requested high contrast cannot be cancelled by an app.
            if appearance.contrast == "high" {
                axes.contrast = "high".into();
                provenance.insert("appearance.contrast".into(), "profile_accessibility".into());
            }
        }
        let selected = look_context(&axes).map_err(|e| vec![e])?;
        let (projection, accent) = compile_context(&doc, selected, app_axis, &mut compiled)
            .map_err(|diagnostics| {
                diagnostics
                    .into_iter()
                    .map(|d| {
                        Diagnostic::new(d.code, &format!("appearance.source.{}", d.path), d.message)
                    })
                    .collect::<Vec<_>>()
            })?;
        let crate::view::Framing {
            decorations,
            caption_side,
            style: window_style,
            menu_style,
            menu_side,
            actions_place,
            actions_align,
            title_align,
        } = look.framing.clone();
        let effective = Effective {
            scheme: axes.scheme,
            style: appearance.style.clone(),
            mode: axes.mode.name().into(),
            contrast: axes.contrast,
            decorations,
            caption_side,
            window_style,
            menu_style,
            menu_side,
            actions_place,
            actions_align,
            title_align,
            icon_stroke: appearance.icon_stroke,
            font_family: appearance.font_family.clone(),
            font_weight: appearance.font_weight,
            font_size: appearance.font_size,
            mono_family: appearance.mono_family.clone(),
            accent,
            roundness: appearance.roundness.name().into(),
            tune: look.tune.0.clone(),
            ui: CommonUi {
                // The legacy tokens' multiplier of the step, which
                // `design::geometry` owns; S8a moves the toolkit onto the
                // step itself.
                density: appearance.density.multiplier(),
                text_scale,
                reduced_motion: look.reduced_motion,
            },
            design: projection,
            provenance,
        };
        result.insert(context, effective);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
