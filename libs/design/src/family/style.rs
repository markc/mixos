//! The style family: how application chrome is shaped, as one closed set of
//! typed tokens. The chrome family says which colours the chrome is; a style
//! says which forms it takes (tab strips or cards, checkboxes or switches,
//! bevels or outlines) and its lengths (title-bar height, bar sizes, radii).
//! A renderer reads the resolved [`ResolvedStyle`] and never asks which
//! scheme it draws, so a new style is new data, not new code.
//!
//! Source shape, under `design.v1.families`:
//!
//! ```text
//! style: {
//!   schemes: { ocean: "plain", ..., pro: "pro" },
//!   styles: { plain: { toggle: "switch", title_bar_height: 30.0, ... }, pro: { ... } }
//! }
//! ```
//!
//! `styles` names token sets; `schemes` binds every scheme to one of them.
//! Every style authors every token: a missing, unknown or ill-typed token is
//! an error, as is a scheme left unbound or bound to a style that does not
//! exist. The binding is per scheme only, so a style is independent of mode
//! and contrast, and modifier blocks cannot alter it.

use crate::source::{DesignV1Source, StyleFamilySource, StyleValueSource};
use crate::{DesignDiagnostic, Scheme};
use std::collections::{BTreeMap, BTreeSet};

const PATH: &str = "design.v1.families.style";

