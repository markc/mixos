// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
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
//! | hovered / pressed buttons | `secondary` surface blended toward its foreground by `hover_mix` / `active_mix` |
//! | `selection` fill / selected text | pair `accent` (the tinted control pair) |
//! | `hyperlink_color` | pair `primary` surface |
//! | `error_fg_color` | pair `destructive` surface |
//! | `warn_fg_color` | primitive `status.warning`, else `destructive` |
//! | widget and window strokes | non-text `border` |
//! | `item_spacing`, margins | `spacing` scale, steps `spacing_steps` |
//! | `button_padding` / `interact_size` | `md + sm/2` by `sm`; `xl` square (10 x 4, 24 x 24 shipped) |
//! | corner radii | metric `radius`; menus `radius` x 2/3, windows twice it |
//! | menu and popup shadow | egui's shadow colour x `menu_shadow`, offset 0 x 2, blur 10 |
//! | stroke widths | metric `button.border_width` |
//! | (no egui slot) | non-text `ring`, `input`: egui draws focus with the selection stroke |
//! | `Body`/`Button` size | typography role `ui`; `Small` role `small`; `Heading` role `ui_display` |
//! | faces and weights | [`crate::fonts`]: each role's weight selects its static face |

use crate::theme::Theme;
use design::{LinearRgba, Mode, ResolvedColours, TypographyRole};
use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, Style, TextStyle, Visuals};

/// A design colour (linear-light sRGB) as the egui colour it renders as.
pub fn colour(value: LinearRgba) -> Color32 {
    let [r, g, b, a] = value.to_srgba8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// `a` moved toward `b` by `t` (0..=1), in gamma space, as egui blends.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

/// A design sRGB colour as the egui colour it renders as.
pub fn srgb(value: design::SrgbColour) -> Color32 {
    let [r, g, b, a] = value.to_srgba8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Defaults for context-free geometry helpers; widgets pass installed values.
pub(crate) fn default_tokens() -> design::ResolvedStyle {
    static TOKENS: std::sync::OnceLock<design::ResolvedStyle> = std::sync::OnceLock::new();
    *TOKENS.get_or_init(|| Theme::embedded().style())
}

pub fn token_colour(value: design::family::style::StyleColour) -> Color32 {
    let [r, g, b, a] = value.0;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// The rendered halves of a named pair.
fn pair(colours: &ResolvedColours, name: &str) -> Option<(Color32, Color32)> {
    colours
        .pairs
        .get(name)
        .map(|p| (colour(p.rendered_surface), colour(p.rendered_foreground)))
}

/// The egui style for `theme`. A style with chrome widgets takes its whole
/// egui style from the chrome family ([`crate::chrome::style`]); one with
/// pair widgets is built on egui's default for the theme's mode by
/// [`pair_style`].
pub fn style(theme: &Theme) -> Style {
    let mut style = if theme.style().widgets == design::family::style::Widgets::Chrome {
        crate::chrome::style(theme)
    } else {
        pair_style(theme)
    };
    let tokens = theme.style();
    style.spacing.scroll = scroll_style(&tokens);
    if theme.overlay().reduced_motion || theme.ui().reduced_motion {
        style.animation_time = 0.0;
        style.scroll_animation = egui::style::ScrollAnimation::none();
    }
    apply_role_overlay(&mut style.visuals, theme);
    apply_interface_size(&mut style, theme);
    style
}

/// Scale the interface's text to the session's font size, when it names one.
/// Every style but the monospace one moves by the same ratio, so the headings
/// keep their place above the body. The design's own size changes nothing.
fn apply_interface_size(style: &mut Style, theme: &Theme) {
    let Some(size) = theme.overlay().size else {
        return;
    };
    let typography = theme.typography();
    let design = design::active_typography(Some(typography), TypographyRole::Ui).font_size as f32;
    let ratio = size / design;
    if (ratio - 1.0).abs() < f32::EPSILON {
        return;
    }
    for (text, font) in style.text_styles.iter_mut() {
        if *text != TextStyle::Monospace {
            font.size *= ratio;
        }
    }
}

/// The pair-based style of the table above, for any scheme. The chrome
/// palette of a hue scheme is read back from it, and a chrome scheme falls
/// back on it for any role its design leaves out.
pub fn pair_style(theme: &Theme) -> Style {
    let mut style = Style {
        visuals: pair_visuals(theme),
        ..Style::default()
    };
    let tokens = theme.style();
    let spacing = &mut style.spacing;
    spacing.item_spacing = egui::vec2(tokens.item_spacing_x as f32, tokens.item_spacing_y as f32);
    spacing.button_padding = egui::vec2(
        tokens.button_padding_x as f32,
        tokens.button_padding_y as f32,
    );
    spacing.interact_size = egui::Vec2::splat(tokens.interact as f32);
    spacing.window_margin = Margin::same(tokens.window_margin as i8);
    spacing.menu_margin = Margin::same(tokens.menu_margin as i8);
    let typography = theme.typography();
    let ui = design::active_typography(Some(typography), TypographyRole::Ui);
    let small = design::active_typography(Some(typography), TypographyRole::Small);
    let display = design::active_typography(Some(typography), TypographyRole::UiDisplay);
    let body = ui.font_size as f32;
    let text = |size: f32| FontId::new(size, FontFamily::Proportional);
    style.text_styles.insert(TextStyle::Body, text(body));
    style.text_styles.insert(TextStyle::Button, text(body));
    style
        .text_styles
        .insert(TextStyle::Small, text(small.font_size as f32));
    style.text_styles.insert(
        TextStyle::Heading,
        crate::fonts::heading(display.font_size as f32),
    );
    let mono = design::active_typography(Some(typography), TypographyRole::Mono);
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(mono.font_size as f32, FontFamily::Monospace),
    );
    for font in style.text_styles.values_mut() {
        font.size *= theme.ui().text_scale as f32;
    }

    for widget in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(tokens.radius_sm);
    }
    style.visuals.menu_corner_radius = CornerRadius::same(tokens.radius);
    style.visuals.window_corner_radius = CornerRadius::same(tokens.radius_lg);
    let shadow = &mut style.visuals.popup_shadow;
    *shadow = egui::Shadow {
        offset: [
            tokens.popup_shadow_offset_x as i8,
            tokens.popup_shadow_offset_y as i8,
        ],
        blur: tokens.popup_shadow_blur as u8,
        spread: tokens.popup_shadow_spread as u8,
        color: shadow
            .color
            .gamma_multiply(tokens.popup_shadow_alpha as f32),
    };
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.bg_stroke.width = tokens.hairline_width as f32;
    }
    style.visuals.window_stroke.width = tokens.hairline_width as f32;
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
        let hovered = mix(surface, text, theme.style().pair_hover_mix as f32);
        let active = mix(surface, text, theme.style().pair_active_mix as f32);
        for (state, fill) in [
            (&mut w.hovered, hovered),
            (&mut w.open, hovered),
            (&mut w.active, active),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.fg_stroke.color = text;
        }
    }
    if let Some((_, text)) = pair(colours, "accent") {
        v.selection.stroke = Stroke::new(v.selection.stroke.width, text);
    }
    // The accent pair's surface, read through the design's one accessor.
    if let Some(accent) =
        design::accent_for(theme.dictionary(), design::family::style::Widgets::Pairs)
    {
        v.selection.bg_fill = srgb(accent);
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
        for state in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
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
    crate::ui_scale::install(ctx, theme.ui());
    ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::unique("toolkit.editor-palette"),
            editor::Palette::from_theme(theme),
        )
    });
    ctx.options_mut(|options| {
        options.input_options.max_double_click_delay = theme.style().double_click_time;
    });
    ctx.set_global_style(style(theme));
    crate::chrome::install(ctx, &crate::chrome::Chrome::for_theme(theme));
    crate::icons::install(ctx, crate::icons::stroke_width(theme));
    crate::fonts::install(ctx, theme);
    ctx.request_repaint();
}

