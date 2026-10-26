// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Dock panel groups (chrome specification §3.10–3.11, §3.22) as plain
//! sections, section labels and the drag-drop insertion line.
//!
//! [`group`] draws a section straight on the pane's background, the same
//! in every style: no card, no border, no rounded corners, no pill fills
//! (Mark, 2026-10-23: a flat pane without border clutter). A 24 pt header
//! holds the tab titles as text, left-aligned (the selected or hovered one
//! in `text`, the rest `text_dim`), then the body 6 pt below. The section
//! keeps its view state (the selected tab, whether it is collapsed) in the
//! context under its id. [`group_with_actions`] puts the caller's actions
//! in the header instead, with the panel menu (three dots) at its right.
//!
//! Dragging the header's top band reports a drag for the dock to move the
//! group, and double-clicking it collapses the group to its header.
//! Overflow follows [`crate::tabs::fit`]. [`stack`] columns groups so those
//! drags reorder them: the insertion line shows where a group would land,
//! and the drop hands back the new order.

use crate::button::IconButton;
use crate::chrome::Chrome;
use crate::icons::Icon;
use crate::tabs;
use egui::emath::GuiRounding;
use egui::{
    Color32, FontId, Frame, Id, Margin, Pos2, Rect, Response, Role, Sense, Stroke, Ui, WidgetInfo,
    pos2, vec2,
};

/// A group's view state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GroupState {
    pub selected: usize,
    pub collapsed: bool,
}

/// What a group did this frame.
pub struct GroupResponse<R> {
    /// The body's value; `None` while collapsed.
    pub inner: Option<R>,
    pub state: GroupState,
    /// The panel menu (three dots) from [`group_with_actions`]; a plain
    /// [`group`] has none, and this is its header, which never clicks.
    pub menu: Response,
    /// The drag handle: the header's top band.
    pub handle: Response,
}

/// A dock group of `tabs` around `body`, which is given the selected tab.
pub fn group<R>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    tabs: &[&str],
    body: impl FnOnce(&mut Ui, usize) -> R,
) -> GroupResponse<R> {
    let id = ui.make_persistent_id(id_salt);
    let mut state: GroupState = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    state.selected = state.selected.min(tabs.len().saturating_sub(1));
    let chrome = Chrome::of(ui.ctx());
    let out = section(ui, id, &chrome, tabs, &mut state, &mut |_| {}, false, body);
    ui.data_mut(|d| d.insert_temp(id, state));
    GroupResponse { state, ..out }
}

/// A dock group with no tabs: `actions` draws in the header, on the left of
/// the panel menu (the three dots), and `body` is the group's content. The
/// header's actions replace the tab labels for panels that have one thing to
/// show.
pub fn group_with_actions<R>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    actions: impl FnOnce(&mut Ui),
    body: impl FnOnce(&mut Ui, usize) -> R,
) -> GroupResponse<R> {
    let id = ui.make_persistent_id(id_salt);
    let mut state: GroupState = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    state.selected = 0;
    let chrome = Chrome::of(ui.ctx());
    let mut actions = Some(actions);
    let mut run = |ui: &mut Ui| {
        if let Some(actions) = actions.take() {
            actions(ui);
        }
    };
    let out = section(ui, id, &chrome, &[], &mut state, &mut run, true, body);
    ui.data_mut(|d| d.insert_temp(id, state));
    GroupResponse { state, ..out }
}

/// A tab's click and double-click on `state`.
fn tab_input(response: &Response, index: usize, state: &mut GroupState) {
    if response.double_clicked() {
        state.collapsed = !state.collapsed;
    } else if response.clicked() {
        state.selected = index;
        state.collapsed = false;
    }
}

/// Lay out `tabs` (natural widths from `font` and `pad`) in `room`.
fn widths(
    ui: &Ui,
    tabs: &[&str],
    font: &FontId,
    pad: f32,
    minimum: f32,
    room: f32,
    selected: usize,
) -> tabs::Fit {
    let naturals: Vec<f32> = tabs
        .iter()
        .map(|t| {
            ui.painter()
                .layout_no_wrap((*t).to_owned(), font.clone(), Color32::PLACEHOLDER)
                .size()
                .x
                + pad
        })
        .collect();
    tabs::fit(&naturals, minimum, room, Some(selected))
}

