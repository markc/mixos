// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Portal values for the `org.freedesktop.appearance` namespace, derived from
//! one accepted appearance projection. The colour scheme and accent come from
//! `design::palette` (ruling 1): settingsd serves that palette as
//! `settings.palette` (S7), and until it does, [`palette_of`] computes the
//! same function here. Pure data; the D-Bus surface lives in `portal`.
use design::derive::FrostedGlass;
use design::geometry::Geometry;
use design::palette::ModePalette;
use design::{
    AccentChoice, Colour8, Contrast, DesignContext, DesignSourceDocument, EMBEDDED_DEFAULT_SOURCE,
    Mode, Scheme, SourceIdentity, parse_design_source,
};
use settings::appearance::AppearanceProjection;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use zbus::zvariant::Value;

pub const NAMESPACE: &str = "org.freedesktop.appearance";
pub const COLOR_SCHEME: &str = "color-scheme";
pub const ACCENT_COLOR: &str = "accent-color";
pub const CONTRAST: &str = "contrast";
/// Every served key, in the order `ReadAll` lists them.
pub const KEYS: [&str; 3] = [COLOR_SCHEME, ACCENT_COLOR, CONTRAST];
pub const MAX_FILTERS: usize = 64;
pub const MAX_FILTER_BYTES: usize = 256;

pub fn valid_filters(filters: &[String]) -> bool {
    filters.len() <= MAX_FILTERS && filters.iter().all(|s| s.len() <= MAX_FILTER_BYTES)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Values {
    /// 1 dark, 2 light (0 is no preference and is never published here).
    pub color_scheme: u32,
    /// 0 normal, 1 high.
    pub contrast: u32,
    /// sRGB 0.0..=1.0 per channel. `None` means the key is not served.
    pub accent: Option<[f64; 3]>,
}

impl Values {
    /// Only used when neither settingsd nor a validated cache is available:
    /// dark and normal contrast, with no accent published.
    pub fn defaults() -> Self {
        Self {
            color_scheme: 1,
            contrast: 0,
            accent: None,
        }
    }

    /// The projection's mode and contrast; its accent as the palette resolves
    /// it. A palette that cannot be computed keeps the projected accent.
    pub fn from_projection(projection: &AppearanceProjection) -> Self {
        Self {
            color_scheme: match projection.mode.as_str() {
                "dark" => 1,
                "light" => 2,
                _ => 1,
            },
            contrast: match projection.contrast.as_str() {
                "high" => 1,
                _ => 0,
            },
            accent: Some(
                palette_of(projection).map_or(projection.accent, |palette| accent_of(&palette)),
            ),
        }
    }

    /// The single-layer value of one key, or `None` when it is not served.
    pub fn get(&self, namespace: &str, key: &str) -> Option<Value<'static>> {
        if namespace != NAMESPACE {
            return None;
        }
        match key {
            COLOR_SCHEME => Some(Value::U32(self.color_scheme)),
            CONTRAST => Some(Value::U32(self.contrast)),
            ACCENT_COLOR => self.accent.map(|[r, g, b]| Value::from((r, g, b))),
            _ => None,
        }
    }

    /// Every served key whose namespace matches one of the globs.
    pub fn matching(&self, globs: &[String]) -> BTreeMap<String, BTreeMap<String, Value<'static>>> {
        let mut out = BTreeMap::new();
        if !valid_filters(globs)
            || !(globs.is_empty()
                || globs
                    .iter()
                    .any(|glob| glob.is_empty() || glob_match(glob, NAMESPACE)))
        {
            return out;
        }
        let mut keys = BTreeMap::new();
        for key in KEYS {
            if let Some(value) = self.get(NAMESPACE, key) {
                keys.insert(key.to_owned(), value);
            }
        }
        if !keys.is_empty() {
            out.insert(NAMESPACE.to_owned(), keys);
        }
        out
    }

    /// Keys whose value differs from `self`, with their new values. A key that
    /// stops being served has no value to announce, so it is not listed.
    pub fn changed(&self, next: &Self) -> Vec<(&'static str, Value<'static>)> {
        let mut out = Vec::new();
        if self.color_scheme != next.color_scheme {
            out.push((COLOR_SCHEME, Value::U32(next.color_scheme)));
        }
        if self.contrast != next.contrast {
            out.push((CONTRAST, Value::U32(next.contrast)));
        }
        if self.accent != next.accent
            && let Some(value) = next.get(NAMESPACE, ACCENT_COLOR)
        {
            out.push((ACCENT_COLOR, value));
        }
        out
    }
}

/// The palette of the projection's mode, from `design::palette`. The
/// projected accent is passed as a custom accent, so the palette's accent is
/// the one settingsd resolved. This is the single adapter until S7 serves
/// `settings.palette`; `None` only if the embedded source does not compile.
pub fn palette_of(projection: &AppearanceProjection) -> Option<ModePalette> {
    let context = DesignContext {
        scheme: Scheme::Mixos,
        mode: Mode::from_name(&projection.mode).unwrap_or(Mode::Dark),
        contrast: Contrast::from_name(&projection.contrast).unwrap_or(Contrast::Normal),
        accent: AccentChoice::Custom(opaque(projection.accent)),
        ..DesignContext::default()
    };
    design::palette::mode_palette(
        embedded_source()?,
        &context,
        &Geometry::default(),
        &FrostedGlass::default(),
    )
    .ok()
}

/// The accent a palette resolves, as the portal publishes it.
pub fn accent_of(palette: &ModePalette) -> [f64; 3] {
    [palette.accent.red, palette.accent.green, palette.accent.blue]
}

