// SPDX-License-Identifier: MIT OR Apache-2.0
//! Window chrome: the colours and geometry of the title bar, the menu bar and
//! its menus, and, for the chrome styles, the whole egui style.
//!
//! Every form and length the chrome takes comes from the theme's style
//! ([`Theme::style`], the design's style family): tab strip or cards,
//! checkbox or switch, bevels or outlines, title-bar height, bar sizes,
//! radii. Components read [`Chrome::style`] and [`Chrome::metrics`] and never
//! ask which scheme they draw, so a new style is design data alone.
//!
//! **Chrome widgets** ([`Widgets::Chrome`]; the shipped `pro`, `studio` and
//! `classic` styles) take every colour from the design's chrome family
//! (`dictionary().chrome`, one exact colour per role), on egui's light or
//! dark base as the chrome colour's lightness says ([`dark_base`]). **Pair widgets** (the `plain`
//! style of the hue schemes) keep the pair-based style of [`crate::style`];
//! their [`Palette`] is read back from that style ([`Palette::from_visuals`]),
//! so the title bar and menus have one code path for every scheme.
//!
//! The egui state table (specification §1.5), for the chrome schemes:
//!
//! | egui state | fill | stroke (1 pt) | text |
//! |---|---|---|---|
//! | noninteractive | `card` | `separator` | `text_dim` |
//! | inactive | `field` | `field_border` | `text` |
//! | hovered | `hover` | `field_border` | `text` |
//! | active | `pressed` | `accent_border` | `text` |
//! | open | `hover` | `field_border` | `text` |
//!
//! A style with [`WidgetStroke::Bevel`] (Classic) replaces every widget
//! stroke with its dark bevel edge, which the Classic theme authors as
//! `text_dim` (the rule [`Palette::widget_stroke`]). Colours never appear as
//! literals here: each is a role, or a rule over roles (an opacity). Lengths
//! the style does not carry are the named constants below, each with its
//! specification section.

use crate::style::colour;
use crate::theme::Theme;
use design::family::style::{ScrollBars, Selection, WidgetStroke, Widgets};
use design::{CaptionSide, Decorations, Mode, ResolvedChrome, ResolvedStyle};
use egui::{
    Color32, Context, CornerRadius, FontFamily, FontId, Id, Margin, Rect, Shadow, Stroke, Style,
    TextStyle, Vec2, Visuals, style::ScrollStyle, vec2,
};

/// Whether egui's dark base palette underlies `theme` (§1.5): for pair
/// widgets the theme's mode; for chrome widgets the palette's own, read from
/// its `chrome` role, so a palette's lightness and not its style decides
/// (the shipped Pro is dark in both modes, its light mode being medium grey;
/// Studio follows the mode; Classic is light).
pub fn dark_base(theme: &Theme) -> bool {
    let chrome = Chrome::for_theme(theme);
    chrome.is_dark(theme.mode())
}

/// A chrome surface at or below this relative luminance is dark: mid grey,
/// CIE L* 50.
const DARK_LUMINANCE: f32 = 0.18;

macro_rules! palette {
    ($($role:ident),+ $(,)?) => {
        /// One colour per chrome role, as egui renders it.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Palette { $(pub $role: Color32),+ }

        impl Palette {
            /// The role names, in field order: the design's chrome vocabulary.
            pub const ROLES: &[&str] = &[$(stringify!($role)),+];

            /// `self` with every role `roles` authors replaced by its exact colour.
            pub fn overlay(mut self, roles: &ResolvedChrome) -> Self {
                $(if let Some(value) = roles.get(stringify!($role)) {
                    self.$role = colour(value);
                })+
                self
            }

            /// The colour of the role `name`.
            pub fn get(&self, name: &str) -> Option<Color32> {
                match name {
                    $(stringify!($role) => Some(self.$role),)+
                    _ => None,
                }
            }
        }
    };
}

palette!(
    chrome,
    canvas,
    canvas_dot,
    dock,
    card,
    card_border,
    tab_strip,
    field,
    field_border,
    hover,
    pressed,
    row_selected,
    text,
    text_dim,
    text_faint,
    icon,
    accent,
    accent_soft,
    accent_border,
    accent_text,
    menu_highlight,
    menu_highlight_text,
    separator,
    shadow,
    scrim,
    primary_bg,
    primary_text,
    danger,
    warning,
    caption_close,
    caption_close_text,
);

