// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Resolve v2: the effective map from the authored [`Values`] (key space v2,
//! R5 as amended). Validation is the registry's ([`registry::validate`]);
//! the inputs are the typed views; the mode is resolved first, so no
//! context ever carries `auto`.
//!
//! **The look seam.** S4's look half (after S2) plugs in at
//! [`look_context`]: it adds the accent and the four tints to the compile
//! (`design::DesignContext` gains them in S2), compiles the derived `mixos`
//! scheme instead of today's stand-in, applies `design::geometry`
//! (roundness, density) after the compile, and seals the resolved accent
//! into [`Effective`], retiring [`Accents`]. Until then the look resolves
//! exactly as today's [`crate::resolve`] does for the same choices.
use super::{APPS, Accents, claims_app_axis, compile_context, error, parsed};
use crate::registry::{self, Values};
use crate::view::{Accessibility, AppLooks, Appearance, Framing, State, View};
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
pub fn effective_mode(authored: &str, state: &State) -> (&'static str, &'static str) {
    match authored {
        "light" => ("light", "profile"),
        "auto" => match (state.mode_override.as_deref(), state.daylight) {
            (Some("light"), _) => ("light", "state.appearance.mode_override"),
            (Some(_), _) => ("dark", "state.appearance.mode_override"),
            (None, Some(true)) => ("light", "state.appearance.daylight"),
            (None, Some(false)) => ("dark", "state.appearance.daylight"),
            (None, None) => ("dark", "auto_without_daylight"),
        },
        _ => ("dark", "profile"),
    }
}

/// The look inputs of one context, after the axes (R5 step 1).
struct Axes<'a> {
    appearance: &'a Appearance,
    scheme: String,
    mode: &'static str,
    contrast: String,
    app: Option<&'a str>,
}

/// LOOK SEAM: the design context for one context's axes. The look half
/// adds the accent and tints here and compiles `mixos` as the derived
/// scheme; until S2 lands, `mixos` compiles as `studio`, today's default
/// scheme, so a profile that never chose a scheme keeps today's look.
fn look_context(axes: &Axes) -> Result<design::DesignContext, Diagnostic> {
    let scheme = match axes.scheme.as_str() {
        "mixos" => design::Scheme::Studio,
        name => design::Scheme::from_name(name)
            .ok_or_else(|| error("appearance.scheme", "Unknown scheme"))?,
    };
    Ok(design::DesignContext {
        scheme,
        mode: design::Mode::from_name(axes.mode)
            .ok_or_else(|| error("appearance.mode", "Unknown mode"))?,
        contrast: design::Contrast::from_name(&axes.contrast)
            .ok_or_else(|| error("appearance.contrast", "Unknown contrast"))?,
        style: axes
            .appearance
            .style
            .as_deref()
            .map(|name| {
                design::Style::from_name(name)
                    .ok_or_else(|| error("appearance.style", "Unknown style"))
            })
            .transpose()?,
        app: axes.app.map(str::to_owned),
    })
}

/// Every context's effective look and accent, from `values` with the
/// profile's pinned `embedded_source`. The values are validated first: an
/// invalid record is refused, never resolved around.
pub fn resolve(
    values: &Values,
    embedded_source: &str,
) -> Result<(BTreeMap<String, Effective>, Accents), Vec<Diagnostic>> {
    registry::validate(values)?;
    let appearance = Appearance::of(values);
    let state = State::of(values);
    let framing = Framing::of(values);
    let accessibility = Accessibility::of(values);
    let app_looks = AppLooks::of(values);
    let (mode, mode_from) = effective_mode(&appearance.mode, &state);

    let source = appearance.source.as_deref().unwrap_or(embedded_source);
    let doc = parsed(source)?;
    let app_axis = claims_app_axis(&doc);
    let mut compiled = BTreeMap::new();
    let mut contexts = BTreeSet::from([String::from("desktop")]);
    contexts.extend(APPS.iter().map(|a| format!("app:{a}")));
    contexts.extend(app_looks.0.keys().map(|a| format!("app:{a}")));

    let mut result = BTreeMap::new();
    let mut accents = Accents::new();
    for context in contexts {
        let app = context.strip_prefix("app:");
        let overlay = app.and_then(|id| app_looks.0.get(id));
        let mut axes = Axes {
            appearance: &appearance,
            scheme: appearance.scheme.clone(),
            mode,
            contrast: appearance.contrast.clone(),
            app,
        };
        let mut text_scale = accessibility.text_scale;
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
                axes.mode = if app_mode == "light" { "light" } else { "dark" };
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
        // LOOK SEAM: the look half seals this into `Effective`.
        if let Some(accent) = accent {
            accents.insert(context.clone(), accent);
        }
        let Framing {
            decorations,
            caption_side,
            style: window_style,
            menu_style,
            menu_side,
            actions_place,
            actions_align,
            title_align,
        } = framing.clone();
        let effective = Effective {
            scheme: axes.scheme,
            style: appearance.style.clone(),
            mode: axes.mode.into(),
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
            ui: CommonUi {
                density: appearance.density.legacy_multiplier(),
                text_scale,
                reduced_motion: accessibility.reduced_motion,
            },
            design: projection,
            provenance,
        };
        result.insert(context, effective);
    }
    Ok((result, accents))
}

#[cfg(test)]
mod tests;
