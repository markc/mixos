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
//! `styles` names token sets; `schemes` binds every scheme to one of them,
//! its own style. A context's style axis ([`crate::DesignContext::style`])
//! selects any authored style instead, so any scheme can take any style.
//! Every style authors every required token: a missing, unknown or
//! ill-typed token is an error, as is a binding to a style that does not
//! exist, or a selected style the design does not author. Tokens added after
//! the first set are defaulted (the `=>` value below): a style that leaves
//! one out keeps the look it had before the token existed, so earlier
//! designs compile unchanged. A scheme the binding leaves out (one added
//! after the design was written) takes [`UNBOUND`]. A style is independent
//! of mode and contrast, and modifier blocks cannot alter it.
//!
//! To add a token: give it a type and span, and a default that reproduces
//! every existing style's pixels; author it only in the styles that differ.
//!
//! Whether the chrome sits on a light or dark base is the palette's, not the
//! style's: a renderer reads it from the chrome colours.

use crate::source::{DesignV1Source, StyleFamilySource, StyleValueSource};
use crate::{DesignDiagnostic, Scheme, Style};
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
        if n.fract() == 0.0 {
            Ok(n as u8)
        } else {
            Err(format!("{n} is not a whole number"))
        }
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
            self.errors.push(DesignDiagnostic::error(
                "missing-style-token",
                path,
                format!("style token `{name}` is not authored"),
            ));
            return T::default();
        };
        T::read(value, range).unwrap_or_else(|message| {
            self.errors.push(DesignDiagnostic::error(
                "invalid-style-token",
                path,
                format!("style token `{name}`: {message}"),
            ));
            T::default()
        })
    }

    /// A defaulted token: `default` when the style does not author it.
    fn get_or<T: Token>(&mut self, name: &str, range: Range, default: T) -> T {
        if self.tokens.contains_key(name) {
            self.get(name, range)
        } else {
            default
        }
    }
}