/// A group as a plain section on the pane's own background: no card, no
/// border, no pill fills. The header holds the tab titles as text (the
/// selected one in `text`, the rest `text_dim`), or the caller's actions,
/// and the panel menu (three dots) only when `dots` asks for it.
#[expect(
    clippy::too_many_arguments,
    reason = "group's inputs, called from its two entry points"
)]
fn section<R>(
    ui: &mut Ui,
    id: Id,
    chrome: &Chrome,
    tabs: &[&str],
    state: &mut GroupState,
    actions: &mut dyn FnMut(&mut Ui),
    dots: bool,
    body: impl FnOnce(&mut Ui, usize) -> R,
) -> GroupResponse<R> {
    let p = chrome.palette;
    let margin = if state.collapsed {
        Margin {
            bottom: crate::chrome::Chrome::of(ui.ctx())
                .style
                .panel_card_margin_collapsed as i8,
            ..Margin {
                left: crate::chrome::Chrome::of(ui.ctx())
                    .style
                    .panel_card_margin_left as i8,
                right: crate::chrome::Chrome::of(ui.ctx())
                    .style
                    .panel_card_margin_right as i8,
                top: crate::chrome::Chrome::of(ui.ctx())
                    .style
                    .panel_card_margin_top as i8,
                bottom: crate::chrome::Chrome::of(ui.ctx())
                    .style
                    .panel_card_margin_bottom as i8,
            }
        }
    } else {
        Margin {
            left: crate::chrome::Chrome::of(ui.ctx())
                .style
                .panel_card_margin_left as i8,
            right: crate::chrome::Chrome::of(ui.ctx())
                .style
                .panel_card_margin_right as i8,
            top: crate::chrome::Chrome::of(ui.ctx())
                .style
                .panel_card_margin_top as i8,
            bottom: crate::chrome::Chrome::of(ui.ctx())
                .style
                .panel_card_margin_bottom as i8,
        }
    };
    let frame = Frame::new().inner_margin(margin);
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let width = ui.available_width();
        let (header, header_response) = ui.allocate_exact_size(
            vec2(
                width,
                crate::chrome::Chrome::of(ui.ctx()).style.panel_header as f32,
            ),
            Sense::hover(),
        );
        let card_top = header.top() - f32::from(margin.top);
        let band = Rect::from_x_y_ranges(
            header.x_range(),
            card_top..=card_top + crate::chrome::Chrome::of(ui.ctx()).style.panel_drag_band as f32,
        );
        let handle = ui.interact(band, id.with("handle"), Sense::click_and_drag());
        if handle.double_clicked() {
            state.collapsed = !state.collapsed;
        }
        let font = crate::fonts::bound(
            ui.ctx(),
            crate::fonts::medium(crate::chrome::Chrome::of(ui.ctx()).style.panel_pill_size as f32),
        );
        let room = if dots {
            width
                - crate::chrome::Chrome::of(ui.ctx()).style.panel_ellipsis as f32
                - crate::chrome::Chrome::of(ui.ctx()).style.panel_ellipsis_gap as f32
        } else if tabs.len() > 1 {
            // Room for the overflow chevron after the last tab that fits.
            width - tabs::chevron_of(ui.ctx())
        } else {
            width
        };
        let fit = widths(
            ui,
            tabs,
            &font,
            crate::chrome::Chrome::of(ui.ctx()).style.panel_pill_pad as f32
                + crate::chrome::Chrome::of(ui.ctx()).style.panel_pill_gap as f32,
            crate::chrome::Chrome::of(ui.ctx()).style.panel_pill_min as f32
                + crate::chrome::Chrome::of(ui.ctx()).style.panel_pill_gap as f32,
            room,
            state.selected,
        );
        let mut x = header.left();
        let mut hidden = Vec::new();
        for (index, (name, width)) in tabs.iter().zip(&fit.widths).enumerate() {
            let Some(width) = *width else {
                hidden.push((index, (*name).to_owned()));
                continue;
            };
            let rect = Rect::from_min_size(
                pos2(x, header.top()),
                vec2(
                    width - crate::chrome::Chrome::of(ui.ctx()).style.panel_pill_gap as f32,
                    crate::chrome::Chrome::of(ui.ctx()).style.panel_header as f32,
                ),
            );
            x += width;
            let response = ui.interact(rect, id.with(("pill", index)), Sense::click());
            let selected = index == state.selected;
            response.widget_info(|| {
                WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, name)
            });
            let (galley, cut) = tabs::label(
                ui,
                name,
                font.clone(),
                rect.width(),
                crate::chrome::Chrome::of(ui.ctx()).style.panel_pill_pad as f32,
            );
            let ink = if selected || response.hovered() {
                p.text
            } else {
                p.text_dim
            };
            // Left-aligned in its slot, so a lone title reads as a heading.
            let at = pos2(rect.left(), rect.center().y - galley.size().y / 2.0)
                .round_to_pixels(ui.pixels_per_point());
            ui.painter().galley(at, galley, ink);
            let response = if cut {
                response.on_hover_text(*name)
            } else {
                response
            };
            tab_input(&response, index, state);
        }
        if fit.overflows() {
            let rect = Rect::from_min_size(
                pos2(x, header.top()),
                vec2(
                    tabs::chevron_of(ui.ctx()),
                    crate::chrome::Chrome::of(ui.ctx()).style.panel_header as f32,
                ),
            );
            if let Some(index) = tabs::chevron(ui, id.with("overflow"), rect, &hidden) {
                state.selected = index;
            }
        }
        if tabs.is_empty() {
            let rect = Rect::from_min_size(
                header.left_top(),
                vec2(
                    room,
                    crate::chrome::Chrome::of(ui.ctx()).style.panel_header as f32,
                ),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| actions(ui));
        }
        let ellipsis = Rect::from_min_size(
            pos2(
                header.right() - crate::chrome::Chrome::of(ui.ctx()).style.panel_ellipsis as f32,
                header.center().y
                    - crate::chrome::Chrome::of(ui.ctx()).style.panel_ellipsis as f32 / 2.0,
            ),
            vec2(
                crate::chrome::Chrome::of(ui.ctx()).style.panel_ellipsis as f32,
                crate::chrome::Chrome::of(ui.ctx()).style.panel_ellipsis as f32,
            ),
        );
        let menu = if dots {
            ui.put(
                ellipsis,
                IconButton::new(Icon::Ellipsis)
                    .size(crate::chrome::Chrome::of(ui.ctx()).style.panel_ellipsis as f32)
                    .tooltip(crate::strings::own("panel-menu")),
            )
        } else {
            header_response
        };
        let inner = (!state.collapsed).then(|| {
            ui.add_space(crate::chrome::Chrome::of(ui.ctx()).style.panel_header_after as f32);
            body(ui, state.selected)
        });
        (inner, menu, handle)
    });
    ui.add_space(crate::chrome::Chrome::of(ui.ctx()).style.panel_card_after as f32);
    let (inner, menu, handle) = shown.inner;
    GroupResponse {
        inner,
        state: *state,
        menu,
        handle,
    }
}

