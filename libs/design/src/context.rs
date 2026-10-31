// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
/// The MixOS colour schemes: six hues, five chrome schemes whose
/// light and dark modes step the brightness (`pro`: Pro and Pro Medium Gray;
/// `studio`: Studio and Studio Light; `classic`: Classic in both; Breeze and
/// Adwaita each have light and dark palettes), and `mixos`, the one derived
/// scheme: authored as seeds, its roles computed by [`crate::derive`]
/// ([`crate::seeds`]). The default is `studio`.
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
    Mixos,
}

impl Scheme {
    pub const ALL: [Self; 12] = [
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
        Self::Mixos,
    ];

    /// The schemes authored as 31 roles, every scheme but `mixos`.
    pub const AUTHORED: [Self; 11] = [
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

    /// The scheme is authored as seeds, so the tints have a slot in it.
    pub const fn is_derived(self) -> bool {
        matches!(self, Self::Mixos)
    }

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
            Self::Pro | Self::Studio | Self::Classic | Self::Breeze | Self::Adwaita | Self::Mixos
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
            Self::Mixos => "mixos",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scheme| scheme.name() == name)
    }
}

/// An exact 8-bit sRGB colour with straight alpha, `[r, g, b, a]`: how a
/// settings colour (`#rrggbb` or `#rrggbbaa`) enters a compile, so a
/// context stays hashable and ordered.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Colour8(pub [u8; 4]);

impl Colour8 {
    /// `#rrggbb` or `#rrggbbaa`, either case.
    pub fn from_hex(text: &str) -> Option<Self> {
        crate::SrgbColour::from_hex(text).map(|colour| Self(colour.to_srgba8()))
    }

    /// `#rrggbb` when opaque, else `#rrggbbaa`, lower case: the settings
    /// wire form.
    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.0;
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }

    /// This colour with alpha 255.
    pub const fn opaque(self) -> Self {
        let [r, g, b, _] = self.0;
        Self([r, g, b, 255])
    }

    pub fn srgb(self) -> crate::SrgbColour {
        let [red, green, blue, alpha] = self.0.map(|c| f64::from(c) / 255.0);
        crate::SrgbColour {
            red,
            green,
            blue,
            alpha,
        }
    }
}

/// The nine accent swatches, resolved per mode from
/// [`crate::cosmic_tables::BasePalette`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Swatch {
    Blue,
    Indigo,
    Purple,
    Pink,
    Red,
    Orange,
    Yellow,
    Green,
    WarmGrey,
}

impl Swatch {
    /// In [`crate::cosmic_tables::SWATCH_NAMES`] order.
    pub const ALL: [Self; 9] = [
        Self::Blue,
        Self::Indigo,
        Self::Purple,
        Self::Pink,
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::WarmGrey,
    ];

    pub const fn name(self) -> &'static str {
        crate::cosmic_tables::SWATCH_NAMES[self as usize]
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|swatch| swatch.name() == name)
    }

    /// The swatch's colour in `mode` (the high-contrast palettes carry the
    /// same colours).
    pub fn colour(self, mode: Mode) -> crate::SrgbColour {
        crate::cosmic_tables::BasePalette::builtin(mode, Contrast::Normal).swatches()[self as usize]
    }
}

/// The accent axis (`appearance.accent`): the scheme's own, a swatch
/// resolved per mode, or one opaque colour for both modes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AccentChoice {
    /// The scheme's authored accent; for `mixos`, the palette's blue.
    #[default]
    Auto,
    Swatch(Swatch),
    /// Opaque; alpha is forced to 255.
    Custom(Colour8),
}

impl AccentChoice {
    /// `auto`, a swatch name, or `#rrggbb`.
    pub fn from_name(text: &str) -> Option<Self> {
        if text == "auto" {
            return Some(Self::Auto);
        }
        if let Some(swatch) = Swatch::from_name(text) {
            return Some(Self::Swatch(swatch));
        }
        (text.len() == 7)
            .then(|| Colour8::from_hex(text))
            .flatten()
            .map(Self::Custom)
    }

    pub fn name(self) -> String {
        match self {
            Self::Auto => "auto".into(),
            Self::Swatch(swatch) => swatch.name().into(),
            Self::Custom(colour) => colour.opaque().to_hex(),
        }
    }

    /// The colour this choice draws in `mode`; `None` for `auto`.
    pub fn resolve(self, mode: Mode) -> Option<crate::SrgbColour> {
        match self {
            Self::Auto => None,
            Self::Swatch(swatch) => Some(swatch.colour(mode)),
            Self::Custom(colour) => Some(colour.opaque().srgb()),
        }
    }
}

