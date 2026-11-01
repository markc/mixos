// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The settings catalogue: each key's kind, range, choices and default, as
//! `settings.describe` reports them. Prefs builds every control's shape from
//! it. Until settingsd answers, the catalogue is the one this binary's
//! registry describes, read through the same parser, so a control never
//! waits for the Bus to know what it is.
use serde_json::Value;
use std::collections::BTreeMap;

/// A key's value shape.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Bool,
    Int {
        min: i64,
        max: i64,
        step: i64,
    },
    /// An integer from a fixed set (panel thickness, slideshow interval).
    Set(Vec<i64>),
    Number {
        min: f64,
        max: f64,
        step: f64,
    },
    Enum(Vec<String>),
    /// A colour, or one of `words` (`auto`, `accent`, the swatch names).
    Colour {
        words: Vec<String>,
        alpha: bool,
    },
    /// `all` (or another word) or a connector name.
    Connector(Vec<String>),
    /// A list of `min`..=`max` entries; the registry checks each entry.
    List { min: usize, max: usize },
    /// Anything the page framework draws only through a custom control:
    /// strings, paths, lists, chords, catalogues.
    Other(String),
}

/// One key, as described.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyInfo {
    /// The registry path, with `<name>` placeholders.
    pub path: String,
    pub kind: Kind,
    /// `stored`, `applied`, `live` or `state`.
    pub class: String,
    pub owner: Option<String>,
    /// A live key's write command; none is a backend its owner lacks.
    pub write: Option<String>,
    pub default: Option<Value>,
    /// Defaults by the first placeholder's value (`top`, `bottom`).
    pub default_by: BTreeMap<String, Value>,
}

impl KeyInfo {
    pub fn live(&self) -> bool {
        self.class == "live"
    }
}

