// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shared look of the capture overlays.
//!
//! Panels, buttons and text are the MixOS theme's (`toolkit::style`, installed
//! on every compositor UI), so the overlays match the apps in every scheme:
//! the accent is the theme's selection colour, a destructive action its error
//! colour. Only the masks carry their own colour: they are translucency over
//! the screen, not themed surfaces.

use egui::{Align2, Color32, Id, Response, RichText, Stroke, Ui};

/// The setup overlay's mask outside the hole: dim, not opaque, so the user can
/// still aim the region at what is underneath.
pub const SETUP_MASK: Color32 = Color32::from_black_alpha(128);
/// The backdrop drawn below windows while capturing, outside the region.
pub const DIM_MASK: Color32 = Color32::from_black_alpha(115);

pub const BORDER_WIDTH: f32 = 2.0;

/// The theme's accent: the region outline, selected choices, the default
/// action.
pub fn accent(ui: &Ui) -> Color32 {
    ui.visuals().selection.bg_fill
}

/// The theme's colour for a destructive action.
pub fn danger(ui: &Ui) -> Color32 {
    ui.visuals().error_fg_color
}

/// Dimmed secondary text.
pub fn weak(ui: &Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

/// A dialog-sized button. `fill` picks a filled style (accent or danger);
/// `None` is the theme's plain button.
pub fn button(ui: &mut Ui, label: String, fill: Option<Color32>) -> Response {
    let mut text = RichText::new(label).size(15.0);
    let mut button = match fill {
        Some(fill) => {
            text = text.color(contrast(fill));
            egui::Button::new(text).fill(fill)
        }
        None => egui::Button::new(text),
    };
    button = button.min_size(egui::vec2(0.0, 32.0));
    ui.add(button)
}

/// A choice in a row of choices: filled with the accent when selected.
pub fn choice(ui: &mut Ui, label: String, selected: bool) -> Response {
    let fill = selected.then(|| accent(ui));
    button(ui, label, fill)
}

/// Black or white, whichever reads on `fill`.
fn contrast(fill: Color32) -> Color32 {
    let [r, g, b, _] = fill.to_array();
    let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if luma > 150.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// A dialog panel centred on the surface, outlined in `border`.
pub fn centred_dialog(ui: &mut Ui, id: &str, border: Color32, body: impl FnOnce(&mut Ui)) {
    egui::Area::new(Id::new(id))
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(24.0)
                .stroke(Stroke::new(1.0, border))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 14.0;
                        body(ui);
                    });
                });
        });
}
