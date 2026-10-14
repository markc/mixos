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
use crate::fit::Fit;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// The only document schema this build reads and writes.
pub const SCHEMA: u32 = 1;
/// Outputs with a wallpaper of their own, at most.
pub const MAX_OUTPUTS: usize = 64;
/// Distinct static images plus slideshow streams one document may name. Each is resident while shown; a kept
/// image is at most 3840 × 3840 × 4 bytes (56.25 MiB) after the downscale.
pub const MAX_IMAGES: usize = 4;
/// Bounded source lists and directory scans; only selected images are decoded.
pub const MAX_SLIDES: usize = 4096;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<Fit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slideshow: Option<Slideshow>,
}

fn default_interval() -> u64 { 600 }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slideshow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
    #[serde(default = "default_interval")]
    pub interval_s: u64,
    #[serde(default)]
    pub shuffle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SlideSource {
    Folder(PathBuf),
    Images(Vec<PathBuf>),
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
        fit: Fit,
    },
    Slideshow {
        source: SlideSource,
        interval_s: u64,
        shuffle: bool,
        fallback: Option<Rgb>,
        fit: Fit,
    },
}

impl Wallpaper {
    /// Validate one fill; `field` names it in the error.
    pub fn from_fill(fill: &Fill, field: &str) -> Result<Self, Invalid> {
        let sources = usize::from(fill.colour.is_some()) + usize::from(fill.image.is_some())
            + usize::from(fill.slideshow.is_some());
        if sources != 1 {
            return Err(Invalid::new(field, "name exactly one of colour, image or slideshow"));
        }
        if let Some(text) = &fill.colour {
            if fill.fallback.is_some() || fill.fit.is_some() {
                return Err(Invalid::new(field, "fallback and fit require an image or slideshow"));
            }
            return Rgb::parse(text).map(Self::Colour)
                .map_err(|e| Invalid::new(format!("{field}.colour"), e));
        }
        let fallback = fill.fallback.as_deref().map(Rgb::parse).transpose()
            .map_err(|e| Invalid::new(format!("{field}.fallback"), e))?;
        let fit = fill.fit.unwrap_or_default();
        if let Some(text) = &fill.image {
            let path = image_path(text).map_err(|e| Invalid::new(format!("{field}.image"), e))?;
            return Ok(Self::Image { path, fallback, fit });
        }
        let slides = fill.slideshow.as_ref().expect("exactly one source validated");
        if slides.interval_s < 5 {
            return Err(Invalid::new(format!("{field}.slideshow.interval_s"), "must be at least 5"));
        }
        let source = match (&slides.folder, &slides.images) {
            (Some(folder), None) => SlideSource::Folder(image_path(folder)
                .map_err(|e| Invalid::new(format!("{field}.slideshow.folder"), e))?),
            (None, Some(images)) if !images.is_empty() && images.len() <= MAX_SLIDES => {
                let paths = images.iter().enumerate().map(|(i, text)| image_path(text)
                    .map_err(|e| Invalid::new(format!("{field}.slideshow.images[{i}]"), e)))
                    .collect::<Result<Vec<_>, _>>()?;
                SlideSource::Images(paths)
            }
            (None, Some(_)) => return Err(Invalid::new(format!("{field}.slideshow.images"),
                format!("must contain 1 to {MAX_SLIDES} paths"))),
            _ => return Err(Invalid::new(format!("{field}.slideshow"), "name exactly one of folder or images")),
        };
        Ok(Self::Slideshow { source, interval_s: slides.interval_s, shuffle: slides.shuffle, fallback, fit })
    }

