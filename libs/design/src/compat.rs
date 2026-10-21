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

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Scheme {
    Legacy(String),
    Palette(SchemePalette),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SchemePalette {
    pub style: String,
    /// Mode to primitive name to colour; both modes are explicit.
    pub chrome: BTreeMap<String, BTreeMap<String, crate::OklchSource>>,
}

impl Scheme {
    pub fn style(&self) -> &str {
        match self {
            Self::Legacy(style) => style,
            Self::Palette(palette) => &palette.style,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub tokens: BTreeMap<String, BTreeMap<String, StyleValueSource>>,
    pub styles: BTreeMap<String, BTreeMap<String, StyleValueSource>>,
    pub schemes: BTreeMap<String, Scheme>,
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
            // `all` is table metadata, never an authored style-name match.
            [Some(style), base, Some(scheme), Some("plain")]
                .into_iter()
                .flatten()
                .filter(|name| *name != "all")
                .find_map(|name| values.get(name))
                .or_else(|| values.get("all"))
                .map(|value| (value, version.as_str()))
        })
    }
}

pub fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| Table::parse(SOURCE).expect("the shipped compatibility table is valid"))
}

/// Human-readable diagnostics generated from structured table provenance.
pub(crate) fn diagnostics(defaulted: &BTreeMap<String, String>) -> Vec<crate::DesignDiagnostic> {
    defaulted
        .iter()
        .map(|(path, version)| {
            let code = if path.contains(".terminal.") {
                "defaulted-terminal"
            } else if path.contains(".schemes.") {
                "defaulted-scheme"
            } else if path
                .rsplit(".styles.")
                .next()
                .is_some_and(|name| name.contains('.'))
            {
                "defaulted-style-token"
            } else {
                "defaulted-style"
            };
            crate::DesignDiagnostic::info(
                code,
                path.clone(),
                format!("defaulted from compatibility {version}"),
            )
        })
        .collect()
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

    #[test]
    fn a_custom_style_named_all_does_not_capture_the_shared_default() {
        let source = SOURCE.replacen("\"v2\":", "v3: { tokens: { future_token: { all: 2, pro: 7, plain: 3 } }, styles: {}, schemes: {}, terminal: {} }, v2:", 1);
        let table = Table::parse(&source).unwrap();
        for (base, scheme, expected) in [(Some("pro"), "classic", 7.0), (None, "absent", 3.0)] {
            let (value, version) = table.token("future_token", "all", base, scheme).unwrap();
            assert_eq!(value, &StyleValueSource::Number(expected));
            assert_eq!(version, "v3");
        }
        assert_eq!(
            table
                .token("field_height", "all", Some("pro"), "studio")
                .unwrap()
                .1,
            "v2"
        );
    }
}
