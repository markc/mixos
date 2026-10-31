// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `settings.describe`, generated from the registry: one entry per key with
//! its kind and range, default, class, owner, domains, page, label and
//! version. Prefs builds its controls from it; agents read it as the
//! catalogue.
use super::{
    Class, DefaultValue, KeyDef, Kind, PLACEHOLDERS, REGISTRY_VERSION, app_domains, constraint,
    keys,
};
use serde_json::{Map, Value, json};

/// The Prefs label id of a key: `settings-` and its literal segments.
pub fn label_id(path: &str) -> String {
    let literal: Vec<&str> = path.split('.').filter(|p| !p.starts_with('<')).collect();
    format!("settings-{}", literal.join("-").replace('_', "-"))
}

fn kind_fields(kind: &Kind, out: &mut Map<String, Value>) {
    let mut put = |k: &str, v: Value| {
        out.insert(k.into(), v);
    };
    match *kind {
        Kind::Bool => put("kind", "bool".into()),
        Kind::Int { min, max, step } => {
            put("kind", "int".into());
            put("min", min.into());
            put("max", max.into());
            put("step", step.into());
        }
        Kind::OneOf(set) => {
            put("kind", "int".into());
            put("values", json!(set));
        }
        Kind::Number { min, max, step } => {
            put("kind", "number".into());
            put("min", min.into());
            put("max", max.into());
            if step > 0.0 {
                put("step", step.into());
            }
        }
        Kind::Enum(names) => {
            put("kind", "enum".into());
            put("variants", json!(names));
        }
        Kind::Known(names) => {
            put("kind", "name".into());
            put("known_values", json!(names));
            put("unknown", "retained; renders as the default".into());
            put("max_bytes", 64.into());
        }
        Kind::Colour(alpha) => {
            put("kind", "colour".into());
            put("alpha", (alpha == super::Alpha::Any).into());
        }
        Kind::ColourOr(words, alpha) => {
            put("kind", "colour_or".into());
            put("words", json!(words));
            put("alpha", (alpha == super::Alpha::Any).into());
        }
        Kind::ConnectorOr(words) => {
            put("kind", "connector_or".into());
            put("words", json!(words));
        }
        Kind::Str(grammar, max_bytes) => {
            put("kind", "string".into());
            put("grammar", grammar.name().into());
            put("max_bytes", max_bytes.into());
        }
        Kind::Path { directory, images } => {
            put("kind", "path".into());
            put("directory", directory.into());
            if images {
                put("images", json!(["png", "jpeg", "webp"]));
            }
        }
        Kind::List(of, min, max) => {
            put("kind", "list".into());
            let mut item = Map::new();
            kind_fields(of, &mut item);
            put("of", Value::Object(item));
            put("min_items", min.into());
            put("max_items", max.into());
        }
        Kind::XkbLayout => put("kind", "xkb_layout".into()),
        Kind::Chords => {
            put("kind", "chords".into());
            put("max_items", crate::keymap::MAX_CHORDS.into());
            put(
                "chord",
                "Ctrl, Alt, Shift, Super (any order) then one key, joined by +".into(),
            );
        }
        Kind::Spawn => put("kind", "spawn".into()),
        Kind::AppDomain => put("kind", "domain".into()),
        Kind::Tunable => {
            put("kind", "tunable".into());
            put("catalogue", "tunables".into());
        }
        Kind::Catalogue(what) => {
            put("kind", "catalogue".into());
            put("type", what.into());
        }
    }
}

/// One key's entry.
pub fn describe_key(def: &KeyDef) -> Value {
    let mut out = Map::new();
    out.insert("path".into(), def.path.into());
    out.insert("class".into(), def.class.name().into());
    kind_fields(&def.kind, &mut out);
    match def.default {
        DefaultValue::None => {}
        DefaultValue::Owner(from) => {
            out.insert("default_from".into(), from.into());
        }
        DefaultValue::Domain => {
            if let Some(schema) = app_domains::schema(def.path) {
                out.insert("domain".into(), schema);
            }
        }
        DefaultValue::ByCapture(table) => {
            let by: Map<String, Value> = table
                .iter()
                .filter_map(|(name, _)| {
                    let captures = [("", (*name).to_owned())];
                    def.default_value(&captures).map(|v| ((*name).into(), v))
                })
                .collect();
            out.insert("default_by".into(), Value::Object(by));
        }
        _ => {
            out.insert(
                "default".into(),
                def.default_value(&[]).unwrap_or(Value::Null),
            );
        }
    }
    if let Some(owner) = def.owner {
        out.insert(
            if def.class == Class::State {
                "writer"
            } else {
                "owner"
            }
            .into(),
            owner.into(),
        );
    }
    if let Some(command) = def.command {
        out.insert("command".into(), command.into());
    }
    if !def.domains.is_empty() {
        let names: Vec<&str> = def.domains.iter().map(|d| d.name()).collect();
        out.insert("domains".into(), json!(names));
    }
    if let Some(page) = def.page {
        out.insert("page".into(), page.into());
        out.insert("label".into(), label_id(def.path).into());
    }
    out.insert("since".into(), def.since.into());
    if def.mixos_only {
        out.insert("mixos_only".into(), true.into());
    }
    if def.export_only {
        out.insert("export_only".into(), true.into());
    }
    if let Some(when) = def.used_when {
        out.insert("used_when".into(), when.into());
    }
    if !def.constraints.is_empty() {
        out.insert("constraints".into(), json!(def.constraints));
    }
    Value::Object(out)
}

/// The registry, or the keys under `prefix` (`appearance`, `shell.panels`).
pub fn describe(prefix: Option<&str>) -> Value {
    let under = prefix.map(|p| format!("{p}."));
    let entries: Vec<Value> = keys()
        .iter()
        .filter(|def| {
            under
                .as_deref()
                .is_none_or(|u| def.path.starts_with(u) || Some(def.path) == prefix)
        })
        .map(describe_key)
        .collect();
    let constraints: Vec<Value> = constraint::CONSTRAINTS
        .iter()
        .map(|c| json!({"id": c.id, "text": c.text, "write_only": c.write_only}))
        .collect();
    let placeholders: Map<String, Value> = PLACEHOLDERS
        .iter()
        .map(|(name, what)| ((*name).into(), (*what).into()))
        .collect();
    let mut out = json!({
        "registry": REGISTRY_VERSION,
        "keys": entries,
        "constraints": constraints,
        "placeholders": placeholders,
        "reset": "null resets a key; a reset entry may end .* to reset a prefix",
    });
    if under.is_none() || prefix == Some("tune") {
        out["tunables"] = serde_json::to_value(design::tunables()).unwrap_or(Value::Null);
    }
    out
}
