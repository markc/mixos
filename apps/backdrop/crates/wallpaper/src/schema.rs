// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The wallpaper document (`schema: 1`): strict data for `backdrop.mx`, the
//! Bus body of `backdrop.set`, and the command line's default. Each entry
//! point validates through [`Config::from_document`].
//!
//! ```text
//! schema: 1
//! default: { colour: "#1e1e2e" }
//! outputs: {
//!   "DP-1": { image: "/usr/share/wallpapers/hills.jpg", fallback: "#000000" },
//! }
//! ```
//!
//! With no `default`, the session theme's colour fills every output.

use crate::colour::Rgb;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// The only document schema this build reads and writes.
pub const SCHEMA: u32 = 1;
/// Outputs with a wallpaper of their own, at most.
pub const MAX_OUTPUTS: usize = 64;
/// Distinct images one document may name. Each is resident while shown; a kept
/// image is at most 3840 × 3840 × 4 bytes (56.25 MiB) after the downscale.
pub const MAX_IMAGES: usize = 4;
const MAX_NAME: usize = 64;
const MAX_PATH: usize = 4096;

/// A rejected value, naming the field it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub field: String,
    pub reason: String,
}

impl Invalid {
    fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.field.is_empty() {
            f.write_str(&self.reason)
        } else {
            write!(f, "{}: {}", self.field, self.reason)
        }
    }
}

impl std::error::Error for Invalid {}

/// The document as written. Unknown keys are refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Fill>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Fill>,
}

/// One wallpaper as written: exactly one of `colour` or `image`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fill {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// The colour shown while the image is not decoded yet (an image only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

/// What one surface shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wallpaper {
    /// The session theme's panel colour: the default when no document names
    /// one.
    Theme,
    /// A solid colour.
    Colour(Rgb),
    /// An absolute path to a PNG or JPEG, centre-cropped to fill the surface.
    /// `fallback` shows while the image is not decoded.
    Image {
        path: PathBuf,
        fallback: Option<Rgb>,
    },
}

impl Wallpaper {
    /// Validate one fill; `field` names it in the error.
    pub fn from_fill(fill: &Fill, field: &str) -> Result<Self, Invalid> {
        match (&fill.colour, &fill.image) {
            (Some(_), Some(_)) => Err(Invalid::new(
                field,
                "names both a colour and an image; use one",
            )),
            (None, None) => Err(Invalid::new(field, "names neither a colour nor an image")),
            (Some(colour), None) => {
                if fill.fallback.is_some() {
                    return Err(Invalid::new(field, "a fallback colour goes with an image"));
                }
                let rgb =
                    Rgb::parse(colour).map_err(|e| Invalid::new(format!("{field}.colour"), e))?;
                Ok(Self::Colour(rgb))
            }
            (None, Some(image)) => {
                let path =
                    image_path(image).map_err(|e| Invalid::new(format!("{field}.image"), e))?;
                let fallback = match &fill.fallback {
                    Some(text) => Some(
                        Rgb::parse(text)
                            .map_err(|e| Invalid::new(format!("{field}.fallback"), e))?,
                    ),
                    None => None,
                };
                Ok(Self::Image { path, fallback })
            }
        }
    }

    /// The fill this wallpaper is written as. The theme has none.
    fn fill(&self) -> Fill {
        match self {
            Self::Theme => Fill::default(),
            Self::Colour(rgb) => Fill {
                colour: Some(rgb.hex()),
                ..Fill::default()
            },
            Self::Image { path, fallback } => Fill {
                image: Some(path.display().to_string()),
                fallback: fallback.map(Rgb::hex),
                ..Fill::default()
            },
        }
    }
}

/// An absolute path, no NUL, not too long. Whether it names an image is for
/// the decoder to say.
fn image_path(text: &str) -> Result<PathBuf, String> {
    if text.len() > MAX_PATH {
        return Err(format!("longer than {MAX_PATH} bytes"));
    }
    if text.contains('\0') {
        return Err("contains a NUL byte".into());
    }
    let path = Path::new(text);
    if !path.is_absolute() {
        return Err(format!("{text:?} is not an absolute path"));
    }
    Ok(path.to_path_buf())
}

