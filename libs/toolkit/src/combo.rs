// SPDX-License-Identifier: MIT OR Apache-2.0
//! Combo boxes (chrome specification §3.17): egui's combo, whose button and
//! list the chrome style already draws to the specification, with three
//! changes.
//!
//! - The arrow is a small chevron: two strokes forming a "v", 6.4 pt wide
//!   and 3.2 pt tall, 1.3 pt thick, in the text colour at 80%.
//! - A click gives the button keyboard focus.
//! - While the list is open, Up and Down step the selection at once,
//!   clamped at the ends, without closing the list.
//!
//! The list is the popup frame hanging from the button, at most 420 pt
//! tall; its items are selectable labels one control high with the item
//! spacing between them (a 30 pt pitch), the selected one in the selection
//! fill. Under the chrome style egui's popup carries every §3.17 value
//! itself: the frame is `card` with a 1 pt `card_border`, `radius`, a 6 pt
//! margin and the popup shadow; rows are the 24 pt interact height with
//! 6 pt item spacing; the selected row takes the selection fill and the
//! resting widget stroke (`field_border`, as measured in Pro); text starts
//! 1 + 6 + 2 = 9 pt in (border, margin, egui's menu-style padding; the
//! specification measures "about 10").

use egui::{
    AsIdSalt, ComboBox, EventFilter, Id, IdSalt, Key, Rect, Response, Stroke, Ui, WidgetText, pos2,
    style::WidgetVisuals,
};

/// The longest the list grows before it scrolls (§3.17).
pub const LIST_HEIGHT: f32 = 420.0;

/// The chevron's half width and height, stroke and ink opacity (§3.17).
const CHEVRON_HALF: f32 = 3.2;
const CHEVRON_HEIGHT: f32 = 3.2;
const CHEVRON_STROKE: f32 = 1.3;
const CHEVRON_ALPHA: f32 = 0.8;

/// The chevron centred in `rect`, in `visuals`' text colour at 80%.
pub fn chevron(ui: &Ui, rect: Rect, visuals: &WidgetVisuals, _open: bool) {
    let c = rect.center();
    let ink = visuals.text_color().gamma_multiply(CHEVRON_ALPHA);
    let points = vec![
        pos2(c.x - CHEVRON_HALF, c.y - CHEVRON_HEIGHT / 2.0),
        pos2(c.x, c.y + CHEVRON_HEIGHT / 2.0),
        pos2(c.x + CHEVRON_HALF, c.y - CHEVRON_HEIGHT / 2.0),
    ];
    ui.painter().add(egui::Shape::line(points, Stroke::new(CHEVRON_STROKE, ink)));
}

/// `selected` in `options`, as a combo `width` wide (egui's `combo_width`,
/// 120, when `None`). Returns the button's response, changed when the
/// selection changed.
pub fn show<T: AsRef<str>>(
    ui: &mut Ui,
    id_salt: impl AsIdSalt,
    selected: &mut usize,
    options: &[T],
    width: Option<f32>,
) -> Response {
    // The button's id, as egui's combo derives it.
    let button = ui.make_persistent_id(IdSalt::new(&id_salt));
    let mut changed = false;
    if ComboBox::is_open(ui.ctx(), button) && !options.is_empty() {
        let last = options.len() - 1;
        ui.input_mut(|i| {
            i.events.retain(|event| match event {
                egui::Event::Key { key: Key::ArrowDown, pressed: true, modifiers, .. } if modifiers.is_none() => {
                    changed |= *selected < last;
                    *selected = (*selected + 1).min(last);
                    false
                }
                egui::Event::Key { key: Key::ArrowUp, pressed: true, modifiers, .. } if modifiers.is_none() => {
                    changed |= *selected > 0;
                    *selected = selected.saturating_sub(1);
                    false
                }
                _ => true,
            });
        });
    }
    let text = options.get(*selected).map_or("", AsRef::as_ref);
    let mut combo = ComboBox::from_id_salt(id_salt).selected_text(WidgetText::from(text)).height(LIST_HEIGHT).icon(chevron);
    if let Some(width) = width {
        combo = combo.width(width);
    }
    let mut response = combo
        .show_ui(ui, |ui| {
            for (index, option) in options.iter().enumerate() {
                if ui.selectable_label(index == *selected, option.as_ref()).clicked() {
                    changed |= index != *selected;
                    *selected = index;
                }
            }
        })
        .response;
    if response.clicked() {
        response.request_focus();
    }
    if response.has_focus() {
        // Up and Down are the list's, not focus navigation's.
        lock_arrows(ui, response.id);
    }
    if changed {
        response.mark_changed();
    }
    response
}

/// Keep Up and Down from moving keyboard focus off `id`.
fn lock_arrows(ui: &Ui, id: Id) {
    ui.memory_mut(|m| m.set_focus_lock_filter(id, EventFilter { vertical_arrows: true, ..EventFilter::default() }));
}
