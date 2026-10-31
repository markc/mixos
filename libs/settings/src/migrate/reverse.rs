// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The reverse of [`super::migrate`]: v2 values back to the format-4
//! [`Desktop`] (design §4.4 step 1).
//!
//! `settingsd downgrade` takes the record only when nothing is
//! [`Reversed::blocking`]; until S7 moves the snapshot to `values`, the
//! snapshot's `desktop` is this projection whatever it blocks. The rule is
//! the round trip: a value has a format-4 home exactly when migrating the
//! reversed record gives it back. So every key only v2 has (the accent, the
//! tints, `wallpaper.*`, `window.gaps`), a roundness the style would not
//! give, `mode: auto` and the derived `mixos` scheme all block.
use super::migrate;
use crate::registry::{self, Values};
use crate::resolve::values::effective_mode;
use crate::view::{State, View};
use crate::{AppOverride, Desktop, Panel};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// The format-4 record of some v2 values, and what it could not hold.
#[derive(Clone, Debug, PartialEq)]
pub struct Reversed {
    pub desktop: Desktop,
    /// The v2 keys format 4 cannot hold as they are, sorted. A downgrade by
    /// reverse conversion refuses while any is listed.
    pub blocking: Vec<String>,
}

/// The app domains, whose leaves go back into their own records.
const DOMAINS: [&str; 3] = ["terminal", "email", "ced"];

/// The v2 keys of the window framing, by their format-4 field.
const FRAMING: [&str; 8] = [
    "decorations",
    "caption_side",
    "style",
    "menu_style",
    "menu_side",
    "actions_place",
    "actions_align",
    "title_align",
];

/// The appearance keys format 4 holds under the same name.
const SAME_NAME: [&str; 10] = [
    "scheme",
    "style",
    "mode",
    "contrast",
    "icon_stroke",
    "font_family",
    "font_weight",
    "font_size",
    "mono_family",
    "source",
];

const PANEL_FIELDS: [&str; 5] = ["enabled", "edge", "mode", "autohide", "thickness"];