impl Palette {
    /// The roles read back from a pair-based egui style, for the hue schemes
    /// and for any role a design leaves out. Close keeps the destructive
    /// colour it always had, with the panel colour as its glyph.
    pub fn from_visuals(v: &Visuals) -> Self {
        let w = &v.widgets;
        let text = v.text_color();
        Self {
            chrome: v.panel_fill,
            canvas: v.extreme_bg_color,
            canvas_dot: v.extreme_bg_color,
            dock: v.faint_bg_color,
            card: v.window_fill,
            card_border: v.window_stroke.color,
            tab_strip: v.faint_bg_color,
            field: v.extreme_bg_color,
            field_border: w.inactive.bg_stroke.color,
            hover: w.hovered.weak_bg_fill,
            pressed: w.active.weak_bg_fill,
            row_selected: v.selection.bg_fill,
            text,
            text_dim: text,
            text_faint: v.weak_text_color(),
            icon: text,
            accent: v.selection.bg_fill,
            accent_soft: v.selection.bg_fill,
            accent_border: v.selection.stroke.color,
            accent_text: v.selection.stroke.color,
            menu_highlight: w.hovered.weak_bg_fill,
            menu_highlight_text: text,
            separator: w.noninteractive.bg_stroke.color,
            shadow: v.popup_shadow.color,
            scrim: v.popup_shadow.color,
            primary_bg: v.hyperlink_color,
            primary_text: v.panel_fill,
            danger: v.error_fg_color,
            warning: v.warn_fg_color,
            caption_close: v.error_fg_color,
            caption_close_text: v.panel_fill,
        }
    }

    /// The 1 pt stroke of every widget state (§1.4): a bevelled style draws
    /// its dark bevel edge (authored as `text_dim`) where the others draw
    /// the role.
    pub fn widget_stroke(&self, style: &ResolvedStyle, role: Color32) -> Color32 {
        match style.widget_stroke {
            WidgetStroke::Bevel => self.text_dim,
            WidgetStroke::Role => role,
        }
    }

    /// The selection fill (§1.4): `accent` (Pro, Classic) or the quieter
    /// `accent_soft` (Studio), as the style says.
    pub fn selection(&self, style: &ResolvedStyle) -> Color32 {
        match style.selection {
            Selection::Accent => self.accent,
            Selection::AccentSoft => self.accent_soft,
        }
    }

    /// A bevel's lit edge (§1.4: white), which the Classic theme authors as
    /// `field`.
    pub fn bevel_light(&self) -> Color32 {
        self.field
    }

    /// A bevel's shaded edge (§1.4: `#404040`), authored as `text_dim`.
    pub fn bevel_dark(&self) -> Color32 {
        self.text_dim
    }
}

/// A Classic bevel round `rect` (§1.4): a raised surface is lit on its top
/// and left edges and shaded on its bottom and right; a sunken one swaps
/// them. Each edge is 1 pt, inside the rect.
pub fn bevel(painter: &egui::Painter, rect: Rect, raised: bool, palette: &Palette) {
    let (lit, shade) = if raised {
        (palette.bevel_light(), palette.bevel_dark())
    } else {
        (palette.bevel_dark(), palette.bevel_light())
    };
    let r = rect.shrink(0.5);
    painter.line_segment([r.left_bottom(), r.left_top()], Stroke::new(1.0, lit));
    painter.line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, lit));
    painter.line_segment([r.right_top(), r.right_bottom()], Stroke::new(1.0, shade));
    painter.line_segment([r.right_bottom(), r.left_bottom()], Stroke::new(1.0, shade));
}

/// Weak text is the text colour at this opacity (§1.4, egui's default).
pub const WEAK_ALPHA: f32 = 0.6;

/// Disabled widgets draw at this opacity (§1.4, egui's default).
pub const DISABLED_ALPHA: f32 = 0.5;

/// A pressed Close caption is `caption_close` at this opacity (§1.4).
pub const CLOSE_PRESSED_ALPHA: f32 = 0.8;