/// `order` with the entry at `from` moved to sit before the entry that was
/// at `before` (`order.len()` for the end).
pub fn moved(order: &[usize], from: usize, before: usize) -> Vec<usize> {
    let mut out = order.to_vec();
    if from >= out.len() {
        return out;
    }
    let item = out.remove(from);
    let at = if before > from { before - 1 } else { before };
    out.insert(at.min(out.len()), item);
    out
}

/// Where a drag at height `y` would insert, given each group's vertical
/// extent in stacking order: before the first group whose middle is below
/// the pointer, else at the end.
pub fn insertion(extents: &[egui::Rangef], y: f32) -> usize {
    extents
        .iter()
        .position(|r| y < r.center())
        .unwrap_or(extents.len())
}

/// A column of panel groups the person can reorder: `order` lists the
/// app's groups (its own indices) top to bottom, and `show` draws one with
/// [`group`], returning its [`GroupResponse::handle`]. Dragging a handle
/// shows the insertion line (§3.22) between groups; the drop returns the
/// new order, which the app stores.
///
/// Each group draws in a Ui whose id comes from the stack and the group,
/// never its position, so its widgets keep their ids (and their scroll
/// positions, focus and AccessKit nodes) wherever it moves.
pub fn stack(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    order: &[usize],
    mut show: impl FnMut(&mut Ui, usize) -> Response,
) -> Option<Vec<usize>> {
    let id = ui.make_persistent_id(id_salt);
    let mut extents = Vec::with_capacity(order.len());
    let mut dragging: Option<(usize, Response)> = None;
    for (position, &group) in order.iter().enumerate() {
        let shown = ui.scope_builder(egui::UiBuilder::new().scope_id(id.with(group)), |ui| {
            show(ui, group)
        });
        extents.push(shown.response.rect.y_range());
        let handle = shown.inner;
        if handle.dragged() || handle.drag_stopped() {
            dragging = Some((position, handle));
        }
    }
    let (from, handle) = dragging?;
    // The pointer can be gone by the drop's frame: keep its last height.
    let key = handle.id.with("toolkit.stack.y");
    if let Some(pos) = handle.interact_pointer_pos() {
        ui.data_mut(|d| d.insert_temp(key, pos.y));
    }
    let y: f32 = ui.data(|d| d.get_temp(key))?;
    let before = insertion(&extents, y);
    let gap = ui.spacing().item_spacing.y;
    let line_y = match extents.get(before) {
        Some(r) => r.min - gap / 2.0,
        None => extents.last().map_or(0.0, |r| r.max + gap / 2.0),
    };
    let x = ui.min_rect().x_range();
    if handle.dragged() && before != from && before != from + 1 {
        drop_line(ui, pos2(x.min, line_y), pos2(x.max, line_y));
    }
    (handle.drag_stopped() && before != from && before != from + 1)
        .then(|| moved(order, from, before))
}

