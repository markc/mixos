// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The frozen format-4 settings path: today's typed [`Desktop`] record, its
//! write vocabulary ([`patch`] and its setter), its hand-written validation
//! (in [`resolve_with_embedded_and_accents`]) and [`describe`].
//!
//! **A migration codec, not a serving path** (design §4.2). settingsd's
//! store is format 5 ([`crate::registry::Values`]); its only calls here
//! read and re-derive a format-4 record, to migrate it on open and to write
//! one for `settingsd downgrade`. Its key names (`appearance.window_style`,
//! `ui.density`, whole `apps.<id>` and `shell.panels.<id>` records) and
//! ranges are not key space v2's. Frozen: no new callers, no new keys. It
//! goes when format 4 leaves the downgrade window, one release after the
//! migration has run on every profile. The v2 path is [`crate::registry`],
//! [`crate::view`] and [`crate::resolve::values`].
use crate::resolve::{APPS, Accents, claims_app_axis, compile_context, error, parsed};
use crate::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn describe() -> Value {
    let mut described = describe_base();
    if let Some(fields) = described["fields"].as_object_mut() {
        let groups = [
            crate::terminal::describe(),
            crate::email::describe(),
            crate::content::describe(crate::content::CED_EDITOR),
        ];
        for group in groups {
            if let Value::Object(group) = group {
                fields.extend(group);
            }
        }
    }
    described
}

#[cfg(test)]
#[test]
fn described_terminal_focus_defaults_match_the_profile() {
    let described = describe();
    let focus = crate::terminal::Terminal::default().focus;
    assert_eq!(
        described["fields"]["terminal.focus.ring"]["default"],
        serde_json::to_value(focus.ring).unwrap()
    );
    assert_eq!(
        described["fields"]["terminal.focus.strength"]["default"],
        focus.strength
    );
}

fn describe_base() -> Value {
    json!({"version": CONTRACT_VERSION, "schema": SCHEMA,
        "implementation": "headless authority; renderer integrations pending",
        "revision_encoding": "canonical decimal u64 string", "max_receipts": MAX_RECEIPTS,
        "receipt_expiry": "missing receipts are unknown; no ordering of opaque operation IDs",
        "fields": {
            "appearance.scheme": {"type":"string", "enum":["ocean","crimson","stone","forest","sunset","mono","pro","studio","classic","breeze","adwaita"]},
            "appearance.style": {"type":"string|null", "enum":["plain","pro","studio","classic","breeze","adwaita"], "null":"the scheme's own style"},
            "appearance.mode": {"type":"string", "enum":["light","dark"]},
            "appearance.decorations": {"type":"string", "enum":["auto","csd","ssd"]},
            "appearance.caption_side": {"type":"string", "known_values":design::CaptionSide::NAMES, "unknown":"retained; renders as the default", "max_bytes":64},
            "appearance.window_style": {"type":"string", "known_values":design::WindowStyle::NAMES, "unknown":"retained; renders as the default", "max_bytes":64},
            "appearance.menu_style": {"type":"string", "known_values":design::MenuStyle::NAMES, "unknown":"retained; renders as the default", "max_bytes":64},
            "appearance.title_align": {"type":"string", "known_values":design::TitleAlign::NAMES, "unknown":"retained; renders as the default", "max_bytes":64},
            "appearance.actions_align": {"type":"string", "known_values":design::ActionsAlign::NAMES, "unknown":"retained; renders as the default", "max_bytes":64},
            "appearance.actions_place": {"type":"string", "known_values":design::ActionsPlace::NAMES, "unknown":"retained; renders as the default", "max_bytes":64},
            "appearance.menu_side": {"type":"string", "known_values":design::MenuSide::NAMES, "unknown":"retained; renders as the default", "max_bytes":64},
            "appearance.icon_stroke": {"type":"number|null", "minimum":1.0, "maximum":3.0, "step":0.25, "null":"the design's own icon.stroke_width token"},
            "appearance.font_family": {"type":"string|null", "max_bytes":128, "null":"Inter"},
            "appearance.font_weight": {"type":"integer|null", "minimum":100, "maximum":900, "step":100, "null":"the scheme's own weight"},
            "appearance.font_size": {"type":"number|null", "minimum":crate::model::FONT_SIZE_MIN, "maximum":crate::model::FONT_SIZE_MAX, "unit":"px", "null":"the design's own UI size"},
            "appearance.mono_family": {"type":"string|null", "max_bytes":128, "null":"JetBrains Mono"},
            "appearance.contrast": {"type":"string", "enum":["normal","high"]},
            "appearance.source": {"type":"string|null", "max_bytes":MAX_SOURCE_BYTES},
            "ui.density": {"type":"number", "minimum":0.5, "maximum":2.0},
            "ui.text_scale": {"type":"number", "minimum":0.5, "maximum":3.0},
            "ui.reduced_motion": {"type":"boolean"},
            "shell.panels.<id>": {"type":"panel", "edge":["top","bottom","left","right"], "mode":["dock","overlay","hidden"], "thickness_range":[16,256]},
            "shell.page_order": {"type":"list", "max_items":64},
            "apps.<id>": {"type":"app_override", "fields":["scheme","mode","contrast","text_scale"]},
            "keymaps.<app>": {"type":"map", "of":"action id to a list of chords", "max_items":crate::keymap::MAX_ACTIONS, "null":"every action back to the app's defaults"},
            "keymaps.<app>.<action>": {"type":"list", "of":"chord", "max_items":crate::keymap::MAX_CHORDS, "empty":"unbound", "null":"the app's default",
                "chord":"Ctrl, Alt, Shift, Super (any order) then one key, joined by +", "max_bytes":crate::keymap::MAX_CHORD_BYTES}
        }, "reset":"remove explicit app values or restore field package default",
        "defaults": Desktop::default(), "native_apps": APPS,
        "read_views": {"full":"the complete snapshot (default)",
            "summary":"identity, revisions and appearance names; no projections or source"},
        "snapshot_schemas": {"1":"every context carries its design projection (default)",
            "2":"each distinct design projection once, by digest; read with schema: 2"},
        "topics": {"snapshot": TOPIC_PREFIX, "compact": crate::COMPACT_TOPIC_PREFIX,
            "summary": SUMMARY_TOPIC_PREFIX},
        "deferred":["renderer application","output overrides","font registration","artifacts","preview","replication","compatibility","policy"]})
}

