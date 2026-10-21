// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Append-only defaults for sources written before a token existed.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::StyleValueSource;

pub const SOURCE: &str = include_str!("defaults/compat.conf.mix");

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Constraint {
    pub kind: String,
    pub a: String,
    pub b: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub ansi: [String; 16],
    pub foreground: String,
    pub background: String,
    /// `invert` or an exact colour.
    pub cursor: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub tokens: BTreeMap<String, BTreeMap<String, StyleValueSource>>,
    pub styles: BTreeMap<String, BTreeMap<String, StyleValueSource>>,
    pub schemes: BTreeMap<String, String>,
    pub terminal: BTreeMap<String, Terminal>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Table {
    pub fallback: Vec<String>,
    pub constraints: Vec<Constraint>,
    #[serde(flatten)]
    pub versions: BTreeMap<String, Version>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    compat: Table,
}

impl Table {
    /// Refuse a name appearing in two versions, even with equal values.
    pub fn parse(source: &str) -> Result<Self, String> {
        let envelope: Envelope = strict::from_str(source).map_err(|e| e.to_string())?;
        let table = envelope.compat;
        if table.fallback != ["name", "base", "scheme-default", "plain"] {
            return Err("the compatibility fallback rule is immutable".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for (version, entries) in &table.versions {
            if version
                .strip_prefix('v')
                .and_then(|v| v.parse::<u32>().ok())
                .is_none()
            {
                return Err(format!("invalid compatibility version `{version}`"));
            }
            for (kind, entries) in [
                ("token", entries.tokens.keys().collect::<Vec<_>>()),
                ("style", entries.styles.keys().collect()),
                ("scheme", entries.schemes.keys().collect()),
                ("terminal", entries.terminal.keys().collect()),
            ] {
                for name in entries {
                    if !names.insert((kind, name.clone())) {
                        return Err(format!("duplicate compatibility {kind} `{name}`"));
                    }
                }
            }
        }
        Ok(table)
    }

    pub fn token(
        &self,
        name: &str,
        style: &str,
        base: Option<&str>,
        scheme: &str,
    ) -> Option<(&StyleValueSource, &str)> {
        self.versions.iter().find_map(|(version, entries)| {
            let values = entries.tokens.get(name)?;
            [Some(style), base, Some(scheme), Some("plain"), Some("all")]
                .into_iter()
                .flatten()
                .find_map(|name| values.get(name))
                .map(|value| (value, version.as_str()))
        })
    }
}

pub fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| Table::parse(SOURCE).expect("the shipped compatibility table is valid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_entries_are_immutable() {
        let previous = Table::parse(include_str!("../tests/fixtures/compat-v2.conf.mix")).unwrap();
        let shipped = table();
        assert_eq!(shipped.fallback, previous.fallback);
        assert_eq!(shipped.constraints, previous.constraints);
        for (version, entries) in previous.versions {
            assert_eq!(shipped.versions.get(&version), Some(&entries));
        }
    }

    #[test]
    fn duplicate_names_are_refused_across_versions() {
        let source = SOURCE.replacen("\"v2\":", "v3: { tokens: { field_height: { all: 24 } }, styles: {}, schemes: {}, terminal: {} }, v2:", 1);
        assert_ne!(source, SOURCE, "the fixture mutation must occur");
        assert!(
            Table::parse(&source)
                .unwrap_err()
                .contains("duplicate compatibility token `field_height`")
        );
    }

    #[test]
    fn fallback_order_includes_custom_names() {
        let table = table();
        for (style, base, scheme, expected) in [
            ("pro", Some("classic"), "plain", 3.0),
            ("custom", Some("pro"), "classic", 3.0),
            ("custom", None, "classic", 0.0),
            ("custom", None, "absent", 4.0),
        ] {
            assert_eq!(
                table.token("field_radius", style, base, scheme).unwrap().0,
                &StyleValueSource::Number(expected)
            );
        }
    }
}
