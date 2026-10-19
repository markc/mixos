// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A prose surface (an email body, a composer, an editor) drawn with its own
//! [`settings::content::Content`]: its padding, font family and size, and
//! whether it shows light whatever the app's mode is. The app's other regions
//! keep the global look.
use crate::chrome::{self, Chrome};
use crate::style;
use crate::theme::Theme;
use egui::{FontFamily, FontId, Frame, Margin, TextStyle, Ui};
use settings::content::Content;
use std::sync::OnceLock;

/// Draw `add` inside the surface described by `content`.
pub fn surface(ui: &mut Ui, content: &Content, add: impl FnOnce(&mut Ui)) {
    if content.light {
        light(ui, content, add);
    } else {
        padded(ui, content, add);
    }
}

/// The light theme a light surface draws with, for code that colours its own
/// text (the editor's palette).
pub fn light_theme() -> &'static Theme {
    static LIGHT: OnceLock<Theme> = OnceLock::new();
    LIGHT.get_or_init(|| {
        Theme::for_context(design::DesignContext {
            mode: design::Mode::Light,
            ..design::DesignContext::default()
        })
    })
}

fn light(ui: &mut Ui, content: &Content, add: impl FnOnce(&mut Ui)) {
    let theme = light_theme();
    let ctx = ui.ctx().clone();
    let previous = Chrome::of(&ctx);
    chrome::install(&ctx, &Chrome::for_theme(theme));
    let light_style = style::style(theme);
    let fill = light_style.visuals.panel_fill;
    Frame::new().fill(fill).show(ui, |ui| {
        ui.set_style(light_style);
        padded(ui, content, add);
    });
    chrome::install(&ctx, &previous);
}

/// The padding and the font, inside the surface's own style scope, so the
/// override reaches no other region.
fn padded(ui: &mut Ui, content: &Content, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .inner_margin(Margin::same(content.padding as i8))
        .show(ui, |ui| {
            ui.scope(|ui| {
                let family = match content.font.family.as_deref() {
                    Some("JetBrains Mono") => FontFamily::Monospace,
                    _ => FontFamily::Proportional,
                };
                ui.style_mut().text_styles.insert(
                    TextStyle::Body,
                    FontId::new(content.font.size as f32, family),
                );
                add(ui);
            });
        });
}
