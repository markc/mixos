// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
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
//! Every style authors every token: a missing, unknown or ill-typed token is
//! an error, as is a scheme left unbound or bound to a style that does not
//! exist, or a selected style the design does not author. A style is
//! independent of mode and contrast, and modifier blocks cannot alter it.
//!
//! Whether the chrome sits on a light or dark base is the palette's, not the
//! style's: a renderer reads it from the chrome colours.

use crate::source::{DesignV1Source, StyleFamilySource, StyleValueSource};
use crate::{DesignDiagnostic, Scheme, Style};
use std::collections::{BTreeMap, BTreeSet};

const PATH: &str = "design.v1.families.style";

/// The original vocabulary. Later tokens default through the compatibility
/// table; these names remain required in every authored style.
pub const REQUIRED: [&str; 40] = [
    "widgets",
    "faces",
    "body_size",
    "bevels",
    "widget_stroke",
    "selection",
    "scroll_bars",
    "canvas_dots",
    "toggle",
    "switch",
    "knob",
    "slider_fill",
    "push_shape",
    "secondary_button",
    "push_height",
    "primary_hover_alpha",
    "primary_press_alpha",
    "panel_groups",
    "document_tabs",
    "tool_bar_rule",
    "dock_width",
    "dock_margin",
    "options_bar",
    "status_bar",
    "tool_bar",
    "tool_button",
    "tool_margin",
    "rail",
    "rail_button",
    "title_bar_height",
    "mark",
    "menu_title_height",
    "menu_row_height",
    "menu_row_padding_x",
    "menu_row_padding_y",
    "menu_min_width",
    "menu_highlight_radius",
    "radius_sm",
    "radius",
    "radius_lg",
];

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
            fn write(&self) -> StyleValueSource {
                StyleValueSource::Name(self.name().into())
            }

            fn variants() -> Option<Vec<String>> {
                Some(Self::ALL.iter().map(|v| v.name().into()).collect())
            }

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
    /// The colour role for a field's focus outline.
    FieldFocusRole {
        AccentText = "accent_text",
        Accent = "accent",
        FieldBorder = "field_border",
    }
}

/// An exact sRGB colour carried by a style token.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StyleColour(pub [u8; 4]);

impl Token for StyleColour {
    fn read(value: &StyleValueSource, Range(min, max): Range) -> Result<Self, String> {
        let StyleValueSource::Name(value) = value else {
            return Err("expected a hexadecimal colour".into());
        };
        let rgba = crate::parse_legacy_v0_hex_colour(value)?;
        if rgba
            .iter()
            .any(|c| !(min..=max).contains(&(f64::from(*c) / 255.0)))
        {
            return Err(format!(
                "each colour channel must be between {min} and {max}"
            ));
        }
        Ok(Self(rgba))
    }

    fn write(&self) -> StyleValueSource {
        let [r, g, b, a] = self.0;
        StyleValueSource::Name(format!("#{r:02x}{g:02x}{b:02x}{a:02x}"))
    }
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
    fn write(&self) -> StyleValueSource;
    fn variants() -> Option<Vec<String>> {
        None
    }
}

impl Token for bool {
    fn write(&self) -> StyleValueSource {
        StyleValueSource::Flag(*self)
    }
    fn read(value: &StyleValueSource, _: Range) -> Result<Self, String> {
        match value {
            StyleValueSource::Flag(flag) => Ok(*flag),
            _ => Err("expected true or false".into()),
        }
    }
}

impl Token for f64 {
    fn write(&self) -> StyleValueSource {
        StyleValueSource::Number(*self)
    }
    fn read(value: &StyleValueSource, Range(min, max): Range) -> Result<Self, String> {
        match value {
            StyleValueSource::Number(n) if n.is_finite() && (min..=max).contains(n) => Ok(*n),
            StyleValueSource::Number(n) => Err(format!("{n} is outside {min} to {max}")),
            _ => Err("expected a number".into()),
        }
    }
}

impl Token for u8 {
    fn write(&self) -> StyleValueSource {
        StyleValueSource::Number(f64::from(*self))
    }
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
}