/// Chrome geometry, in points: the style's lengths ([`Metrics::of`]) and
/// the specification's fixed ones (§2, §3.1–3.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// Body and Button text size (§2.1: 12 Pro, 12.5 otherwise).
    pub body_size: f32,
    /// A push button's height (§3.13: 28 Pro, 30 otherwise).
    pub push_height: f32,
    /// A primary push button's opacity hovered, then pressed (§1.4).
    pub primary_alpha: [f32; 2],
    /// The options, status, tool and rail bars (§2.6).
    pub bars: crate::bars::Sizes,
    /// The right dock's default width and its inner margin (§2.6).
    pub dock_width: f32,
    pub dock_margin: f32,
    /// Title-bar height (§2.6: 32 Pro, 38 Studio and Classic).
    pub title_bar_height: f32,
    /// Inner margin before the mark (§3.1).
    pub title_bar_margin: f32,
    /// The app mark's square size (§3.1: 18 Pro, 20 otherwise).
    pub mark: f32,
    /// From the mark's right edge to the first menu title (§3.1: 6 + 8).
    pub mark_gap: f32,
    /// The window title keeps this far from the menus and controls (§3.1).
    pub title_gap: f32,
    /// The window title's size, Inter Medium (§2.1).
    pub title_size: f32,
    /// Width of one caption button (§3.2).
    pub caption_width: f32,
    /// Side of a caption glyph's box (§3.2).
    pub caption_glyph: f32,
    /// Free space kept left of the caption buttons (§3.1).
    pub caption_gap: f32,
    /// Resize zone thickness and corner size (§3.1).
    pub resize_edge: f32,
    pub resize_corner: f32,
    /// Menu-bar title padding and height (§3.3: 6 x 3, 24 high).
    pub menu_title_padding: Vec2,
    pub menu_title_height: f32,
    /// Minimum menu content width (§3.4).
    pub menu_min_width: f32,
    /// Menu row height and padding (§3.5: 24; 10 x 4 Pro, 2 x 0 otherwise).
    pub menu_row_height: f32,
    pub menu_row_padding: Vec2,
    /// Corner radius of the menu-row highlight (§2.4: 3 in Pro).
    pub menu_highlight_radius: u8,
    /// A separator's band height (§3.5).
    pub menu_separator_height: f32,
    /// From a menu frame's right edge to its submenu's left edge (§3.5).
    pub submenu_gap: f32,
    /// Least space between a row's label and its shortcut. The
    /// specification only fixes the 220 pt minimum width; a two-item gap
    /// keeps the longest measured row ("Open As…  Ctrl+Alt+Shift+O") inside it.
    pub shortcut_gap: f32,
    /// Menus keep this far from the window edges (§3.4).
    pub edge_gap: f32,
    /// Radii: widgets, menus and popups, floating windows (§1.3).
    pub radius_sm: u8,
    pub radius: u8,
    pub radius_lg: u8,
    /// The gap between items, and a button's inner padding (§2.2).
    pub item_spacing: Vec2,
    pub button_padding: Vec2,
    /// An interactive control's height and least width (§2.2: 24).
    pub control_height: f32,
    /// A push button's width beyond its label (§3.13: 14 pt each side).
    pub push_padding: f32,
}

/// Lengths shared by every style (§2.2, §3.1–3.5).
const MENU_MARGIN: i8 = 6;
const WINDOW_MARGIN: i8 = 16;

