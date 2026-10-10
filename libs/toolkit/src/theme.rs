// SPDX-License-Identifier: MIT OR Apache-2.0
//! A compiled MixOS theme: the resolved design for one scheme, style, mode
//! and contrast, together with the context it was compiled for, and how its
//! windows are framed (decorations and caption side, which change no token).
//!
//! The `design` crate's resolved artifact does not record its context, so a
//! consumer that compiled it would otherwise lose the scheme and mode it
//! asked for. [`Theme`] keeps the two together. It is built from the
//! embedded default design, from a full `theme.conf.mix` document, or from
//! the shared selection-only file (`scheme:`, `mode:`, `style:`,
//! `decorations:` and `caption_side:` alone), which is resolved against the
//! embedded design.
//!
//! Transplanted from `libs/appearance/src/theme.rs` (markc/mixos-iced), less
//! the iced font installation.

use std::fmt;
use std::path::{Path, PathBuf};

use design::{
    CaptionSide, Contrast, Decorations, DesignCompileResult, DesignContext, DesignDiagnostic,
    DesignSourceDocument, DesignSourceError, Mode, ResolvedDictionary, ResolvedStyle,
    ResolvedTypography, Scheme, SourceIdentity, UnstampedResolvedDesign,
};

/// The shared theme file, `theme.conf.mix`, in the MixOS etc directory.
pub const THEME_FILE: &str = "theme.conf.mix";

/// Where [`Theme::load`] reads the shared theme: `theme.conf.mix` under
/// `config::path(Dir::Etc)` (`$MIXOS_ETC`, else `$MIXOS/etc`, else
/// `/etc/mixos` for root, else `$XDG_CONFIG_HOME/mixos`).
pub fn theme_path() -> PathBuf {
    config::path(config::Dir::Etc).join(THEME_FILE)
}

/// Why a theme could not be built.
#[derive(Debug)]
pub enum Error {
    /// The text is neither a design document nor a selection-only file.
    Source(DesignSourceError),
    /// A selection names a scheme or mode the design does not have.
    Selection { field: &'static str, value: String },
    /// The design did not compile for the requested context.
    Compile(Vec<DesignDiagnostic>),
    /// The theme file could not be read.
    Read {
        path: PathBuf,
        error: std::io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => write!(f, "theme source: {error}"),
            Self::Selection { field, value } => write!(f, "unknown {field} {value:?}"),
            Self::Compile(diagnostics) => {
                write!(f, "theme did not compile:")?;
                for diagnostic in diagnostics {
                    write!(
                        f,
                        " [{} {}: {}]",
                        diagnostic.code, diagnostic.path, diagnostic.message
                    )?;
                }
                Ok(())
            }
            Self::Read { path, error } => write!(f, "read {}: {error}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Read { error, .. } => Some(error),
            Self::Selection { .. } | Self::Compile(_) => None,
        }
    }
}

/// The resolved design for one context, and how its windows are framed.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    context: DesignContext,
    design: UnstampedResolvedDesign,
    style: ResolvedStyle,
    decorations: Decorations,
    captions: CaptionSide,
}

/// The selection a selection-only file or a full document's legacy record
/// makes; every axis optional.
#[derive(Default)]
struct Selection {
    scheme: Option<String>,
    mode: Option<String>,
    style: Option<String>,
    decorations: Option<String>,
    caption_side: Option<String>,
}

impl Theme {
    /// The embedded default design in the default selection: studio, dark,
    /// normal contrast, the scheme's own style; client-side decorations with
    /// the captions at the right.
    pub fn embedded() -> Self {
        Self::for_context(DesignContext::default())
    }

    /// The embedded default design compiled for `context` (scheme, style,
    /// mode, contrast), with the default decorations.
    ///
    /// # Panics
    /// The embedded design claims every scheme, mode and contrast and the
    /// compiler compiles each claimed context before it returns one, so a
    /// failure here is a broken build, not a runtime condition.
    pub fn for_context(context: DesignContext) -> Self {
        let document = embedded_document();
        Self::compile(&document, context).expect("the embedded default design compiles")
    }