/// The resolved style's fields, each one token: its type and its span.
macro_rules! style {
    ($($(#[doc = $doc:literal])* $field:ident: $ty:ty = $range:expr),+ $(,)?) => {
        /// One style, every token resolved.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct ResolvedStyle { $($(#[doc = $doc])* pub $field: $ty),+ }

        /// Every style token, in field order.
        pub const TOKENS: &[&str] = &[$(stringify!($field)),+];

        impl ResolvedStyle {
            /// Exact token values, suitable for introspection or an overlay.
            pub fn values(&self) -> BTreeMap<String, StyleValueSource> {
                BTreeMap::from([$( (stringify!($field).into(), self.$field.write()) ),+])
            }

            /// Validate a complete style after applying a partial overlay.
            pub fn with_values(self, values: &BTreeMap<String, StyleValueSource>)
                -> Result<Self, Vec<DesignDiagnostic>>
            {
                let mut tokens = self.values();
                tokens.extend(values.clone());
                let mut reader = Reader { tokens: &tokens, path: "style".into(), errors: Vec::new() };
                let value = read(&mut reader);
                validate_constraints(&value, "style", &mut reader.errors);
                for name in values.keys().filter(|name| !TOKENS.contains(&name.as_str())) {
                    reader.errors.push(DesignDiagnostic::error("unknown-style-token", format!("style.{name}"), "unknown style token"));
                }
                if reader.errors.is_empty() { Ok(value) } else { Err(reader.errors) }
            }
        }

        pub(crate) fn declarations() -> Vec<crate::Tunable> {
            vec![$( crate::tunable::style_entry(stringify!($field), stringify!($ty), $range.0, $range.1, <$ty as Token>::variants()) ),+]
        }

        fn read(reader: &mut Reader<'_>) -> ResolvedStyle {
            ResolvedStyle { $($field: reader.get(stringify!($field), $range)),+ }
        }
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
    animation_time: f64 = LENGTH,
    bar_row_gap: f64 = LENGTH,
    bars_bar_inset: f64 = LENGTH,
    bars_rail_rule: f64 = LENGTH,
    button_icon_share: f64 = LENGTH,
    button_link_icon: f64 = LENGTH,
    button_link_size: f64 = LENGTH,
    button_padding_x: f64 = LENGTH,
    button_padding_y: f64 = LENGTH,
    button_pro_outline: f64 = LENGTH,
    button_push_padding: f64 = LENGTH,
    canvas_bar: f64 = LENGTH,
    canvas_dot_pitch: f64 = Range(4.0, 1000.0),
    canvas_dot_size: f64 = LENGTH,
    canvas_thumb_inset: f64 = LENGTH,
    canvas_thumb_min: f64 = LENGTH,
    caption_gap: f64 = LENGTH,
    caption_glyph: f64 = LENGTH,
    caption_width: f64 = LENGTH,
    checkbox_box: f64 = LENGTH,
    close_pressed_alpha: f64 = ALPHA,
    combo_chevron_alpha: f64 = ALPHA,
    combo_chevron_half: f64 = LENGTH,
    combo_chevron_height: f64 = LENGTH,
    combo_chevron_stroke: f64 = LENGTH,
    combo_list_height: f64 = LENGTH,
    combo_width: f64 = LENGTH,
    dialog_above_buttons: f64 = LENGTH,
    dialog_above_rule: f64 = LENGTH,
    dialog_below_rule: f64 = LENGTH,
    dialog_button_gap: f64 = LENGTH,
    dialog_button_min: f64 = LENGTH,
    dialog_centre_drop: f64 = LENGTH,
    dialog_max_width: f64 = LENGTH,
    dialog_min_width: f64 = LENGTH,
    dialog_resize_max_share: f64 = LENGTH,
    dialog_title_size: f64 = LENGTH,
    disabled_alpha: f64 = ALPHA,
    divider_height: f64 = LENGTH,
    divider_slot: f64 = LENGTH,
    double_click_time: f64 = Range(0.1, 1.5),
    edge_gap: f64 = LENGTH,
    field_focus_role: FieldFocusRole = NONE,
    field_focus_width: f64 = Range(0.0, 3.0),
    field_height: f64 = LENGTH,
    field_padding_x: f64 = Range(0.0, 127.0),
    field_padding_y: f64 = Range(0.0, 127.0),
    field_radius: u8 = RADIUS,
    fine_scrub_rate: f64 = Range(0.001, 0.5),
    fine_span: f64 = Range(0.0, 1000.0),
    focus_offset: f64 = LENGTH,
    focus_ring: f64 = LENGTH,
    hairline_width: f64 = LENGTH,
    heading_size: f64 = LENGTH,
    hot_distance: f64 = Range(0.0, 12.0),
    icon_button: f64 = LENGTH,
    icon_mask: StyleColour = Range(0.1, 1.0),
    icon_width: f64 = LENGTH,
    interact: f64 = LENGTH,
    item_spacing_x: f64 = LENGTH,
    item_spacing_y: f64 = LENGTH,
    link_gap: f64 = LENGTH,
    mark_gap: f64 = LENGTH,
    md_bullet: f64 = LENGTH,
    md_code_padding_x: f64 = LENGTH,
    md_code_padding_y: f64 = LENGTH,
    md_gap: f64 = LENGTH,
    md_gap_after_heading: f64 = LENGTH,
    md_gap_before_heading: f64 = LENGTH,
    md_gutter: f64 = LENGTH,
    md_heading_line: f64 = LENGTH,
    md_heading_scale_1: f64 = LENGTH,
    md_heading_scale_2: f64 = LENGTH,
    md_heading_scale_3: f64 = LENGTH,
    md_heading_scale_4: f64 = LENGTH,
    md_heading_scale_5: f64 = LENGTH,
    md_heading_scale_6: f64 = LENGTH,
    md_inline_code_scale: f64 = LENGTH,
    md_item_gap: f64 = LENGTH,
    md_line: f64 = LENGTH,
    md_measure: f64 = LENGTH,
    md_quote_indent: f64 = LENGTH,
    md_quote_rule: f64 = LENGTH,
    menu_margin: f64 = Range(0.0, 127.0),
    menu_scroll_speed: f64 = Range(100.0, 3000.0),
    menu_search_width: f64 = LENGTH,
    menu_separator_height: f64 = LENGTH,
    menu_title_padding_x: f64 = LENGTH,
    menu_title_padding_y: f64 = LENGTH,
    mono_size: f64 = LENGTH,
    pair_active_mix: f64 = ALPHA,
    pair_hover_mix: f64 = ALPHA,
    panel_card_after: f64 = LENGTH,
    panel_card_margin_bottom: f64 = Range(0.0, 127.0),
    panel_card_margin_collapsed: f64 = Range(0.0, 127.0),
    panel_card_margin_left: f64 = Range(0.0, 127.0),
    panel_card_margin_right: f64 = Range(0.0, 127.0),
    panel_card_margin_top: f64 = Range(0.0, 127.0),
    panel_drag_band: f64 = LENGTH,
    panel_drop_inset_horizontal: f64 = LENGTH,
    panel_drop_inset_vertical: f64 = LENGTH,
    panel_drop_line: f64 = LENGTH,
    panel_ellipsis: f64 = LENGTH,
    panel_ellipsis_gap: f64 = LENGTH,
    panel_header: f64 = LENGTH,
    panel_header_after: f64 = LENGTH,
    panel_menu_box_h: f64 = LENGTH,
    panel_menu_box_w: f64 = LENGTH,
    panel_menu_from_right: f64 = LENGTH,
    panel_menu_line: f64 = LENGTH,
    panel_menu_pitch: f64 = LENGTH,
    panel_pill_gap: f64 = LENGTH,
    panel_pill_hover_alpha: f64 = ALPHA,
    panel_pill_min: f64 = LENGTH,
    panel_pill_pad: f64 = LENGTH,
    panel_pill_size: f64 = LENGTH,
    panel_pro_after: f64 = LENGTH,
    panel_pro_hover_alpha: f64 = ALPHA,
    panel_pro_margin: f64 = Range(0.0, 127.0),
    panel_pro_min: f64 = LENGTH,
    panel_pro_pad: f64 = LENGTH,
    panel_pro_radius: u8 = RADIUS,
    panel_pro_size: f64 = LENGTH,
    panel_section_size: f64 = LENGTH,
    panel_strip: f64 = LENGTH,
    popup_shadow_alpha: f64 = ALPHA,
    popup_shadow_blur: f64 = Range(0.0, 255.0),
    popup_shadow_offset_x: f64 = Range(-128.0, 127.0),
    popup_shadow_offset_y: f64 = Range(-128.0, 127.0),
    popup_shadow_spread: f64 = Range(0.0, 255.0),
    push_label_size: f64 = LENGTH,
    resize_corner: f64 = Range(2.0, 32.0),
    resize_edge: f64 = Range(2.0, 16.0),
    scroll_allocated: f64 = LENGTH,
    scroll_handle_alpha_active: f64 = ALPHA,
    scroll_handle_alpha_hot: f64 = ALPHA,
    scroll_handle_alpha_rest: f64 = ALPHA,
    scroll_handle_min: f64 = LENGTH,
    scroll_margin: f64 = LENGTH,
    scroll_outer_margin: f64 = LENGTH,
    scroll_track_alpha_active: f64 = ALPHA,
    scroll_track_alpha_hot: f64 = ALPHA,
    scroll_track_alpha_rest: f64 = ALPHA,
    scroll_width: f64 = LENGTH,
    scroll_width_hot: f64 = LENGTH,
    scrub_rate: f64 = Range(0.05, 5.0),
    search_icon: f64 = LENGTH,
    search_lead: f64 = Range(0.0, 127.0),
    shortcut_gap: f64 = LENGTH,
    slider_big_step: f64 = LENGTH,
    slider_double_click: f64 = LENGTH,
    slider_double_click_distance: f64 = LENGTH,
    slider_gradient_track: f64 = LENGTH,
    slider_halo_radius: f64 = LENGTH,
    slider_halo_width: f64 = LENGTH,
    slider_height: f64 = LENGTH,
    slider_inset: f64 = LENGTH,
    slider_key_step_share: f64 = LENGTH,
    slider_knob_classic_h: f64 = LENGTH,
    slider_knob_classic_w: f64 = LENGTH,
    slider_knob_pro: f64 = LENGTH,
    slider_knob_pro_fill: StyleColour = ALPHA,
    slider_knob_pro_ring: StyleColour = ALPHA,
    slider_knob_studio: f64 = LENGTH,
    slider_knob_studio_fill: StyleColour = ALPHA,
    slider_long_press: f64 = LENGTH,
    slider_min_width: f64 = LENGTH,
    slider_plain_track: f64 = LENGTH,
    slider_row_gap: f64 = LENGTH,
    slider_shadow: StyleColour = ALPHA,
    slider_track_radius: f64 = LENGTH,
    slider_width: f64 = LENGTH,
    small_size: f64 = LENGTH,
    space_lg: f64 = LENGTH,
    space_md: f64 = LENGTH,
    space_sm: f64 = LENGTH,
    space_xl: f64 = LENGTH,
    space_xs: f64 = LENGTH,
    submenu_gap: f64 = LENGTH,
    switch_track_h: f64 = LENGTH,
    switch_track_w: f64 = LENGTH,
    tabs_chevron: f64 = LENGTH,
    tabs_chevron_hover_alpha: f64 = ALPHA,
    tabs_chevron_icon: f64 = LENGTH,
    tabs_chevron_inset_x: f64 = LENGTH,
    tabs_chevron_inset_y: f64 = LENGTH,
    tabs_chevron_menu_width: f64 = LENGTH,
    tabs_drop_outline: f64 = LENGTH,
    tabs_marker: f64 = LENGTH,
    tabs_marker_gap: f64 = LENGTH,
    tabs_pro_close: f64 = LENGTH,
    tabs_pro_close_from_right: f64 = LENGTH,
    tabs_pro_close_icon: f64 = LENGTH,
    tabs_pro_hover_alpha: f64 = ALPHA,
    tabs_pro_min: f64 = LENGTH,
    tabs_pro_pad: f64 = LENGTH,
    tabs_pro_size: f64 = LENGTH,
    tabs_pro_title_left: f64 = LENGTH,
    tabs_progress: f64 = LENGTH,
    tabs_strip: f64 = LENGTH,
    tabs_studio_close: f64 = LENGTH,
    tabs_studio_close_from_right: f64 = LENGTH,
    tabs_studio_close_icon: f64 = LENGTH,
    tabs_studio_close_radius: f64 = LENGTH,
    tabs_studio_gap: f64 = LENGTH,
    tabs_studio_hover_alpha: f64 = ALPHA,
    tabs_studio_margin_bottom: f64 = Range(0.0, 127.0),
    tabs_studio_margin_left: f64 = Range(0.0, 127.0),
    tabs_studio_margin_right: f64 = Range(0.0, 127.0),
    tabs_studio_margin_top: f64 = Range(0.0, 127.0),
    tabs_studio_meta_gap: f64 = LENGTH,
    tabs_studio_meta_size: f64 = LENGTH,
    tabs_studio_name_left: f64 = LENGTH,
    tabs_studio_name_size: f64 = LENGTH,
    tabs_studio_pad: f64 = LENGTH,
    title_bar_margin: f64 = LENGTH,
    title_combo_max_width: f64 = LENGTH,
    title_combo_min_width: f64 = LENGTH,
    title_control_spacing: f64 = LENGTH,
    title_gap: f64 = LENGTH,
    title_link_room: f64 = LENGTH,
    title_min_room: f64 = LENGTH,
    title_size: f64 = LENGTH,
    toggle_box_radius: f64 = LENGTH,
    toggle_knob: f64 = LENGTH,
    toggle_knob_inset: f64 = LENGTH,
    toggle_label_gap: f64 = LENGTH,
    toggle_mark: StyleColour = ALPHA,
    toggle_outline: f64 = LENGTH,
    toggle_tick: f64 = LENGTH,
    tooltip_delay: f64 = LENGTH,
    tooltip_grace: f64 = LENGTH,
    tooltip_width: f64 = LENGTH,
    value_number_size: f64 = LENGTH,
    value_unit_gap: f64 = LENGTH,
    value_unit_inset: f64 = LENGTH,
    value_unit_reserve: f64 = LENGTH,
    value_unit_size: f64 = LENGTH,
    value_width: f64 = LENGTH,
    weak_alpha: f64 = ALPHA,
    window_margin: f64 = Range(0.0, 127.0),
    window_shadow_blur: f64 = Range(0.0, 255.0),
    window_shadow_offset_x: f64 = Range(-128.0, 127.0),
    window_shadow_offset_y: f64 = Range(-128.0, 127.0),
    window_shadow_spread: f64 = Range(0.0, 255.0),
    dark_luminance: f64 = ALPHA,
    square_radius: u8 = RADIUS,
    bar_padding_y: f64 = Range(0.0, 127.0),
}

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
    let (family, _) = complete_family(family, crate::compat::table());
    let styles = compile_styles(&family)?;
    let name = style.map_or_else(
        || family.schemes[scheme.name()].clone(),
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

fn complete_family(
    family: &StyleFamilySource,
    table: &crate::compat::Table,
) -> (StyleFamilySource, BTreeMap<String, String>) {
    let mut family = family.clone();
    let mut provenance = BTreeMap::new();
    for (version, entries) in &table.versions {
        for (name, tokens) in &entries.styles {
            if !family.styles.contains_key(name) {
                family.styles.insert(name.clone(), tokens.clone());
                provenance.insert(format!("{PATH}.styles.{name}"), version.clone());
            }
        }
        for (scheme, style) in &entries.schemes {
            if Scheme::from_name(scheme).is_some() && !family.schemes.contains_key(scheme) {
                family
                    .schemes
                    .insert(scheme.clone(), style.style().to_owned());
                provenance.insert(format!("{PATH}.schemes.{scheme}"), version.clone());
            }
        }
    }
    (family, provenance)
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
        let tokens = filled_tokens(family, name, tokens, crate::compat::table());
        let mut reader = Reader {
            tokens: &tokens,
            path: path.clone(),
            errors: Vec::new(),
        };
        let style = read(&mut reader);
        validate_constraints(&style, &path, &mut reader.errors);
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
    for unbound in Scheme::ALL
        .into_iter()
        .filter(|s| !family.schemes.contains_key(s.name()))
    {
        let name = unbound.name();
        errors.push(DesignDiagnostic::error(
            "missing-style-binding",
            format!("{PATH}.schemes.{name}"),
            format!("scheme `{name}` has no style"),
        ));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(styles)
}

fn filled_tokens(
    family: &StyleFamilySource,
    name: &str,
    tokens: &BTreeMap<String, StyleValueSource>,
    table: &crate::compat::Table,
) -> BTreeMap<String, StyleValueSource> {
    let mut filled = tokens.clone();
    let base = match tokens.get("base") {
        Some(StyleValueSource::Name(base)) if Style::from_name(base).is_some() => {
            Some(base.as_str())
        }
        _ => None,
    };
    let scheme = default_scheme(family, name);
    for token in TOKENS.iter().filter(|token| !REQUIRED.contains(token)) {
        if !filled.contains_key(*token)
            && let Some((value, _)) = table.token(token, name, base, scheme)
        {
            filled.insert((*token).into(), value.clone());
        }
    }
    filled
}

fn default_scheme(family: &StyleFamilySource, name: &str) -> &'static str {
    family
        .schemes
        .iter()
        .find_map(|(scheme, bound)| (bound == name).then(|| Scheme::from_name(scheme)).flatten())
        .map(|s| {
            if s.is_chrome_scheme() {
                s.name()
            } else {
                "plain"
            }
        })
        .unwrap_or("plain")
}

fn validate_constraints(style: &ResolvedStyle, path: &str, errors: &mut Vec<DesignDiagnostic>) {
    let values = style.values();
    for constraint in &crate::compat::table().constraints {
        if let (Some(StyleValueSource::Number(a)), Some(StyleValueSource::Number(b))) =
            (values.get(&constraint.a), values.get(&constraint.b))
            && constraint.kind == "le"
            && a > b
        {
            errors.push(DesignDiagnostic::error(
                "invalid-style-token",
                path,
                format!(
                    "`{}` ({a}) must not exceed `{}` ({b})",
                    constraint.a, constraint.b
                ),
            ));
        }
    }
}

#[cfg(test)]
fn default_diagnostics(source: &DesignV1Source) -> Vec<DesignDiagnostic> {
    crate::compat::diagnostics(&default_provenance(source, crate::compat::table()))
}

pub(crate) fn default_provenance(
    source: &DesignV1Source,
    table: &crate::compat::Table,
) -> BTreeMap<String, String> {
    let Some(family) = &source.families.style else {
        return BTreeMap::new();
    };
    let (family, mut provenance) = complete_family(family, table);
    for (name, tokens) in &family.styles {
        let filled = filled_tokens(&family, name, tokens, table);
        let base = match tokens.get("base") {
            Some(StyleValueSource::Name(base)) if Style::from_name(base).is_some() => {
                Some(base.as_str())
            }
            _ => None,
        };
        for token in filled.keys().filter(|token| !tokens.contains_key(*token)) {
            if let Some((_, version)) =
                table.token(token, name, base, default_scheme(&family, name))
            {
                provenance.insert(format!("{PATH}.styles.{name}.{token}"), version.to_owned());
            }
        }
    }
    provenance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesignCompileResult, DesignContext, Mode, SourceIdentity};

    #[test]
    fn required_names_match_the_frozen_source() {
        let old = crate::parse_design_source(
            SourceIdentity::new("frozen"),
            include_str!("../../tests/fixtures/revision-1-before-axes.theme.conf.mix"),
        )
        .unwrap();
        let family = old.v1.families.style.unwrap();
        let names = family.styles["studio"]
            .keys()
            .filter(|name| !RETIRED.contains(&name.as_str()))
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(REQUIRED.len(), 40);
        assert_eq!(REQUIRED.into_iter().collect::<BTreeSet<_>>(), names);
        assert_eq!(&TOKENS[..40], REQUIRED);
    }

    #[test]
    fn a_custom_style_keeps_its_defaults_when_a_later_table_is_appended() {
        let source = crate::parse_design_source(
            SourceIdentity::new("custom"),
            include_str!("../../tests/fixtures/custom-named-style.theme.conf.mix"),
        )
        .unwrap();
        let family = source.v1.families.style.as_ref().unwrap();
        let v2 = crate::compat::table();
        let mut v3 = v2.clone();
        v3.versions.insert(
            "v4".into(),
            crate::compat::Version {
                tokens: BTreeMap::from([(
                    "future_token".into(),
                    BTreeMap::from([("all".into(), StyleValueSource::Number(2.0))]),
                )]),
                ..Default::default()
            },
        );
        let original = &family.styles["custom"];
        assert_eq!(
            filled_tokens(family, "custom", original, v2),
            filled_tokens(family, "custom", original, &v3)
        );
        assert_eq!(compile_family(family, Scheme::Studio).unwrap(), studio());
        let DesignCompileResult::Success(compiled) = crate::compile_design(
            &source,
            DesignContext {
                scheme: Scheme::Studio,
                ..Default::default()
            },
        ) else {
            panic!("custom style compiles");
        };
        assert!(
            compiled
                .diagnostics
                .iter()
                .any(|d| d.code == "defaulted-style-token")
        );
        assert!(!compiled.candidate.dictionary().defaulted.is_empty());
    }

    #[test]
    fn unsafe_behaviour_values_and_an_unreachable_corner_are_refused() {
        let style = compile_family(&family(), Scheme::Studio).unwrap();
        for (name, below) in [
            ("resize_edge", 1.99),
            ("resize_corner", 1.99),
            ("scrub_rate", 0.049),
            ("fine_scrub_rate", 0.0009),
            ("fine_span", -0.01),
            ("hot_distance", -0.01),
            ("menu_scroll_speed", 99.0),
            ("double_click_time", 0.099),
        ] {
            let result = style.with_values(&BTreeMap::from([(
                name.into(),
                StyleValueSource::Number(below),
            )]));
            assert!(
                result
                    .unwrap_err()
                    .iter()
                    .any(|e| e.code == "invalid-style-token"),
                "{name}"
            );
        }
        assert!(
            style
                .with_values(&BTreeMap::from([(
                    "icon_mask".into(),
                    StyleValueSource::Name("#001111".into())
                )]))
                .is_err()
        );
        let errors = style
            .with_values(&BTreeMap::from([(
                "resize_edge".into(),
                StyleValueSource::Number(13.0),
            )]))
            .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("resize_edge") && e.message.contains("resize_corner"))
        );
    }

    #[test]
    fn canvas_pitch_refuses_unbounded_paint_density_before_drawing() {
        let style = compile_family(&family(), Scheme::Studio).unwrap();
        for value in [f64::NAN, f64::INFINITY, -1.0, 0.0, 0.001, 3.99, 1000.01] {
            assert!(
                style
                    .with_values(&BTreeMap::from([(
                        "canvas_dot_pitch".into(),
                        StyleValueSource::Number(value)
                    )]))
                    .is_err(),
                "{value}"
            );
        }
        for value in [4.0, 16.0, 1000.0] {
            assert_eq!(
                style
                    .with_values(&BTreeMap::from([(
                        "canvas_dot_pitch".into(),
                        StyleValueSource::Number(value)
                    )]))
                    .unwrap()
                    .canvas_dot_pitch,
                value
            );
        }
        let mut source = embedded();
        source
            .v1
            .families
            .style
            .as_mut()
            .unwrap()
            .styles
            .get_mut("studio")
            .unwrap()
            .insert("canvas_dot_pitch".into(), StyleValueSource::Number(0.001));
        assert!(matches!(
            crate::compile_design(&source, DesignContext::default()),
            DesignCompileResult::Fatal(_)
        ));
    }

    #[test]
    fn structured_defaults_keep_the_actual_supplier_version_for_a_style_named_all() {
        let mut source = crate::parse_design_source(
            SourceIdentity::new("all"),
            include_str!("../../tests/fixtures/custom-named-style.theme.conf.mix"),
        )
        .unwrap();
        let family = source.v1.families.style.as_mut().unwrap();
        let tokens = family.styles.remove("custom").unwrap();
        family.styles.insert("all".into(), tokens);
        for bound in family.schemes.values_mut() {
            if bound == "custom" {
                *bound = "all".into();
            }
        }
        assert!(matches!(
            crate::compile_design(&source, DesignContext::default()),
            DesignCompileResult::Success(_)
        ));
        let mut later = crate::compat::table().clone();
        let values = later
            .versions
            .get_mut("v2")
            .unwrap()
            .tokens
            .remove("canvas_dot_pitch")
            .unwrap();
        later.versions.insert(
            "v4".into(),
            crate::compat::Version {
                tokens: BTreeMap::from([("canvas_dot_pitch".into(), values)]),
                ..Default::default()
            },
        );
        let origins = default_provenance(&source.v1, &later);
        assert_eq!(
            origins["design.v1.families.style.styles.all.canvas_dot_pitch"],
            "v4"
        );
        assert_eq!(
            origins["design.v1.families.style.styles.all.field_radius"],
            "v2"
        );
        let diagnostics = crate::compat::diagnostics(&origins);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.path.ends_with("all.canvas_dot_pitch") && d.message.ends_with("v4"))
        );
    }

    #[test]
    fn embedded_tokens_need_no_defaults_and_old_sources_report_them() {
        assert!(default_diagnostics(&embedded().v1).is_empty());
        let old = crate::parse_design_source(
            SourceIdentity::new("old"),
            include_str!("../../tests/fixtures/revision-1-before-axes.theme.conf.mix"),
        )
        .unwrap();
        let diagnostics = default_diagnostics(&old.v1);
        assert!(!diagnostics.is_empty());
        assert!(
            diagnostics
                .iter()
                .all(|d| d.severity == crate::DiagnosticSeverity::Info)
        );
    }

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
            ..compile_family(&family(), Scheme::Studio).unwrap()
        }
    }

    fn breeze() -> ResolvedStyle {
        ResolvedStyle {
            field_radius: 5,
            menu_highlight_radius: 5,
            radius_sm: 5,
            radius: 5,
            radius_lg: 6,
            ..studio()
        }
    }

    fn adwaita() -> ResolvedStyle {
        ResolvedStyle {
            field_radius: 6,
            radius: 9,
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
            field_radius: 3,
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
            field_radius: 0,
            scroll_width: 6.0,
            scroll_width_hot: 6.0,
            scroll_allocated: 0.0,
            scroll_handle_alpha_rest: 0.0,
            scroll_track_alpha_active: 0.4,
            scroll_track_alpha_hot: 0.7,
            radius_sm: 0,
            radius: 0,
            radius_lg: 0,
            ..studio()
        }
    }

    /// The hue schemes: today's derived look, as data.
    fn plain() -> ResolvedStyle {
        ResolvedStyle {
            item_spacing_x: 4.0,
            item_spacing_y: 3.0,
            window_margin: 8.0,
            popup_shadow_offset_y: 2.0,
            popup_shadow_blur: 10.0,
            popup_shadow_alpha: 0.45,
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
                Scheme::Breeze => breeze(),
                Scheme::Adwaita => adwaita(),
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
        unbound.schemes.remove("forest");
        assert_eq!(
            codes(compile_family(&unbound, Scheme::Ocean)),
            ["missing-style-binding"]
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
                    let DesignCompileResult::Success(old) =
                        crate::compile_design(&before, context.clone())
                    else {
                        panic!("{scheme:?}/{mode:?}/{style:?} must compile");
                    };
                    let DesignCompileResult::Success(new) = crate::compile_design(&today, context)
                    else {
                        panic!("today's design compiles");
                    };
                    if matches!(scheme, Scheme::Breeze | Scheme::Adwaita) && style.is_none() {
                        assert!(old.candidate.dictionary().defaulted.keys().any(|path| path.ends_with(scheme.name())));
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
            (Style::Breeze, breeze()),
            (Style::Adwaita, adwaita()),
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

    #[test]
    fn modern_schemes_compile_in_every_style_and_old_sources_take_the_palette() {
        assert_eq!((Scheme::ALL.len(), Style::ALL.len()), (11, 6));
        let document = embedded();
        let before = crate::parse_design_source(
            SourceIdentity::new("before-modern-schemes"),
            include_str!("../../tests/fixtures/revision-1-before-axes.theme.conf.mix"),
        )
        .unwrap();
        for scheme in Scheme::ALL {
            for style in Style::ALL {
                for mode in Mode::ALL {
                    let context = DesignContext {
                        scheme,
                        mode,
                        style: Some(style),
                        ..Default::default()
                    };
                    let crate::DesignCompileResult::Success(today) =
                        crate::compile_design(&document, context.clone())
                    else {
                        panic!("{scheme:?}/{style:?}/{mode:?} must compile");
                    };
                    if matches!(scheme, Scheme::Breeze | Scheme::Adwaita) {
                        let crate::DesignCompileResult::Success(old) =
                            crate::compile_design(&before, context)
                        else {
                            panic!("old source must compile in {scheme:?}/{style:?}/{mode:?}");
                        };
                        assert_eq!(
                            old.candidate.dictionary().chrome,
                            today.candidate.dictionary().chrome
                        );
                        assert_eq!(old.candidate.dictionary().colours.pairs, today.candidate.dictionary().colours.pairs);
                        assert!(old.diagnostics.iter().any(|d| d.code == "defaulted-scheme"));
                        assert!(old.candidate.dictionary().terminal.is_some());
                        let provenance = old
                            .candidate
                            .provenance()
                            .value(&crate::DesignValueId::ColourPrimitive(
                                "chrome.chrome".into(),
                            ))
                            .unwrap();
                        assert!(
                            provenance
                                .value_origin_rule
                                .starts_with("compat.v3.schemes.")
                        );
                        assert_eq!(
                            old.candidate.dictionary().defaulted["design.v1.primitives.colors.chrome.chrome"],
                            "v3"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_authored_modern_binding_keeps_its_palette() {
        let mut document = embedded();
        for modifier in &mut document.v1.modifiers {
            if modifier
                .when
                .get(&crate::source::ModifierAxis::Scheme)
                .map(String::as_str)
                == Some("breeze")
            {
                let color = modifier.primitives.colors["chrome.dock"];
                modifier
                    .primitives
                    .colors
                    .insert("chrome.chrome".into(), color);
            }
        }
        let crate::DesignCompileResult::Success(result) = crate::compile_design(
            &document,
            DesignContext {
                scheme: Scheme::Breeze,
                ..Default::default()
            },
        ) else {
            panic!("authored Breeze compiles");
        };
        let chrome = result.candidate.dictionary().chrome.as_ref().unwrap();
        assert_eq!(chrome.get("chrome"), chrome.get("dock"));
        assert!(
            !result
                .candidate
                .dictionary()
                .defaulted
                .contains_key("design.v1.primitives.colors.chrome.chrome")
        );
        let provenance = result
            .candidate
            .provenance()
            .value(&crate::DesignValueId::ColourPrimitive(
                "chrome.chrome".into(),
            ))
            .unwrap();
        assert!(
            provenance
                .value_origin_rule
                .starts_with("design.v1.modifiers[")
        );
    }

    #[test]
    fn modern_palette_defaults_without_a_style_family() {
        let mut document = embedded();
        document.v1.families.style = None;
        document.v1.modifiers.retain(|modifier| {
            modifier
                .when
                .get(&crate::source::ModifierAxis::Scheme)
                .is_none_or(|scheme| scheme != "breeze")
        });
        let crate::DesignCompileResult::Success(result) = crate::compile_design(
            &document,
            DesignContext {
                scheme: Scheme::Breeze,
                ..Default::default()
            },
        ) else {
            panic!("a source without a style family takes the palette");
        };
        assert!(result.candidate.dictionary().style.is_none());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "defaulted-scheme")
        );
        assert_eq!(
            result
                .candidate
                .dictionary()
                .chrome
                .as_ref()
                .unwrap()
                .get("chrome")
                .unwrap()
                .to_srgba8(),
            [38, 42, 48, 255]
        );
    }

    #[test]
    fn a_scheme_modifier_without_a_binding_takes_the_complete_default_scheme() {
        let mut document = embedded();
        document.v1.families.style.as_mut().unwrap().schemes.remove("breeze");
        for modifier in &mut document.v1.modifiers {
            if modifier.when.get(&crate::source::ModifierAxis::Scheme).map(String::as_str) == Some("breeze") {
                modifier.primitives.colors.insert("chrome.chrome".into(), modifier.primitives.colors["chrome.dock"]);
            }
        }
        let crate::DesignCompileResult::Success(result) = crate::compile_design(&document, DesignContext {
            scheme: Scheme::Breeze, ..Default::default()
        }) else { panic!("the missing binding selects the complete default scheme"); };
        assert_eq!(result.candidate.dictionary().chrome.as_ref().unwrap().get("chrome").unwrap().to_srgba8(), [38,42,48,255]);
        assert_eq!(result.candidate.dictionary().defaulted["design.v1.families.style.schemes.breeze"], "v3");
    }
}
