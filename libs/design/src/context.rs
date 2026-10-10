/// The MixOS colour schemes: six hues, and five chrome schemes whose
/// light and dark modes step the brightness (`pro`: Pro and Pro Medium Gray;
/// `studio`: Studio and Studio Light; `classic`: Classic in both;
/// `adwaita`: Adwaita Light and Dark; `solarized`: Studio Light and
/// Solarized Dark). The default is `studio`.
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
    Adwaita,
    Solarized,
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
        Self::Adwaita,
        Self::Solarized,
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
            Self::Pro | Self::Studio | Self::Classic | Self::Adwaita | Self::Solarized
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
            Self::Adwaita => "adwaita",
            Self::Solarized => "solarized",
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
    /// A modern desktop style: a tall header bar, roomy rows, pill buttons,
    /// flat surfaces separated by tone.
    Gnome,
}

impl Style {
    pub const ALL: [Self; 5] = [Self::Plain, Self::Pro, Self::Studio, Self::Classic, Self::Gnome];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Pro => "pro",
            Self::Studio => "studio",
            Self::Classic => "classic",
            Self::Gnome => "gnome",
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
