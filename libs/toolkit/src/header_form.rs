// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared header rows and removable attachment chips. Callers own all state,
//! labels, identities, completion lists and validation.
use egui::Ui;
use crate::field;
pub const GAP: f32 = 8.0;

pub fn column(ui: &Ui, labels: impl IntoIterator<Item = String>) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    labels.into_iter().map(|text| ui.painter().layout_no_wrap(text, font.clone(), egui::Color32::PLACEHOLDER).size().x).fold(0.0_f32, f32::max).ceil()
}

pub fn text(ui: &mut Ui, id: egui::Id, label: egui::Id, buffer: &mut String, width: f32) -> egui::Response {
    ui.add(field::text_edit(buffer, "", width).id(id)).labelled_by(label)
}

/// One form row as Thunderbird lays it out: the label outside the field,
/// right-aligned in a column `column` wide, then `add` with the rest of the
/// row and the label's id (for the field's accessible name).
pub fn row<R>(ui: &mut Ui, text: &str, column: f32, add: impl FnOnce(&mut Ui, egui::Id) -> R) -> R {
    row_of(ui, text, column, field::height_of(ui.ctx()), add)
}

/// [`row`] for a row `height` tall (the From row holds a push button,
/// taller than a field): the label centres on the row's middle.
pub fn row_of<R>(
    ui: &mut Ui,
    text: &str,
    column: f32,
    height: f32,
    add: impl FnOnce(&mut Ui, egui::Id) -> R,
) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let label = ui
            .allocate_ui_with_layout(
                egui::vec2(column, height),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.set_min_width(column);
                    ui.label(text)
                },
            )
            .inner;
        add(ui, label.id)
    })
    .inner
}

pub fn attachment_chip(ui: &mut Ui, key: impl std::hash::Hash + std::fmt::Debug, path: &std::path::Path, text: &str, bad: bool, icon: f32, remove_label: &str) -> bool {
    let text = text.to_owned();
    // Decide wrapping before Frame::group consumes the row remainder. A
    // long name gets the full row's label budget, rather than shrinking
    // into the space left after the preceding chip.
    let frame = egui::Frame::group(ui.style());
    let margin = frame.total_margin().sum().x;
    let spacing = ui.spacing().item_spacing.x;
    let font = egui::TextStyle::Body.resolve(ui.style());
    let minimum_label_width = font.size * 4.0;
    let natural = ui
        .painter()
        .layout_no_wrap(text.clone(), font, ui.visuals().text_color())
        .size();
    let full_width = ui.max_rect().width();
    let width = natural
        .x
        .min((full_width - icon - spacing - margin).max(0.0))
        .max(minimum_label_width);
    let outer_width = width + icon + spacing + margin;
    // available_width() is the full row width in a wrapping layout. Scopes
    // advance their already drawn rect without wrapping it, so wrap before
    // push_id/Frame using the space left on this row instead.
    let remaining_width = ui.available_size_before_wrap().x;
    if outer_width > remaining_width && remaining_width < full_width {
        ui.end_row();
    }
    let height = icon.max(natural.y);
    let mut removed = false;
    ui.push_id(key, |ui| {
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut text = egui::RichText::new(text);
                if bad {
                    text = text.color(ui.visuals().error_fg_color);
                }
                ui.add_sized([width, height], egui::Label::new(text).truncate())
                    .on_hover_text(path.display().to_string());
                let remove = crate::button::IconButton::new(crate::Icon::X)
                    .size(icon)
                    .tooltip(remove_label);
                if ui.add(remove).clicked() {
                    removed = true;
                }
            });
        });
    });
    removed
}