/// The bar geometry and state opacities of the installed style.
pub fn scroll_style(tokens: &design::ResolvedStyle) -> egui::style::ScrollStyle {
    let solid = tokens.scroll_bars == design::family::style::ScrollBars::Solid;
    egui::style::ScrollStyle {
        floating: !solid,
        bar_width: if solid {
            tokens.scroll_width
        } else {
            tokens.scroll_width_hot
        } as f32,
        floating_width: tokens.scroll_width as f32,
        floating_allocated_width: if solid {
            0.0
        } else {
            tokens.scroll_allocated as f32
        },
        handle_min_length: tokens.scroll_handle_min as f32,
        bar_inner_margin: tokens.scroll_margin as f32,
        bar_outer_margin: tokens.scroll_outer_margin as f32,
        foreground_color: true,
        dormant_background_opacity: tokens.scroll_track_alpha_rest as f32,
        active_background_opacity: tokens.scroll_track_alpha_active as f32,
        interact_background_opacity: tokens.scroll_track_alpha_hot as f32,
        dormant_handle_opacity: tokens.scroll_handle_alpha_rest as f32,
        active_handle_opacity: tokens.scroll_handle_alpha_active as f32,
        interact_handle_opacity: tokens.scroll_handle_alpha_hot as f32,
        ..egui::style::ScrollStyle::solid()
    }
}