    /// A theme from `source`: a full design document (its own `scheme:`,
    /// `mode:`, `style:`, `decorations:` and `caption_side:` select the
    /// context and framing), or the shared selection-only file
    /// holding nothing but `scheme:`, `mode:`, `style:`, `decorations:` and
    /// `caption_side:`, resolved against the embedded design. `identity`
    /// names the source in diagnostics.
    pub fn from_source(identity: &str, source: &str) -> Result<Self, Error> {
        let (document, selection) =
            match design::parse_design_source(SourceIdentity::new(identity), source) {
                Ok(document) => {
                    let selection = Selection {
                        scheme: document.legacy.scheme.clone(),
                        mode: document.legacy.mode.clone(),
                        style: document.presentation.style.clone(),
                        decorations: document.presentation.decorations.clone(),
                        caption_side: document.presentation.caption_side.clone(),
                    };
                    (document, selection)
                }
                Err(error) => {
                    let Some(selection) = selection_only(source) else {
                        return Err(Error::Source(error));
                    };
                    (embedded_document(), selection)
                }
            };
        let context = DesignContext {
            scheme: axis(selection.scheme.as_deref(), "scheme", Scheme::from_name)?,
            mode: axis(selection.mode.as_deref(), "mode", Mode::from_name)?,
            contrast: Contrast::default(),
            style: selection
                .style
                .as_deref()
                .map(|name| {
                    design::Style::from_name(name).ok_or_else(|| Error::Selection {
                        field: "style",
                        value: name.to_owned(),
                    })
                })
                .transpose()?,
            app: None,
        };
        let mut theme = Self::compile(&document, context)?;
        theme.decorations = axis(
            selection.decorations.as_deref(),
            "decorations",
            Decorations::from_name,
        )?;
        theme.captions = axis(
            selection.caption_side.as_deref(),
            "caption_side",
            CaptionSide::from_name,
        )?;
        Ok(theme)
    }

    /// The theme in the file at `path`: `None` when there is no file, an
    /// error when there is one that does not read, parse or compile.
    pub fn read(path: &Path) -> Result<Option<Self>, Error> {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(Error::Read {
                    path: path.to_path_buf(),
                    error,
                });
            }
        };
        Self::from_source(&path.display().to_string(), &source).map(Some)
    }

    /// The shared theme file ([`theme_path`]), or the embedded default when
    /// there is none or it is unusable. A caller that wants the reason uses
    /// [`Theme::read`].
    pub fn load() -> Self {
        let path = theme_path();
        match Self::read(&path) {
            Ok(Some(theme)) => theme,
            Ok(None) => Self::embedded(),
            Err(error) => {
                // Never silently: the session asked for a theme it is not getting.
                eprintln!(
                    "toolkit: theme {}: {error}; using the embedded theme",
                    path.display()
                );
                Self::embedded()
            }
        }
    }

    fn compile(document: &DesignSourceDocument, context: DesignContext) -> Result<Self, Error> {
        match design::compile_design(document, context.clone()) {
            DesignCompileResult::Success(success) => {
                // A design that authors no style family takes the embedded
                // design's style for its scheme and style axis.
                let style = match success.candidate.dictionary().style {
                    Some(style) => style,
                    None => Self::for_context(context.clone()).style,
                };
                Ok(Self {
                    context,
                    design: success.candidate,
                    style,
                    decorations: Decorations::default(),
                    captions: CaptionSide::default(),
                })
            }
            DesignCompileResult::Fatal(failure) => Err(Error::Compile(failure.diagnostics)),
        }
    }

    /// This (session) theme with an app's own choice laid over it, as the
    /// Theme menu ([`crate::theme_menu`]) makes it. With no colour axis
    /// chosen (scheme, style, mode) the design is this theme's own, so a
    /// custom session design stays intact; else it is the embedded design in
    /// the chosen axes (each axis left `None` keeps this theme's), at this
    /// theme's contrast. Decorations and caption side overlay either way.
    /// The session's theme file is never touched.
    pub fn with_choice(&self, choice: &Choice) -> Self {
        let mut theme =
            if choice.scheme.is_none() && choice.style.is_none() && choice.mode.is_none() {
                self.clone()
            } else {
                let mut theme = Self::for_context(DesignContext {
                    scheme: choice.scheme.unwrap_or(self.scheme()),
                    mode: choice.mode.unwrap_or(self.mode()),
                    style: choice.style.unwrap_or(self.context.style),
                    ..self.context.clone()
                });
                theme.decorations = self.decorations;
                theme.captions = self.captions;
                theme
            };
        theme.decorations = choice.decorations.unwrap_or(theme.decorations);
        theme.captions = choice.captions.unwrap_or(theme.captions);
        theme
    }

    /// `self` with `decorations` and `captions`.
    pub fn framed(mut self, decorations: Decorations, captions: CaptionSide) -> Self {
        self.decorations = decorations;
        self.captions = captions;
        self
    }

    pub fn context(&self) -> &DesignContext {
        &self.context
    }

    pub fn scheme(&self) -> Scheme {
        self.context.scheme
    }

    pub fn mode(&self) -> Mode {
        self.context.mode
    }

    pub fn contrast(&self) -> Contrast {
        self.context.contrast
    }

    /// The style axis: `None` is the scheme's own style.
    pub fn style_axis(&self) -> Option<design::Style> {
        self.context.style
    }

    /// Who draws the title bar.
    pub fn decorations(&self) -> Decorations {
        self.decorations
    }

    /// Where a client-side title bar puts its caption buttons.
    pub fn captions(&self) -> CaptionSide {
        self.captions
    }

    pub fn dictionary(&self) -> &ResolvedDictionary {
        self.design.dictionary()
    }

    pub fn typography(&self) -> &ResolvedTypography {
        self.design.typography()
    }

    /// The chrome's forms and lengths (the design's style family).
    pub fn style(&self) -> ResolvedStyle {
        self.style
    }

    /// The resolved design.
    pub fn design(&self) -> &UnstampedResolvedDesign {
        &self.design
    }

    /// The accent this theme draws ([`design::accent_for`] in its style's
    /// widgets): the selection and accent fills, and what a desktop portal
    /// reports.
    pub fn accent(&self) -> Option<design::SrgbColour> {
        design::accent_for(self.dictionary(), self.style.widgets)
    }
}