    /// The fill this wallpaper is written as. The theme has none.
    fn fill(&self) -> Fill {
        match self {
            Self::Theme => Fill::default(),
            Self::Colour(rgb) => Fill {
                colour: Some(rgb.hex()),
                ..Fill::default()
            },
            Self::Image { path, fallback, fit } => Fill {
                image: Some(path.display().to_string()),
                fallback: fallback.map(Rgb::hex),
                fit: (*fit != Fit::Fill).then_some(*fit),
                ..Fill::default()
            },
            Self::Slideshow { source, interval_s, shuffle, fallback, fit } => {
                let (folder, images) = match source {
                    SlideSource::Folder(path) => (Some(path.display().to_string()), None),
                    SlideSource::Images(paths) => (None, Some(paths.iter().map(|p| p.display().to_string()).collect())),
                };
                Fill {
                    slideshow: Some(Slideshow { folder, images, interval_s: *interval_s, shuffle: *shuffle }),
                    fallback: fallback.map(Rgb::hex),
                    fit: (*fit != Fit::Fill).then_some(*fit),
                    ..Fill::default()
                }
            }
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
        let slides = std::iter::once(&config.default).chain(config.outputs.values())
            .filter(|w| matches!(w, Wallpaper::Slideshow { .. })).count();
        let images = config.images().len() + slides;
        if images > MAX_IMAGES {
            return Err(Invalid::new(
                "",
                format!("at most {MAX_IMAGES} distinct images or slideshow streams, found {images}"),
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
                fit: Fit::Fill,
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

    #[test]
    fn slideshow_defaults_and_fit_round_trip_without_touching_the_source() {
        let config = parse("schema: 1\ndefault: { slideshow: { images: [\"/a.png\", \"/b.jpg\"] }, fit: \"fit\", fallback: \"#010203\" }").unwrap();
        assert!(matches!(config.default, Wallpaper::Slideshow { interval_s: 600, shuffle: false, fit: Fit::Fit, .. }));
        assert_eq!(parse(&config.to_conf_text().unwrap()).unwrap(), config);
        let folder = parse("schema: 1\ndefault: { slideshow: { folder: \"/wallpapers\", interval_s: 5, shuffle: true }, fit: \"tile\" }").unwrap();
        assert_eq!(parse(&folder.to_conf_text().unwrap()).unwrap(), folder);
        assert!(folder.images().is_empty(), "a slideshow is not eagerly decoded");
    }

    #[test]
    fn bad_slideshow_and_misplaced_fit_are_refused() {
        for fill in [
            r##"{ colour: "#000000", fit: "fill" }"##,
            r#"{ image: "/a.png", fit: "zoom" }"#,
            r#"{ image: "/a.png", slideshow: { folder: "/w" } }"#,
            r#"{ slideshow: {} }"#,
            r#"{ slideshow: { folder: "/w", images: ["/a.png"] } }"#,
            r#"{ slideshow: { images: [] } }"#,
            r#"{ slideshow: { folder: "relative" } }"#,
            r#"{ slideshow: { images: ["relative.png"] } }"#,
            r#"{ slideshow: { images: ["/a.png"], interval_s: 4 } }"#,
            r#"{ slideshow: { folder: "/w", interval_s: -1 } }"#,
            r#"{ slideshow: { folder: "/w", interval_s: 5.5 } }"#,
            r#"{ slideshow: { folder: "/w", shuffle: "false" } }"#,
            r#"{ slideshow: { folder: "/w", bogus: true } }"#,
        ] { assert!(parse(&format!("schema: 1\ndefault: {fill}")).is_err(), "{fill}"); }
    }

    #[test]
    fn all_fit_modes_are_valid_and_default_fill_keeps_legacy_document_shape() {
        for mode in ["fill", "fit", "centre", "tile", "stretch"] {
            assert!(parse(&format!("schema: 1\ndefault: {{ image: \"/a.png\", fit: \"{mode}\" }}")).is_ok());
        }
        let old = parse("schema: 1\ndefault: { image: \"/a.png\" }").unwrap();
        assert!(!old.to_conf_text().unwrap().contains("fit"));
    }

    #[test]
    fn large_slide_lists_are_bounded_but_do_not_count_as_resident_images() {
        let mut doc = Config::default().document();
        doc.default = Some(Fill { slideshow: Some(Slideshow { folder: None,
            images: Some((0..MAX_SLIDES).map(|i| format!("/w/{i}.png")).collect()), interval_s: 5, shuffle: false }), ..Fill::default() });
        assert!(Config::from_document(doc.clone()).is_ok());
        doc.default.as_mut().unwrap().slideshow.as_mut().unwrap().images.as_mut().unwrap().push("/extra.png".into());
        assert!(Config::from_document(doc).is_err());
    }
}
