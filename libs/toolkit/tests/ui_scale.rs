// SPDX-License-Identifier: MIT OR Apache-2.0
use egui::{Context, TextStyle};
use settings::model::CommonUi;
use toolkit::{Theme, chrome::Chrome};

fn theme(text_scale: f64, density: f64) -> Theme {
    Theme::embedded().with_ui(CommonUi {
        text_scale,
        density,
        ..CommonUi::default()
    })
}

fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
}

#[test]
fn larger_text_scales_roles_buttons_and_content_without_spacing() {
    for scheme in [design::Scheme::Studio, design::Scheme::Ocean] {
        let base = Theme::for_context(design::DesignContext {
            scheme,
            ..Default::default()
        });
        let larger = base.clone().with_ui(CommonUi {
            text_scale: 1.5,
            ..Default::default()
        });
        let normal = toolkit::style::style(&base);
        let scaled = toolkit::style::style(&larger);
        for role in normal.text_styles.keys() {
            close(
                scaled.text_styles[role].size,
                normal.text_styles[role].size * 1.5,
            );
        }
        assert_eq!(scaled.spacing, normal.spacing);
        close(
            Chrome::for_theme(&larger).style.push_label_size as f32,
            Chrome::for_theme(&base).style.push_label_size as f32 * 1.5,
        );
    }
    let ctx = Context::default();
    toolkit::install(&ctx, &theme(1.5, 1.0));
    let mut content = settings::content::Content::default();
    content.font.size = 18.0;
    for light in [false, true] {
        content.light = light;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            toolkit::content::surface(ui, &content, |ui| {
                close(TextStyle::Body.resolve(ui.style()).size, 27.0);
            });
        });
        output.textures_delta.clear();
    }
    assert_eq!(content.font.size, 18.0);
}

#[test]
fn density_scales_spacing_rows_and_buttons_without_text_or_icons() {
    let base = theme(1.0, 1.0);
    let normal = toolkit::style::style(&base);
    let chrome = Chrome::for_theme(&base);
    for density in toolkit::ui_scale::DENSITY_STEPS {
        let scaled_theme = theme(1.0, density);
        let scaled = toolkit::style::style(&scaled_theme);
        let scaled_chrome = Chrome::for_theme(&scaled_theme);
        close(
            scaled.spacing.item_spacing.x,
            normal.spacing.item_spacing.x * density as f32,
        );
        close(
            scaled.spacing.button_padding.y,
            normal.spacing.button_padding.y * density as f32,
        );
        close(
            scaled_chrome.metrics.menu_row_height,
            chrome.metrics.menu_row_height * density as f32,
        );
        close(
            scaled_chrome.metrics.push_height,
            chrome.metrics.push_height * density as f32,
        );
        assert_eq!(scaled.text_styles, normal.text_styles);
        assert_eq!(scaled.spacing.icon_width, normal.spacing.icon_width);
        let ctx = Context::default();
        toolkit::install(&ctx, &scaled_theme);
        close(
            toolkit::space::of(&ctx).md,
            chrome.style.space_md as f32 * density as f32,
        );
    }
    assert_eq!(toolkit::ui_scale::nearest_density(0.9), 0);
    assert_eq!(toolkit::ui_scale::nearest_density(1.08), 1);
    assert_eq!(toolkit::ui_scale::nearest_density(1.7), 2);
}

#[test]
fn live_install_replaces_rather_than_compounds_and_clamps() {
    let ctx = Context::default();
    toolkit::install(&ctx, &theme(1.0, 1.0));
    let original = ctx.global_style();
    toolkit::install(&ctx, &theme(1.5, 0.85));
    close(
        ctx.global_style().text_styles[&TextStyle::Body].size,
        original.text_styles[&TextStyle::Body].size * 1.5,
    );
    toolkit::install(&ctx, &theme(1.5, 0.85));
    assert_eq!(toolkit::ui_scale::of(&ctx).text_scale, 1.5);
    toolkit::install(&ctx, &theme(1.0, 1.0));
    assert_eq!(ctx.global_style().text_styles, original.text_styles);
    assert_eq!(ctx.global_style().spacing, original.spacing);
    assert_eq!(theme(100.0, -2.0).ui(), theme(3.0, 0.5).ui());
    assert_eq!(theme(-1.0, 7.0).ui(), theme(0.5, 2.0).ui());
    assert_eq!(theme(f64::NAN, f64::NAN).ui(), &CommonUi::default());
}

#[test]
fn theme_choices_and_style_overlays_preserve_desktop_scaling_once() {
    let theme = theme(1.5, 1.2);
    let chosen = theme
        .with_choice(&toolkit::theme::Choice {
            scheme: Some(design::Scheme::Forest),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(chosen.ui(), theme.ui());
    let before = toolkit::style::style(&chosen);
    let overlaid = chosen.with_overlay(Default::default()).unwrap();
    let after = toolkit::style::style(&overlaid);
    assert_eq!(after.text_styles, before.text_styles);
    assert_eq!(after.spacing, before.spacing);
}
