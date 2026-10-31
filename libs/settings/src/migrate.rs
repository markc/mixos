// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The pure migration from format 4 to key space v2 (design §4.1, §4.2).
//!
//! [`migrate`] turns the format-4 [`Desktop`] record, with its terminal,
//! email and ced domains, into v2 [`Values`]. [`migrate_backdrop`] turns the
//! parsed `backdrop.mx` document into the `wallpaper.*` values. Each returns
//! what it dropped or changed in a [`MigrationReport`]. Neither does I/O or
//! reads a clock or store: the store format bump and the receipt are S6.
//!
//! Values are stored the way [`registry`] stores them: normalised, and never
//! at their default. The whole map is validated, but the write-only C8 (a
//! chord bound twice) is reported rather than refused, as §2.0 asks.
use crate::registry::{self, Values, constraint};
use crate::{AppOverride, Appearance, CommonUi, Desktop, Diagnostic, Shell};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A value the migration did not carry over as it was, or at all.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The format-4 or backdrop path the value came from.
    pub from: String,
    /// The value as it was.
    pub old: Value,
    /// The v2 path it was stored at, if any.
    pub to: Option<String>,
    /// Why it was dropped or changed.
    pub reason: String,
}

/// What one migration dropped or changed: the migration receipt's content.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MigrationReport {
    /// Values v2 has no key for.
    pub dropped: Vec<Entry>,
    /// Values stored at a different value: off-step densities and snapped
    /// panel thicknesses.
    pub changed: Vec<Entry>,
    /// Write-only constraints the migrated map breaks (C8). Reported, not
    /// refused.
    pub conflicts: Vec<Diagnostic>,
}

/// The migrated v2 values and their report.
#[derive(Clone, Debug, PartialEq)]
pub struct Migrated {
    pub values: Values,
    pub report: MigrationReport,
}

/// The v2 panel thickness steps (`shell.panels.<panel>.thickness`).
const PANEL_STEPS: [u32; 5] = [32, 40, 56, 64, 80];

const PER_OUTPUT_COLOUR: &str = "a per-output colour has no v2 key (F10)";
const PER_OUTPUT_FIT: &str = "a per-output fit has no v2 key (F10)";
const PER_OUTPUT_SLIDESHOW: &str = "a per-output slideshow has no v2 key (F10)";
const NO_FALLBACK: &str = "a fallback colour has no v2 key";
const NO_IMAGE_LIST: &str = "a slideshow image list has no v2 key: the folder form is kept";

/// The backdrop document's schema, the only one this build reads.
const BACKDROP_SCHEMA: u32 = 1;

/// Collects one migration's values. Errors are gathered, so every bad value
/// is reported at once.
#[derive(Default)]
struct Builder {
    values: Values,
    report: MigrationReport,
    errors: Vec<Diagnostic>,
}

impl Builder {
    /// Store `value` at `path` if the registry accepts it and it is not the
    /// key's default. A null is no value.
    fn set(&mut self, path: &str, value: Value) {
        if value.is_null() {
            return;
        }
        match registry::check_value(path, &value) {
            Ok(value) => {
                let default = registry::lookup(path).ok().and_then(|m| m.default_value());
                if default.as_ref() != Some(&value) {
                    self.values.insert(path.to_owned(), value);
                }
            }
            Err(error) => self.errors.push(error),
        }
    }

    fn dropped(&mut self, from: &str, old: Value, reason: &str) {
        self.report.dropped.push(Entry {
            from: from.to_owned(),
            old,
            to: None,
            reason: reason.to_owned(),
        });
    }

    fn changed(&mut self, from: &str, old: Value, to: &str, reason: String) {
        self.report.changed.push(Entry {
            from: from.to_owned(),
            old,
            to: Some(to.to_owned()),
            reason,
        });
    }

    fn finish(mut self) -> Result<Migrated, Vec<Diagnostic>> {
        if !self.errors.is_empty() {
            return Err(self.errors);
        }
        registry::validate(&self.values)?;
        self.report.conflicts = constraint::conflicts(&self.values);
        Ok(Migrated {
            values: self.values,
            report: self.report,
        })
    }
}