fn bounded(value: f64, min: f64, max: f64, path: &str) -> Result<(), Diagnostic> {
    if !value.is_finite() || !(min..=max).contains(&value) {
        Err(error(path, format!("Expected {min}..{max}")))
    } else {
        Ok(())
    }
}
fn key(value: &str, path: &str) -> Result<(), Diagnostic> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Err(error(path, "Invalid scoped identifier"))
    } else {
        Ok(())
    }
}

/// Build every advertised app context before acceptance. Compiler itself checks
/// all claimed source axes. Returned projections cannot become apply lineage.
pub fn resolve(desktop: &Desktop) -> Result<BTreeMap<String, Effective>, Vec<Diagnostic>> {
    resolve_with_embedded(desktop, design::EMBEDDED_DEFAULT_SOURCE)
}

/// Authorities pin the package source at profile creation. A binary upgrade
/// cannot silently alter effective values within an accepted revision.
pub fn resolve_with_embedded(
    desktop: &Desktop,
    embedded_source: &str,
) -> Result<BTreeMap<String, Effective>, Vec<Diagnostic>> {
    resolve_with_embedded_and_accents(desktop, embedded_source).map(|(effective, _)| effective)
}

/// [`resolve_with_embedded`] plus the accent of each context, from the same
/// compiled design, so the two cannot disagree.
pub fn resolve_with_embedded_and_accents(
    desktop: &Desktop,
    embedded_source: &str,
) -> Result<(BTreeMap<String, Effective>, Accents), Vec<Diagnostic>> {
    let validate = || -> Result<(), Diagnostic> {
        // The derived scheme cannot be stored in format 4: the previous gold
        // binary kept for rollback would refuse the record. S6 (format 5)
        // lifts this.
        let stored_scheme = |path: &str, name: &str| {
            design::Scheme::from_name(name)
                .is_some_and(design::Scheme::is_derived)
                .then(|| error(path, "Scheme not writable until store format 5"))
        };
        if let Some(refused) = stored_scheme("appearance.scheme", &desktop.appearance.scheme) {
            return Err(refused);
        }
        for (app, value) in &desktop.apps {
            if let Some(refused) = value
                .scheme
                .as_deref()
                .and_then(|name| stored_scheme(&format!("apps.{app}.scheme"), name))
            {
                return Err(refused);
            }
        }
        bounded(desktop.ui.density, 0.5, 2.0, "ui.density")?;
        bounded(desktop.ui.text_scale, 0.5, 3.0, "ui.text_scale")?;
        if desktop.shell.panels.len() > 16
            || desktop.shell.page_order.len() > 64
            || desktop.apps.len() > 16
        {
            return Err(error("desktop", "Collection limit exceeded"));
        }
        for (id, panel) in &desktop.shell.panels {
            key(id, "shell.panels")?;
            if !["top", "bottom", "left", "right"].contains(&panel.edge.as_str())
                || !["dock", "overlay", "hidden"].contains(&panel.mode.as_str())
                || !(16..=256).contains(&panel.thickness)
            {
                return Err(error(
                    &format!("shell.panels.{id}"),
                    "Invalid edge, mode or thickness",
                ));
            }
        }
        let mut pages = BTreeSet::new();
        for id in &desktop.shell.page_order {
            key(id, "shell.page_order")?;
            if !pages.insert(id) {
                return Err(error("shell.page_order", "Duplicate page"));
            }
        }
        for (app, value) in &desktop.apps {
            key(app, "apps")?;
            if value
                .scheme
                .as_deref()
                .is_some_and(|name| design::Scheme::from_name(name).is_none())
            {
                return Err(error(&format!("apps.{app}.scheme"), "Unknown scheme"));
            }
            if value
                .mode
                .as_deref()
                .is_some_and(|name| design::Mode::from_name(name).is_none())
            {
                return Err(error(&format!("apps.{app}.mode"), "Unknown mode"));
            }
            if value
                .contrast
                .as_deref()
                .is_some_and(|name| design::Contrast::from_name(name).is_none())
            {
                return Err(error(&format!("apps.{app}.contrast"), "Unknown contrast"));
            }
            if let Some(scale) = value.text_scale {
                bounded(scale, 0.5, 3.0, &format!("apps.{app}.text_scale"))?;
            }
        }
        crate::keymap::validate(&desktop.keymaps, key)?;
        crate::terminal::validate(&desktop.terminal)?;
        crate::email::validate(&desktop.email)?;
        crate::content::validate_ced(&desktop.ced)?;
        if design::RequestedDecorations::from_name(&desktop.appearance.decorations).is_none() {
            return Err(error("appearance.decorations", "Unknown decorations"));
        }
        if !design::CaptionSide::valid_name(&desktop.appearance.caption_side) {
            return Err(error("appearance.caption_side", "Unknown caption side"));
        }
        if !design::WindowStyle::valid_name(&desktop.appearance.window_style) {
            return Err(error(
                "appearance.window_style",
                "Use a plain window style name of up to 64 bytes",
            ));
        }
        for (path, name, valid) in [
            (
                "appearance.actions_place",
                &desktop.appearance.actions_place,
                design::ActionsPlace::valid_name as fn(&str) -> bool,
            ),
            (
                "appearance.actions_align",
                &desktop.appearance.actions_align,
                design::ActionsAlign::valid_name as fn(&str) -> bool,
            ),
            (
                "appearance.title_align",
                &desktop.appearance.title_align,
                design::TitleAlign::valid_name as fn(&str) -> bool,
            ),
            (
                "appearance.menu_style",
                &desktop.appearance.menu_style,
                design::MenuStyle::valid_name as fn(&str) -> bool,
            ),
            (
                "appearance.menu_side",
                &desktop.appearance.menu_side,
                design::MenuSide::valid_name as fn(&str) -> bool,
            ),
        ] {
            if !valid(name) {
                return Err(error(path, "Use a plain menu name of up to 64 bytes"));
            }
        }
        if let Some(stroke) = desktop.appearance.icon_stroke {
            // Quarter steps only: a stroke that is not on the grid is refused,
            // so every client quantises the same way.
            bounded(
                stroke,
                ICON_STROKE_MIN,
                ICON_STROKE_MAX,
                "appearance.icon_stroke",
            )?;
            if !stroke.is_finite() || (stroke / ICON_STROKE_STEP).fract() != 0.0 {
                return Err(error(
                    "appearance.icon_stroke",
                    "Icon stroke must be a multiple of 0.25",
                ));
            }
        }
        if let Some(weight) = desktop.appearance.font_weight
            && (!(100..=900).contains(&weight) || weight % 100 != 0)
        {
            return Err(error(
                "appearance.font_weight",
                "Font weight must be 100 to 900 in steps of 100",
            ));
        }
        if let Some(size) = desktop.appearance.font_size
            && (!size.is_finite()
                || !(crate::model::FONT_SIZE_MIN..=crate::model::FONT_SIZE_MAX).contains(&size))
        {
            return Err(error(
                "appearance.font_size",
                format!(
                    "Font size must be {}..{} pixels",
                    crate::model::FONT_SIZE_MIN,
                    crate::model::FONT_SIZE_MAX
                ),
            ));
        }
        for (path, family) in [
            ("appearance.font_family", &desktop.appearance.font_family),
            ("appearance.mono_family", &desktop.appearance.mono_family),
        ] {
            if let Some(family) = family {
                // Existence is not checked: settingsd cannot know a client's
                // installed fonts. Shape only.
                if family.is_empty()
                    || family.len() > 128
                    || family.chars().any(char::is_control)
                    || family.trim() != family.as_str()
                {
                    return Err(error(path, "Font family must be a plain name"));
                }
            }
        }
        if desktop
            .appearance
            .source
            .as_ref()
            .is_some_and(|s| s.len() > MAX_SOURCE_BYTES)
        {
            return Err(error("appearance.source", "Source limit exceeded"));
        }
        Ok(())
    };
    validate().map_err(|e| vec![e])?;
    let source = desktop
        .appearance
        .source
        .as_deref()
        .unwrap_or(embedded_source);
    let doc = parsed(source)?;
    let app_axis = claims_app_axis(&doc);
    let mut compiled = BTreeMap::new();
    let mut contexts = BTreeSet::from([String::from("desktop")]);
    contexts.extend(APPS.iter().map(|a| format!("app:{a}")));
    contexts.extend(desktop.apps.keys().map(|a| format!("app:{a}")));
    let mut result = BTreeMap::new();
    let mut accents = Accents::new();
    for context in contexts {
        let app = context.strip_prefix("app:");
        let overlay = app.and_then(|id| desktop.apps.get(id));
        let mut appearance = desktop.appearance.clone();
        let mut ui = desktop.ui.clone();
        // Styles, decorations and caption side come only from the profile and
        // record no provenance, so a profile from before them keeps its
        // effective digest.
        let mut provenance = BTreeMap::from([
            ("appearance.scheme".into(), "profile".into()),
            ("appearance.mode".into(), "profile".into()),
            ("appearance.contrast".into(), "profile".into()),
            ("ui.text_scale".into(), "profile".into()),
        ]);
        if let Some(overlay) = overlay {
            for (name, input, destination) in [
                ("scheme", &overlay.scheme, &mut appearance.scheme),
                ("mode", &overlay.mode, &mut appearance.mode),
                ("contrast", &overlay.contrast, &mut appearance.contrast),
            ] {
                if let Some(value) = input {
                    *destination = value.clone();
                    provenance.insert(
                        format!("appearance.{name}"),
                        format!("apps.{}", app.unwrap()),
                    );
                }
            }
            if let Some(scale) = overlay.text_scale {
                ui.text_scale = scale;
                provenance.insert("ui.text_scale".into(), format!("apps.{}", app.unwrap()));
            }
            // A requested high contrast cannot be cancelled by an app.
            if desktop.appearance.contrast == "high" {
                appearance.contrast = "high".into();
                provenance.insert("appearance.contrast".into(), "profile_accessibility".into());
            }
        }
        let selection = || -> Result<design::DesignContext, Diagnostic> {
            Ok(design::DesignContext {
                scheme: design::Scheme::from_name(&appearance.scheme)
                    .ok_or_else(|| error("appearance.scheme", "Unknown scheme"))?,
                mode: design::Mode::from_name(&appearance.mode)
                    .ok_or_else(|| error("appearance.mode", "Unknown mode"))?,
                contrast: design::Contrast::from_name(&appearance.contrast)
                    .ok_or_else(|| error("appearance.contrast", "Unknown contrast"))?,
                style: appearance
                    .style
                    .as_deref()
                    .map(|name| {
                        design::Style::from_name(name)
                            .ok_or_else(|| error("appearance.style", "Unknown style"))
                    })
                    .transpose()?,
                app: app.map(str::to_owned),
                accent: design::AccentChoice::Auto,
                tints: design::Tints::default(),
            })
        };
        let selected = selection().map_err(|e| vec![e])?;
        let (projection, accent) = compile_context(&doc, selected, app_axis, &mut compiled)
            .map_err(|diagnostics| {
                diagnostics
                    .into_iter()
                    .map(|d| {
                        Diagnostic::new(d.code, &format!("appearance.source.{}", d.path), d.message)
                    })
                    .collect::<Vec<_>>()
            })?;
        if let Some(accent) = accent {
            accents.insert(context.clone(), accent);
        }
        result.insert(
            context.clone(),
            Effective {
                scheme: appearance.scheme,
                style: appearance.style,
                mode: appearance.mode,
                contrast: appearance.contrast,
                decorations: appearance.decorations,
                caption_side: appearance.caption_side,
                window_style: appearance.window_style,
                menu_style: appearance.menu_style,
                menu_side: appearance.menu_side,
                actions_place: appearance.actions_place,
                actions_align: appearance.actions_align,
                title_align: appearance.title_align,
                icon_stroke: appearance.icon_stroke,
                font_family: appearance.font_family,
                font_weight: appearance.font_weight,
                font_size: appearance.font_size,
                mono_family: appearance.mono_family,
                // Format 4 has none of these: the accent stays a sidecar.
                accent: None,
                roundness: crate::model::default_roundness(),
                tune: BTreeMap::new(),
                ui,
                design: projection,
                provenance,
            },
        );
    }
    Ok((result, accents))
}

