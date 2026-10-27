// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
/// The MixOS colour schemes: six hues, and five chrome schemes whose
/// light and dark modes step the brightness (`pro`: Pro and Pro Medium Gray;
/// `studio`: Studio and Studio Light; `classic`: Classic in both; Breeze and
/// Adwaita each have light and dark palettes). The
/// default is `studio`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Scheme {
    Ocean,
    Crimson,
    Stone,
    Forest,
    Sunset,
    Mono,
    Pro,
    #[default]
    Studio,
    Classic,
    Breeze,
    Adwaita,
}

impl Scheme {
    pub const ALL: [Self; 11] = [
        Self::Ocean,
        Self::Crimson,
        Self::Stone,
        Self::Forest,
        Self::Sunset,
        Self::Mono,
        Self::Pro,
        Self::Studio,
        Self::Classic,
        Self::Breeze,
        Self::Adwaita,
    ];

    /// The six hue schemes of design revision one. Tests that pin revision
    /// one's exact derivation walks iterate these; the chrome schemes carry
    /// their own parity tests.
    pub const REVISION_ONE: [Self; 6] = [
        Self::Ocean,
        Self::Crimson,
        Self::Stone,
        Self::Forest,
        Self::Sunset,
        Self::Mono,
    ];

    /// The schemes drawn from the chrome family.
    pub const fn is_chrome_scheme(self) -> bool {
        matches!(
            self,
            Self::Pro | Self::Studio | Self::Classic | Self::Breeze | Self::Adwaita
        )
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Ocean => "ocean",
            Self::Crimson => "crimson",
            Self::Stone => "stone",
            Self::Forest => "forest",
            Self::Sunset => "sunset",
            Self::Mono => "mono",
            Self::Pro => "pro",
            Self::Studio => "studio",
            Self::Classic => "classic",
            Self::Breeze => "breeze",
            Self::Adwaita => "adwaita",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scheme| scheme.name() == name)
    }
}

/// Light or dark presentation mode. The default is dark.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mode {
    Light,
    #[default]
    Dark,
}

impl Mode {
    pub const ALL: [Self; 2] = [Self::Light, Self::Dark];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

/// Normal or high-contrast selection axis.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Contrast {
    #[default]
    Normal,
    High,
}

impl Contrast {
    pub const ALL: [Self; 2] = [Self::Normal, Self::High];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::High => "high",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|contrast| contrast.name() == name)
    }
}

/// The chrome style axis: how the chrome is shaped, independent of the
/// scheme's colours. Each names a style the design authors in its style
/// family; [`DesignContext::style`] `None` takes the scheme's own.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Style {
    /// The hue schemes' own style: pair-based widgets.
    Plain,
    Pro,
    Studio,
    Classic,
    Breeze,
    Adwaita,
}

impl Style {
    pub const ALL: [Self; 6] = [
        Self::Plain,
        Self::Pro,
        Self::Studio,
        Self::Classic,
        Self::Breeze,
        Self::Adwaita,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Pro => "pro",
            Self::Studio => "studio",
            Self::Classic => "classic",
            Self::Breeze => "breeze",
            Self::Adwaita => "adwaita",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.name() == name)
    }
}

/// Who draws a window's title bar. A presentation choice, not a design one:
/// it changes no token, so it is not part of [`DesignContext`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Decorations {
    /// The application draws its own title bar (client-side).
    #[default]
    Client,
    /// The compositor decorates the window (server-side); the application
    /// shows its menus in a menu-bar row.
    Server,
}

impl Decorations {
    pub const ALL: [Self; 2] = [Self::Client, Self::Server];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Client => "csd",
            Self::Server => "ssd",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|decorations| decorations.name() == name)
    }
}

/// The requested framing. `Auto` is resolved before reaching the renderer.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RequestedDecorations {
    #[default]
    Auto,
    Client,
    Server,
}

impl RequestedDecorations {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Client, Self::Server];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Client => "csd",
            Self::Server => "ssd",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

impl From<Decorations> for RequestedDecorations {
    fn from(mode: Decorations) -> Self {
        match mode {
            Decorations::Client => Self::Client,
            Decorations::Server => Self::Server,
        }
    }
}

/// Which end of a client-side title bar carries the caption buttons.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CaptionSide {
    /// Minimize, Maximize, Close at the right, Close in the corner.
    #[default]
    Right,
    /// Close, Minimize, Maximize at the left, Close in the corner.
    Left,
}

impl CaptionSide {
    pub const ALL: [Self; 2] = [Self::Right, Self::Left];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Left => "left",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|side| side.name() == name)
    }
}

/// Window style names, Prefs labels and compositor preset entry points.
#[macro_export]
macro_rules! window_styles {
    ($registry:ident) => {
        $registry! {
            #[default]
            Mac => ("mac", "look-window-style-mac", mac_preset),
            Win11 => ("win11", "look-window-style-win11", win11_preset),
            Mixos => ("mixos", "look-window-style-mixos", mixos),
        }
    };
}

