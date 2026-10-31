// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Cross-key constraints (key space v2 §2.0), checked on every write. C1, C2
//! and C4 refuse nothing: they record when a key takes effect, so Prefs can
//! disable rows coherently, and their keys carry `used_when`. C7 is the
//! kind of `shell.panels.<panel>.output`. The rest are checked here over the
//! whole map, with defaults filled.
use super::{Values, effective};
use crate::model::Diagnostic;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// One registered constraint.
pub struct Constraint {
    pub id: &'static str,
    pub text: &'static str,
    /// None: recorded only, or enforced by the key's kind.
    pub check: Option<fn(&Values) -> Vec<Diagnostic>>,
}

pub const CONSTRAINTS: &[Constraint] = &[
    Constraint {
        id: "C1",
        text: "appearance.frosted.* flags are meaningful only with appearance.frosted.thickness",
        check: None,
    },
    Constraint {
        id: "C2",
        text: "window.focus_delay_ms is ignored unless window.focus_follows_cursor",
        check: None,
    },
    Constraint {
        id: "C3",
        text: "wallpaper.slideshow.enabled requires wallpaper.slideshow.folder",
        check: Some(slideshow),
    },
    Constraint {
        id: "C4",
        text: "wallpaper.outputs.<connector>.image is ignored while wallpaper.same_on_all",
        check: None,
    },
    Constraint {
        id: "C5",
        text: "displays.<a>.mirror_of = <b> requires b != a and displays.<b>.mirror_of null",
        check: Some(mirrors),
    },
    Constraint {
        id: "C6",
        text: "at least one known output stays enabled",
        check: Some(enabled_output),
    },
    Constraint {
        id: "C7",
        text: "shell.panels.<panel>.output names all or a connector (the key's kind)",
        check: None,
    },
    Constraint {
        id: "C8",
        text: "a chord binds at most one action of an app, and is listed once",
        check: Some(unique_chords),
    },
    Constraint {
        id: "panel_edges",
        text: "two enabled panels may not share an edge",
        check: Some(panel_edges),
    },
    Constraint {
        id: "user_keys",
        text: "keymaps.comp.* never binds a bare F1-F12: those keys are the user's",
        check: Some(user_keys),
    },
];

fn refuse(id: &str, path: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        "constraint_failed",
        path,
        format!("{id}: {}", message.into()),
    )
}

pub fn check(values: &Values) -> Vec<Diagnostic> {
    CONSTRAINTS
        .iter()
        .filter_map(|c| c.check)
        .flat_map(|check| check(values))
        .collect()
}

fn flag(values: &Values, path: &str) -> bool {
    effective(values, path).and_then(|v| v.as_bool()) == Some(true)
}

fn slideshow(values: &Values) -> Vec<Diagnostic> {
    let folder = values
        .get("wallpaper.slideshow.folder")
        .is_some_and(|v| !v.is_null());
    if flag(values, "wallpaper.slideshow.enabled") && !folder {
        vec![refuse(
            "C3",
            "wallpaper.slideshow.enabled",
            "a slideshow needs wallpaper.slideshow.folder",
        )]
    } else {
        Vec::new()
    }
}

/// The connectors named by any stored `displays.<connector>.*` key.
fn known_outputs(values: &Values) -> BTreeSet<&str> {
    values
        .keys()
        .filter_map(|k| k.strip_prefix("displays."))
        .filter_map(|rest| rest.split('.').next())
        .collect()
}

fn mirrors(values: &Values) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    for output in known_outputs(values) {
        let path = format!("displays.{output}.mirror_of");
        let Some(target) = values.get(&path).and_then(Value::as_str) else {
            continue;
        };
        if target == output {
            errors.push(refuse("C5", &path, "an output cannot mirror itself"));
        } else if values
            .get(&format!("displays.{target}.mirror_of"))
            .is_some_and(|v| !v.is_null())
        {
            errors.push(refuse(
                "C5",
                &path,
                format!("{target} mirrors another output: no chains"),
            ));
        }
    }
    errors
}

fn enabled_output(values: &Values) -> Vec<Diagnostic> {
    let known = known_outputs(values);
    if known.is_empty()
        || known
            .iter()
            .any(|o| flag(values, &format!("displays.{o}.enabled")))
    {
        return Vec::new();
    }
    let first = known.first().copied().unwrap_or_default();
    vec![refuse(
        "C6",
        &format!("displays.{first}.enabled"),
        "at least one output must stay enabled",
    )]
}

/// Each stored binding of a keymap app: (chord, path), with spawn entries'
/// bindings counted as `comp`'s.
fn bindings(values: &Values) -> BTreeMap<&str, Vec<(&str, &str)>> {
    let mut apps: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (path, value) in values {
        let Some(rest) = path.strip_prefix("keymaps.") else {
            continue;
        };
        let Some((app, action)) = rest.split_once('.') else {
            continue;
        };
        let chords = if app == "comp" && action.starts_with("spawn.") {
            value.get("bindings")
        } else {
            Some(value)
        };
        for chord in chords.and_then(Value::as_array).into_iter().flatten() {
            if let Some(chord) = chord.as_str() {
                apps.entry(app).or_default().push((chord, path.as_str()));
            }
        }
    }
    apps
}

fn unique_chords(values: &Values) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    for (app, list) in bindings(values) {
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for (chord, path) in list {
            match seen.get(chord) {
                Some(first) if *first != path => errors.push(refuse(
                    "C8",
                    path,
                    format!("{chord:?} already binds {first} in {app}"),
                )),
                _ => {
                    seen.insert(chord, path);
                }
            }
        }
    }
    errors
}

fn panel_edges(values: &Values) -> Vec<Diagnostic> {
    let edge = |id: &str| effective(values, &format!("shell.panels.{id}.edge"));
    let on = |id: &str| flag(values, &format!("shell.panels.{id}.enabled"));
    if on("top") && on("bottom") && edge("top") == edge("bottom") {
        vec![refuse(
            "panel_edges",
            "shell.panels.top.edge",
            "the top and bottom panels share an edge",
        )]
    } else {
        Vec::new()
    }
}

/// A bare function key: `F1` to `F12` with no modifier.
pub fn bare_function_key(chord: &str) -> bool {
    chord
        .strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=12).contains(&n) && !chord[1..].starts_with('0'))
}

/// The HARD RULE as validation: no exporter can ever write a bare function
/// key into the compositor's bindings. Right-modifier chords cannot be
/// written at all: the grammar has no sided modifiers.
fn user_keys(values: &Values) -> Vec<Diagnostic> {
    bindings(values)
        .get("comp")
        .into_iter()
        .flatten()
        .filter(|(chord, _)| bare_function_key(chord))
        .map(|(chord, path)| {
            refuse(
                "user_keys",
                path,
                format!("{chord} is the user's key: bind it with a modifier"),
            )
        })
        .collect()
}
