/// The MixOS colour schemes: six hues, and three chrome schemes whose
/// light and dark modes step the brightness (`pro`: Pro and Pro Medium Gray;
/// `studio`: Studio and Studio Light; `classic`: Classic in both).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Scheme {
    #[default]
    Ocean,
    Crimson,
    Stone,
    Forest,
    Sunset,
    Mono,
    Pro,
    Studio,
    Classic,
}

impl Scheme {
    pub const ALL: [Self; 9] = [
        Self::Ocean,
        Self::Crimson,
        Self::Stone,
        Self::Forest,
        Self::Sunset,
        Self::Mono,
        Self::Pro,
        Self::Studio,
        Self::Classic,
    ];

    /// The six hue schemes of design revision one. Tests that pin revision
    /// one's exact derivation walks iterate these; the chrome schemes carry
    /// their own parity tests.
    pub const REVISION_ONE: [Self; 6] = [Self::Ocean, Self::Crimson, Self::Stone, Self::Forest, Self::Sunset, Self::Mono];

    /// The schemes drawn from the chrome family.
    pub const fn is_chrome_scheme(self) -> bool {
        matches!(self, Self::Pro | Self::Studio | Self::Classic)
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
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scheme| scheme.name() == name)
    }
}

/// Light or dark presentation mode.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mode {
    #[default]
    Light,
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

/// The compile-time selection used to flatten a design source.
///
/// Per-app overlays are compile-time selections, never runtime fallbacks.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DesignContext {
    pub scheme: Scheme,
    pub mode: Mode,
    pub contrast: Contrast,
    /// Stable application identity selecting a compile-time per-app overlay.
    /// `None` is the unoverlaid context used by the v0 equivalence gate.
    pub app: Option<String>,
}
