// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The style of one prose surface: an email body, a composer, an editor. Each
//! surface keeps its own [`Content`] in the profile, so the reader can be dark
//! while the composer is light, and the editor can use another size. The
//! toolkit applies it to the surface's region; this module only checks and
//! stores it.
//!
//! Fonts are the bundled faces only: [`FAMILIES`]. Every value is omitted from
//! the serialised form at its default, and the whole group while it is all
//! defaults, so a profile written before it re-serialises byte for byte.
use crate::model::{Diagnostic, deserialize_integer_u32};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The padding choices, in logical pixels: the terminal's own set.
pub const PADDINGS: [u32; 5] = [1, 2, 4, 8, 16];
pub const FONT_SIZE_MIN: f64 = 6.0;
pub const FONT_SIZE_MAX: f64 = 48.0;
/// The bundled families a surface may choose. None is the design's UI face
/// (Inter), so an unset family follows the design.
pub const FAMILIES: [&str; 2] = ["Inter", "JetBrains Mono"];

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn is_default_padding(padding: &u32) -> bool {
    *padding == DEFAULT_PADDING
}

/// The design's UI type size, the default for every surface.
pub fn default_font_size() -> f64 {
    design::default_typography(design::TypographyRole::Ui).font_size
}

/// The padding a surface starts with: the terminal's default.
pub const DEFAULT_PADDING: u32 = 8;

fn error(path: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("validation_failed", path, message)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Content {
    /// Space between the surface's edge and its text, one of [`PADDINGS`].
    #[serde(
        skip_serializing_if = "is_default_padding",
        deserialize_with = "deserialize_integer_u32"
    )]
    pub padding: u32,
    #[serde(skip_serializing_if = "is_default")]
    pub font: ContentFont,
    /// Whether the surface shows light, whatever the app's mode is.
    #[serde(skip_serializing_if = "is_default")]
    pub light: bool,
}

impl Default for Content {
    fn default() -> Self {
        Self {
            padding: DEFAULT_PADDING,
            font: ContentFont::default(),
            light: false,
        }
    }
}

impl Content {
    pub fn is_default(&self) -> bool {
        is_default(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContentFont {
    /// One of [`FAMILIES`], or None for the design's UI face.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// Logical pixels, [`FONT_SIZE_MIN`] to [`FONT_SIZE_MAX`].
    #[serde(skip_serializing_if = "is_default_size")]
    pub size: f64,
}

impl Default for ContentFont {
    fn default() -> Self {
        Self {
            family: None,
            size: default_font_size(),
        }
    }
}

fn is_default_size(size: &f64) -> bool {
    *size == default_font_size()
}

/// Check every range and name. `path` is the surface's settings path, such as
/// `email.reader`.
pub fn validate(content: &Content, path: &str) -> Result<(), Diagnostic> {
    if !PADDINGS.contains(&content.padding) {
        return Err(error(
            &format!("{path}.padding"),
            "Padding must be 1, 2, 4, 8 or 16",
        ));
    }
    if let Some(family) = &content.font.family
        && !FAMILIES.contains(&family.as_str())
    {
        return Err(error(
            &format!("{path}.font.family"),
            "Font must be Inter or JetBrains Mono",
        ));
    }
    let size = content.font.size;
    if !size.is_finite() || !(FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&size) {
        return Err(error(
            &format!("{path}.font.size"),
            format!("Expected {FONT_SIZE_MIN}..{FONT_SIZE_MAX}"),
        ));
    }
    Ok(())
}

/// The `describe()` entries for one surface, under `path`.
pub fn describe(path: &str) -> Value {
    serde_json::json!({
        format!("{path}.padding"): {"type":"integer", "enum":PADDINGS, "default":DEFAULT_PADDING, "unit":"px"},
        format!("{path}.font.family"): {"type":"string|null", "names":FAMILIES, "default":null, "null":"the design's UI face"},
        format!("{path}.font.size"): {"type":"number", "minimum":FONT_SIZE_MIN, "maximum":FONT_SIZE_MAX, "unit":"px"},
        format!("{path}.light"): {"type":"boolean", "default":false},
    })
}

/// Set (Some) or reset (None) one field. `field` is the path after the
/// surface; empty is the whole group.
pub(crate) fn set(
    content: &mut Content,
    path: &str,
    field: &[&str],
    value: Option<Value>,
) -> Result<(), Diagnostic> {
    let defaults = Content::default();
    match field {
        [] => {
            *content = value
                .filter(|v| !v.is_null())
                .map_or(Ok(Content::default()), |v| typed(path, v))?
        }
        ["padding"] => content.padding = value.map_or(Ok(defaults.padding), |v| whole(path, v))?,
        ["font", "family"] => {
            content.font.family = typed(path, value.unwrap_or(Value::Null))?;
        }
        ["font", "size"] => {
            content.font.size = value.map_or(Ok(defaults.font.size), |v| typed(path, v))?
        }
        ["light"] => content.light = value.map_or(Ok(defaults.light), |v| typed(path, v))?,
        _ => return Err(error(path, "Unknown field or scope")),
    }
    Ok(())
}

fn typed<T: serde::de::DeserializeOwned>(path: &str, value: Value) -> Result<T, Diagnostic> {
    serde_json::from_value(value).map_err(|e| error(path, e.to_string()))
}

/// A whole number, as Mix data sends it (8 or 8.0).
fn whole(path: &str, value: Value) -> Result<u32, Diagnostic> {
    crate::model::integer_u32(typed::<f64>(path, value)?)
        .ok_or_else(|| error(path, "Expected an integral u32 number"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_serialise_to_nothing() {
        let value = serde_json::to_value(Content::default()).unwrap();
        assert_eq!(value, json!({}));
    }

    #[test]
    fn a_bundled_family_and_a_padding_validate() {
        let mut content = Content::default();
        set(
            &mut content,
            "email.reader",
            &["font", "family"],
            Some(json!("JetBrains Mono")),
        )
        .unwrap();
        set(&mut content, "email.reader", &["padding"], Some(json!(16))).unwrap();
        assert!(validate(&content, "email.reader").is_ok());
    }

    #[test]
    fn an_unbundled_family_is_refused() {
        let mut content = Content::default();
        set(
            &mut content,
            "email.reader",
            &["font", "family"],
            Some(json!("Comic Sans")),
        )
        .unwrap();
        let error = validate(&content, "email.reader").unwrap_err();
        assert_eq!(error.path, "email.reader.font.family");
    }

    #[test]
    fn padding_outside_the_choices_is_refused() {
        let mut content = Content::default();
        set(&mut content, "email.reader", &["padding"], Some(json!(3))).unwrap();
        assert_eq!(
            validate(&content, "email.reader").unwrap_err().path,
            "email.reader.padding"
        );
    }

    #[test]
    fn size_must_be_in_range() {
        let mut content = Content::default();
        set(
            &mut content,
            "email.reader",
            &["font", "size"],
            Some(json!(4.0)),
        )
        .unwrap();
        assert_eq!(
            validate(&content, "email.reader").unwrap_err().path,
            "email.reader.font.size"
        );
    }
}
