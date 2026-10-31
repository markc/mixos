// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The contract 1.0.0 topics (key space v2 §5.1): retained, exact names,
//! node-local. One `keys` topic per prefix, plus the `look` and `snapshot`
//! topics. A consumer subscribes to the prefixes it reads and is woken only
//! when one of them changed.
//!
//! A `keys` body carries every key under its prefix with the defaults
//! filled ([`prefix_values`]) and, for `appearance`, the resolved mode and
//! accent beside the authored choices ([`Resolved`]). Live keys are in no
//! topic: their owners publish them.
use crate::model::{Binding, Revision};
use crate::registry::{self, Class, DefaultValue, Values};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The schema of every 1.0.0 topic body.
pub const TOPIC_SCHEMA: u32 = 3;

pub const KEYS_TOPIC_PREFIX: &str = "settingsd.desktop.keys.";
pub const LOOK_TOPIC_PREFIX: &str = "settingsd.desktop.look.";
pub const SNAPSHOT_TOPIC_PREFIX: &str = "settingsd.desktop.snapshot.";

/// The prefixes with a topic of their own, one per Prefs page.
pub const PREFIXES: [&str; 22] = [
    "wallpaper",
    "appearance",
    "window",
    "shell.panels",
    "workspaces",
    "displays",
    "sound",
    "power",
    "input",
    "keymaps",
    "apps_default",
    "apps_startup",
    "xwayland",
    "time",
    "region",
    "accessibility",
    "terminal",
    "email",
    "ced",
    "apps",
    "tune",
    "state",
];

/// `settingsd.desktop.keys.<profile>.<prefix>`.
pub fn keys_topic(profile: &str, prefix: &str) -> String {
    format!("{KEYS_TOPIC_PREFIX}{profile}.{prefix}")
}

/// `settingsd.desktop.look.<profile>`: the follower view.
pub fn look_topic(profile: &str) -> String {
    format!("{LOOK_TOPIC_PREFIX}{profile}")
}

/// `settingsd.desktop.snapshot.<profile>`: the schema 3 compact snapshot.
pub fn snapshot_topic(profile: &str) -> String {
    format!("{SNAPSHOT_TOPIC_PREFIX}{profile}")
}

/// The topic prefix a key belongs to; None for a key in no topic.
pub fn prefix_of(path: &str) -> Option<&'static str> {
    PREFIXES
        .iter()
        .filter(|p| path == **p || path.strip_prefix(**p).is_some_and(|r| r.starts_with('.')))
        .max_by_key(|p| p.len())
        .copied()
}

/// The prefixes whose authored values differ between two records. This is
/// not what moves a topic: the resolved mode reads `state`, so a `state.*`
/// change also moves `appearance` and the look. Publication and the
/// validate preview compare whole contents ([`contents`], [`look_content`]).
pub fn changed_prefixes(before: &Values, after: &Values) -> BTreeSet<&'static str> {
    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter(|k| before.get(*k) != after.get(*k))
        .filter_map(|k| prefix_of(k))
        .collect()
}

/// A key's value for a topic: authored, else the registry default, else the
/// app domain module's default, else null (an owner-supplied default never
/// authored: the owner knows it, settingsd does not).
fn filled(values: &Values, m: &registry::Matched, path: &str) -> Value {
    if let Some(value) = values.get(path) {
        return value.clone();
    }
    if let Some(value) = m.default_value() {
        return value;
    }
    if matches!(m.def.default, DefaultValue::Domain) {
        return registry::domain_default(path).unwrap_or(Value::Null);
    }
    Value::Null
}

/// A placeholder whose members the registry knows, so every member is
/// listed with its defaults; an open one lists only the members authored.
fn closed_members(placeholder: &str) -> Option<&'static [&'static str]> {
    match placeholder {
        "<panel>" => Some(registry::PANELS),
        "<category>" => Some(registry::CATEGORIES),
        _ => None,
    }
}

