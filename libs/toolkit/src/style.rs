// SPDX-License-Identifier: MIT OR Apache-2.0
//! The resolved design mapped onto egui's [`Style`]: colours from the
//! semantic pairs and non-text colours, sizes from the metrics, scales and
//! typography roles.
//!
//! The chrome schemes (`pro`, `studio`, `classic`) are the exception: their
//! whole style comes from the chrome family and the chrome specification,
//! in [`crate::chrome`]. The table below is the hue schemes' mapping.
//!
//! The mapping, field by field. A value the design does not author takes
//! egui's own default for the theme's mode, never a literal colour:
//!
//! | egui | design source |
//! |---|---|
//! | `panel_fill`, body text | pair `base`, rendered surface / foreground |
//! | `window_fill`, menus, popups | pair `popover` |
//! | `faint_bg_color` (stripes, group frames) | pair `card` surface |
//! | `extreme_bg_color` (text edits, scroll tracks) | pair `muted` surface |
//! | `code_bg_color` | pair `muted` surface |
//! | weak text | base foreground at egui's `weak_text_alpha` |
//! | buttons at rest (`inactive`) | pair `secondary` |
//! | hovered / pressed buttons | `secondary` surface blended toward its foreground by [`HOVER_MIX`] / [`ACTIVE_MIX`] |
//! | `selection` fill / selected text | pair `accent` (the tinted control pair) |
//! | `hyperlink_color` | pair `primary` surface |
//! | `error_fg_color` | pair `destructive` surface |
//! | `warn_fg_color` | primitive `status.warning`, else `destructive` |
//! | widget and window strokes | non-text `border` |
//! | `item_spacing`, margins | `spacing` scale, steps [`SPACING_STEPS`] |
//! | `button_padding` / `interact_size` | `md + sm/2` by `sm`; `xl` square (10 x 4, 24 x 24 shipped) |
//! | corner radii | metric `radius`; menus `radius` x 2/3, windows twice it |
//! | menu and popup shadow | egui's shadow colour x [`MENU_SHADOW`], offset 0 x 2, blur 10 |
//! | stroke widths | metric `button.border_width` |
//! | (no egui slot) | non-text `ring`, `input`: egui draws focus with the selection stroke |
//! | `Body`/`Button` size | typography role `ui`; `Small` role `small`; `Heading` role `ui_display` |
//! | faces and weights | [`crate::fonts`]: each role's weight selects its static face |

use crate::theme::Theme;
use design::{LinearRgba, Mode, ResolvedColours, ResolvedMetricKind, TypographyRole};
use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, Style, TextStyle, Visuals};

/// Indices into the design's `spacing` scale for xs, sm, md, lg and xl. On
/// the shipped scale (0, 2, 4, 6, 8, 10, 12, 16, 20, 24) they are 2, 4, 8,
/// 16 and 24.
pub const SPACING_STEPS: [usize; 5] = [1, 2, 4, 7, 9];

/// How far a hovered button's surface moves toward its foreground.
pub const HOVER_MIX: f32 = 0.08;

/// How far a pressed or open button's surface moves toward its foreground.
pub const ACTIVE_MIX: f32 = 0.16;

/// How much of egui's popup shadow strength menus keep.
pub const MENU_SHADOW: f32 = 0.45;

/// A design colour (linear-light sRGB) as the egui colour it renders as.
pub fn colour(value: LinearRgba) -> Color32 {
    let [r, g, b, a] = value.to_srgba8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// `a` moved toward `b` by `t` (0..=1), in gamma space, as egui blends.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()), lerp(a.a(), b.a()))
}

/// The rendered halves of a named pair.
fn pair(colours: &ResolvedColours, name: &str) -> Option<(Color32, Color32)> {
    colours.pairs.get(name).map(|p| (colour(p.rendered_surface), colour(p.rendered_foreground)))
}

/// The egui style for `theme`. A style with chrome widgets takes its whole
/// egui style from the chrome family ([`crate::chrome::style`]); one with
/// pair widgets is built on egui's default for the theme's mode by
/// [`pair_style`].
pub fn style(theme: &Theme) -> Style {
    if theme.style().widgets == design::family::style::Widgets::Chrome {
        return crate::chrome::style(theme);
    }
    pair_style(theme)
}