/// Migrate the format-4 record, and its three app domains, to v2 values.
pub fn migrate(desktop: &Desktop) -> Result<Migrated, Vec<Diagnostic>> {
    let mut b = Builder::default();
    appearance(&mut b, &desktop.appearance);
    common(&mut b, &desktop.ui);
    shell(&mut b, &desktop.shell);
    apps(&mut b, &desktop.apps);
    for (app, actions) in &desktop.keymaps {
        for (action, chords) in actions {
            b.set(&format!("keymaps.{app}.{action}"), json!(chords));
        }
    }
    domain(&mut b, "terminal", &desktop.terminal);
    domain(&mut b, "email", &desktop.email);
    domain(&mut b, "ced", &desktop.ced);
    b.finish()
}

fn appearance(b: &mut Builder, appearance: &Appearance) {
    b.set("appearance.scheme", json!(appearance.scheme));
    b.set("appearance.style", json!(appearance.style));
    b.set("appearance.mode", json!(appearance.mode));
    b.set("appearance.contrast", json!(appearance.contrast));
    // Radii now come from roundness: a named style maps by its own name, a
    // null style by the scheme's own (F2).
    let style = appearance
        .style
        .as_deref()
        .or_else(|| scheme_style(&appearance.scheme));
    if let Some(roundness) = style.and_then(roundness) {
        b.set("appearance.roundness", json!(roundness));
    }
    b.set("window.decorations", json!(appearance.decorations));
    b.set("window.caption_side", json!(appearance.caption_side));
    b.set("window.style", json!(appearance.window_style));
    b.set("window.menu_style", json!(appearance.menu_style));
    b.set("window.menu_side", json!(appearance.menu_side));
    b.set("window.actions_place", json!(appearance.actions_place));
    b.set("window.actions_align", json!(appearance.actions_align));
    b.set("window.title_align", json!(appearance.title_align));
    b.set("appearance.icon_stroke", json!(appearance.icon_stroke));
    b.set("appearance.font_family", json!(appearance.font_family));
    b.set("appearance.font_weight", json!(appearance.font_weight));
    b.set("appearance.font_size", json!(appearance.font_size));
    b.set("appearance.mono_family", json!(appearance.mono_family));
    b.set("appearance.source", json!(appearance.source));
}

/// The style a scheme draws in when the profile names none: the design's
/// `style.schemes` binding. `mixos` compiles as studio (`look_context`).
fn scheme_style(scheme: &str) -> Option<&'static str> {
    Some(match scheme {
        "ocean" | "crimson" | "stone" | "forest" | "sunset" | "mono" => "plain",
        "pro" => "pro",
        "studio" | "mixos" => "studio",
        "classic" => "classic",
        "breeze" => "breeze",
        "adwaita" => "adwaita",
        _ => return None,
    })
}

/// The corner preset of a style (F2): `square` is zero on every radius token.
fn roundness(style: &str) -> Option<&'static str> {
    Some(match style {
        "studio" | "adwaita" => "round",
        "breeze" | "pro" => "slightly_round",
        "classic" | "plain" => "square",
        _ => return None,
    })
}

fn common(b: &mut Builder, ui: &CommonUi) {
    density(b, ui.density);
    b.set("accessibility.text_scale", json!(ui.text_scale));
    b.set("accessibility.reduced_motion", json!(ui.reduced_motion));
}

/// `ui.density` to its named step. The three steps map exactly and are not
/// reported. Any other value maps by the midpoints (below 0.925 is compact,
/// up to and including 1.1 comfortable, above that spacious) and is reported.
fn density(b: &mut Builder, density: f64) {
    let name = if density < 0.925 {
        "compact"
    } else if density <= 1.1 {
        "comfortable"
    } else {
        "spacious"
    };
    if ![0.85, 1.0, 1.2].contains(&density) {
        b.changed(
            "ui.density",
            json!(density),
            "appearance.density",
            format!("off-step density {density} mapped to {name} by the midpoints"),
        );
    }
    b.set("appearance.density", json!(name));
}