impl Metrics {
    /// The geometry of `style`: its own lengths, and the specification's
    /// fixed ones.
    pub fn of(style: &ResolvedStyle) -> Self {
        let pt = |value: f64| value as f32;
        Self {
            body_size: pt(style.body_size),
            push_height: pt(style.push_height),
            primary_alpha: [pt(style.primary_hover_alpha), pt(style.primary_press_alpha)],
            bars: crate::bars::Sizes {
                options: pt(style.options_bar),
                status: pt(style.status_bar),
                tool: pt(style.tool_bar),
                tool_button: pt(style.tool_button),
                tool_margin: pt(style.tool_margin),
                rail: pt(style.rail),
                rail_button: pt(style.rail_button),
            },
            dock_width: pt(style.dock_width),
            dock_margin: pt(style.dock_margin),
            title_bar_height: pt(style.title_bar_height),
            title_bar_margin: 10.0,
            mark: pt(style.mark),
            mark_gap: 14.0,
            title_gap: 16.0,
            title_size: 13.0,
            caption_width: 46.0,
            caption_glyph: 10.0,
            caption_gap: 4.0,
            resize_edge: 5.0,
            resize_corner: 12.0,
            menu_title_padding: vec2(6.0, 3.0),
            menu_title_height: pt(style.menu_title_height),
            menu_min_width: pt(style.menu_min_width),
            menu_row_height: pt(style.menu_row_height),
            menu_row_padding: vec2(pt(style.menu_row_padding_x), pt(style.menu_row_padding_y)),
            menu_highlight_radius: style.menu_highlight_radius,
            menu_separator_height: 9.0,
            // §3.5 gives 3.5 from the parent frame's inner edge; outer edge to
            // outer edge the evidence measures 2.5.
            submenu_gap: 2.5,
            shortcut_gap: 2.0 * pt(style.item_spacing_x),
            edge_gap: 6.0,
            radius_sm: style.radius_sm,
            radius: style.radius,
            radius_lg: style.radius_lg,
            item_spacing: vec2(pt(style.item_spacing_x), pt(style.item_spacing_y)),
            button_padding: vec2(pt(style.button_padding_x), pt(style.button_padding_y)),
            control_height: pt(style.control_height),
            push_padding: pt(style.push_padding),
        }
    }

    /// Geometry read back from an egui style, over `tokens`' (for a context
    /// no theme was installed on): the title bar is one control plus an item
    /// gap above and below, and menu rows are one control tall, inset by two
    /// thirds of that, at least eight wide. The shipped `plain` style
    /// authors exactly this for the hue schemes' pair style.
    pub fn from_egui(style: &Style, tokens: &ResolvedStyle) -> Self {
        let spacing = &style.spacing;
        let row = spacing.interact_size.y;
        let v = &style.visuals;
        Self {
            title_bar_height: row + 2.0 * spacing.item_spacing.y,
            mark: spacing.icon_width * 1.25,
            menu_row_height: row,
            menu_title_height: row,
            menu_row_padding: vec2(row * 2.0 / 3.0, 0.0),
            menu_min_width: row * 8.0,
            menu_highlight_radius: v.widgets.hovered.corner_radius.nw,
            radius_sm: v.widgets.inactive.corner_radius.nw,
            radius: v.menu_corner_radius.nw,
            radius_lg: v.window_corner_radius.nw,
            ..Self::of(tokens)
        }
    }
}

/// Everything the title bar and menus draw with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chrome {
    /// The forms the chrome takes (the theme's style).
    pub style: ResolvedStyle,
    pub palette: Palette,
    pub metrics: Metrics,
    /// Who draws the title bar, and where its captions go.
    pub decorations: Decorations,
    pub captions: CaptionSide,
}

fn key() -> Id {
    Id::new("toolkit.chrome")
}

impl Chrome {
    /// The chrome of `theme`: chrome-family roles over the pair-based
    /// palette for chrome widgets, the pair-based palette for pair widgets,
    /// and the style's geometry. A design that authors no style family and
    /// takes pair widgets keeps the geometry its own pair style derives
    /// ([`Metrics::from_egui`]), so its spacing and radii still size the
    /// title bar and menus.
    pub fn for_theme(theme: &Theme) -> Self {
        let style = theme.style();
        let pairs = crate::style::pair_style(theme);
        let mut palette = Palette::from_visuals(&pairs.visuals);
        if style.widgets == Widgets::Chrome
            && let Some(roles) = &theme.dictionary().chrome
        {
            palette = palette.overlay(roles);
        }
        // The accent as the design resolves it for this style.
        if let Some(accent) = theme.accent() {
            palette.accent = crate::style::srgb(accent);
        }
        let metrics = if theme.dictionary().style.is_none() && style.widgets == Widgets::Pairs {
            Metrics::from_egui(&pairs, &style)
        } else {
            Metrics::of(&style)
        };
        Self {
            style,
            palette,
            metrics,
            decorations: theme.decorations(),
            captions: theme.captions(),
        }
    }

