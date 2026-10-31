// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The look's authored inputs: `appearance.*`, the `state.*` keys the mode
//! resolves from, the per-app overrides `apps.<app>.*` and `tune.*`. These
//! are the values as authored (the mode may be `auto`); [`super::Look`]
//! composes them for followers and resolution.
use super::{Reader, View, family};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Appearance {
    /// `dark`, `light` or `auto`, as authored; resolution turns `auto` into
    /// one of the other two ([`crate::resolve::values::effective_mode`]).
    pub mode: String,
    pub scheme: String,
    /// None is the scheme's own style.
    pub style: Option<String>,
    pub accent: Accent,
    /// The derived scheme's seeds (§3.3 A3); None is `auto`.
    pub window_bg: Option<String>,
    pub container_bg: Option<String>,
    pub text_tint: Option<String>,
    pub control_tint: Option<String>,
    /// `normal` or `high`.
    pub contrast: String,
    /// The geometry axes, as `design::geometry` names them (A4, A5).
    pub roundness: Roundness,
    pub density: Density,
    pub frosted: Frosted,
    pub font_family: Option<String>,
    pub font_weight: Option<u16>,
    pub font_size: Option<f64>,
    pub mono_family: Option<String>,
    pub icon_stroke: Option<f64>,
    /// Nine swatches replacing COSMIC's; None is the nine of `libs/design`.
    pub accent_palette: Option<Vec<String>>,
    pub export_toolkits: bool,
    pub export_desktop: bool,
    /// A complete strict design source; None is the profile's pinned one.
    pub source: Option<String>,
}

/// `appearance.accent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Accent {
    /// The scheme's own accent.
    Auto,
    /// One of the nine swatch names, resolved per mode in `libs/design`.
    Swatch(String),
    /// `#rrggbb`, used as is.
    Colour(String),
}

impl Accent {
    fn of(text: &str) -> Self {
        match text {
            "auto" => Self::Auto,
            colour if colour.starts_with('#') => Self::Colour(colour.into()),
            name => Self::Swatch(name.into()),
        }
    }
}

/// `appearance.density` (three steps, never a number, F3) and
/// `appearance.roundness` (three presets) are `design::geometry`'s own axes:
/// `comfortable` is its `Standard`.
pub use design::geometry::{Density, Roundness};

#[derive(Clone, Debug, PartialEq)]
pub struct Frosted {
    /// 0 to 13; 0 is no blur.
    pub thickness: u32,
    pub opacity: f64,
    pub panel: bool,
    pub applets: bool,
    pub system: bool,
    pub windows: bool,
}

/// A colour key whose `auto` is None.
fn colour_or_auto(r: &Reader, path: &str) -> Option<String> {
    r.opt_text(path).filter(|v| v != "auto")
}

impl View for Appearance {
    fn read(r: &Reader) -> Self {
        Self {
            mode: r.text("appearance.mode"),
            scheme: r.text("appearance.scheme"),
            style: r.opt_text("appearance.style"),
            accent: Accent::of(&r.text("appearance.accent")),
            window_bg: colour_or_auto(r, "appearance.window_bg"),
            container_bg: colour_or_auto(r, "appearance.container_bg"),
            text_tint: colour_or_auto(r, "appearance.text_tint"),
            control_tint: colour_or_auto(r, "appearance.control_tint"),
            contrast: r.text("appearance.contrast"),
            roundness: Roundness::from_name(&r.text("appearance.roundness")).unwrap_or_default(),
            density: Density::from_name(&r.text("appearance.density")).unwrap_or_default(),
            frosted: Frosted {
                thickness: r.uint("appearance.frosted.thickness"),
                opacity: r.number("appearance.frosted.opacity"),
                panel: r.bool("appearance.frosted.panel"),
                applets: r.bool("appearance.frosted.applets"),
                system: r.bool("appearance.frosted.system"),
                windows: r.bool("appearance.frosted.windows"),
            },
            font_family: r.opt_text("appearance.font_family"),
            font_weight: r
                .opt_int("appearance.font_weight")
                .and_then(|w| u16::try_from(w).ok()),
            font_size: r.opt_number("appearance.font_size"),
            mono_family: r.opt_text("appearance.mono_family"),
            icon_stroke: r.opt_number("appearance.icon_stroke"),
            accent_palette: match r.value("appearance.accent_palette") {
                Value::Null => None,
                list => Some(super::strings(&list)),
            },
            export_toolkits: r.bool("appearance.export_toolkits"),
            export_desktop: r.bool("appearance.export_desktop"),
            source: r.opt_text("appearance.source"),
        }
    }
}

/// The internal `state.*` keys (§2.17).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The sun's state, from the daylight citizen; None while it has never
    /// written (Auto is then disabled in Prefs, and resolves dark).
    pub daylight: Option<bool>,
    /// A manual `dark` or `light` while the mode is `auto`, held until the
    /// next sunrise or sunset.
    pub mode_override: Option<String>,
}

impl View for State {
    fn read(r: &Reader) -> Self {
        Self {
            daylight: r.opt_bool("state.appearance.daylight"),
            mode_override: r.opt_text("state.appearance.mode_override"),
        }
    }
}

/// `apps.<app>.*`: one app's look overrides. None takes the desktop's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppLook {
    pub scheme: Option<String>,
    /// `dark` or `light`: an app override never says `auto`.
    pub mode: Option<String>,
    pub contrast: Option<String>,
    pub text_scale: Option<f64>,
}

/// The apps with an override, by id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppLooks(pub BTreeMap<String, AppLook>);

impl View for AppLooks {
    fn read(r: &Reader) -> Self {
        Self(family(r, "apps.", 1, |app| AppLook {
            scheme: r.opt_text(&format!("apps.{app}.scheme")),
            mode: r.opt_text(&format!("apps.{app}.mode")),
            contrast: r.opt_text(&format!("apps.{app}.contrast")),
            text_scale: r.opt_number(&format!("apps.{app}.text_scale")),
        }))
    }
}

/// `tune.*` (ADR R4): authored tunables by name (`style.radius`,
/// `chrome.accent`), each checked against `design::tunables()`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tune(pub BTreeMap<String, Value>);

impl View for Tune {
    fn read(r: &Reader) -> Self {
        let paths: Vec<&String> = r
            .values()
            .keys()
            .filter(|k| k.starts_with("tune."))
            .collect();
        Self(
            paths
                .into_iter()
                .filter_map(|p| Some((p["tune.".len()..].to_owned(), r.authored(p)?)))
                .collect(),
        )
    }
}