/// The resolved style's fields, each one token: its type and its span.
macro_rules! style {
    ($($(#[doc = $doc:literal])* $field:ident: $ty:ty = $range:expr $(=> $default:expr)?),+ $(,)?) => {
        /// One style, every token resolved.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct ResolvedStyle { $($(#[doc = $doc])* pub $field: $ty),+ }

        /// Every style token, in field order.
        pub const TOKENS: &[&str] = &[$(stringify!($field)),+];

        fn read(reader: &mut Reader<'_>) -> ResolvedStyle {
            ResolvedStyle { $($field: token!(reader, stringify!($field), $range $(, $default)?)),+ }
        }
    };
}

/// One token's read: required, or defaulted.
macro_rules! token {
    ($reader:ident, $name:expr, $range:expr) => {
        $reader.get($name, $range)
    };
    ($reader:ident, $name:expr, $range:expr, $default:expr) => {
        $reader.get_or($name, $range, $default)
    };
}

style! {
    // The renderer.
    widgets: Widgets = NONE,
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
    // Spacing and outlines (chrome widgets). Defaulted: a style that does not
    // author them keeps the specification's fixed values, so every earlier
    // style and design is unchanged.
    /// The gap between items, across and down.
    item_spacing_x: f64 = LENGTH => 8.0,
    item_spacing_y: f64 = LENGTH => 6.0,
    /// The height (and least width) of an interactive control.
    control_height: f64 = LENGTH => 24.0,
    /// A button's padding inside its frame, across and down.
    button_padding_x: f64 = LENGTH => 10.0,
    button_padding_y: f64 = LENGTH => 4.0,
    /// A push button's width beyond its label.
    push_padding: f64 = LENGTH => 28.0,
    /// Fields, buttons and cards draw a 1 pt outline; without, surfaces part
    /// by tone alone.
    outlines: bool = NONE => true,
    /// The options, status, tool and rail bars draw their rule lines.
    bar_rules: bool = NONE => true,
}

/// The style a scheme the binding leaves out takes.
pub const UNBOUND: &str = "plain";

/// Tokens earlier designs authored that no longer select anything. They are
/// accepted and ignored, so a design saved before they were retired still
/// compiles: `base` (the light or dark base is now the palette's own).
pub const RETIRED: &[&str] = &["base"];

/// Compile the style family for `scheme`: `style`, or the scheme's own when
/// `None`. `None` when the design authors no style family. Every style and
/// binding is checked, not only the one selected, so a design that compiles
/// for one context compiles for all.
pub(crate) fn compile(
    source: &DesignV1Source,
    scheme: Scheme,
    style: Option<Style>,
) -> Result<Option<ResolvedStyle>, Vec<DesignDiagnostic>> {
    source
        .families
        .style
        .as_ref()
        .map_or(Ok(None), |family| select(family, scheme, style).map(Some))
}

fn select(
    family: &StyleFamilySource,
    scheme: Scheme,
    style: Option<Style>,
) -> Result<ResolvedStyle, Vec<DesignDiagnostic>> {
    let styles = compile_styles(family)?;
    let name = style.map_or_else(
        || {
            family
                .schemes
                .get(scheme.name())
                .cloned()
                .unwrap_or_else(|| UNBOUND.to_owned())
        },
        |s| s.name().to_owned(),
    );
    styles.get(&name).copied().ok_or_else(|| {
        vec![DesignDiagnostic::error(
            "unknown-style-selection",
            format!("{PATH}.styles.{name}"),
            format!("the selected style `{name}` is not a style in `styles`"),
        )]
    })
}

/// The scheme's own style.
#[cfg(test)]
fn compile_family(
    family: &StyleFamilySource,
    scheme: Scheme,
) -> Result<ResolvedStyle, Vec<DesignDiagnostic>> {
    select(family, scheme, None)
}

/// Every authored style, once every style and binding checks.
fn compile_styles(
    family: &StyleFamilySource,
) -> Result<BTreeMap<String, ResolvedStyle>, Vec<DesignDiagnostic>> {
    let mut errors = Vec::new();
    let mut styles = BTreeMap::new();
    for (name, tokens) in &family.styles {
        let path = format!("{PATH}.styles.{name}");
        let mut reader = Reader {
            tokens,
            path: path.clone(),
            errors: Vec::new(),
        };
        let style = read(&mut reader);
        errors.append(&mut reader.errors);
        let known: BTreeSet<&str> = TOKENS.iter().chain(RETIRED.iter()).copied().collect();
        for token in tokens
            .keys()
            .filter(|token| !known.contains(token.as_str()))
        {
            errors.push(DesignDiagnostic::error(
                "unknown-style-token",
                format!("{path}.{token}"),
                format!("`{token}` is not a style token"),
            ));
        }
        styles.insert(name.clone(), style);
    }
    for (name, style) in &family.schemes {
        let path = format!("{PATH}.schemes.{name}");
        if Scheme::from_name(name).is_none() {
            errors.push(DesignDiagnostic::error(
                "unknown-style-scheme",
                path,
                format!("`{name}` is not a scheme"),
            ));
        } else if !styles.contains_key(style.as_str()) {
            errors.push(DesignDiagnostic::error(
                "unknown-style",
                path,
                format!("`{style}` is not a style in `styles`"),
            ));
        }
    }
    // A scheme the binding leaves out (one added after the design was
    // written) takes `plain`; a design without `plain` must bind every scheme.
    if !styles.contains_key(UNBOUND) {
        for unbound in Scheme::ALL
            .into_iter()
            .filter(|s| !family.schemes.contains_key(s.name()))
        {
            let name = unbound.name();
            errors.push(DesignDiagnostic::error(
                "missing-style-binding",
                format!("{PATH}.schemes.{name}"),
                format!("scheme `{name}` has no style, and there is no `{UNBOUND}` style"),
            ));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(styles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesignCompileResult, DesignContext, Mode, SourceIdentity};

    fn embedded() -> crate::DesignSourceDocument {
        crate::parse_design_source(
            SourceIdentity::new("embedded"),
            crate::EMBEDDED_DEFAULT_SOURCE,
        )
        .expect("embedded design parses")
    }

    fn family() -> StyleFamilySource {
        embedded()
            .v1
            .families
            .style
            .expect("the embedded design authors the style family")
    }

    fn codes(result: Result<ResolvedStyle, Vec<DesignDiagnostic>>) -> Vec<&'static str> {
        result
            .expect_err("must refuse")
            .iter()
            .map(|d| d.code)
            .collect()
    }

    /// The studio style as the embedded design authors it.
    fn studio() -> ResolvedStyle {
        ResolvedStyle {
            widgets: Widgets::Chrome,
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
            item_spacing_x: 8.0,
            item_spacing_y: 6.0,
            control_height: 24.0,
            button_padding_x: 10.0,
            button_padding_y: 4.0,
            push_padding: 28.0,
            outlines: true,
            bar_rules: true,
        }
    }

    /// Adwaita's own style: Studio with 9 pt cards and popups.
    fn adwaita() -> ResolvedStyle {
        ResolvedStyle {
            radius: 9,
            ..studio()
        }
    }

    /// The modern desktop style.
    fn gnome() -> ResolvedStyle {
        ResolvedStyle {
            body_size: 13.0,
            canvas_dots: false,
            slider_fill: SliderFill::Accent,
            push_shape: PushShape::Pill,
            push_height: 34.0,
            dock_width: 320.0,
            dock_margin: 12.0,
            options_bar: 46.0,
            status_bar: 34.0,
            tool_bar: 52.0,
            tool_button: 38.0,
            rail: 48.0,
            rail_button: 36.0,
            title_bar_height: 47.0,
            menu_title_height: 34.0,
            menu_row_height: 34.0,
            menu_row_padding_x: 12.0,
            menu_min_width: 240.0,
            radius_sm: 8,
            radius: 12,
            item_spacing_x: 10.0,
            item_spacing_y: 8.0,
            control_height: 34.0,
            button_padding_x: 16.0,
            button_padding_y: 6.0,
            push_padding: 40.0,
            outlines: false,
            bar_rules: false,
            ..studio()
        }
    }

    fn pro() -> ResolvedStyle {
        ResolvedStyle {
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
                Scheme::Adwaita => adwaita(),
                Scheme::Solarized => studio(),
                _ => plain(),
            };
            for mode in Mode::ALL {
                let context = DesignContext {
                    scheme,
                    mode,
                    ..DesignContext::default()
                };
                let DesignCompileResult::Success(success) =
                    crate::compile_design(&document, context)
                else {
                    panic!("{scheme:?}/{mode:?} must compile");
                };
                assert_eq!(
                    success.candidate.dictionary().style,
                    Some(want),
                    "{scheme:?}/{mode:?}"
                );
            }
        }
    }

    #[test]
    fn a_missing_or_unknown_token_is_a_compile_error() {
        let mut missing = family();
        missing.styles.get_mut("pro").unwrap().remove("knob");
        assert_eq!(
            codes(compile_family(&missing, Scheme::Ocean)),
            ["missing-style-token"],
            "every style is checked, not only the selected one"
        );

        let mut unknown = family();
        unknown
            .styles
            .get_mut("studio")
            .unwrap()
            .insert("glow".into(), StyleValueSource::Flag(true));
        assert_eq!(
            codes(compile_family(&unknown, Scheme::Studio)),
            ["unknown-style-token"]
        );

        // The whole document refuses to compile with the stable code.
        let mut document = embedded();
        document
            .v1
            .families
            .style
            .as_mut()
            .unwrap()
            .styles
            .get_mut("studio")
            .unwrap()
            .remove("toggle");
        let DesignCompileResult::Fatal(failure) =
            crate::compile_design(&document, DesignContext::default())
        else {
            panic!("a missing style token must not compile");
        };
        assert!(
            failure
                .diagnostics
                .iter()
                .any(|d| d.code == "missing-style-token"
                    && d.path == "design.v1.families.style.styles.studio.toggle"),
            "{:?}",
            failure.diagnostics
        );
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
        assert_eq!(
            codes(compile_family(&bad, Scheme::Studio)),
            ["invalid-style-token"; 5]
        );
    }

    #[test]
    fn every_scheme_must_be_bound_to_a_known_style() {
        let mut unbound = family();
        unbound.schemes.remove("pro");
        assert_eq!(
            compile_family(&unbound, Scheme::Pro).unwrap(),
            plain(),
            "an unbound scheme takes plain"
        );
        unbound.styles.remove("plain");
        for scheme in ["ocean", "crimson", "stone", "forest", "sunset", "mono"] {
            unbound.schemes.insert(scheme.into(), "studio".into());
        }
        assert_eq!(
            codes(compile_family(&unbound, Scheme::Ocean)),
            ["missing-style-binding"],
            "without plain, every scheme must be bound"
        );
        let mut dangling = family();
        dangling.schemes.insert("forest".into(), "neon".into());
        assert_eq!(
            codes(compile_family(&dangling, Scheme::Ocean)),
            ["unknown-style"]
        );
        let mut stray = family();
        stray.schemes.insert("teal".into(), "plain".into());
        assert_eq!(
            codes(compile_family(&stray, Scheme::Ocean)),
            ["unknown-style-scheme"]
        );
    }

    #[test]
    fn a_design_without_a_style_family_has_none() {
        let mut document = embedded();
        document.v1.families.style = None;
        assert_eq!(compile(&document.v1, Scheme::Pro, None).unwrap(), None);
    }

    /// A design saved before the style axis (its styles still author the
    /// retired `base` token) compiles in every scheme and mode, with the
    /// same styles as today's design, and takes the style axis.
    #[test]
    fn a_design_from_before_the_axes_still_compiles() {
        let before = crate::parse_design_source(
            SourceIdentity::new("before-axes"),
            include_str!("../../tests/fixtures/revision-1-before-axes.theme.conf.mix"),
        )
        .expect("the old design parses");
        let today = embedded();
        for scheme in Scheme::ALL {
            for mode in Mode::ALL {
                for style in [None, Some(Style::Studio)] {
                    let context = DesignContext {
                        scheme,
                        mode,
                        style,
                        ..DesignContext::default()
                    };
                    let old = match crate::compile_design(&before, context.clone()) {
                        DesignCompileResult::Success(old) => old,
                        DesignCompileResult::Fatal(failure) => {
                            panic!(
                                "{scheme:?}/{mode:?}/{style:?} must compile: {:?}",
                                failure.diagnostics
                            )
                        }
                    };
                    let DesignCompileResult::Success(new) = crate::compile_design(&today, context)
                    else {
                        panic!("today's design compiles");
                    };
                    // Schemes added since take `plain` in the old design (it
                    // never bound them); the rest keep today's styles.
                    if matches!(scheme, Scheme::Adwaita | Scheme::Solarized) && style.is_none() {
                        assert_eq!(old.candidate.dictionary().style, Some(plain()));
                        continue;
                    }
                    assert_eq!(
                        old.candidate.dictionary().style,
                        new.candidate.dictionary().style,
                        "{scheme:?}/{mode:?}/{style:?}"
                    );
                }
            }
        }
    }

    /// The style axis takes any authored style for any scheme, whatever the
    /// scheme's own; a style the design does not author is refused.
    #[test]
    fn the_style_axis_selects_any_style_for_any_scheme() {
        let document = embedded();
        for (style, want) in [
            (Style::Plain, plain()),
            (Style::Pro, pro()),
            (Style::Studio, studio()),
            (Style::Classic, classic()),
            (Style::Gnome, gnome()),
        ] {
            for scheme in Scheme::ALL {
                let context = DesignContext {
                    scheme,
                    style: Some(style),
                    ..DesignContext::default()
                };
                let DesignCompileResult::Success(success) =
                    crate::compile_design(&document, context)
                else {
                    panic!("{scheme:?} in {style:?} must compile");
                };
                assert_eq!(
                    success.candidate.dictionary().style,
                    Some(want),
                    "{scheme:?} in {style:?}"
                );
            }
        }
        let mut missing = family();
        missing.schemes.insert("classic".into(), "studio".into());
        missing.styles.remove("classic");
        let codes: Vec<_> = select(&missing, Scheme::Forest, Some(Style::Classic))
            .unwrap_err()
            .iter()
            .map(|d| d.code)
            .collect();
        assert_eq!(codes, ["unknown-style-selection"]);
    }
}