/// An app's own theme choice over the session's: `None` on an axis follows
/// the session. On `style`, `Some(None)` is the scheme's own style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub scheme: Option<Scheme>,
    pub style: Option<Option<design::Style>>,
    pub mode: Option<Mode>,
    pub decorations: Option<Decorations>,
    pub captions: Option<CaptionSide>,
}

fn embedded_document() -> DesignSourceDocument {
    design::parse_design_source(
        SourceIdentity::new("embedded"),
        design::EMBEDDED_DEFAULT_SOURCE,
    )
    .expect("the embedded default design parses")
}

/// The selection in a file that holds only selection keys, each a string.
fn selection_only(source: &str) -> Option<Selection> {
    let value = config::parse(source).ok()?;
    let config::Value::Map(fields) = &value else {
        return None;
    };
    let mut selection = Selection::default();
    for (key, value) in fields {
        let config::Value::String(text) = value else {
            return None;
        };
        let slot = match key.as_str() {
            "scheme" => &mut selection.scheme,
            "mode" => &mut selection.mode,
            "style" => &mut selection.style,
            "decorations" => &mut selection.decorations,
            "caption_side" => &mut selection.caption_side,
            _ => return None,
        };
        *slot = Some(text.clone());
    }
    (!fields.is_empty()).then_some(selection)
}

