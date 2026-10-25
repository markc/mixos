// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Text section: the style of each prose surface, one card each (the
//! Email reader, the Email composer, the Ced editor). Each control applies as
//! it is made; the values are settingsd's, as its last read gave them.
use super::*;
use preferences::text::{FONT_SIZE_MAX, FONT_SIZE_MIN, PADDINGS, Setting, Surface};

/// The Text section: a card for each surface, in the order they are listed.
pub(super) fn section(
    ui: &mut Ui,
    engine: &Engine,
    surfaces: &[Surface],
    events: &mut Vec<UiEvent>,
) {
    ui.vertical(|ui| {
        if engine.look.settings == Settings::Missing {
            ui.add(egui::Label::new(RichText::new(label("look-missing")).weak()).wrap());
            return;
        }
        ui.add_enabled_ui(engine.can_edit_text(), |ui| {
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                for &surface in surfaces {
                    ui.push_id(surface.path(), |ui| {
                        surface_card(ui, engine, surface, events)
                    });
                }
            });
        });
    });
}

/// One surface's controls: its font, size, padding and light mode.
fn surface_card(ui: &mut Ui, engine: &Engine, surface: Surface, events: &mut Vec<UiEvent>) {
    group_heading(ui, surface.title(), surface == Surface::CedEditor);
    let Some(style) = engine.text_style(surface) else {
        ui.label(RichText::new(label("look-reading")).weak());
        return;
    };

    row(ui, "text-family-row", &label("text-family"), |ui| {
        let mut families: Vec<(usize, String)> = vec![(0, label("text-family-default"))];
        families.extend(
            surface
                .families()
                .iter()
                .enumerate()
                .map(|(i, family)| (i + 1, (*family).to_owned())),
        );
        let current = style.font.family.as_deref().map_or(0, |family| {
            surface
                .families()
                .iter()
                .position(|f| *f == family)
                .map_or(0, |i| i + 1)
        });
        if let Some(index) = choices(ui, current, &families) {
            let family = (index > 0).then(|| surface.families()[index - 1].to_owned());
            events.push(UiEvent::Text(Setting::Family(surface, family)));
        }
        if surface == Surface::CedEditor {
            ui.label(RichText::new(label("text-editor-mono")).weak());
        }
    });

    let size = Numeric {
        key: size_key(surface),
        title: label("text-size"),
        unit: "px",
        places: 1,
        slider: true,
    };
    if let Some(value) = number(ui, size, style.font.size, FONT_SIZE_MIN..=FONT_SIZE_MAX) {
        events.push(UiEvent::Text(Setting::Size(surface, value)));
    }

    row(ui, "text-padding-row", &label("text-padding"), |ui| {
        let paddings: Vec<(u32, String)> = PADDINGS.iter().map(|&p| (p, p.to_string())).collect();
        if let Some(padding) = choices(ui, style.padding, &paddings) {
            events.push(UiEvent::Text(Setting::Padding(surface, padding)));
        }
    });

    row(ui, "text-light-row", &label("text-light"), |ui| {
        let mut light = style.light;
        if toggle(ui, &mut light, "text-light") {
            events.push(UiEvent::Text(Setting::Light(surface, light)));
        }
    });
}

/// The drag memory key for a surface's size: each control needs its own,
/// since the memory is shared across the window.
fn size_key(surface: Surface) -> &'static str {
    match surface {
        Surface::EmailReader => "text-reader-size",
        Surface::EmailComposer => "text-composer-size",
        Surface::CedEditor => "text-editor-size",
    }
}