/// The embedded default design source, parsed once.
fn embedded_source() -> Option<&'static DesignSourceDocument> {
    static SOURCE: OnceLock<Option<DesignSourceDocument>> = OnceLock::new();
    SOURCE
        .get_or_init(|| {
            parse_design_source(SourceIdentity::new("embedded"), EMBEDDED_DEFAULT_SOURCE).ok()
        })
        .as_ref()
}

/// An opaque colour from sRGB 0.0..=1.0 channels.
fn opaque([red, green, blue]: [f64; 3]) -> Colour8 {
    let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    Colour8([channel(red), channel(green), channel(blue), 255])
}

/// Iterative dynamic programming: O(pattern bytes * text bytes) time and
/// O(text bytes) space, with no recursive or combinatorial backtracking.
/// `*` matches any run, `?` one byte. Portal callers bound filter size/count.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern.len() > MAX_FILTER_BYTES {
        return false;
    }
    let mut matches = vec![false; text.len() + 1];
    matches[0] = true;
    for byte in pattern.bytes() {
        if byte == b'*' {
            for index in 1..=text.len() {
                matches[index] |= matches[index - 1];
            }
        } else {
            for index in (1..=text.len()).rev() {
                matches[index] =
                    matches[index - 1] && (byte == b'?' || byte == text.as_bytes()[index - 1]);
            }
            matches[0] = false;
        }
    }
    matches[text.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use settings::Binding;

    fn projection(mode: &str, contrast: &str, accent: [f64; 3]) -> AppearanceProjection {
        AppearanceProjection {
            schema: settings::appearance::APPEARANCE_SCHEMA,
            binding: Binding {
                instance: "host".into(),
                profile: "default".into(),
            },
            incarnation: "inc".into(),
            revision: settings::Revision(1),
            design_revision: settings::Revision(1),
            mode: mode.into(),
            contrast: contrast.into(),
            caption_side: "right".into(),
            accent,
        }
    }

    #[test]
    fn maps_mode_contrast_and_accent_to_portal_values() {
        let dark = Values::from_projection(&projection("dark", "normal", [0.1, 0.2, 0.3]));
        assert_eq!(dark.color_scheme, 1);
        assert_eq!(dark.contrast, 0);
        let light = Values::from_projection(&projection("light", "high", [0.1, 0.2, 0.3]));
        assert_eq!(light.color_scheme, 2);
        assert_eq!(light.contrast, 1);
        assert!(matches!(
            light.get(NAMESPACE, ACCENT_COLOR),
            Some(Value::Structure(_))
        ));
    }

    #[test]
    fn the_served_accent_is_the_palettes_accent_for_the_projected_mode() {
        let projected = projection("light", "normal", [0.2, 0.4, 0.6]);
        let palette = palette_of(&projected).expect("the embedded source compiles");
        assert_eq!(palette.mode, "light");
        assert_eq!(palette.contrast, "normal");
        let served = Values::from_projection(&projected);
        let [red, green, blue] = served.accent.expect("an accent is served");
        // Eight bits per channel, so each channel is within one step.
        for (got, asked) in [red, green, blue].into_iter().zip([0.2, 0.4, 0.6]) {
            assert!((got - asked).abs() <= 1.0 / 255.0, "{got} vs {asked}");
        }
        assert_eq!(served.accent, Some(accent_of(&palette)));
    }

    #[test]
    fn the_palette_contrast_follows_the_projection() {
        let high = palette_of(&projection("dark", "high", [0.1, 0.2, 0.3])).unwrap();
        assert_eq!(high.contrast, "high");
        assert_eq!(high.mode, "dark");
    }

    #[test]
    fn defaults_serve_dark_normal_and_no_accent() {
        let defaults = Values::defaults();
        assert_eq!(defaults.get(NAMESPACE, COLOR_SCHEME), Some(Value::U32(1)));
        assert_eq!(defaults.get(NAMESPACE, CONTRAST), Some(Value::U32(0)));
        assert!(defaults.get(NAMESPACE, ACCENT_COLOR).is_none());
    }

    #[test]
    fn unknown_namespaces_and_keys_are_not_served() {
        let values = Values::defaults();
        assert!(values.get("org.gnome.desktop", COLOR_SCHEME).is_none());
        assert!(values.get(NAMESPACE, "font-name").is_none());
    }

    #[test]
    fn globs_select_namespaces() {
        assert!(glob_match("org.freedesktop.*", NAMESPACE));
        assert!(glob_match("*", NAMESPACE));
        assert!(glob_match("org.freedesktop.appearanc?", NAMESPACE));
        assert!(!glob_match("org.gnome.*", NAMESPACE));
        assert!(!glob_match("org.freedesktop", NAMESPACE));
        let values = Values::defaults();
        assert_eq!(
            values.matching(&["org.freedesktop.*".into()])[NAMESPACE].len(),
            2
        );
        assert!(values.matching(&["org.gnome.*".into()]).is_empty());
    }

    #[test]
    fn changed_lists_only_keys_whose_values_moved() {
        let base = Values::from_projection(&projection("dark", "normal", [0.1, 0.2, 0.3]));
        assert!(base.changed(&base.clone()).is_empty());
        let light = Values::from_projection(&projection("light", "normal", [0.1, 0.2, 0.3]));
        let changes = base.changed(&light);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, COLOR_SCHEME);
        assert_eq!(changes[0].1, Value::U32(2));
    }
}