/// The patch vocabulary is explicit. No recursive JSON merge and no executing
/// data. null resets an optional app override; reset is the same transaction.
pub fn patch(
    current: &Desktop,
    changes: &BTreeMap<String, Value>,
    reset: &[String],
) -> Result<Desktop, Diagnostic> {
    if changes.len() + reset.len() > 64 {
        return Err(error("changes", "Batch limit exceeded"));
    }
    let mut result = current.clone();
    for path in reset {
        if changes.contains_key(path) {
            return Err(error(path, "Cannot apply and reset the same field"));
        }
        set(&mut result, path, None)?;
    }
    for (path, value) in changes {
        set(&mut result, path, Some(value.clone()))?;
    }
    Ok(result)
}
fn typed<T: serde::de::DeserializeOwned>(path: &str, value: Value) -> Result<T, Diagnostic> {
    serde_json::from_value(value).map_err(|e| error(path, e.to_string()))
}
fn set(desktop: &mut Desktop, path: &str, value: Option<Value>) -> Result<(), Diagnostic> {
    let defaults = Desktop::default();
    match path {
        "appearance.scheme" => {
            desktop.appearance.scheme =
                typed(path, value.unwrap_or(json!(defaults.appearance.scheme)))?
        }
        "appearance.mode" => {
            desktop.appearance.mode = typed(path, value.unwrap_or(json!(defaults.appearance.mode)))?
        }
        "appearance.contrast" => {
            desktop.appearance.contrast =
                typed(path, value.unwrap_or(json!(defaults.appearance.contrast)))?
        }
        "appearance.style" => desktop.appearance.style = typed(path, value.unwrap_or(Value::Null))?,
        "appearance.decorations" => {
            desktop.appearance.decorations = typed(
                path,
                value.unwrap_or(json!(defaults.appearance.decorations)),
            )?
        }
        "appearance.caption_side" => {
            desktop.appearance.caption_side = typed(
                path,
                value.unwrap_or(json!(defaults.appearance.caption_side)),
            )?
        }
        "appearance.window_style" => {
            desktop.appearance.window_style = typed(
                path,
                value.unwrap_or(json!(defaults.appearance.window_style)),
            )?
        }
        "appearance.menu_style" => {
            desktop.appearance.menu_style =
                typed(path, value.unwrap_or(json!(defaults.appearance.menu_style)))?
        }
        "appearance.title_align" => {
            desktop.appearance.title_align = typed(
                path,
                value.unwrap_or(json!(defaults.appearance.title_align)),
            )?;
        }
        "appearance.actions_place" => {
            desktop.appearance.actions_place = typed(
                path,
                value.unwrap_or(json!(defaults.appearance.actions_place)),
            )?;
        }
        "appearance.actions_align" => {
            desktop.appearance.actions_align = typed(
                path,
                value.unwrap_or(json!(defaults.appearance.actions_align)),
            )?;
        }
        "appearance.menu_side" => {
            desktop.appearance.menu_side =
                typed(path, value.unwrap_or(json!(defaults.appearance.menu_side)))?
        }
        // Each type setting is optional: null (or a reset) clears it back to
        // the design's own value.
        "appearance.icon_stroke" => {
            desktop.appearance.icon_stroke = typed(path, value.unwrap_or(Value::Null))?
        }
        "appearance.font_family" => {
            desktop.appearance.font_family = typed(path, value.unwrap_or(Value::Null))?
        }
        "appearance.font_weight" => {
            desktop.appearance.font_weight = typed(path, value.unwrap_or(Value::Null))?
        }
        "appearance.font_size" => {
            desktop.appearance.font_size = typed(path, value.unwrap_or(Value::Null))?
        }
        "appearance.mono_family" => {
            desktop.appearance.mono_family = typed(path, value.unwrap_or(Value::Null))?
        }
        "appearance.source" => {
            desktop.appearance.source = typed(path, value.unwrap_or(Value::Null))?
        }
        "ui.density" => {
            desktop.ui.density = typed(path, value.unwrap_or(json!(defaults.ui.density)))?
        }
        "ui.text_scale" => {
            desktop.ui.text_scale = typed(path, value.unwrap_or(json!(defaults.ui.text_scale)))?
        }
        "ui.reduced_motion" => {
            desktop.ui.reduced_motion =
                typed(path, value.unwrap_or(json!(defaults.ui.reduced_motion)))?
        }
        "shell.page_order" => desktop.shell.page_order = typed(path, value.unwrap_or(json!([])))?,
        _ => {
            let parts: Vec<_> = path.split('.').collect();
            match parts.as_slice() {
                ["shell", "panels", id] => {
                    key(id, path)?;
                    if let Some(value) = value {
                        desktop
                            .shell
                            .panels
                            .insert((*id).into(), typed(path, value)?);
                    } else if let Some(panel) = defaults.shell.panels.get(*id) {
                        desktop.shell.panels.insert((*id).into(), panel.clone());
                    } else {
                        desktop.shell.panels.remove(*id);
                    }
                }
                ["shell", "panels", id, "thickness"] => {
                    let panel = desktop
                        .shell
                        .panels
                        .get_mut(*id)
                        .ok_or_else(|| error(path, "Unknown panel"))?;
                    let number =
                        typed::<f64>(path, value.unwrap_or(json!(Panel::default().thickness)))?;
                    panel.thickness = crate::model::integer_u32(number)
                        .ok_or_else(|| error(path, "Expected an integral u32 number"))?;
                }
                ["apps", id] => {
                    key(id, path)?;
                    if let Some(value) = value {
                        desktop.apps.insert((*id).into(), typed(path, value)?);
                    } else {
                        desktop.apps.remove(*id);
                    }
                }
                ["keymaps", app] => {
                    key(app, path)?;
                    match value {
                        Some(value) => {
                            let map: crate::keymap::AppKeymap = typed(path, value)?;
                            if map.is_empty() {
                                desktop.keymaps.remove(*app);
                            } else {
                                desktop.keymaps.insert((*app).into(), map);
                            }
                        }
                        None => {
                            desktop.keymaps.remove(*app);
                        }
                    }
                }
                // Action ids are dotted: everything after the app is the id.
                ["keymaps", app, action @ ..] => {
                    key(app, path)?;
                    let action = action.join(".");
                    match value {
                        Some(value) => {
                            let chords: Vec<String> = typed(path, value)?;
                            desktop
                                .keymaps
                                .entry((*app).into())
                                .or_default()
                                .insert(action, chords);
                        }
                        None => {
                            if let Some(map) = desktop.keymaps.get_mut(*app) {
                                map.remove(&action);
                                if map.is_empty() {
                                    desktop.keymaps.remove(*app);
                                }
                            }
                        }
                    }
                }
                ["terminal", field @ ..] => {
                    crate::terminal::set(&mut desktop.terminal, path, field, value)?
                }
                ["email", field @ ..] => crate::email::set(&mut desktop.email, path, field, value)?,
                ["ced", field @ ..] => {
                    crate::content::set_ced(&mut desktop.ced, path, field, value)?
                }
                _ => return Err(error(path, "Unknown field or scope")),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Format 4 refuses the derived scheme until S6, for the profile and
    /// for an app override alike.
    #[test]
    fn the_derived_scheme_is_not_writable_in_format_4() {
        let mut profile = Desktop::default();
        profile.appearance.scheme = "mixos".into();
        let mut app = Desktop::default();
        app.apps.insert(
            "term".into(),
            crate::AppOverride {
                scheme: Some("mixos".into()),
                ..Default::default()
            },
        );
        for (path, desktop) in [("appearance.scheme", profile), ("apps.term.scheme", app)] {
            let errors = resolve(&desktop).expect_err(path);
            assert!(
                errors
                    .iter()
                    .any(|e| e.path == path
                        && e.message.contains("not writable until store format 5")),
                "{path}: {errors:?}"
            );
        }
        let mut studio = Desktop::default();
        studio.appearance.scheme = "studio".into();
        assert!(resolve(&studio).is_ok());
    }

    #[test]
    fn every_concrete_schema_path_has_a_setter() {
        for path in describe()["fields"].as_object().unwrap().keys() {
            if !path.contains('<') {
                let next = patch(
                    &Desktop::default(),
                    &BTreeMap::new(),
                    std::slice::from_ref(path),
                )
                .unwrap_or_else(|error| panic!("{path}: {error:?}"));
                resolve(&next).unwrap();
            }
        }
    }

    #[test]
    fn interface_size_is_patched_summarised_and_reset() {
        let path = "appearance.font_size";
        let initial = Desktop::default();
        let sized = patch(&initial, &BTreeMap::from([(path.into(), json!(16.0))]), &[]).unwrap();
        let snapshot = Snapshot {
            schema: SCHEMA,
            binding: Binding {
                instance: "example".into(),
                profile: "default".into(),
            },
            incarnation: "test".into(),
            revision: Revision(2),
            design_revision: Revision(1),
            source_digest: source_digest(EMBEDDED_DEFAULT_SOURCE),
            effective: resolve(&sized).unwrap(),
            desktop: sized.clone(),
            values: Default::default(),
        };
        assert_eq!(Summary::of(&snapshot).appearance.font_size, Some(16.0));
        assert_eq!(snapshot.effective["desktop"].font_size, Some(16.0));
        assert_eq!(
            patch(&sized, &BTreeMap::new(), &[path.into()]).unwrap(),
            initial
        );
        assert_eq!(
            patch(&sized, &BTreeMap::from([(path.into(), Value::Null)]), &[]).unwrap(),
            initial
        );
        for size in [8.9, 18.1] {
            let bad = patch(&initial, &BTreeMap::from([(path.into(), json!(size))]), &[]).unwrap();
            assert_eq!(resolve(&bad).unwrap_err()[0].path, path);
        }
        assert!(
            patch(
                &initial,
                &BTreeMap::from([(path.into(), json!("large"))]),
                &[]
            )
            .is_err()
        );
    }
    #[test]
    fn shared_compiles_and_the_parse_cache_change_no_effective_byte() {
        let mut desktop = Desktop::default();
        desktop.appearance.scheme = "forest".into();
        desktop.apps.insert(
            "term".into(),
            AppOverride {
                scheme: Some("ocean".into()),
                ..Default::default()
            },
        );
        let (first, accents) =
            resolve_with_embedded_and_accents(&desktop, design::EMBEDDED_DEFAULT_SOURCE).unwrap();
        // Again, from the cache: the same bytes, digest and accents.
        let (again, accents_again) =
            resolve_with_embedded_and_accents(&desktop, design::EMBEDDED_DEFAULT_SOURCE).unwrap();
        assert_eq!(
            crate::digest(&first).unwrap(),
            crate::digest(&again).unwrap()
        );
        assert_eq!(accents, accents_again);
        // An overlaid app has its own scheme's design; the others share the
        // desktop's, each a full copy as before.
        assert_ne!(
            serde_json::to_string(&first["app:term"].design).unwrap(),
            serde_json::to_string(&first["desktop"].design).unwrap()
        );
        for app in APPS.iter().filter(|a| **a != "term") {
            assert_eq!(
                serde_json::to_string(&first[&format!("app:{app}")].design).unwrap(),
                serde_json::to_string(&first["desktop"].design).unwrap()
            );
        }
    }

    #[test]
    fn mix_whole_numbers_are_accepted_but_fractional_string_and_overflow_are_not() {
        let current = Desktop::default();
        for value in [json!(48), json!(48.0)] {
            let next = patch(
                &current,
                &BTreeMap::from([("shell.panels.bottom.thickness".into(), value.clone())]),
                &[],
            )
            .unwrap();
            assert_eq!(next.shell.panels["bottom"].thickness, 48);
            let record: Panel = serde_json::from_value(json!({"thickness":value})).unwrap();
            assert_eq!(record.thickness, 48);
        }
        for value in [json!(48.1), json!("48"), json!(-1), json!(4294967296u64)] {
            assert!(
                patch(
                    &current,
                    &BTreeMap::from([("shell.panels.bottom.thickness".into(), value.clone())]),
                    &[]
                )
                .is_err()
            );
            assert!(serde_json::from_value::<Panel>(json!({"thickness":value})).is_err());
        }
    }
    #[test]
    fn high_contrast_cannot_hide_invalid_authored_app_values() {
        let mut desktop = Desktop::default();
        desktop.appearance.contrast = "high".into();
        desktop.apps.insert(
            "term".into(),
            AppOverride {
                contrast: Some("typo".into()),
                ..Default::default()
            },
        );
        assert_eq!(resolve(&desktop).unwrap_err()[0].path, "apps.term.contrast");
        desktop.apps.get_mut("term").unwrap().contrast = Some("normal".into());
        assert_eq!(resolve(&desktop).unwrap()["app:term"].contrast, "high");
    }
    #[test]
    fn bounds_and_strict_source_fail_before_acceptance() {
        let mut desktop = Desktop::default();
        desktop.ui.density = f64::NAN;
        assert!(resolve(&desktop).is_err());
        desktop.ui.density = 1.0;
        desktop.appearance.source = Some("run_argv([\"do-not-execute\"])".into());
        assert!(resolve(&desktop).is_err());
        desktop.appearance.source = None;
        desktop.shell.page_order = vec!["same".into(), "same".into()];
        assert!(resolve(&desktop).is_err());
    }
    #[test]
    fn bad_batch_cannot_partially_mutate_and_unknown_fields_are_rejected() {
        let current = Desktop::default();
        assert!(
            patch(
                &current,
                &BTreeMap::from([
                    ("appearance.mode".into(), json!("light")),
                    ("bad.field".into(), json!(2))
                ]),
                &[]
            )
            .is_err()
        );
        assert_eq!(current.appearance.mode, "dark");
        assert!(serde_json::from_value::<Desktop>(json!({"typo":1})).is_err());
    }
    #[test]
    fn app_override_and_reset_do_not_change_other_apps() {
        let current = Desktop::default();
        let next = patch(
            &current,
            &BTreeMap::from([("apps.term".into(), json!({"mode":"light"}))]),
            &[],
        )
        .unwrap();
        let effective = resolve(&next).unwrap();
        assert_eq!(effective["app:term"].mode, "light");
        assert_eq!(effective["app:ced"].mode, "dark");
        assert_eq!(
            patch(&next, &BTreeMap::new(), &["apps.term".into()]).unwrap(),
            current
        );
    }

    #[test]
    fn window_style_resolves_retains_unknown_names_and_resets() {
        let original = Desktop::default();
        assert_eq!(resolve(&original).unwrap()["desktop"].window_style, "mac");
        for name in design::WindowStyle::NAMES
            .iter()
            .copied()
            .chain(["future-style"])
        {
            let changed = patch(
                &original,
                &BTreeMap::from([("appearance.window_style".into(), json!(name))]),
                &[],
            )
            .unwrap();
            let resolved = resolve(&changed).unwrap();
            assert!(
                resolved
                    .values()
                    .all(|effective| effective.window_style == name)
            );
            assert!(
                !resolved["desktop"]
                    .provenance
                    .contains_key("appearance.window_style")
            );
            let reset = patch(
                &changed,
                &BTreeMap::new(),
                &["appearance.window_style".into()],
            )
            .unwrap();
            assert_eq!(reset, original);
        }
        for value in [json!("bad name"), json!(""), json!(false)] {
            let result = patch(
                &original,
                &BTreeMap::from([("appearance.window_style".into(), value)]),
                &[],
            )
            .and_then(|desktop| {
                resolve(&desktop)
                    .map(|_| desktop)
                    .map_err(|errors| errors[0].clone())
            });
            assert_eq!(result.unwrap_err().path, "appearance.window_style");
        }
        assert_eq!(
            describe()["fields"]["appearance.window_style"]["known_values"],
            json!(design::WindowStyle::NAMES)
        );
    }

    #[test]
    fn menu_axes_resolve_validate_and_reset() {
        let original = Desktop::default();
        for (key, names) in [
            ("appearance.menu_style", design::MenuStyle::NAMES),
            ("appearance.menu_side", design::MenuSide::NAMES),
        ] {
            assert_eq!(describe()["fields"][key]["known_values"], json!(names));
            for name in names.iter().copied().chain(["future-menu"]) {
                let changed =
                    patch(&original, &BTreeMap::from([(key.into(), json!(name))]), &[]).unwrap();
                let resolved = resolve(&changed).unwrap();
                for effective in resolved.values() {
                    let value = if key.ends_with("style") {
                        &effective.menu_style
                    } else {
                        &effective.menu_side
                    };
                    assert_eq!(value, name);
                    assert!(!effective.provenance.contains_key(key));
                }
                assert_eq!(
                    patch(&changed, &BTreeMap::new(), &[key.into()]).unwrap(),
                    original
                );
            }
            for value in [
                json!("bad name"),
                json!(""),
                json!(false),
                json!("x".repeat(65)),
            ] {
                let result = patch(&original, &BTreeMap::from([(key.into(), value)]), &[])
                    .and_then(|desktop| {
                        resolve(&desktop)
                            .map(|_| desktop)
                            .map_err(|errors| errors[0].clone())
                    });
                assert_eq!(result.unwrap_err().path, key);
            }
        }
    }

    #[test]
    fn style_decorations_and_caption_side_default_resolve_and_validate() {
        let current = Desktop::default();
        assert_eq!(current.appearance.style, None, "the scheme's own style");
        assert_eq!(
            (
                current.appearance.decorations.as_str(),
                current.appearance.caption_side.as_str()
            ),
            ("auto", "auto")
        );
        let effective = resolve(&current).unwrap();
        assert_eq!(effective["desktop"].style, None);
        assert_eq!(effective["desktop"].decorations, "auto");
        let next = patch(
            &current,
            &BTreeMap::from([
                ("appearance.style".into(), json!("classic")),
                ("appearance.decorations".into(), json!("ssd")),
                ("appearance.caption_side".into(), json!("left")),
            ]),
            &[],
        )
        .unwrap();
        let effective = resolve(&next).unwrap();
        let desktop = &effective["desktop"];
        assert_eq!(
            (
                desktop.style.as_deref(),
                desktop.decorations.as_str(),
                desktop.caption_side.as_str()
            ),
            (Some("classic"), "ssd", "left")
        );
        assert_eq!(
            effective["app:ced"].style.as_deref(),
            Some("classic"),
            "every app context"
        );
        for (path, value) in [
            ("appearance.style", json!("neon")),
            ("appearance.decorations", json!("both")),
            ("appearance.caption_side", json!("bad name")),
        ] {
            let bad = patch(&current, &BTreeMap::from([(path.into(), value)]), &[]).unwrap();
            let errors = resolve(&bad).unwrap_err();
            assert_eq!(errors[0].path, path, "{errors:?}");
        }
        let reset = patch(
            &next,
            &BTreeMap::new(),
            &[
                "appearance.style".into(),
                "appearance.decorations".into(),
                "appearance.caption_side".into(),
            ],
        )
        .unwrap();
        assert_eq!(reset, current, "a reset restores each default");
        // A desktop saved before these keys loads with their defaults.
        let old: Desktop =
            serde_json::from_value(json!({"appearance":{"scheme":"forest","mode":"light"}}))
                .unwrap();
        assert_eq!(
            (old.appearance.style, old.appearance.decorations.as_str()),
            (None, "auto")
        );
    }

    #[test]
    fn decorations_schema_accepts_auto_and_refuses_unknown_values() {
        assert_eq!(
            describe()["fields"]["appearance.decorations"]["enum"],
            json!(["auto", "csd", "ssd"])
        );
        for name in ["auto", "csd", "ssd"] {
            let next = patch(
                &Desktop::default(),
                &BTreeMap::from([("appearance.decorations".into(), json!(name))]),
                &[],
            )
            .unwrap();
            assert_eq!(resolve(&next).unwrap()["desktop"].decorations, name);
        }
        let bad = patch(
            &Desktop::default(),
            &BTreeMap::from([("appearance.decorations".into(), json!("bogus"))]),
            &[],
        )
        .unwrap();
        let errors = resolve(&bad).unwrap_err();
        assert_eq!(errors[0].path, "appearance.decorations");
        assert_eq!(errors[0].code, "validation_failed");
        assert_eq!(errors[0].message, "Unknown decorations");
    }

    #[test]
    fn keymaps_apply_per_action_reset_and_leave_the_design_alone() {
        let current = Desktop::default();
        assert!(
            !serde_json::to_string(&current).unwrap().contains("keymaps"),
            "omitted while empty"
        );
        let next = patch(
            &current,
            &BTreeMap::from([
                ("keymaps.dopus.nav.parent".into(), json!(["Ctrl+U"])),
                ("keymaps.dopus.file.delete".into(), json!([])),
                ("keymaps.ced.file.save".into(), json!(["Ctrl+S", "F12"])),
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(next.keymaps["dopus"]["nav.parent"], ["Ctrl+U"]);
        assert!(next.keymaps["dopus"]["file.delete"].is_empty(), "unbound");
        assert_eq!(
            crate::digest(&resolve(&next).unwrap()).unwrap(),
            crate::digest(&resolve(&current).unwrap()).unwrap(),
            "bindings do not touch the effective design"
        );
        // Reset one action, then the rest of the app: the map empties away.
        let one = patch(&next, &BTreeMap::new(), &["keymaps.ced.file.save".into()]).unwrap();
        assert!(!one.keymaps.contains_key("ced"));
        let none = patch(&one, &BTreeMap::new(), &["keymaps.dopus".into()]).unwrap();
        assert_eq!(none, current);
        // The grammar is checked before acceptance.
        let bad = patch(
            &current,
            &BTreeMap::from([("keymaps.dopus.nav.back".into(), json!(["Hyper+X"]))]),
            &[],
        )
        .unwrap();
        assert_eq!(resolve(&bad).unwrap_err()[0].path, "keymaps.dopus.nav.back");
        for (path, value) in [
            ("keymaps.dopus.nav.back", json!("Ctrl+U")),
            ("keymaps.bad app.nav.back", json!(["F1"])),
        ] {
            assert!(patch(&current, &BTreeMap::from([(path.into(), value)]), &[]).is_err());
        }
        // The summary carries them.
        let summary = serde_json::to_value(Summary::of(&Snapshot {
            schema: SCHEMA,
            binding: Binding {
                instance: "i".into(),
                profile: "default".into(),
            },
            incarnation: "i".into(),
            revision: Revision(1),
            design_revision: Revision(1),
            source_digest: String::new(),
            desktop: next.clone(),
            effective: BTreeMap::new(),
            values: Default::default(),
        }))
        .unwrap();
        assert_eq!(summary["keymaps"]["dopus"]["nav.parent"], json!(["Ctrl+U"]));
    }
}