fn shell(b: &mut Builder, shell: &Shell) {
    if !shell.page_order.is_empty() {
        b.dropped(
            "shell.page_order",
            json!(shell.page_order),
            "retired: Prefs-internal, COSMIC's order is fixed",
        );
    }
    for (id, panel) in &shell.panels {
        let root = format!("shell.panels.{id}");
        if !registry::PANELS.contains(&id.as_str()) {
            b.dropped(&root, json!(panel), "only the top and bottom panels exist");
            continue;
        }
        b.set(&format!("{root}.edge"), json!(panel.edge));
        // `hidden` is `dock` with autohide on; a default is not stored.
        let (mode, autohide) = match panel.mode.as_str() {
            "dock" => ("dock", false),
            "overlay" => ("overlay", false),
            "hidden" => ("dock", true),
            other => {
                b.errors.push(Diagnostic::new(
                    "validation_failed",
                    &format!("{root}.mode"),
                    format!("Unknown panel mode {other:?}"),
                ));
                continue;
            }
        };
        b.set(&format!("{root}.mode"), json!(mode));
        b.set(&format!("{root}.autohide"), json!(autohide));
        let snapped = snap_thickness(panel.thickness);
        if snapped != panel.thickness {
            b.changed(
                &format!("{root}.thickness"),
                json!(panel.thickness),
                &format!("{root}.thickness"),
                format!(
                    "thickness {} snapped to {snapped} (nearest step, ties up)",
                    panel.thickness
                ),
            );
        }
        b.set(&format!("{root}.thickness"), json!(snapped));
    }
}

/// The nearest v2 thickness step. A tie goes up: the steps are read from the
/// lowest, and a step at the same distance replaces the one before it.
fn snap_thickness(thickness: u32) -> u32 {
    let mut best = PANEL_STEPS[0];
    for step in PANEL_STEPS {
        if thickness.abs_diff(step) <= thickness.abs_diff(best) {
            best = step;
        }
    }
    best
}

fn apps(b: &mut Builder, apps: &BTreeMap<String, AppOverride>) {
    for (id, look) in apps {
        let root = format!("apps.{id}");
        b.set(&format!("{root}.scheme"), json!(look.scheme));
        b.set(&format!("{root}.mode"), json!(look.mode));
        b.set(&format!("{root}.contrast"), json!(look.contrast));
        b.set(&format!("{root}.text_scale"), json!(look.text_scale));
    }
}

/// An app domain's leaves, named as the module serialises them. The modules
/// omit their defaults, so every leaf that is present is a stored value.
fn domain<T: Serialize>(b: &mut Builder, root: &str, record: &T) {
    match serde_json::to_value(record) {
        Ok(value) => leaves(b, root, &value),
        Err(error) => b.errors.push(Diagnostic::new(
            "validation_failed",
            root,
            error.to_string(),
        )),
    }
}

fn leaves(b: &mut Builder, path: &str, value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                leaves(b, &format!("{path}.{key}"), child);
            }
        }
        leaf => b.set(path, leaf.clone()),
    }
}

/// The `backdrop.mx` document (`schema: 1`), as parsed, to `wallpaper.*`
/// values. The file is not in the store; S6 imports it once.
pub fn migrate_backdrop(document: &Value) -> Result<Migrated, Vec<Diagnostic>> {
    let file = BackdropFile::deserialize(document).map_err(|error| {
        vec![Diagnostic::new(
            "validation_failed",
            "backdrop",
            error.to_string(),
        )]
    })?;
    if file.schema != BACKDROP_SCHEMA {
        return Err(vec![Diagnostic::new(
            "validation_failed",
            "backdrop.schema",
            format!("Expected schema {BACKDROP_SCHEMA}"),
        )]);
    }
    let mut b = Builder::default();
    if let Some(fill) = &file.default {
        default_fill(&mut b, fill);
    }
    let mut any_image = false;
    for (output, fill) in &file.outputs {
        let root = format!("backdrop.outputs.{output}");
        if let Some(image) = &fill.image {
            b.set(&format!("wallpaper.outputs.{output}.image"), json!(image));
            any_image = true;
        }
        drop_if(&mut b, &format!("{root}.colour"), &fill.colour, PER_OUTPUT_COLOUR);
        drop_if(&mut b, &format!("{root}.fit"), &fill.fit, PER_OUTPUT_FIT);
        drop_if(&mut b, &format!("{root}.fallback"), &fill.fallback, NO_FALLBACK);
        drop_if(
            &mut b,
            &format!("{root}.slideshow"),
            &fill.slideshow,
            PER_OUTPUT_SLIDESHOW,
        );
    }
    // Per-output images need `same_on_all` off (C4).
    if any_image {
        b.set("wallpaper.same_on_all", json!(false));
    }
    b.finish()
}