    /// Whether this chrome draws on egui's dark base ([`dark_base`]).
    pub fn is_dark(&self, mode: Mode) -> bool {
        match self.style.widgets {
            Widgets::Pairs => mode == Mode::Dark,
            Widgets::Chrome => {
                let c = egui::Rgba::from(self.palette.chrome);
                0.2126 * c.r() + 0.7152 * c.g() + 0.0722 * c.b() <= DARK_LUMINANCE
            }
        }
    }

    /// The chrome installed on `ctx` ([`install`]), else one read back from
    /// the context's current style in the hue schemes' style.
    pub fn of(ctx: &Context) -> Self {
        ctx.data(|d| d.get_temp(key())).unwrap_or_else(|| {
            static PLAIN: std::sync::OnceLock<ResolvedStyle> = std::sync::OnceLock::new();
            let tokens = *PLAIN
                .get_or_init(|| Theme::for_context(design::DesignContext::revision_one()).style());
            let style = ctx.global_style();
            Self {
                style: tokens,
                palette: Palette::from_visuals(&style.visuals),
                metrics: Metrics::from_egui(&style, &tokens),
                decorations: Decorations::default(),
                captions: CaptionSide::default(),
            }
        })
    }

    /// The menu-bar and menu text: egui's Button style.
    pub fn menu_font(style: &Style) -> FontId {
        TextStyle::Button.resolve(style)
    }
}

/// Make `chrome` the one [`Chrome::of`] returns on `ctx`.
pub fn install(ctx: &Context, chrome: &Chrome) {
    ctx.data_mut(|d| d.insert_temp(key(), *chrome));
}

/// Small, Heading and Monospace sizes (§2.1).
pub const SMALL_SIZE: f32 = 10.5;
pub const HEADING_SIZE: f32 = 15.0;
pub const MONO_SIZE: f32 = 12.0;

/// The complete egui style of chrome widgets (§1.5, §2).
pub fn style(theme: &Theme) -> Style {
    let chrome = Chrome::for_theme(theme);
    let m = chrome.metrics;
    let mut style = Style {
        visuals: visuals(theme, &chrome),
        ..Style::default()
    };

    let spacing = &mut style.spacing;
    spacing.item_spacing = m.item_spacing;
    spacing.button_padding = m.button_padding;
    spacing.interact_size = vec2(m.control_height, m.control_height);
    spacing.slider_width = 150.0;
    spacing.combo_width = 120.0;
    spacing.menu_margin = Margin::same(MENU_MARGIN);
    spacing.window_margin = Margin::same(WINDOW_MARGIN);
    spacing.icon_width = 14.0;
    spacing.tooltip_width = 280.0;
    spacing.menu_width = m.menu_min_width;
    // Thin floating bars, or solid ones (Classic, §3.19). egui's presets
    // carry exactly the specified widths, margins and opacities.
    spacing.scroll = match chrome.style.scroll_bars {
        ScrollBars::Thin => ScrollStyle::thin(),
        ScrollBars::Solid => ScrollStyle::solid(),
    };

    let interaction = &mut style.interaction;
    interaction.tooltip_delay = 0.35;
    interaction.tooltip_grace_time = 0.2;
    interaction.show_tooltips_only_when_still = true;
    style.animation_time = 0.2;

    let body = m.body_size;
    let proportional = |size: f32| FontId::new(size, FontFamily::Proportional);
    style
        .text_styles
        .insert(TextStyle::Small, proportional(SMALL_SIZE));
    style
        .text_styles
        .insert(TextStyle::Body, proportional(body));
    style
        .text_styles
        .insert(TextStyle::Button, proportional(body));
    style
        .text_styles
        .insert(TextStyle::Heading, crate::fonts::heading(HEADING_SIZE));
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(MONO_SIZE, FontFamily::Monospace),
    );
    style
}