/// A section label: Inter Medium 11.5 in `text_faint` (§3.22).
pub fn section_label(ui: &mut Ui, text: &str) -> Response {
    let font = crate::fonts::bound(
        ui.ctx(),
        crate::fonts::medium(crate::chrome::Chrome::of(ui.ctx()).style.panel_section_size as f32),
    );
    let ink = Chrome::of(ui.ctx()).palette.text_faint;
    ui.label(egui::RichText::new(text).font(font).color(ink))
}

/// The drag-drop insertion line from `a` to `b` (§3.22): 2 pt `accent`,
/// inset 2 pt at the ends of a vertical line and 4 pt of a horizontal one.
pub fn drop_line(ui: &Ui, a: Pos2, b: Pos2) {
    let vertical = (b.x - a.x).abs() < (b.y - a.y).abs();
    let inset = if vertical {
        crate::chrome::Chrome::of(ui.ctx())
            .style
            .panel_drop_inset_vertical as f32
    } else {
        crate::chrome::Chrome::of(ui.ctx())
            .style
            .panel_drop_inset_horizontal as f32
    };
    let along = (b - a).normalized() * inset;
    let accent = Chrome::of(ui.ctx()).palette.accent;
    ui.painter().line_segment(
        [a + along, b - along],
        Stroke::new(
            crate::chrome::Chrome::of(ui.ctx()).style.panel_drop_line as f32,
            accent,
        ),
    );
}

/// The installed value of the former `STRIP` constant.
pub fn strip_of(ctx: &egui::Context) -> f32 {
    crate::chrome::Chrome::of(ctx).style.panel_strip as f32
}

/// The installed value of the former `HEADER` constant.
pub fn header_of(ctx: &egui::Context) -> f32 {
    crate::chrome::Chrome::of(ctx).style.panel_header as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Rangef;

    #[test]
    fn a_move_lands_before_the_named_group() {
        assert_eq!(moved(&[0, 1, 2], 2, 0), [2, 0, 1]);
        assert_eq!(moved(&[0, 1, 2], 0, 3), [1, 2, 0]);
        assert_eq!(moved(&[0, 1, 2], 0, 2), [1, 0, 2]);
        assert_eq!(moved(&[0, 1, 2], 1, 1), [0, 1, 2], "onto itself");
    }

    #[test]
    fn the_insertion_point_follows_the_group_middles() {
        let extents = [Rangef::new(0.0, 100.0), Rangef::new(110.0, 150.0)];
        assert_eq!(insertion(&extents, 20.0), 0);
        assert_eq!(insertion(&extents, 60.0), 1);
        assert_eq!(insertion(&extents, 140.0), 2);
    }
}