macro_rules! window_style_registry {
    ($($(#[$meta:meta])* $variant:ident => ($name:literal, $label:literal, $preset:ident),)*) => {
        /// Server-side window furniture, independent of widget style.
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum WindowStyle { $($(#[$meta])* $variant,)* }

        impl WindowStyle {
            pub const ALL: [Self; Self::NAMES.len()] = [$(Self::$variant,)*];
            pub const NAMES: &'static [&'static str] = &[$($name,)*];

            pub const fn name(self) -> &'static str {
                match self { $(Self::$variant => $name,)* }
            }

            pub const fn label_id(self) -> &'static str {
                match self { $(Self::$variant => $label,)* }
            }
        }
    };
}

window_styles!(window_style_registry);

/// Menu forms and their Prefs labels.
#[macro_export]
macro_rules! menu_styles {
    ($registry:ident) => {
        $registry! {
            #[default]
            Bar => ("bar", "look-menu-style-bar"),
            Button => ("button", "look-menu-style-button"),
            Titlebar => ("titlebar", "look-menu-style-titlebar"),
        }
    };
}

/// Menu button positions and their Prefs labels.
#[macro_export]
macro_rules! menu_sides {
    ($registry:ident) => {
        $registry! {
            #[default]
            Left => ("left", "look-menu-side-left"),
            Right => ("right", "look-menu-side-right"),
        }
    };
}

macro_rules! menu_axis {
    ($axis:ident, $($(#[$meta:meta])* $variant:ident => ($name:literal, $label:literal),)*) => {
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum $axis { $($(#[$meta])* $variant,)* }

        impl $axis {
            pub const ALL: [Self; Self::NAMES.len()] = [$(Self::$variant,)*];
            pub const NAMES: &'static [&'static str] = &[$($name,)*];

            pub const fn name(self) -> &'static str {
                match self { $(Self::$variant => $name,)* }
            }

            pub const fn label_id(self) -> &'static str {
                match self { $(Self::$variant => $label,)* }
            }

            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.into_iter().find(|axis| axis.name() == name)
            }

            pub fn resolve(name: &str) -> Self {
                Self::from_name(name).unwrap_or_default()
            }

            pub fn valid_name(name: &str) -> bool {
                valid_appearance_name(name)
            }
        }
    };
}

macro_rules! menu_style_registry {
    ($($entry:tt)*) => { menu_axis!(MenuStyle, $($entry)*); };
}
macro_rules! menu_side_registry {
    ($($entry:tt)*) => { menu_axis!(MenuSide, $($entry)*); };
}
menu_styles!(menu_style_registry);
menu_sides!(menu_side_registry);

impl MenuStyle {
    pub fn effective(self, decorations: Decorations) -> Self {
        match (self, decorations) {
            (Self::Titlebar, Decorations::Client) => Self::Button,
            _ => self,
        }
    }
}

impl WindowStyle {
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.name() == name)
    }

    pub fn resolve(name: &str) -> Self {
        Self::from_name(name).unwrap_or_default()
    }

    /// Unknown names are retained for profiles shared with newer binaries.
    pub fn valid_name(name: &str) -> bool {
        valid_appearance_name(name)
    }
}

fn valid_appearance_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The compile-time selection used to flatten a design source.
///
/// Per-app overlays are compile-time selections, never runtime fallbacks.
/// The default is studio, dark, normal contrast, the scheme's own style, no
/// app. The scheme chooses the colours and the style the chrome's forms, so
/// any scheme can take any style.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DesignContext {
    pub scheme: Scheme,
    pub mode: Mode,
    pub contrast: Contrast,
    /// The chrome style; `None` takes the style the design binds to the
    /// scheme.
    pub style: Option<Style>,
    /// Stable application identity selecting a compile-time per-app overlay.
    /// `None` is the unoverlaid context used by the v0 equivalence gate.
    pub app: Option<String>,
}

impl DesignContext {
    /// Revision one's selection, the default before Studio Dark: ocean,
    /// light, normal contrast, no app. Tests that pin revision one's exact
    /// values, and the v0 equivalence gate's legacy fields, are written
    /// against it.
    pub const fn revision_one() -> Self {
        Self {
            scheme: Scheme::Ocean,
            mode: Mode::Light,
            contrast: Contrast::Normal,
            style: None,
            app: None,
        }
    }
}

#[cfg(test)]
mod window_style_tests {
    use super::WindowStyle;

    #[test]
    fn registry_names_labels_and_fallback() {
        for style in WindowStyle::ALL {
            assert!(WindowStyle::valid_name(style.name()));
            assert_eq!(WindowStyle::from_name(style.name()), Some(style));
            assert!(style.label_id().starts_with("look-window-style-"));
        }
        assert!(WindowStyle::valid_name("future-style"));
        assert_eq!(WindowStyle::resolve("future-style"), WindowStyle::Mac);
        for name in ["", "MAC", "two words", "../style"] {
            assert!(!WindowStyle::valid_name(name));
        }
        assert!(!WindowStyle::valid_name(&"a".repeat(65)));
    }
}

#[cfg(test)]
mod menu_tests {
    use super::*;

    #[test]
    fn menu_registries_and_decoration_resolution() {
        for style in MenuStyle::ALL {
            assert_eq!(MenuStyle::from_name(style.name()), Some(style));
            assert!(MenuStyle::valid_name(style.name()));
            assert!(style.label_id().starts_with("look-menu-style-"));
            assert_eq!(style.effective(Decorations::Server), style);
        }
        for side in MenuSide::ALL {
            assert_eq!(MenuSide::from_name(side.name()), Some(side));
            assert!(MenuSide::valid_name(side.name()));
            assert!(side.label_id().starts_with("look-menu-side-"));
        }
        assert_eq!(
            MenuStyle::Titlebar.effective(Decorations::Client),
            MenuStyle::Button
        );
        assert_eq!(MenuStyle::resolve("future-style"), MenuStyle::Bar);
        assert_eq!(MenuSide::resolve("future-side"), MenuSide::Left);
        for name in ["", "two words", "LEFT", "../menu"] {
            assert!(!MenuStyle::valid_name(name));
            assert!(!MenuSide::valid_name(name));
        }
    }
}