/// An output name: its connector (`DP-1`), as the compositor reports it.
fn check_name(name: &str) -> Result<(), Invalid> {
    let shaped = !name.is_empty()
        && name.len() <= MAX_NAME
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if shaped {
        Ok(())
    } else {
        Err(Invalid::new(
            "outputs",
            format!(
                "output name {name:?} is not 1 to {MAX_NAME} of a-z, A-Z, 0-9, '-', '_' or '.'"
            ),
        ))
    }
}

/// A validated wallpaper configuration: the default and the outputs that
/// have their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub default: Wallpaper,
    pub outputs: BTreeMap<String, Wallpaper>,
}

impl Default for Config {
    /// The session theme on every output.
    fn default() -> Self {
        Self {
            default: Wallpaper::Theme,
            outputs: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Parse and validate strict data (`backdrop.mx`).
    pub fn from_text(text: &str) -> Result<Self, Invalid> {
        let document: Document = config::from_conf_mix_str(text)
            .map_err(|e| Invalid::new("", format!("document: {e}")))?;
        Self::from_document(document)
    }

    /// Validate a document (the file's, or a Bus body's).
    pub fn from_document(document: Document) -> Result<Self, Invalid> {
        if document.schema != SCHEMA {
            return Err(Invalid::new(
                "schema",
                format!("must be {SCHEMA}, found {}", document.schema),
            ));
        }
        let default = match &document.default {
            Some(fill) => Wallpaper::from_fill(fill, "default")?,
            None => Wallpaper::Theme,
        };
        if document.outputs.len() > MAX_OUTPUTS {
            return Err(Invalid::new(
                "outputs",
                format!(
                    "at most {MAX_OUTPUTS} outputs, found {}",
                    document.outputs.len()
                ),
            ));
        }
        let mut outputs = BTreeMap::new();
        for (name, fill) in &document.outputs {
            check_name(name)?;
            outputs.insert(
                name.clone(),
                Wallpaper::from_fill(fill, &format!("outputs.{name}"))?,
            );
        }
        let config = Self { default, outputs };
        let images = config.images().len();
        if images > MAX_IMAGES {
            return Err(Invalid::new(
                "",
                format!("at most {MAX_IMAGES} distinct images, found {images}"),
            ));
        }
        Ok(config)
    }

    /// The document this configuration is written as.
    pub fn document(&self) -> Document {
        Document {
            schema: SCHEMA,
            default: match &self.default {
                Wallpaper::Theme => None,
                wallpaper => Some(wallpaper.fill()),
            },
            outputs: self
                .outputs
                .iter()
                .map(|(name, wallpaper)| (name.clone(), wallpaper.fill()))
                .collect(),
        }
    }

    /// The document as strict data, for `backdrop.mx`.
    pub fn to_conf_text(&self) -> Result<String, Invalid> {
        config::to_conf_mix_string(&self.document())
            .map_err(|e| Invalid::new("", format!("document: {e}")))
    }

    /// What `output` shows: its own wallpaper, else the default.
    pub fn for_output(&self, output: Option<&str>) -> &Wallpaper {
        output
            .and_then(|name| self.outputs.get(name))
            .unwrap_or(&self.default)
    }

    /// The distinct image paths this configuration names.
    pub fn images(&self) -> BTreeSet<&Path> {
        std::iter::once(&self.default)
            .chain(self.outputs.values())
            .filter_map(|wallpaper| match wallpaper {
                Wallpaper::Image { path, .. } => Some(path.as_path()),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, Invalid> {
        Config::from_text(text)
    }

    #[test]
    fn a_document_with_no_default_is_the_session_theme() {
        assert_eq!(parse("schema: 1").unwrap(), Config::default());
        assert_eq!(
            Config::default().for_output(Some("DP-1")),
            &Wallpaper::Theme
        );
    }

    #[test]
    fn a_document_names_a_default_and_per_output_overrides() {
        let config = parse(
            r##"
            schema: 1
            default: { colour: "#1E1E2E" }
            outputs: {
              "DP-1": { image: "/usr/share/wallpapers/hills.jpg", fallback: "#000000" },
              HDMI_2: { colour: "#ffffff" },
            }
            "##,
        )
        .unwrap();
        assert_eq!(config.default, Wallpaper::Colour(Rgb([0x1e, 0x1e, 0x2e])));
        assert_eq!(
            config.for_output(Some("DP-1")),
            &Wallpaper::Image {
                path: "/usr/share/wallpapers/hills.jpg".into(),
                fallback: Some(Rgb([0, 0, 0])),
            }
        );
        assert_eq!(
            config.for_output(Some("HDMI_2")),
            &Wallpaper::Colour(Rgb([255, 255, 255]))
        );
        assert_eq!(
            config.for_output(Some("eDP-1")),
            &config.default,
            "an output without one takes the default"
        );
        assert_eq!(config.for_output(None), &config.default);
    }

    #[test]
    fn the_document_refuses_what_it_does_not_mean() {
        for text in [
            "schema: 2",
            "schema: \"1\"",
            "schema: 1\nbogus: 3",
            "schema: 1\ndefault: { colour: \"#000000\", extra: 1 }",
            "schema: 1\ndefault: { colour: \"red\" }",
            "schema: 1\ndefault: { colour: \"#12345\" }",
            "schema: 1\ndefault: { colour: \"#GG0000\" }",
            "schema: 1\ndefault: { image: \"relative.png\" }",
            "schema: 1\ndefault: { image: \"\" }",
            "schema: 1\ndefault: { colour: \"#000000\", image: \"/a.png\" }",
            "schema: 1\ndefault: {}",
            "schema: 1\ndefault: { colour: \"#000000\", fallback: \"#111111\" }",
            "schema: 1\noutputs: { \"a/b\": { colour: \"#000000\" } }",
            "schema: 1\noutputs: { \"..\": { colour: \"#000000\" } }",
            "schema: 1\ndefault: { colour: $X }",
        ] {
            assert!(parse(text).is_err(), "must refuse {text:?}");
        }
    }

    #[test]
    fn too_many_distinct_images_are_refused_before_any_decode() {
        let five: Vec<String> = (0..5)
            .map(|i| format!("o{i}: {{ image: \"/w/{i}.png\" }}"))
            .collect();
        let text = format!("schema: 1\noutputs: {{ {} }}", five.join(", "));
        let error = parse(&text).unwrap_err();
        assert!(error.reason.contains("distinct images"), "{error}");
        let four: Vec<String> = (0..4)
            .map(|i| format!("o{i}: {{ image: \"/w/{i}.png\" }}"))
            .collect();
        assert!(parse(&format!("schema: 1\noutputs: {{ {} }}", four.join(", "))).is_ok());
    }

    #[test]
    fn the_bus_body_and_the_file_mean_the_same() {
        let document: Document =
            serde_json::from_str(r##"{"schema":1,"default":{"colour":"#abcdef"}}"##).unwrap();
        let from_bus = Config::from_document(document).unwrap();
        assert_eq!(
            from_bus,
            parse("schema: 1\ndefault: { colour: \"#abcdef\" }").unwrap()
        );
        let unknown: Result<Document, _> = serde_json::from_str(r#"{"schema":1,"extra":true}"#);
        assert!(unknown.is_err(), "unknown keys are refused on the Bus too");
    }

    #[test]
    fn a_configuration_round_trips_through_its_text() {
        let config = parse(
            "schema: 1\ndefault: { image: \"/a.png\", fallback: \"#010203\" }\noutputs: { \"DP-1\": { colour: \"#0a0b0c\" } }",
        )
        .unwrap();
        let text = config.to_conf_text().unwrap();
        assert_eq!(Config::from_text(&text).unwrap(), config);
        assert_eq!(
            Config::default()
                .to_conf_text()
                .map(|t| t.contains("schema")),
            Ok(true)
        );
    }
}