pub fn reverse(values: &Values) -> Reversed {
    let value = |path: &str| registry::effective(values, path).unwrap_or(Value::Null);
    let text = |path: &str| value(path).as_str().unwrap_or_default().to_owned();
    let authored_text = |path: &str| values.get(path).and_then(Value::as_str).map(str::to_owned);
    let authored_number = |path: &str| values.get(path).and_then(Value::as_f64);
    let mut blocking = BTreeSet::new();
    let mut handled = BTreeSet::new();
    let mut desktop = Desktop::default();

    let a = &mut desktop.appearance;
    a.scheme = text("appearance.scheme");
    if a.scheme == "mixos" {
        blocking.insert("appearance.scheme".to_owned());
    }
    a.style = authored_text("appearance.style");
    a.mode = text("appearance.mode");
    if a.mode == "auto" {
        blocking.insert("appearance.mode".to_owned());
        a.mode = effective_mode("auto", &State::of(values))
            .map_or("dark", |(mode, _)| mode.name())
            .to_owned();
    }
    a.contrast = text("appearance.contrast");
    a.icon_stroke = authored_number("appearance.icon_stroke");
    a.font_family = authored_text("appearance.font_family");
    // A whole number, stored as an integer or a float.
    a.font_weight = values
        .get("appearance.font_weight")
        .and_then(Value::as_f64)
        .filter(|w| w.fract() == 0.0 && (1.0..=f64::from(u16::MAX)).contains(w))
        .map(|w| w as u16);
    a.font_size = authored_number("appearance.font_size");
    a.mono_family = authored_text("appearance.mono_family");
    a.source = authored_text("appearance.source");
    for name in SAME_NAME {
        handled.insert(format!("appearance.{name}"));
    }
    // Roundness is the style's: the round trip checks it agrees.
    handled.insert("appearance.roundness".to_owned());
    let [
        decorations,
        caption_side,
        style,
        menu_style,
        menu_side,
        place,
        align,
        title,
    ] = FRAMING.map(|name| {
        handled.insert(format!("window.{name}"));
        text(&format!("window.{name}"))
    });
    a.decorations = decorations;
    a.caption_side = caption_side;
    a.window_style = style;
    a.menu_style = menu_style;
    a.menu_side = menu_side;
    a.actions_place = place;
    a.actions_align = align;
    a.title_align = title;

    desktop.ui.density = match text("appearance.density").as_str() {
        "compact" => 0.85,
        "spacious" => 1.2,
        _ => 1.0,
    };
    desktop.ui.text_scale = value("accessibility.text_scale").as_f64().unwrap_or(1.0);
    desktop.ui.reduced_motion = value("accessibility.reduced_motion")
        .as_bool()
        .unwrap_or_default();
    handled.extend(
        [
            "appearance.density",
            "accessibility.text_scale",
            "accessibility.reduced_motion",
        ]
        .map(str::to_owned),
    );

    desktop.shell.panels.clear();
    for id in registry::PANELS {
        let field = |name: &str| format!("shell.panels.{id}.{name}");
        handled.extend(PANEL_FIELDS.map(&field));
        if !value(&field("enabled")).as_bool().unwrap_or_default() {
            continue;
        }
        let mut mode = text(&field("mode"));
        // Format 4 hides a panel as the mode `hidden`, which docks.
        if value(&field("autohide")).as_bool() == Some(true) {
            if mode == "dock" {
                mode = "hidden".to_owned();
            } else {
                blocking.insert(field("autohide"));
            }
        }
        let thickness = value(&field("thickness")).as_u64().unwrap_or(40);
        desktop.shell.panels.insert(
            (*id).to_owned(),
            Panel {
                edge: text(&field("edge")),
                mode,
                thickness: u32::try_from(thickness).unwrap_or(40),
            },
        );
    }

    for (path, stored) in values {
        let mut parts = path.splitn(3, '.');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("apps"), Some(app), Some(field)) => {
                let look = desktop.apps.entry(app.to_owned()).or_default();
                let named = || stored.as_str().map(str::to_owned);
                match field {
                    "scheme" => look.scheme = named(),
                    "mode" => look.mode = named(),
                    "contrast" => look.contrast = named(),
                    "text_scale" => look.text_scale = stored.as_f64(),
                    _ => continue,
                }
                handled.insert(path.clone());
            }
            (Some("keymaps"), Some(app), Some(action)) if app != "comp" => {
                let Some(chords) = stored.as_array().and_then(|chords| {
                    chords
                        .iter()
                        .map(|chord| chord.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                }) else {
                    continue;
                };
                desktop
                    .keymaps
                    .entry(app.to_owned())
                    .or_default()
                    .insert(action.to_owned(), chords);
                handled.insert(path.clone());
            }
            _ => {}
        }
    }
    desktop
        .apps
        .retain(|_, look| *look != AppOverride::default());

    for root in DOMAINS {
        let under = format!("{root}.");
        let leaves: Vec<(&String, &Value)> = values
            .iter()
            .filter(|(path, _)| path.starts_with(&under))
            .collect();
        let mut record = Map::new();
        for (path, leaf) in &leaves {
            insert_leaf(&mut record, &path[under.len()..], (*leaf).clone());
        }
        let record = Value::Object(record);
        let set = match root {
            "terminal" => serde_json::from_value(record).map(|t| desktop.terminal = t),
            "email" => serde_json::from_value(record).map(|e| desktop.email = e),
            _ => serde_json::from_value(record).map(|c| desktop.ced = c),
        };
        if set.is_ok() {
            handled.extend(leaves.iter().map(|(path, _)| (*path).clone()));
        }
    }

    blocking.extend(values.keys().filter(|k| !handled.contains(*k)).cloned());
    // The round trip: every value the reversed record does not give back.
    match migrate(&desktop) {
        Ok(back) => {
            let keys: BTreeSet<&String> = values.keys().chain(back.values.keys()).collect();
            blocking.extend(
                keys.into_iter()
                    .filter(|k| values.get(*k).map(loose) != back.values.get(*k).map(loose))
                    .cloned(),
            );
        }
        Err(failed) => blocking.extend(failed.errors.into_iter().map(|e| e.path)),
    }
    Reversed {
        desktop,
        blocking: blocking.into_iter().collect(),
    }
}

/// `value` with every number a float: a stored whole number and the same
/// number as a float compare equal.
fn loose(value: &Value) -> Value {
    match value {
        Value::Number(n) => n
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        Value::Array(items) => Value::Array(items.iter().map(loose).collect()),
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), loose(v))).collect())
        }
        other => other.clone(),
    }
}

/// `value` at the dotted `path` inside `record`, making the objects between.
fn insert_leaf(record: &mut Map<String, Value>, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            record.insert(path.to_owned(), value);
        }
        Some((head, rest)) => {
            let child = record
                .entry(head.to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(child) = child {
                insert_leaf(child, rest, value);
            }
        }
    }
}