/// The default block: colour, image, fit and slideshow map; the fallback is
/// dropped.
fn default_fill(b: &mut Builder, fill: &BackdropFill) {
    if let Some(colour) = &fill.colour {
        b.set("wallpaper.color", json!(colour));
    }
    if let Some(image) = &fill.image {
        b.set("wallpaper.image", json!(image));
    }
    drop_if(b, "backdrop.default.fallback", &fill.fallback, NO_FALLBACK);
    if let Some(fit) = fill.fit {
        match fit.v2_name() {
            Some(name) => b.set("wallpaper.fit", json!(name)),
            None => b.dropped(
                "backdrop.default.fit",
                json!(fit),
                "centre and tile have no v2 fit (fill, fit, stretch)",
            ),
        }
    }
    if let Some(show) = &fill.slideshow {
        slideshow(b, show);
    }
}

fn slideshow(b: &mut Builder, show: &BackdropSlideshow) {
    match (&show.folder, &show.images) {
        (Some(folder), images) => {
            b.set("wallpaper.slideshow.folder", json!(folder));
            b.set("wallpaper.slideshow.enabled", json!(true));
            b.set("wallpaper.slideshow.interval_s", json!(show.interval_s));
            b.set("wallpaper.slideshow.shuffle", json!(show.shuffle));
            if images.is_some() {
                b.dropped(
                    "backdrop.default.slideshow.images",
                    json!(images),
                    NO_IMAGE_LIST,
                );
            }
        }
        (None, Some(_)) => b.dropped(
            "backdrop.default.slideshow",
            json!(show),
            "a slideshow of images has no v2 key without a folder",
        ),
        (None, None) => b.errors.push(Diagnostic::new(
            "validation_failed",
            "backdrop.default.slideshow",
            "A slideshow needs a folder or images",
        )),
    }
}

/// Reports a field that is present and has no v2 key.
fn drop_if<T: Serialize>(b: &mut Builder, from: &str, value: &Option<T>, reason: &str) {
    if let Some(value) = value {
        b.dropped(from, json!(value), reason);
    }
}

/// The parsed document, as `backdrop.mx` writes it. Unknown keys are refused.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackdropFile {
    schema: u32,
    #[serde(default)]
    default: Option<BackdropFill>,
    #[serde(default)]
    outputs: BTreeMap<String, BackdropFill>,
}

/// One wallpaper as written.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BackdropFill {
    #[serde(default)]
    colour: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    fallback: Option<String>,
    #[serde(default)]
    fit: Option<BackdropFit>,
    #[serde(default)]
    slideshow: Option<BackdropSlideshow>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum BackdropFit {
    Fill,
    Fit,
    Centre,
    Tile,
    Stretch,
}

impl BackdropFit {
    /// The v2 `wallpaper.fit` name, where v2 has one.
    fn v2_name(self) -> Option<&'static str> {
        match self {
            Self::Fill => Some("fill"),
            Self::Fit => Some("fit"),
            Self::Stretch => Some("stretch"),
            Self::Centre | Self::Tile => None,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BackdropSlideshow {
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    images: Option<Vec<String>>,
    #[serde(default = "default_interval")]
    interval_s: u64,
    #[serde(default)]
    shuffle: bool,
}

fn default_interval() -> u64 {
    600
}

#[cfg(test)]
mod tests;