/// The colours of chrome widgets (§1.5).
fn visuals(theme: &Theme, chrome: &Chrome) -> Visuals {
    let (style, p, m) = (&chrome.style, &chrome.palette, &chrome.metrics);
    let mut v = if chrome.is_dark(theme.mode()) {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = p.chrome;
    v.window_fill = p.card;
    v.window_stroke = if style.outlines {
        Stroke::new(1.0, p.card_border)
    } else {
        Stroke::NONE
    };
    v.extreme_bg_color = p.field;
    v.code_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.faint_bg_color = p.card;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    v.override_text_color = Some(p.text);
    v.weak_text_alpha = WEAK_ALPHA;
    v.disabled_alpha = DISABLED_ALPHA;
    v.selection.bg_fill = p.selection(style);
    v.selection.stroke = Stroke::new(1.0, p.accent_text);

    let radius = CornerRadius::same(m.radius_sm);
    // A style without outlines parts its widgets by tone; the pressed
    // state keeps its accent edge.
    let stroke = |role| {
        if style.outlines {
            Stroke::new(1.0, p.widget_stroke(style, role))
        } else {
            Stroke::NONE
        }
    };
    let pressed = Stroke::new(1.0, p.widget_stroke(style, p.accent_border));
    let w = &mut v.widgets;
    let states = [
        (
            &mut w.noninteractive,
            p.card,
            Stroke::new(1.0, p.separator),
            p.text_dim,
        ),
        (&mut w.inactive, p.field, stroke(p.field_border), p.text),
        (&mut w.hovered, p.hover, stroke(p.field_border), p.text),
        (&mut w.active, p.pressed, pressed, p.text),
        (&mut w.open, p.hover, stroke(p.field_border), p.text),
    ];
    for (state, fill, bg_stroke, text) in states {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = bg_stroke;
        state.fg_stroke.color = text;
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    v.window_corner_radius = CornerRadius::same(m.radius_lg);
    v.menu_corner_radius = CornerRadius::same(m.radius);
    // §2.5. A transparent shadow role (Classic) draws nothing.
    v.window_shadow = Shadow {
        offset: [0, 10],
        blur: 32,
        spread: 0,
        color: p.shadow,
    };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: p.shadow,
    };
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.striped = false;
    v.indent_has_left_vline = false;
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use design::{DesignContext, Scheme};

    fn theme(scheme: Scheme, mode: Mode) -> Theme {
        Theme::for_context(DesignContext {
            scheme,
            mode,
            ..DesignContext::default()
        })
    }

    fn hex(c: Color32) -> String {
        let [r, g, b, a] = c.to_srgba_unmultiplied();
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }

    const CONTEXTS: [(Scheme, Mode); 6] = [
        (Scheme::Pro, Mode::Dark),
        (Scheme::Pro, Mode::Light),
        (Scheme::Studio, Mode::Dark),
        (Scheme::Studio, Mode::Light),
        (Scheme::Classic, Mode::Light),
        (Scheme::Classic, Mode::Dark),
    ];

    #[test]
    fn the_palette_speaks_the_design_vocabulary() {
        let mut ours: Vec<_> = Palette::ROLES.to_vec();
        let mut theirs: Vec<_> = design::family::chrome::ROLES.to_vec();
        ours.sort_unstable();
        theirs.sort_unstable();
        assert_eq!(ours, theirs);
    }

    #[test]
    fn every_chrome_role_is_the_design_colour_exactly() {
        for (scheme, mode) in CONTEXTS {
            let t = theme(scheme, mode);
            let roles = t.dictionary().chrome.clone().expect("chrome family");
            let palette = Chrome::for_theme(&t).palette;
            for (role, value) in roles.iter() {
                assert_eq!(
                    palette.get(role),
                    Some(colour(value)),
                    "{scheme:?}/{mode:?} {role}"
                );
            }
        }
    }

    /// The §1.5 state table and visuals, checked against the specification's
    /// values for each theme.
    #[test]
    fn roles_map_onto_egui_visuals() {
        // (scheme, mode, dark base, panel, window, field, hover, pressed, text, selection, widget stroke)
        let expected = [
            (
                Scheme::Pro,
                Mode::Dark,
                true,
                "#323232",
                "#323232",
                "#242424",
                "#424242",
                "#4E4E4E",
                "#DEDEDE",
                "#378EF0",
                "#4A4A4A",
            ),
            (
                Scheme::Pro,
                Mode::Light,
                true,
                "#535353",
                "#535353",
                "#454545",
                "#626262",
                "#707070",
                "#EEEEEE",
                "#378EF0",
                "#686868",
            ),
            (
                Scheme::Studio,
                Mode::Dark,
                true,
                "#141415",
                "#1A1A1C",
                "#232326",
                "#2C2C30",
                "#38383E",
                "#ECECF0",
                "#8B7CF62E",
                "#343439",
            ),
            (
                Scheme::Studio,
                Mode::Light,
                false,
                "#F6F6F8",
                "#FCFCFD",
                "#F2F2F5",
                "#E8E8ED",
                "#DCDCE2",
                "#18181C",
                "#6C5CE724",
                "#D6D6DC",
            ),
            (
                Scheme::Classic,
                Mode::Light,
                false,
                "#D4D0C8",
                "#D4D0C8",
                "#FFFFFF",
                "#E2DED6",
                "#BEBAB2",
                "#000000",
                "#0A246A",
                "#404040",
            ),
        ];
        for (scheme, mode, dark, panel, window, field, hover, pressed, text, selection, stroke) in
            expected
        {
            let t = theme(scheme, mode);
            let radius_sm = t.style().radius_sm;
            let v = style(&t).visuals;
            let rgb = |c: Color32| hex(c)[..7].to_owned();
            let at = format!("{scheme:?}/{mode:?}");
            assert_eq!(v.dark_mode, dark, "{at}");
            assert_eq!(rgb(v.panel_fill), panel, "{at}");
            assert_eq!(rgb(v.window_fill), window, "{at}");
            assert_eq!(rgb(v.extreme_bg_color), field, "{at}");
            assert_eq!(rgb(v.widgets.inactive.bg_fill), field, "{at}");
            assert_eq!(rgb(v.widgets.hovered.bg_fill), hover, "{at}");
            assert_eq!(rgb(v.widgets.open.bg_fill), hover, "{at}");
            assert_eq!(rgb(v.widgets.active.bg_fill), pressed, "{at}");
            assert_eq!(
                v.override_text_color.map(rgb).as_deref(),
                Some(text),
                "{at}"
            );
            assert_eq!(rgb(v.widgets.inactive.bg_stroke.color), stroke, "{at}");
            assert_eq!(rgb(v.widgets.hovered.bg_stroke.color), stroke, "{at}");
            // Studio selects with accent_soft, the accent at low alpha. egui
            // stores colours premultiplied, so compare as egui will paint.
            assert_eq!(
                v.selection.bg_fill,
                Color32::from_hex(selection).unwrap(),
                "{at}"
            );
            for w in [
                v.widgets.noninteractive,
                v.widgets.inactive,
                v.widgets.hovered,
                v.widgets.active,
                v.widgets.open,
            ] {
                assert_eq!(w.expansion, 0.0, "{at}: widgets never grow on hover");
                assert_eq!(w.corner_radius, CornerRadius::same(radius_sm), "{at}");
            }
        }
    }

    #[test]
    fn studio_selects_quietly_and_strokes_with_accent_text() {
        let t = theme(Scheme::Studio, Mode::Dark);
        let v = style(&t).visuals;
        // Translucent roles compare as egui stores them (premultiplied).
        let authored = |hex: &str| Color32::from_hex(hex).unwrap();
        assert_eq!(v.selection.bg_fill, authored("#8B7CF62E"));
        assert_eq!(hex(v.selection.stroke.color), "#D6D0FFFF");
        assert_eq!(v.widgets.active.bg_stroke.color, authored("#A094FF6E"));
    }

    #[test]
    fn style_metrics_follow_the_specification() {
        let metrics = |scheme| Chrome::for_theme(&theme(scheme, Mode::Dark)).metrics;
        let (pro, studio, classic) = (
            metrics(Scheme::Pro),
            metrics(Scheme::Studio),
            metrics(Scheme::Classic),
        );
        assert_eq!(
            (
                pro.title_bar_height,
                studio.title_bar_height,
                classic.title_bar_height
            ),
            (32.0, 38.0, 38.0)
        );
        // Menus start 42 pt in (Pro) and 44 pt (Studio).
        assert_eq!(pro.title_bar_margin + pro.mark + pro.mark_gap, 42.0);
        assert_eq!(
            studio.title_bar_margin + studio.mark + studio.mark_gap,
            44.0
        );
        assert_eq!(
            (pro.menu_row_padding, studio.menu_row_padding),
            (vec2(10.0, 4.0), vec2(2.0, 0.0))
        );
        assert_eq!(
            (
                pro.menu_highlight_radius,
                studio.menu_highlight_radius,
                classic.menu_highlight_radius
            ),
            (3, 6, 0)
        );
        assert_eq!(
            (classic.radius_sm, classic.radius, classic.radius_lg),
            (0, 0, 0)
        );
    }

    #[test]
    fn classic_draws_no_shadow_and_dark_bevel_strokes() {
        let v = style(&theme(Scheme::Classic, Mode::Light)).visuals;
        assert_eq!(v.popup_shadow.color.a(), 0);
        assert_eq!(hex(v.widgets.active.bg_stroke.color), "#404040FF");
        assert_eq!(v.menu_corner_radius, CornerRadius::ZERO);
    }

    #[test]
    fn sizes_follow_the_style() {
        let pro = style(&theme(Scheme::Pro, Mode::Light));
        let studio = style(&theme(Scheme::Studio, Mode::Dark));
        assert_eq!(pro.text_styles[&TextStyle::Button].size, 12.0);
        assert_eq!(studio.text_styles[&TextStyle::Button].size, 12.5);
        assert_eq!(pro.text_styles[&TextStyle::Heading].size, 15.0);
        assert_eq!(pro.spacing.interact_size, vec2(24.0, 24.0));
        assert_eq!(pro.spacing.menu_margin, Margin::same(6));
        assert_eq!(pro.visuals.popup_shadow.blur, 20);
    }

    #[test]
    fn hue_schemes_read_their_palette_back_from_the_pair_style() {
        let t = Theme::for_context(DesignContext::revision_one());
        let chrome = Chrome::for_theme(&t);
        assert_eq!(chrome.style.widgets, Widgets::Pairs);
        assert_eq!(
            chrome.palette.chrome,
            crate::style::style(&t).visuals.panel_fill
        );
    }

    /// The `plain` style authors, as data, exactly the geometry the hue
    /// schemes used to derive from their pair style, in every hue scheme and
    /// mode: a drift between the two is caught here.
    #[test]
    fn the_plain_style_is_the_pair_style_geometry() {
        for scheme in Scheme::REVISION_ONE {
            for mode in Mode::ALL {
                let t = theme(scheme, mode);
                let derived = Metrics::from_egui(&crate::style::pair_style(&t), &t.style());
                assert_eq!(
                    Chrome::for_theme(&t).metrics,
                    derived,
                    "{scheme:?}/{mode:?}"
                );
            }
        }
    }

    /// The design's accent accessor is what the toolkit draws: the accent
    /// fill, and the selection where the style selects with the accent, in
    /// every scheme, style and mode.
    #[test]
    fn the_design_accent_is_the_drawn_accent() {
        for scheme in Scheme::ALL {
            for style in design::Style::ALL {
                for mode in Mode::ALL {
                    let t = Theme::for_context(DesignContext {
                        scheme,
                        mode,
                        style: Some(style),
                        ..DesignContext::default()
                    });
                    let at = format!("{scheme:?}/{style:?}/{mode:?}");
                    let accent = design::resolved_accent(t.design()).expect("an accent");
                    let drawn = crate::style::srgb(accent);
                    let chrome = Chrome::for_theme(&t);
                    assert_eq!(chrome.palette.accent, drawn, "{at}");
                    let visuals = crate::style::style(&t).visuals;
                    if t.style().widgets == Widgets::Pairs
                        || t.style().selection == Selection::Accent
                    {
                        assert_eq!(visuals.selection.bg_fill, drawn, "{at}: the selection");
                    }
                    assert_eq!(t.accent(), Some(accent), "{at}");
                }
            }
        }
    }
}