/// A closed set of named token values: the variants, their source names, and
/// lookups both ways. The first variant is the default a refused token
/// stands in with while the compiler collects every error.
macro_rules! choice {
    ($(#[doc = $doc:literal])* $name:ident { $($(#[doc = $vdoc:literal])* $variant:ident = $text:literal),+ $(,)? }) => {
        $(#[doc = $doc])*
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum $name { $($(#[doc = $vdoc])* $variant),+ }

        impl $name {
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            pub const fn name(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }

            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.name() == name)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::ALL[0]
            }
        }

        impl Token for $name {
            fn read(value: &StyleValueSource, _: Range) -> Result<Self, String> {
                let names = || Self::ALL.iter().map(|v| format!("`{}`", v.name())).collect::<Vec<_>>().join(", ");
                match value {
                    StyleValueSource::Name(name) => Self::from_name(name).ok_or_else(|| format!("`{name}` is not one of {}", names())),
                    _ => Err(format!("expected one of {}", names())),
                }
            }
        }
    };
}

choice! {
    /// Where widget colours and spacing come from.
    Widgets {
        /// The semantic pairs and the design's own metrics (the hue schemes).
        Pairs = "pairs",
        /// The chrome family's roles and the style's own geometry.
        Chrome = "chrome",
    }
}

choice! {
    /// The renderer's base palette under the chrome.
    Base {
        /// Follow the selected mode.
        Mode = "mode",
        Light = "light",
        Dark = "dark",
    }
}

choice! {
    /// Interface faces: the typography roles' weights, or the chrome's fixed
    /// Regular body with Medium and SemiBold accents.
    Faces {
        Typography = "typography",
        Fixed = "fixed",
    }
}

choice! {
    /// The edge colour of every widget state.
    WidgetStroke {
        /// The state's own role (field border, accent border).
        Role = "role",
        /// A bevel's shaded edge.
        Bevel = "bevel",
    }
}

choice! {
    /// The selection fill.
    Selection {
        Accent = "accent",
        AccentSoft = "accent_soft",
    }
}

choice! {
    /// Scroll bars.
    ScrollBars {
        /// Thin bars floating over the content.
        Thin = "thin",
        /// Solid bars beside it.
        Solid = "solid",
    }
}

choice! {
    /// The on/off control a style offers.
    Toggle {
        Switch = "switch",
        Checkbox = "checkbox",
    }
}

choice! {
    /// A switch's track and knob.
    Switch {
        /// A round track and knob.
        Round = "round",
        /// A sunken square track with a raised square knob.
        Block = "block",
    }
}

choice! {
    /// A slider knob.
    Knob {
        /// A plain disc.
        Plain = "plain",
        /// A smaller disc with a ring.
        Ringed = "ringed",
        /// A raised square block.
        Block = "block",
    }
}

choice! {
    /// The filled part of a slider track.
    SliderFill {
        TextDim = "text_dim",
        Accent = "accent",
    }
}

choice! {
    /// A push button's corners.
    PushShape {
        /// The small radius.
        Rounded = "rounded",
        /// Fully round ends.
        Pill = "pill",
    }
}

choice! {
    /// A secondary push button.
    SecondaryButton {
        /// A field-filled button with a border.
        Filled = "filled",
        /// An outline only, filled on hover.
        Outline = "outline",
    }
}

choice! {
    /// How a dock group shows its tabs.
    PanelGroups {
        /// A card with pill tabs in its header.
        Cards = "cards",
        /// A tab strip across the group's top.
        TabStrip = "tab_strip",
    }
}

choice! {
    /// The document tab strip.
    DocumentTabs {
        /// Separate cards with name and detail.
        Cards = "cards",
        /// Flush tabs with rules between them.
        Strip = "strip",
    }
}

/// The accepted span of a numeric token.
#[derive(Clone, Copy, Debug)]
struct Range(f64, f64);

/// Any length a chrome can sensibly take, in points.
const LENGTH: Range = Range(0.0, 1000.0);
/// An opacity.
const ALPHA: Range = Range(0.0, 1.0);
/// A corner radius, in whole points.
const RADIUS: Range = Range(0.0, 255.0);
/// A choice or a flag carries no span.
const NONE: Range = Range(0.0, 0.0);

trait Token: Sized + Default {
    fn read(value: &StyleValueSource, range: Range) -> Result<Self, String>;
}

impl Token for bool {
    fn read(value: &StyleValueSource, _: Range) -> Result<Self, String> {
        match value {
            StyleValueSource::Flag(flag) => Ok(*flag),
            _ => Err("expected true or false".into()),
        }
    }
}

impl Token for f64 {
    fn read(value: &StyleValueSource, Range(min, max): Range) -> Result<Self, String> {
        match value {
            StyleValueSource::Number(n) if n.is_finite() && (min..=max).contains(n) => Ok(*n),
            StyleValueSource::Number(n) => Err(format!("{n} is outside {min} to {max}")),
            _ => Err("expected a number".into()),
        }
    }
}

impl Token for u8 {
    fn read(value: &StyleValueSource, range: Range) -> Result<Self, String> {
        let n = f64::read(value, range)?;
        if n.fract() == 0.0 { Ok(n as u8) } else { Err(format!("{n} is not a whole number")) }
    }
}

/// Reads one style's tokens, collecting every error.
struct Reader<'a> {
    tokens: &'a BTreeMap<String, StyleValueSource>,
    path: String,
    errors: Vec<DesignDiagnostic>,
}

impl Reader<'_> {
    fn get<T: Token>(&mut self, name: &str, range: Range) -> T {
        let path = format!("{}.{name}", self.path);
        let Some(value) = self.tokens.get(name) else {
            self.errors.push(DesignDiagnostic::error("missing-style-token", path, format!("style token `{name}` is not authored")));
            return T::default();
        };
        T::read(value, range).unwrap_or_else(|message| {
            self.errors.push(DesignDiagnostic::error("invalid-style-token", path, format!("style token `{name}`: {message}")));
            T::default()
        })
    }
}

/// The resolved style's fields, each one token: its type and its span.
macro_rules! style {
    ($($(#[doc = $doc:literal])* $field:ident: $ty:ty = $range:expr),+ $(,)?) => {
        /// One style, every token resolved.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct ResolvedStyle { $($(#[doc = $doc])* pub $field: $ty),+ }

        /// Every style token, in field order.
        pub const TOKENS: &[&str] = &[$(stringify!($field)),+];

        fn read(reader: &mut Reader<'_>) -> ResolvedStyle {
            ResolvedStyle { $($field: reader.get(stringify!($field), $range)),+ }
        }
    };
}

style! {
    // The renderer.
    widgets: Widgets = NONE,
    base: Base = NONE,
    faces: Faces = NONE,
    /// Body and button text size (chrome widgets only).
    body_size: f64 = LENGTH,
    // Surfaces.
    /// Surfaces draw a lit and a shaded edge instead of an outline.
    bevels: bool = NONE,
    widget_stroke: WidgetStroke = NONE,
    selection: Selection = NONE,
    scroll_bars: ScrollBars = NONE,
    /// The canvas surround shows its dot grid.
    canvas_dots: bool = NONE,
    // Controls.
    toggle: Toggle = NONE,
    switch: Switch = NONE,
    knob: Knob = NONE,
    slider_fill: SliderFill = NONE,
    push_shape: PushShape = NONE,
    secondary_button: SecondaryButton = NONE,
    push_height: f64 = LENGTH,
    /// A primary push button's opacity when hovered and when pressed.
    primary_hover_alpha: f64 = ALPHA,
    primary_press_alpha: f64 = ALPHA,
    // Panels and tabs.
    panel_groups: PanelGroups = NONE,
    document_tabs: DocumentTabs = NONE,
    /// A rule down the tool bar's right edge.
    tool_bar_rule: bool = NONE,
    dock_width: f64 = LENGTH,
    dock_margin: f64 = LENGTH,
    /// Bar sizes along their short side.
    options_bar: f64 = LENGTH,
    status_bar: f64 = LENGTH,
    tool_bar: f64 = LENGTH,
    tool_button: f64 = LENGTH,
    tool_margin: f64 = LENGTH,
    rail: f64 = LENGTH,
    rail_button: f64 = LENGTH,
    // The title bar and menus.
    title_bar_height: f64 = LENGTH,
    /// The app mark's square size.
    mark: f64 = LENGTH,
    menu_title_height: f64 = LENGTH,
    menu_row_height: f64 = LENGTH,
    menu_row_padding_x: f64 = LENGTH,
    menu_row_padding_y: f64 = LENGTH,
    menu_min_width: f64 = LENGTH,
    menu_highlight_radius: u8 = RADIUS,
    /// Corner radii: widgets, menus and popups, floating windows.
    radius_sm: u8 = RADIUS,
    radius: u8 = RADIUS,
    radius_lg: u8 = RADIUS,
}

/// Compile the style family for `scheme`. `None` when the design authors no
/// style family. Every style and binding is checked, not only the one
/// selected, so a design that compiles for one scheme compiles for all.
pub(crate) fn compile(source: &DesignV1Source, scheme: Scheme) -> Result<Option<ResolvedStyle>, Vec<DesignDiagnostic>> {
    source.families.style.as_ref().map_or(Ok(None), |family| compile_family(family, scheme).map(Some))
}

fn compile_family(family: &StyleFamilySource, scheme: Scheme) -> Result<ResolvedStyle, Vec<DesignDiagnostic>> {
    let mut errors = Vec::new();
    let mut styles = BTreeMap::new();
    for (name, tokens) in &family.styles {
        let path = format!("{PATH}.styles.{name}");
        let mut reader = Reader { tokens, path: path.clone(), errors: Vec::new() };
        let style = read(&mut reader);
        errors.append(&mut reader.errors);
        let known: BTreeSet<&str> = TOKENS.iter().copied().collect();
        for token in tokens.keys().filter(|token| !known.contains(token.as_str())) {
            errors.push(DesignDiagnostic::error("unknown-style-token", format!("{path}.{token}"), format!("`{token}` is not a style token")));
        }
        styles.insert(name.as_str(), style);
    }
    for (name, style) in &family.schemes {
        let path = format!("{PATH}.schemes.{name}");
        if Scheme::from_name(name).is_none() {
            errors.push(DesignDiagnostic::error("unknown-style-scheme", path, format!("`{name}` is not a scheme")));
        } else if !styles.contains_key(style.as_str()) {
            errors.push(DesignDiagnostic::error("unknown-style", path, format!("`{style}` is not a style in `styles`")));
        }
    }
    for unbound in Scheme::ALL.into_iter().filter(|s| !family.schemes.contains_key(s.name())) {
        let name = unbound.name();
        errors.push(DesignDiagnostic::error("missing-style-binding", format!("{PATH}.schemes.{name}"), format!("scheme `{name}` has no style")));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(styles[family.schemes[scheme.name()].as_str()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesignCompileResult, DesignContext, Mode, SourceIdentity};

    fn embedded() -> crate::DesignSourceDocument {
        crate::parse_design_source(SourceIdentity::new("embedded"), crate::EMBEDDED_DEFAULT_SOURCE).expect("embedded design parses")
    }

    fn family() -> StyleFamilySource {
        embedded().v1.families.style.expect("the embedded design authors the style family")
    }

    fn codes(result: Result<ResolvedStyle, Vec<DesignDiagnostic>>) -> Vec<&'static str> {
        result.expect_err("must refuse").iter().map(|d| d.code).collect()
    }

    /// The studio style as the embedded design authors it.
    fn studio() -> ResolvedStyle {
        ResolvedStyle {
            widgets: Widgets::Chrome,
            base: Base::Mode,
            faces: Faces::Fixed,
            body_size: 12.5,
            bevels: false,
            widget_stroke: WidgetStroke::Role,
            selection: Selection::AccentSoft,
            scroll_bars: ScrollBars::Thin,
            canvas_dots: true,
            toggle: Toggle::Switch,
            switch: Switch::Round,
            knob: Knob::Plain,
            slider_fill: SliderFill::TextDim,
            push_shape: PushShape::Rounded,
            secondary_button: SecondaryButton::Filled,
            push_height: 30.0,
            primary_hover_alpha: 0.93,
            primary_press_alpha: 0.85,
            panel_groups: PanelGroups::Cards,
            document_tabs: DocumentTabs::Cards,
            tool_bar_rule: false,
            dock_width: 300.0,
            dock_margin: 8.0,
            options_bar: 42.0,
            status_bar: 30.0,
            tool_bar: 50.0,
            tool_button: 36.0,
            tool_margin: 7.0,
            rail: 44.0,
            rail_button: 32.0,
            title_bar_height: 38.0,
            mark: 20.0,
            menu_title_height: 24.0,
            menu_row_height: 24.0,
            menu_row_padding_x: 2.0,
            menu_row_padding_y: 0.0,
            menu_min_width: 220.0,
            menu_highlight_radius: 6,
            radius_sm: 6,
            radius: 8,
            radius_lg: 12,
        }
    }

    fn pro() -> ResolvedStyle {
        ResolvedStyle {
            base: Base::Dark,
            body_size: 12.0,
            selection: Selection::Accent,
            toggle: Toggle::Checkbox,
            knob: Knob::Ringed,
            push_shape: PushShape::Pill,
            secondary_button: SecondaryButton::Outline,
            push_height: 28.0,
            primary_hover_alpha: 0.9,
            primary_press_alpha: 0.8,
            panel_groups: PanelGroups::TabStrip,
            document_tabs: DocumentTabs::Strip,
            tool_bar_rule: true,
            dock_width: 290.0,
            dock_margin: 2.0,
            options_bar: 36.0,
            status_bar: 24.0,
            tool_bar: 40.0,
            tool_button: 30.0,
            tool_margin: 5.0,
            rail: 36.0,
            rail_button: 28.0,
            title_bar_height: 32.0,
            mark: 18.0,
            menu_row_padding_x: 10.0,
            menu_row_padding_y: 4.0,
            menu_highlight_radius: 3,
            radius_sm: 3,
            radius: 4,
            radius_lg: 6,
            ..studio()
        }
    }

    fn classic() -> ResolvedStyle {
        ResolvedStyle {
            base: Base::Light,
            bevels: true,
            widget_stroke: WidgetStroke::Bevel,
            selection: Selection::Accent,
            scroll_bars: ScrollBars::Solid,
            canvas_dots: false,
            switch: Switch::Block,
            knob: Knob::Block,
            slider_fill: SliderFill::Accent,
            menu_highlight_radius: 0,
            radius_sm: 0,
            radius: 0,
            radius_lg: 0,
            ..studio()
        }
    }

    /// The hue schemes: today's derived look, as data.
    fn plain() -> ResolvedStyle {
        ResolvedStyle {
            widgets: Widgets::Pairs,
            faces: Faces::Typography,
            selection: Selection::Accent,
            title_bar_height: 30.0,
            mark: 17.5,
            menu_row_padding_x: 16.0,
            menu_min_width: 192.0,
            radius: 4,
            ..studio()
        }
    }

    /// Compiling the embedded design yields each scheme's style, in either
    /// mode.
    #[test]
    fn the_embedded_design_styles_every_scheme() {
        let document = embedded();
        for scheme in Scheme::ALL {
            let want = match scheme {
                Scheme::Pro => pro(),
                Scheme::Studio => studio(),
                Scheme::Classic => classic(),
                _ => plain(),
            };
            for mode in Mode::ALL {
                let context = DesignContext { scheme, mode, ..DesignContext::default() };
                let DesignCompileResult::Success(success) = crate::compile_design(&document, context) else {
                    panic!("{scheme:?}/{mode:?} must compile");
                };
                assert_eq!(success.candidate.dictionary().style, Some(want), "{scheme:?}/{mode:?}");
            }
        }
    }

    #[test]
    fn a_missing_or_unknown_token_is_a_compile_error() {
        let mut missing = family();
        missing.styles.get_mut("pro").unwrap().remove("knob");
        assert_eq!(codes(compile_family(&missing, Scheme::Ocean)), ["missing-style-token"], "every style is checked, not only the selected one");

        let mut unknown = family();
        unknown.styles.get_mut("studio").unwrap().insert("glow".into(), StyleValueSource::Flag(true));
        assert_eq!(codes(compile_family(&unknown, Scheme::Studio)), ["unknown-style-token"]);

        // The whole document refuses to compile with the stable code.
        let mut document = embedded();
        document.v1.families.style.as_mut().unwrap().styles.get_mut("studio").unwrap().remove("toggle");
        let DesignCompileResult::Fatal(failure) = crate::compile_design(&document, DesignContext::default()) else {
            panic!("a missing style token must not compile");
        };
        assert!(failure.diagnostics.iter().any(|d| d.code == "missing-style-token" && d.path == "design.v1.families.style.styles.studio.toggle"), "{:?}", failure.diagnostics);
    }

    #[test]
    fn ill_typed_tokens_are_refused() {
        let mut bad = family();
        let studio = bad.styles.get_mut("studio").unwrap();
        studio.insert("knob".into(), StyleValueSource::Name("square".into()));
        studio.insert("bevels".into(), StyleValueSource::Name("yes".into()));
        studio.insert("radius".into(), StyleValueSource::Number(2.5));
        studio.insert("primary_hover_alpha".into(), StyleValueSource::Number(1.5));
        studio.insert("mark".into(), StyleValueSource::Flag(false));
        assert_eq!(codes(compile_family(&bad, Scheme::Studio)), ["invalid-style-token"; 5]);
    }

    #[test]
    fn every_scheme_must_be_bound_to_a_known_style() {
        let mut unbound = family();
        unbound.schemes.remove("forest");
        assert_eq!(codes(compile_family(&unbound, Scheme::Ocean)), ["missing-style-binding"]);
        let mut dangling = family();
        dangling.schemes.insert("forest".into(), "neon".into());
        assert_eq!(codes(compile_family(&dangling, Scheme::Ocean)), ["unknown-style"]);
        let mut stray = family();
        stray.schemes.insert("teal".into(), "plain".into());
        assert_eq!(codes(compile_family(&stray, Scheme::Ocean)), ["unknown-style-scheme"]);
    }

    #[test]
    fn a_design_without_a_style_family_has_none() {
        let mut document = embedded();
        document.v1.families.style = None;
        assert_eq!(compile(&document.v1, Scheme::Pro).unwrap(), None);
    }
}