/// The pair-based style of the table above, for any scheme. The chrome
/// palette of a hue scheme is read back from it, and a chrome scheme falls
/// back on it for any role its design leaves out.
pub fn pair_style(theme: &Theme) -> Style {
    let mut style = Style { visuals: pair_visuals(theme), ..Style::default() };
    let dictionary = theme.dictionary();
    let px = |name: &str| {
        dictionary
            .metrics
            .get(name)
            .filter(|metric| metric.kind == ResolvedMetricKind::Px)
            .map(|metric| metric.value as f32)
    };
    let step = |index: usize| dictionary.scales.get("spacing").and_then(|s| s.get(index)).map(|v| *v as f32);
    let [xs, sm, md, _lg, xl] = SPACING_STEPS.map(step);
    let spacing = &mut style.spacing;
    if let (Some(xs), Some(sm)) = (xs, sm) {
        spacing.item_spacing = egui::vec2(sm, xs.max(sm * 0.75));
    }
    // Controls and menus at desktop proportions: padding md + sm/2 by sm
    // (10 x 4 on the shipped scale) and xl-square controls (24 x 24).
    if let (Some(sm), Some(md)) = (sm, md) {
        spacing.button_padding = egui::vec2(md + sm / 2.0, sm);
    }
    if let Some(xl) = xl {
        spacing.interact_size = egui::vec2(xl, xl);
    }
    if let Some(md) = md {
        spacing.window_margin = Margin::same(md.round() as i8);
        spacing.menu_margin = Margin::same((md * 0.75).round() as i8);
    }

    let typography = theme.typography();
    let ui = design::active_typography(Some(typography), TypographyRole::Ui);
    let small = design::active_typography(Some(typography), TypographyRole::Small);
    let display = design::active_typography(Some(typography), TypographyRole::UiDisplay);
    let body = ui.font_size as f32;
    let text = |size: f32| FontId::new(size, FontFamily::Proportional);
    style.text_styles.insert(TextStyle::Body, text(body));
    style.text_styles.insert(TextStyle::Button, text(body));
    style.text_styles.insert(TextStyle::Small, text(small.font_size as f32));
    style.text_styles.insert(TextStyle::Heading, crate::fonts::heading(display.font_size as f32));
    let mono = design::active_typography(Some(typography), TypographyRole::Mono);
    style.text_styles.insert(TextStyle::Monospace, FontId::new(mono.font_size as f32, FontFamily::Monospace));

    if let Some(radius) = px("radius") {
        let small = CornerRadius::same(radius.round() as u8);
        let large = CornerRadius::same((radius * 2.0).round() as u8);
        let widgets = &mut style.visuals.widgets;
        for w in [&mut widgets.noninteractive, &mut widgets.inactive, &mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
            w.corner_radius = small;
        }
        // Menus are tight panels (about 4 px on the shipped design); only
        // windows take the large radius.
        style.visuals.menu_corner_radius = CornerRadius::same((radius * 2.0 / 3.0).round() as u8);
        style.visuals.window_corner_radius = large;
    }
    // A soft, close shadow under menus and popups: egui's own shadow colour at
    // under half strength, barely offset (egui's default sits 6 x 10 away).
    let shadow = &mut style.visuals.popup_shadow;
    *shadow = egui::Shadow { offset: [0, 2], blur: 10, spread: 0, color: shadow.color.gamma_multiply(MENU_SHADOW) };
    if let Some(width) = px("button.border_width") {
        let widgets = &mut style.visuals.widgets;
        for w in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
            w.bg_stroke.width = width;
        }
        style.visuals.window_stroke.width = width;
    }
    style
}

/// The colours of [`style`].
pub fn visuals(theme: &Theme) -> Visuals {
    style(theme).visuals
}

