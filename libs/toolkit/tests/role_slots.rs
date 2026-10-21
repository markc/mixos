// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Compare the native renderer with the project's frozen renderer writers.

pub use toolkit::{bars, fonts, icons, theme};

#[allow(
    dead_code,
    reason = "frozen renderer writers used as the independent baseline"
)]
#[path = "fixtures/chrome_before.rs"]
mod before_chrome;
#[allow(
    dead_code,
    reason = "frozen renderer writers used as the independent baseline"
)]
#[path = "fixtures/style_before.rs"]
mod before_style;

fn write_slot(v: &mut egui::Visuals, name: &str, colour: egui::Color32) {
    match name {
        "panel_fill" => v.panel_fill = colour,
        "window_fill" => v.window_fill = colour,
        "faint_bg_color" => v.faint_bg_color = colour,
        "window_stroke.color" => v.window_stroke.color = colour,
        "extreme_bg_color" => v.extreme_bg_color = colour,
        "code_bg_color" => v.code_bg_color = colour,
        "text_edit_bg_color" => v.text_edit_bg_color = Some(colour),
        "hyperlink_color" => v.hyperlink_color = colour,
        "warn_fg_color" => v.warn_fg_color = colour,
        "error_fg_color" => v.error_fg_color = colour,
        "override_text_color" => v.override_text_color = Some(colour),
        "selection.bg_fill" => v.selection.bg_fill = colour,
        "selection.stroke.color" => v.selection.stroke.color = colour,
        "window_shadow.color" => v.window_shadow.color = colour,
        "popup_shadow.color" => v.popup_shadow.color = colour,
        name => {
            let parts: Vec<_> = name.split('.').collect();
            assert_eq!(parts[0], "widgets");
            let widget = match parts[1] {
                "noninteractive" => &mut v.widgets.noninteractive,
                "inactive" => &mut v.widgets.inactive,
                "hovered" => &mut v.widgets.hovered,
                "active" => &mut v.widgets.active,
                "open" => &mut v.widgets.open,
                _ => panic!("unknown widget slot {name}"),
            };
            match parts[2] {
                "bg_fill" => widget.bg_fill = colour,
                "weak_bg_fill" => widget.weak_bg_fill = colour,
                "bg_stroke" => widget.bg_stroke.color = colour,
                "fg_stroke" => widget.fg_stroke.color = colour,
                _ => panic!("unknown widget value {name}"),
            }
        }
    }
}

#[test]
fn every_slot_keeps_its_old_writer_and_each_role_forwards_only_to_its_slots() {
    for scheme in design::Scheme::ALL {
        for mode in design::Mode::ALL {
            for style in design::Style::ALL {
                let theme = toolkit::Theme::for_context(design::DesignContext {
                    scheme,
                    mode,
                    style: Some(style),
                    ..Default::default()
                });
                let expected = before_style::style(&theme).visuals;
                let ctx = egui::Context::default();
                toolkit::install(&ctx, &theme);
                assert_eq!(
                    ctx.global_style().visuals,
                    expected,
                    "{scheme:?}/{mode:?}/{style:?}"
                );
                for role in toolkit::chrome::Palette::ROLES {
                    let mut overlay = toolkit::theme::Overlay::default();
                    overlay.chrome.insert(
                        (*role).into(),
                        design::family::style::StyleColour([41, 117, 203, 255]),
                    );
                    let tuned = theme.clone().with_overlay(overlay);
                    toolkit::install(&ctx, &tuned);
                    let mut want = expected.clone();
                    for slot in toolkit::chrome::Palette::slots(role, &theme.style()) {
                        write_slot(&mut want, slot, egui::Color32::from_rgb(41, 117, 203));
                    }
                    assert_eq!(
                        ctx.global_style().visuals,
                        want,
                        "{scheme:?}/{mode:?}/{style:?}/{role}"
                    );
                    assert_eq!(
                        toolkit::chrome::Chrome::of(&ctx).palette.get(role),
                        Some(egui::Color32::from_rgb(41, 117, 203))
                    );
                }
            }
        }
    }
}