/// The four tint axes: seeds of the derived scheme, ignored under an
/// authored one (`None` is `auto`). The two backgrounds keep their alpha;
/// the two tints are opaque.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Tints {
    /// `appearance.window_bg`.
    pub window_bg: Option<Colour8>,
    /// `appearance.container_bg`.
    pub container_bg: Option<Colour8>,
    /// `appearance.text_tint`.
    pub text_tint: Option<Colour8>,
    /// `appearance.control_tint`.
    pub control_tint: Option<Colour8>,
}

impl Tints {
    pub const fn is_auto(&self) -> bool {
        self.window_bg.is_none()
            && self.container_bg.is_none()
            && self.text_tint.is_none()
            && self.control_tint.is_none()
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

/// Window control alignment in client and server title bands.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CaptionSide {
    #[default]
    Auto,
    /// Minimize, Maximize, Close at the right, Close in the corner.
    Right,
    /// Close, Minimize, Maximize at the left, Close in the corner.
    Left,
    Center,
}

impl CaptionSide {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Left, Self::Center, Self::Right];
    pub const NAMES: &'static [&'static str] = &["auto", "left", "center", "right"];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Right => "right",
            Self::Left => "left",
            Self::Center => "center",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|side| side.name() == name)
    }

    pub fn resolve(name: &str) -> Self {
        Self::from_name(name).unwrap_or_default()
    }

    pub fn valid_name(name: &str) -> bool {
        valid_appearance_name(name)
    }
    pub fn label_id(self) -> &'static str {
        match self {
            Self::Auto => "look-align-auto",
            Self::Left => "look-menu-side-left",
            Self::Center => "look-menu-side-center",
            Self::Right => "look-menu-side-right",
        }
    }
    pub fn alignment(self, own: crate::title_band::Alignment) -> crate::title_band::Alignment {
        use crate::title_band::Alignment;
        match self {
            Self::Auto => own,
            Self::Left => Alignment::Left,
            Self::Center => Alignment::Center,
            Self::Right => Alignment::Right,
        }
    }
}

/// Window style names, Prefs labels and compositor preset entry points.
#[macro_export]
macro_rules! window_styles {
    ($registry:ident) => {
        $registry! {
            #[default]
            Mac => ("mac", "look-window-style-mac", mac_preset, Center),
            Win11 => ("win11", "look-window-style-win11", win11_preset, Left),
            Mixos => ("mixos", "look-window-style-mixos", mixos, Center),
        }
    };
}

macro_rules! window_style_registry {
    ($($(#[$meta:meta])* $variant:ident => ($name:literal, $label:literal, $preset:ident, $title:ident),)*) => {
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

            pub const fn title_alignment(self) -> crate::title_band::Alignment {
                match self { $(Self::$variant => crate::title_band::Alignment::$title,)* }
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
            Center => ("center", "look-menu-side-center"),
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

menu_axis!(TitleAlign,
    #[default] Auto => ("auto", "look-align-auto"),
    Left => ("left", "look-menu-side-left"),
    Center => ("center", "look-menu-side-center"),
    Right => ("right", "look-menu-side-right"),
);
menu_axis!(ActionsAlign,
    Left => ("left", "look-menu-side-left"),
    Center => ("center", "look-menu-side-center"),
    #[default] Right => ("right", "look-menu-side-right"),
);
menu_axis!(ActionsPlace,
    #[default] Row => ("row", "look-actions-place-row"),
    Titlebar => ("titlebar", "look-actions-place-titlebar"),
);

impl MenuSide {
    pub fn alignment(self) -> crate::title_band::Alignment {
        use crate::title_band::Alignment;
        match self {
            Self::Left => Alignment::Left,
            Self::Center => Alignment::Center,
            Self::Right => Alignment::Right,
        }
    }
}
impl ActionsAlign {
    pub fn alignment(self) -> crate::title_band::Alignment {
        use crate::title_band::Alignment;
        match self {
            Self::Left => Alignment::Left,
            Self::Center => Alignment::Center,
            Self::Right => Alignment::Right,
        }
    }
}
impl TitleAlign {
    pub fn alignment(self, own: crate::title_band::Alignment) -> crate::title_band::Alignment {
        use crate::title_band::Alignment;
        match self {
            Self::Auto => own,
            Self::Left => Alignment::Left,
            Self::Center => Alignment::Center,
            Self::Right => Alignment::Right,
        }
    }
}

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
    /// The accent axis; `auto` keeps the scheme's own.
    pub accent: AccentChoice,
    /// The derived scheme's seeds; every authored scheme ignores them.
    pub tints: Tints,
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
            accent: AccentChoice::Auto,
            tints: Tints {
                window_bg: None,
                container_bg: None,
                text_tint: None,
                control_tint: None,
            },
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