/// The pair-based colours. Every pair the design lacks keeps egui's value
/// for the mode.
fn pair_visuals(theme: &Theme) -> Visuals {
    let mut v = match theme.mode() {
        Mode::Dark => Visuals::dark(),
        _ => Visuals::light(),
    };
    let colours = &theme.dictionary().colours;
    let non_text = |name: &str| colours.non_text.get(name).map(|c| colour(c.value));

    if let Some((surface, text)) = pair(colours, "base") {
        v.panel_fill = surface;
        v.override_text_color = Some(text);
        v.widgets.noninteractive.bg_fill = surface;
        v.widgets.noninteractive.weak_bg_fill = surface;
        v.widgets.noninteractive.fg_stroke.color = text;
        v.text_edit_bg_color = Some(surface);
    }
    if let Some((surface, _)) = pair(colours, "popover") {
        v.window_fill = surface;
    }
    if let Some((surface, _)) = pair(colours, "card") {
        v.faint_bg_color = surface;
    }
    // Weak text stays egui's base text at `weak_text_alpha`: the muted pair's
    // foreground is text *on* the muted surface, not dim text on the base.
    if let Some((surface, _)) = pair(colours, "muted") {
        v.extreme_bg_color = surface;
        v.code_bg_color = surface;
    }
    if let Some((surface, text)) = pair(colours, "secondary") {
        let w = &mut v.widgets;
        w.inactive.bg_fill = surface;
        w.inactive.weak_bg_fill = surface;
        w.inactive.fg_stroke.color = text;
        let hovered = mix(surface, text, HOVER_MIX);
        let active = mix(surface, text, ACTIVE_MIX);
        for (state, fill) in [(&mut w.hovered, hovered), (&mut w.open, hovered), (&mut w.active, active)] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.fg_stroke.color = text;
        }
    }
    if let Some((surface, text)) = pair(colours, "accent") {
        v.selection.bg_fill = surface;
        v.selection.stroke = Stroke::new(v.selection.stroke.width, text);
    }
    if let Some((surface, _)) = pair(colours, "primary") {
        v.hyperlink_color = surface;
    }
    if let Some((surface, _)) = pair(colours, "destructive") {
        v.error_fg_color = surface;
        v.warn_fg_color = surface;
    }
    if let Some(warning) = colours.primitives.get("status.warning") {
        v.warn_fg_color = colour(*warning);
    }
    if let Some(border) = non_text("border") {
        let w = &mut v.widgets;
        for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.bg_stroke.color = border;
        }
        v.window_stroke.color = border;
    }
    v
}

/// Install `theme` on `ctx`: its style, its chrome and its fonts. The style
/// goes in both egui theme slots: the MixOS theme, not the system
/// preference, picks the mode.
pub fn apply(ctx: &egui::Context, theme: &Theme) {
    ctx.set_global_style(style(theme));
    crate::chrome::install(ctx, &crate::chrome::Chrome::for_theme(theme));
    crate::icons::install(ctx, crate::icons::stroke_width(theme));
    crate::fonts::install(ctx, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use design::{DesignContext, Scheme};

    #[test]
    fn every_hue_context_maps_without_falling_back_on_the_core_pairs() {
        for scheme in Scheme::ALL.into_iter().filter(|s| !s.is_chrome_scheme()) {
            for mode in Mode::ALL {
                let theme = Theme::for_context(DesignContext { scheme, mode, ..DesignContext::default() });
                let colours = &theme.dictionary().colours;
                let v = visuals(&theme);
                let (base, text) = pair(colours, "base").expect("base pair");
                assert_eq!(v.panel_fill, base, "{scheme:?}/{mode:?}");
                assert_eq!(v.override_text_color, Some(text));
                assert_eq!(v.dark_mode, mode == Mode::Dark);
                assert_ne!(v.widgets.hovered.bg_fill, v.widgets.inactive.bg_fill, "hover must be visible");
            }
        }
    }

    #[test]
    fn sizes_come_from_the_design() {
        let theme = Theme::for_context(DesignContext::revision_one());
        let style = style(&theme);
        let ui = design::active_typography(Some(theme.typography()), TypographyRole::Ui);
        assert_eq!(style.text_styles[&TextStyle::Body].size, ui.font_size as f32);
        assert!(style.spacing.item_spacing.x > 0.0);
    }

    #[test]
    fn mix_is_endpoint_exact() {
        let (a, b) = (Color32::from_gray(10), Color32::from_gray(250));
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
    }
}