/// The concrete paths of one entry: its path when it has no placeholder;
/// every member of a closed placeholder; for an open one, the members some
/// authored key of the same family names (`displays.<connector>`), each
/// with all its keys.
fn concrete(def: &'static registry::KeyDef, values: &Values) -> Vec<String> {
    let parts: Vec<&str> = def.path.split('.').collect();
    let Some(last) = parts.iter().rposition(|p| p.starts_with('<')) else {
        return vec![def.path.to_owned()];
    };
    let family = &parts[..=last];
    let mut out = BTreeSet::new();
    if family
        .iter()
        .filter(|p| p.starts_with('<'))
        .all(|p| closed_members(p).is_some())
    {
        let mut paths = vec![String::new()];
        for part in &parts {
            let choices: Vec<&str> =
                closed_members(part).map_or_else(|| vec![*part], <[_]>::to_vec);
            paths = paths
                .iter()
                .flat_map(|head| {
                    choices.iter().map(move |c| {
                        if head.is_empty() {
                            (*c).to_owned()
                        } else {
                            format!("{head}.{c}")
                        }
                    })
                })
                .collect();
        }
        out.extend(paths);
    } else {
        for key in values.keys() {
            let Ok(m) = registry::lookup(key) else {
                continue;
            };
            let theirs: Vec<&str> = m.def.path.split('.').collect();
            if theirs.len() <= last || theirs[..=last] != *family {
                continue;
            }
            // `<action>` takes the rest of the path, so it is always the
            // last part: the family is the whole key.
            if parts[last] == "<action>" {
                if m.def.path == def.path {
                    out.insert(key.clone());
                }
                continue;
            }
            let segments: Vec<&str> = key.split('.').collect();
            let mut path: Vec<&str> = segments[..=last].to_vec();
            path.extend_from_slice(&parts[last + 1..]);
            out.insert(path.join("."));
        }
    }
    out.into_iter().collect()
}

/// Every stored key under `prefix`, with its defaults filled. Live entries
/// are left out.
pub fn prefix_values(values: &Values, prefix: &str) -> Map<String, Value> {
    let mut out = Map::new();
    for def in registry::keys() {
        if def.class == Class::Live || prefix_of(def.path) != Some(prefix) {
            continue;
        }
        for path in concrete(def, values) {
            let Ok(m) = registry::lookup(&path) else {
                continue;
            };
            let value = filled(values, &m, &path);
            out.insert(path, value);
        }
    }
    out
}

/// The `values` view of `settings.get`: every stored key with defaults
/// filled, or those under `prefix` (whole segments), or the named `keys`.
/// A live key is refused with `live_key` and its owner's commands; an
/// unknown one as the registry refuses it.
pub fn view_values(
    values: &Values,
    prefix: Option<&str>,
    keys: Option<&[String]>,
) -> Result<Map<String, Value>, Vec<crate::Diagnostic>> {
    if let Some(keys) = keys {
        let mut out = Map::new();
        let mut errors = Vec::new();
        for path in keys {
            match registry::lookup(path) {
                Ok(m) if m.def.is_live() => errors.push(registry::live_refusal(m.def, path)),
                Ok(m) => {
                    out.insert(path.clone(), filled(values, &m, path));
                }
                Err(e) => errors.push(e),
            }
        }
        return if errors.is_empty() {
            Ok(out)
        } else {
            Err(errors)
        };
    }
    let mut all = Map::new();
    for topic in PREFIXES {
        all.extend(prefix_values(values, topic));
    }
    let Some(prefix) = prefix else {
        return Ok(all);
    };
    let under = format!("{prefix}.");
    let out: Map<String, Value> = all
        .into_iter()
        .filter(|(k, _)| k == prefix || k.starts_with(&under))
        .collect();
    if out.is_empty() {
        // A prefix of live keys only (`system`), or no prefix at all.
        if let Some(def) = registry::keys()
            .iter()
            .find(|d| (d.path == prefix || d.path.starts_with(&under)) && d.is_live())
        {
            return Err(vec![registry::live_refusal(def, prefix)]);
        }
        if !registry::is_prefix(prefix) {
            return Err(vec![crate::Diagnostic::new(
                "validation_failed",
                prefix,
                "Unknown settings prefix",
            )]);
        }
    }
    Ok(out)
}