fn axis<T: Default>(
    name: Option<&str>,
    field: &'static str,
    from_name: fn(&str) -> Option<T>,
) -> Result<T, Error> {
    match name {
        None => Ok(T::default()),
        Some(name) => from_name(name).ok_or_else(|| Error::Selection {
            field,
            value: name.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_theme_is_studio_dark() {
        let theme = Theme::embedded();
        assert_eq!(theme.scheme(), Scheme::Studio);
        assert_eq!(theme.mode(), Mode::Dark);
        assert_eq!(theme.contrast(), Contrast::Normal);
        assert_eq!(theme.context().app, None);
        assert!(theme.dictionary().colours.pairs.contains_key("base"));
    }

    #[test]
    fn every_shipped_context_compiles() {
        for scheme in Scheme::ALL {
            for mode in Mode::ALL {
                let theme = Theme::for_context(DesignContext {
                    scheme,
                    mode,
                    ..DesignContext::default()
                });
                assert_eq!((theme.scheme(), theme.mode()), (scheme, mode));
            }
        }
    }

    #[test]
    fn a_choice_overlays_the_session_theme_and_no_choice_keeps_it() {
        let session = Theme::for_context(DesignContext {
            scheme: Scheme::Pro,
            mode: Mode::Dark,
            contrast: Contrast::High,
            ..DesignContext::default()
        });
        assert_eq!(
            session.with_choice(&Choice::default()),
            session,
            "no choice: the session theme itself, unchanged"
        );
        let forest = session.with_choice(&Choice {
            scheme: Some(Scheme::Forest),
            ..Choice::default()
        });
        assert_eq!(
            (forest.scheme(), forest.mode(), forest.contrast()),
            (Scheme::Forest, Mode::Dark, Contrast::High)
        );
        let light = session.with_choice(&Choice {
            mode: Some(Mode::Light),
            ..Choice::default()
        });
        assert_eq!(
            (light.scheme(), light.mode(), light.contrast()),
            (Scheme::Pro, Mode::Light, Contrast::High)
        );
    }

    /// The style axis and the framing overlay independently: a style alone
    /// recompiles in this theme's scheme and mode; decorations alone keep
    /// this theme's design.
    #[test]
    fn a_choice_sets_the_style_and_framing_axes() {
        let session = Theme::for_context(DesignContext {
            scheme: Scheme::Forest,
            mode: Mode::Light,
            ..DesignContext::default()
        });
        let studio = session.with_choice(&Choice {
            style: Some(Some(design::Style::Studio)),
            ..Choice::default()
        });
        assert_eq!(
            (studio.scheme(), studio.mode(), studio.style_axis()),
            (Scheme::Forest, Mode::Light, Some(design::Style::Studio))
        );
        assert_eq!(
            studio.style(),
            Theme::for_context(DesignContext {
                style: Some(design::Style::Studio),
                ..DesignContext::default()
            })
            .style()
        );
        let own = studio.with_choice(&Choice {
            style: Some(None),
            ..Choice::default()
        });
        assert_eq!(own.style(), session.style(), "the scheme's own style again");
        let framed = session.with_choice(&Choice {
            decorations: Some(Decorations::Server),
            captions: Some(CaptionSide::Left),
            ..Choice::default()
        });
        assert_eq!(
            (framed.decorations(), framed.captions()),
            (Decorations::Server, CaptionSide::Left)
        );
        assert_eq!(
            framed.dictionary(),
            session.dictionary(),
            "the design is the session's"
        );
        let kept = framed.with_choice(&Choice {
            scheme: Some(Scheme::Ocean),
            ..Choice::default()
        });
        assert_eq!(
            (kept.decorations(), kept.captions()),
            (Decorations::Server, CaptionSide::Left),
            "a recompile keeps the framing"
        );
    }

    #[test]
    fn a_selection_file_sets_every_axis() {
        let theme = Theme::from_source(
            "test",
            "scheme: \"forest\"\nstyle: \"pro\"\nmode: \"dark\"\ndecorations: \"ssd\"\ncaption_side: \"left\"\n",
        )
        .unwrap();
        assert_eq!(
            (theme.scheme(), theme.style_axis(), theme.mode()),
            (Scheme::Forest, Some(design::Style::Pro), Mode::Dark)
        );
        assert_eq!(
            (theme.decorations(), theme.captions()),
            (Decorations::Server, CaptionSide::Left)
        );
        let plain = Theme::from_source("test", "scheme: \"pro\"\n").unwrap();
        assert_eq!(
            (plain.style_axis(), plain.decorations(), plain.captions()),
            (None, Decorations::Client, CaptionSide::Right)
        );
        let bad = Theme::from_source("test", "style: \"neon\"\n").unwrap_err();
        assert_eq!(bad.to_string(), "unknown style \"neon\"");
        assert!(matches!(
            Theme::from_source("test", "decorations: \"both\"\n").unwrap_err(),
            Error::Selection {
                field: "decorations",
                ..
            }
        ));
    }

    /// A full theme document takes the presentation keys beside its own
    /// scheme and mode; an invalid value is an error, never a silent default.
    #[test]
    fn a_full_document_takes_every_presentation_key() {
        let full = |extra: &str| format!("{extra}{}", design::EMBEDDED_DEFAULT_SOURCE);
        let theme = Theme::from_source(
            "test",
            &full("style: \"classic\"\ndecorations: \"ssd\"\ncaption_side: \"left\"\n"),
        )
        .unwrap();
        assert_eq!(
            (theme.scheme(), theme.mode()),
            (Scheme::Ocean, Mode::Light),
            "the document's own selection"
        );
        assert_eq!(theme.style_axis(), Some(design::Style::Classic));
        assert!(theme.style().bevels, "Classic's forms");
        assert_eq!(
            (theme.decorations(), theme.captions()),
            (Decorations::Server, CaptionSide::Left)
        );
        let plain = Theme::from_source("test", design::EMBEDDED_DEFAULT_SOURCE).unwrap();
        assert_eq!(
            (plain.style_axis(), plain.decorations(), plain.captions()),
            (None, Decorations::Client, CaptionSide::Right)
        );
        for (extra, field) in [
            ("style: \"neon\"\n", "style"),
            ("decorations: \"both\"\n", "decorations"),
            ("caption_side: \"top\"\n", "caption_side"),
        ] {
            let error = Theme::from_source("test", &full(extra)).unwrap_err();
            assert!(
                matches!(error, Error::Selection { field: f, .. } if f == field),
                "{extra}: {error}"
            );
        }
        assert!(matches!(
            Theme::from_source("test", &full("style: 4\n")).unwrap_err(),
            Error::Source(_)
        ));
    }

    #[test]
    fn selection_only_file_selects_against_the_embedded_design() {
        let theme = Theme::from_source("test", "scheme: \"forest\"\nmode: \"dark\"\n").unwrap();
        assert_eq!((theme.scheme(), theme.mode()), (Scheme::Forest, Mode::Dark));
        let partial = Theme::from_source("test", "mode: \"light\"\n").unwrap();
        assert_eq!(
            (partial.scheme(), partial.mode()),
            (Scheme::Studio, Mode::Light),
            "an absent axis takes the default"
        );
    }

    #[test]
    fn a_design_without_a_style_family_takes_the_embedded_style() {
        let mut document = embedded_document();
        document.v1.families.style = None;
        let context = DesignContext {
            scheme: Scheme::Classic,
            ..DesignContext::default()
        };
        let theme = Theme::compile(&document, context.clone()).unwrap();
        assert_eq!(theme.dictionary().style, None);
        assert_eq!(theme.style(), Theme::for_context(context).style());
    }

    /// A custom design with no style family keeps deriving the hue
    /// schemes' title bar and menus from its own spacing and radius: the
    /// embedded style's fixed lengths would shrink its hit targets.
    #[test]
    fn a_custom_design_without_a_style_family_derives_its_chrome_geometry() {
        let mut document = embedded_document();
        document.v1.families.style = None;
        document.v1.primitives.scales.get_mut("spacing").unwrap()[9] =
            design::MetricSource::px(48.0);
        document
            .v1
            .primitives
            .metrics
            .insert("radius".into(), design::MetricSource::px(10.0));
        let theme = Theme::compile(&document, DesignContext::revision_one()).unwrap();
        let m = crate::chrome::Chrome::for_theme(&theme).metrics;
        // One 48 pt control, and a 3 pt item gap above and below it.
        assert_eq!(
            (m.menu_row_height, m.menu_title_height, m.title_bar_height),
            (48.0, 48.0, 54.0)
        );
        assert_eq!((m.menu_row_padding.x, m.menu_min_width), (32.0, 384.0));
        assert_eq!(
            (m.radius_sm, m.radius, m.radius_lg, m.menu_highlight_radius),
            (10, 7, 20, 10)
        );
        // Explicit style tokens stay authoritative.
        let mut styled = embedded_document();
        styled.v1.primitives.scales.get_mut("spacing").unwrap()[9] = design::MetricSource::px(48.0);
        let theme = Theme::compile(&styled, DesignContext::revision_one()).unwrap();
        assert_eq!(
            crate::chrome::Chrome::for_theme(&theme)
                .metrics
                .title_bar_height,
            30.0
        );
    }

    #[test]
    fn bad_sources_are_reported() {
        let unknown = Theme::from_source("test", "scheme: \"neon\"\n").unwrap_err();
        assert_eq!(unknown.to_string(), "unknown scheme \"neon\"");
        let extra =
            Theme::from_source("test", "scheme: \"ocean\"\nsurface: \"#ffffff\"\n").unwrap_err();
        assert!(matches!(extra, Error::Source(_)), "{extra}");
        assert!(matches!(
            Theme::from_source("test", "{{{").unwrap_err(),
            Error::Source(_)
        ));
    }

    #[test]
    fn read_distinguishes_missing_from_broken() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(THEME_FILE);
        assert!(Theme::read(&path).unwrap().is_none());
        std::fs::write(&path, "scheme: \"stone\"\nmode: \"light\"\n").unwrap();
        assert_eq!(Theme::read(&path).unwrap().unwrap().scheme(), Scheme::Stone);
        std::fs::write(&path, "mode: \"dusk\"\n").unwrap();
        assert!(matches!(
            Theme::read(&path).unwrap_err(),
            Error::Selection { field: "mode", .. }
        ));
    }
}