/// Every described key.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    keys: Vec<KeyInfo>,
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn kind_of(entry: &Value) -> Kind {
    let number = |field: &str| entry[field].as_f64();
    match entry["kind"].as_str().unwrap_or_default() {
        "bool" => Kind::Bool,
        "int" if entry["values"].is_array() => Kind::Set(
            entry["values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_i64)
                .collect(),
        ),
        "int" => Kind::Int {
            min: entry["min"].as_i64().unwrap_or(i64::MIN),
            max: entry["max"].as_i64().unwrap_or(i64::MAX),
            step: entry["step"].as_i64().unwrap_or(1).max(1),
        },
        "number" => Kind::Number {
            min: number("min").unwrap_or(f64::MIN),
            max: number("max").unwrap_or(f64::MAX),
            step: number("step").unwrap_or(0.0),
        },
        "enum" => Kind::Enum(strings(&entry["variants"])),
        "colour" => Kind::Colour {
            words: Vec::new(),
            alpha: entry["alpha"].as_bool().unwrap_or(false),
        },
        "colour_or" => Kind::Colour {
            words: strings(&entry["words"]),
            alpha: entry["alpha"].as_bool().unwrap_or(false),
        },
        "connector_or" => Kind::Connector(strings(&entry["words"])),
        "list" => Kind::List {
            min: entry["min_items"].as_u64().unwrap_or(0) as usize,
            max: entry["max_items"].as_u64().unwrap_or(u64::MAX) as usize,
        },
        other => Kind::Other(other.to_owned()),
    }
}

fn hex_colour(text: &str, alpha: bool) -> bool {
    let Some(hex) = text.strip_prefix('#') else {
        return false;
    };
    (hex.len() == 6 || (alpha && hex.len() == 8)) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Check `value` against a described `kind`: `None` for a shape the
/// catalogue cannot state (strings, paths, lists), which the registry
/// checks instead; `Some(Err(why))` for a refusal.
pub fn check(kind: &Kind, value: &Value) -> Option<Result<(), String>> {
    let fail = |why: String| Some(Err(why));
    match kind {
        Kind::Bool if value.is_boolean() => Some(Ok(())),
        Kind::Bool => fail("expected true or false".into()),
        Kind::Int { min, max, step } => match value.as_i64() {
            Some(i) if i < *min || i > *max => fail(format!("{i} is outside {min}..{max}")),
            Some(i) if (i - min) % step != 0 => fail(format!("{i} is not a step of {step}")),
            Some(_) => Some(Ok(())),
            None => fail("expected a whole number".into()),
        },
        Kind::Set(values) => match value.as_i64() {
            Some(i) if values.contains(&i) => Some(Ok(())),
            _ => fail(format!("expected one of {values:?}")),
        },
        Kind::Number { min, max, .. } => match value.as_f64() {
            Some(f) if f.is_finite() && f >= *min && f <= *max => Some(Ok(())),
            Some(f) => fail(format!("{f} is outside {min}..{max}")),
            None => fail("expected a number".into()),
        },
        Kind::Enum(names) => match value.as_str() {
            Some(s) if names.iter().any(|n| n == s) => Some(Ok(())),
            _ => fail(format!("expected one of {}", names.join(", "))),
        },
        Kind::Colour { words, alpha } => match value.as_str() {
            Some(s) if words.iter().any(|w| w == s) || hex_colour(s, *alpha) => Some(Ok(())),
            _ => fail("expected a #rrggbb colour".into()),
        },
        Kind::Connector(words) => match value.as_str() {
            Some(s) if words.iter().any(|w| w == s) => Some(Ok(())),
            Some(s)
                if (1..=32).contains(&s.len())
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') =>
            {
                Some(Ok(()))
            }
            _ => fail("expected a connector name".into()),
        },
        Kind::List { min, max } => match value.as_array() {
            Some(items) if (*min..=*max).contains(&items.len()) => None,
            Some(items) => fail(format!("{} entries is outside {min}..{max}", items.len())),
            None => fail("expected a list".into()),
        },
        Kind::Other(_) => None,
    }
}

/// Whether `pattern` (with `<name>` segments) names `path`, and how many
/// of its segments are literal; the first placeholder's value.
fn fit<'a>(pattern: &str, path: &'a str) -> Option<(usize, Option<&'a str>)> {
    let (mut parts, mut segments) = (pattern.split('.'), path.split('.'));
    let (mut literals, mut capture) = (0, None);
    loop {
        match (parts.next(), segments.next()) {
            (None, None) => return Some((literals, capture)),
            (Some(part), Some(segment)) if part.starts_with('<') && part.ends_with('>') => {
                capture = capture.or(Some(segment));
            }
            (Some(part), Some(segment)) if part == segment => literals += 1,
            _ => return None,
        }
    }
}

impl Catalog {
    /// This binary's own registry, as `settings.describe` would report it.
    pub fn builtin() -> Self {
        Self::from_describe(&settings::registry::describe(None)).unwrap_or_default()
    }

    /// The catalogue in a `settings.describe` answer.
    pub fn from_describe(described: &Value) -> Result<Self, String> {
        let entries = described["keys"]
            .as_array()
            .ok_or("describe: no keys list")?;
        let keys = entries
            .iter()
            .filter_map(|entry| {
                let path = entry["path"].as_str()?.to_owned();
                Some(KeyInfo {
                    kind: kind_of(entry),
                    class: entry["class"].as_str().unwrap_or("stored").to_owned(),
                    owner: entry["owner"].as_str().map(str::to_owned),
                    write: entry["write"].as_str().map(str::to_owned),
                    default: entry.get("default").cloned(),
                    default_by: entry["default_by"]
                        .as_object()
                        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default(),
                    path,
                })
            })
            .collect();
        Ok(Self { keys })
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The entry naming the concrete `path`: where several patterns fit,
    /// the one with the most literal segments, as settingsd chooses.
    pub fn get(&self, path: &str) -> Option<&KeyInfo> {
        self.matched(path).map(|(info, _)| info)
    }

    fn matched<'a>(&self, path: &'a str) -> Option<(&KeyInfo, Option<&'a str>)> {
        self.keys
            .iter()
            .filter_map(|info| fit(&info.path, path).map(|(n, capture)| (n, info, capture)))
            .max_by_key(|(n, ..)| *n)
            .map(|(_, info, capture)| (info, capture))
    }

    /// `path`'s default: its own, or the one for its placeholder's value.
    pub fn default_of(&self, path: &str) -> Option<Value> {
        let (info, capture) = self.matched(path)?;
        info.default
            .clone()
            .or_else(|| capture.and_then(|c| info.default_by.get(c).cloned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_builtin_catalogue_describes_the_page_keys() {
        let catalog = Catalog::builtin();
        assert!(catalog.len() > 150, "{} keys", catalog.len());
        assert_eq!(
            catalog.get("appearance.mode").unwrap().kind,
            Kind::Enum(vec!["dark".into(), "light".into(), "auto".into()])
        );
        assert_eq!(
            catalog.get("shell.panels.top.thickness").unwrap().kind,
            Kind::Set(vec![32, 40, 56, 64, 80])
        );
        assert_eq!(
            catalog.default_of("shell.panels.top.enabled"),
            Some(json!(false))
        );
        assert_eq!(
            catalog.default_of("shell.panels.bottom.enabled"),
            Some(json!(true))
        );
        assert!(catalog.get("network.wifi.enabled").unwrap().live());
        let layouts = &catalog.get("input.keyboard.layouts").unwrap().kind;
        assert_eq!(*layouts, Kind::List { min: 1, max: 8 });
        assert!(check(layouts, &json!([])).unwrap().is_err());
        assert!(check(layouts, &json!([["us", ""]])).is_none());
        assert!(catalog.get("appearance.nonsense").is_none());
    }

    #[test]
    fn the_most_literal_pattern_wins() {
        let described = json!({"keys":[
            {"path":"keymaps.<app>.<action>","kind":"chords","class":"stored"},
            {"path":"keymaps.comp.spawn.<name>","kind":"spawn","class":"stored"},
        ]});
        let catalog = Catalog::from_describe(&described).unwrap();
        assert_eq!(
            catalog.get("keymaps.comp.spawn.term").unwrap().kind,
            Kind::Other("spawn".into())
        );
        assert_eq!(
            catalog.get("keymaps.ced.save").unwrap().kind,
            Kind::Other("chords".into())
        );
    }
}