/// Apply only explicitly overridden roles to their forward-owned egui slots.
/// Pair surfaces retain their own base writes when no role is overridden.
fn apply_role_overlay(visuals: &mut Visuals, theme: &Theme) {
    let tokens = theme.style();
    let palette = crate::chrome::Chrome::for_theme(theme).palette;
    for role in theme.overlay().chrome.keys() {
        let Some(value) = palette.get(role) else {
            continue;
        };
        match role.as_str() {
            "chrome" => visuals.panel_fill = value,
            "card" => {
                visuals.window_fill = value;
                visuals.faint_bg_color = value;
                visuals.widgets.noninteractive.bg_fill = value;
                visuals.widgets.noninteractive.weak_bg_fill = value;
            }
            "card_border" => visuals.window_stroke.color = value,
            "field" => {
                visuals.extreme_bg_color = value;
                visuals.code_bg_color = value;
                visuals.text_edit_bg_color = Some(value);
                visuals.widgets.inactive.bg_fill = value;
                visuals.widgets.inactive.weak_bg_fill = value;
            }
            "accent" => {
                visuals.hyperlink_color = value;
                if tokens.selection == design::family::style::Selection::Accent {
                    visuals.selection.bg_fill = value;
                }
            }
            "accent_soft" if tokens.selection == design::family::style::Selection::AccentSoft => {
                visuals.selection.bg_fill = value
            }
            "accent_text" => visuals.selection.stroke.color = value,
            "warning" => visuals.warn_fg_color = value,
            "danger" => visuals.error_fg_color = value,
            "text" => {
                visuals.override_text_color = Some(value);
                for state in [
                    &mut visuals.widgets.inactive,
                    &mut visuals.widgets.hovered,
                    &mut visuals.widgets.open,
                    &mut visuals.widgets.active,
                ] {
                    state.fg_stroke.color = value;
                }
            }
            "text_dim" => visuals.widgets.noninteractive.fg_stroke.color = value,
            "separator" => visuals.widgets.noninteractive.bg_stroke.color = value,
            "hover" => {
                for state in [&mut visuals.widgets.hovered, &mut visuals.widgets.open] {
                    state.bg_fill = value;
                    state.weak_bg_fill = value;
                }
            }
            "pressed" => {
                visuals.widgets.active.bg_fill = value;
                visuals.widgets.active.weak_bg_fill = value;
            }
            "field_border" => {
                for state in [
                    &mut visuals.widgets.inactive,
                    &mut visuals.widgets.hovered,
                    &mut visuals.widgets.open,
                ] {
                    state.bg_stroke.color = value;
                }
            }
            "accent_border" => visuals.widgets.active.bg_stroke.color = value,
            "shadow" => {
                visuals.window_shadow.color = value;
                visuals.popup_shadow.color = value;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use design::{DesignContext, Scheme};

    #[test]
    fn every_hue_context_maps_without_falling_back_on_the_core_pairs() {
        for scheme in Scheme::ALL.into_iter().filter(|s| !s.is_chrome_scheme()) {
            for mode in Mode::ALL {
                let theme = Theme::for_context(DesignContext {
                    scheme,
                    mode,
                    ..DesignContext::default()
                });
                let colours = &theme.dictionary().colours;
                let v = visuals(&theme);
                let (base, text) = pair(colours, "base").expect("base pair");
                assert_eq!(v.panel_fill, base, "{scheme:?}/{mode:?}");
                assert_eq!(v.override_text_color, Some(text));
                assert_eq!(v.dark_mode, mode == Mode::Dark);
                assert_ne!(
                    v.widgets.hovered.bg_fill, v.widgets.inactive.bg_fill,
                    "hover must be visible"
                );
            }
        }
    }

    #[test]
    fn sizes_come_from_the_design() {
        let theme = Theme::for_context(DesignContext::revision_one());
        let style = style(&theme);
        let ui = design::active_typography(Some(theme.typography()), TypographyRole::Ui);
        assert_eq!(
            style.text_styles[&TextStyle::Body].size,
            ui.font_size as f32
        );
        assert!(style.spacing.item_spacing.x > 0.0);
    }

    #[test]
    fn interface_size_scales_proportional_styles_once_and_preserves_monospace() {
        for scheme in [Scheme::Ocean, Scheme::Studio] {
            let base = Theme::for_context(DesignContext {
                scheme,
                ..Default::default()
            });
            let before = style(&base);
            let ui = design::active_typography(Some(base.typography()), TypographyRole::Ui);
            let sized = base
                .clone()
                .with_overlay(crate::theme::Overlay {
                    size: Some(18.0),
                    ..Default::default()
                })
                .unwrap();
            let after = style(&sized);
            for (text, font) in &before.text_styles {
                let ratio = if *text == TextStyle::Monospace {
                    1.0
                } else {
                    18.0 / ui.font_size as f32
                };
                assert!((after.text_styles[text].size - font.size * ratio).abs() < 0.001);
            }
            assert_eq!(
                style(&base.clone().with_overlay(Default::default()).unwrap()).text_styles,
                before.text_styles
            );
        }
    }

    #[test]
    fn mix_is_endpoint_exact() {
        let (a, b) = (Color32::from_gray(10), Color32::from_gray(250));
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
    }
}
