// SPDX-License-Identifier: MIT OR Apache-2.0
//! A compiled MixOS theme: the resolved design for one scheme, mode and
//! contrast, together with the context it was compiled for.
//!
//! The `design` crate's resolved artifact does not record its context, so a
//! consumer that compiled it would otherwise lose the scheme and mode it
//! asked for. [`Theme`] keeps the two together. It is built from the
//! embedded default design, from a full `theme.conf.mix` document, or from
//! the shared selection-only file (`scheme:` and `mode:` alone), which is
//! resolved against the embedded design.
//!
//! Transplanted from `libs/appearance/src/theme.rs` (markc/mixos-iced), less
//! the iced font installation.

use std::fmt;
use std::path::{Path, PathBuf};

use design::{
    Contrast, DesignCompileResult, DesignContext, DesignDiagnostic, DesignSourceDocument,
    DesignSourceError, LegacyV0Source, Mode, ResolvedDictionary, ResolvedTypography, Scheme,
    SourceIdentity, UnstampedResolvedDesign,
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
    Read { path: PathBuf, error: std::io::Error },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => write!(f, "theme source: {error}"),
            Self::Selection { field, value } => write!(f, "unknown {field} {value:?}"),
            Self::Compile(diagnostics) => {
                write!(f, "theme did not compile:")?;
                for diagnostic in diagnostics {
                    write!(f, " [{} {}: {}]", diagnostic.code, diagnostic.path, diagnostic.message)?;
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

/// The resolved design for one context.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    context: DesignContext,
    design: UnstampedResolvedDesign,
}

impl Theme {
    /// The embedded default design in the default selection: studio, dark,
    /// normal contrast.
    pub fn embedded() -> Self {
        Self::for_context(DesignContext::default())
    }

    /// The embedded default design compiled for `context`.
    ///
    /// # Panics
    /// The embedded design claims every scheme, mode and contrast and the
    /// compiler compiles each claimed context before it returns one, so a
    /// failure here is a broken build, not a runtime condition.
    pub fn for_context(context: DesignContext) -> Self {
        let document = embedded_document();
        Self::compile(&document, context).expect("the embedded default design compiles")
    }

    /// A theme from `source`: a full design document (its own `scheme:` and
    /// `mode:` select the context), or the shared selection-only file
    /// holding nothing but `scheme:` and `mode:`, resolved against the
    /// embedded design. `identity` names the source in diagnostics.
    pub fn from_source(identity: &str, source: &str) -> Result<Self, Error> {
        let (document, selection) = match design::parse_design_source(SourceIdentity::new(identity), source) {
            Ok(document) => {
                let selection = document.legacy.clone();
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
            app: None,
        };
        Self::compile(&document, context)
    }

    /// The theme in the file at `path`: `None` when there is no file, an
    /// error when there is one that does not read, parse or compile.
    pub fn read(path: &Path) -> Result<Option<Self>, Error> {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Error::Read { path: path.to_path_buf(), error }),
        };
        Self::from_source(&path.display().to_string(), &source).map(Some)
    }

    /// The shared theme file ([`theme_path`]), or the embedded default when
    /// there is none or it is unusable. A caller that wants the reason uses
    /// [`Theme::read`].
    pub fn load() -> Self {
        match Self::read(&theme_path()) {
            Ok(Some(theme)) => theme,
            Ok(None) | Err(_) => Self::embedded(),
        }
    }

    fn compile(document: &DesignSourceDocument, context: DesignContext) -> Result<Self, Error> {
        match design::compile_design(document, context.clone()) {
            DesignCompileResult::Success(success) => Ok(Self { context, design: success.candidate }),
            DesignCompileResult::Fatal(failure) => Err(Error::Compile(failure.diagnostics)),
        }
    }

    /// This (session) theme with an app's own choice laid over it, as the
    /// Theme menu ([`crate::theme_menu`]) makes it: with neither axis chosen,
    /// the theme itself, so a custom session design stays intact; else the
    /// embedded design in the chosen scheme and mode (each axis left `None`
    /// keeps this theme's), at this theme's contrast. The session's theme
    /// file is never touched.
    pub fn with_choice(&self, scheme: Option<Scheme>, mode: Option<Mode>) -> Self {
        if scheme.is_none() && mode.is_none() {
            return self.clone();
        }
        Self::for_context(DesignContext {
            scheme: scheme.unwrap_or(self.scheme()),
            mode: mode.unwrap_or(self.mode()),
            ..self.context.clone()
        })
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

    pub fn dictionary(&self) -> &ResolvedDictionary {
        self.design.dictionary()
    }

    pub fn typography(&self) -> &ResolvedTypography {
        self.design.typography()
    }
}

fn embedded_document() -> DesignSourceDocument {
    design::parse_design_source(SourceIdentity::new("embedded"), design::EMBEDDED_DEFAULT_SOURCE)
        .expect("the embedded default design parses")
}

/// The selection in a file that holds only `scheme:` and `mode:`.
fn selection_only(source: &str) -> Option<LegacyV0Source> {
    let value = config::parse(source).ok()?;
    let config::Value::Map(fields) = &value else {
        return None;
    };
    if fields.keys().any(|key| key != "scheme" && key != "mode") {
        return None;
    }
    let selection = design::parse_legacy_v0_source(source).ok()?;
    selection.is_selection_only().then_some(selection)
}

fn axis<T: Default>(name: Option<&str>, field: &'static str, from_name: fn(&str) -> Option<T>) -> Result<T, Error> {
    match name {
        None => Ok(T::default()),
        Some(name) => from_name(name).ok_or_else(|| Error::Selection { field, value: name.to_owned() }),
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
                let theme = Theme::for_context(DesignContext { scheme, mode, ..DesignContext::default() });
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
        assert_eq!(session.with_choice(None, None), session, "no choice: the session theme itself, unchanged");
        let forest = session.with_choice(Some(Scheme::Forest), None);
        assert_eq!((forest.scheme(), forest.mode(), forest.contrast()), (Scheme::Forest, Mode::Dark, Contrast::High));
        let light = session.with_choice(None, Some(Mode::Light));
        assert_eq!((light.scheme(), light.mode(), light.contrast()), (Scheme::Pro, Mode::Light, Contrast::High));
    }

    #[test]
    fn selection_only_file_selects_against_the_embedded_design() {
        let theme = Theme::from_source("test", "scheme: \"forest\"\nmode: \"dark\"\n").unwrap();
        assert_eq!((theme.scheme(), theme.mode()), (Scheme::Forest, Mode::Dark));
        let partial = Theme::from_source("test", "mode: \"light\"\n").unwrap();
        assert_eq!((partial.scheme(), partial.mode()), (Scheme::Studio, Mode::Light), "an absent axis takes the default");
    }

    #[test]
    fn bad_sources_are_reported() {
        let unknown = Theme::from_source("test", "scheme: \"neon\"\n").unwrap_err();
        assert_eq!(unknown.to_string(), "unknown scheme \"neon\"");
        let extra = Theme::from_source("test", "scheme: \"ocean\"\nsurface: \"#ffffff\"\n").unwrap_err();
        assert!(matches!(extra, Error::Source(_)), "{extra}");
        assert!(matches!(Theme::from_source("test", "{{{").unwrap_err(), Error::Source(_)));
    }

    #[test]
    fn read_distinguishes_missing_from_broken() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(THEME_FILE);
        assert!(Theme::read(&path).unwrap().is_none());
        std::fs::write(&path, "scheme: \"stone\"\nmode: \"light\"\n").unwrap();
        assert_eq!(Theme::read(&path).unwrap().unwrap().scheme(), Scheme::Stone);
        std::fs::write(&path, "mode: \"dusk\"\n").unwrap();
        assert!(matches!(Theme::read(&path).unwrap_err(), Error::Selection { field: "mode", .. }));
    }
}