/// What the registry says is resolved, set beside the authored choice:
/// `appearance.mode` (never `auto`) and the accent as `#rrggbb`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Resolved {
    pub mode: Option<String>,
    pub accent: Option<String>,
}

impl Resolved {
    fn of(&self, prefix: &str) -> Option<Map<String, Value>> {
        if prefix != "appearance" {
            return None;
        }
        let mut out = Map::new();
        if let Some(mode) = &self.mode {
            out.insert("appearance.mode".into(), mode.clone().into());
        }
        if let Some(accent) = &self.accent {
            out.insert("appearance.accent".into(), accent.clone().into());
        }
        Some(out)
    }
}

/// The resolved mode of `values` (§3.3 A1), from the registry's view.
pub fn resolved_mode(values: &Values) -> Option<String> {
    use crate::view::{State, View};
    let authored = registry::effective(values, "appearance.mode")?;
    let (mode, _) =
        crate::resolve::values::effective_mode(authored.as_str()?, &State::of(values)).ok()?;
    Some(mode.name().into())
}

/// The content of one prefix topic: what publication compares.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeysContent {
    pub values: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Map<String, Value>>,
}

/// The content of every prefix topic.
pub fn contents(values: &Values, resolved: &Resolved) -> BTreeMap<&'static str, KeysContent> {
    PREFIXES
        .iter()
        .map(|prefix| {
            (
                *prefix,
                KeysContent {
                    values: prefix_values(values, prefix),
                    resolved: resolved.of(prefix),
                },
            )
        })
        .collect()
}

/// The window framing keys the look carries (the [`crate::view::Framing`]
/// view's).
pub const FRAMING: [&str; 8] = [
    "window.decorations",
    "window.caption_side",
    "window.style",
    "window.menu_style",
    "window.menu_side",
    "window.actions_place",
    "window.actions_align",
    "window.title_align",
];

/// Whether a key is part of the follower view: the appearance and what
/// resolves its mode, the framing, the accessibility text scale and motion,
/// the per-app looks and the tunables.
pub fn in_look(path: &str) -> bool {
    ["appearance", "state", "apps", "tune"].contains(&prefix_of(path).unwrap_or(""))
        || FRAMING.contains(&path)
        || path == "accessibility.text_scale"
        || path == "accessibility.reduced_motion"
}

/// The content of the `look` topic: the follower view's keys with defaults
/// filled (a follower reads them back with `View::of`), the resolved mode
/// and accent, and the digest of the source the look compiles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LookContent {
    pub values: Map<String, Value>,
    pub resolved: Map<String, Value>,
    pub source_digest: String,
}

pub fn look_content(values: &Values, resolved: &Resolved, source_digest: &str) -> LookContent {
    let mut out = Map::new();
    for prefix in [
        "appearance",
        "state",
        "window",
        "accessibility",
        "apps",
        "tune",
    ] {
        out.extend(
            prefix_values(values, prefix)
                .into_iter()
                .filter(|(k, _)| in_look(k)),
        );
    }
    LookContent {
        values: out,
        resolved: resolved.of("appearance").unwrap_or_default(),
        source_digest: source_digest.to_owned(),
    }
}

/// The `look` topic body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookBody {
    pub schema: u32,
    pub binding: Binding,
    pub incarnation: String,
    pub revision: Revision,
    pub changed_at: Revision,
    pub values: Map<String, Value>,
    pub resolved: Map<String, Value>,
    pub source_digest: String,
}

/// A `keys` topic body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeysBody {
    pub schema: u32,
    pub binding: Binding,
    pub incarnation: String,
    pub revision: Revision,
    /// The revision that last changed this prefix.
    pub changed_at: Revision,
    pub values: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Map<String, Value>>,
}

#[cfg(test)]
mod tests;
