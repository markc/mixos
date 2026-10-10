// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dock panel groups (chrome specification §3.10–3.11, §3.22): the Pro tab
//! strip and body, the Studio card with pill tabs (Classic follows Studio,
//! square and bevelled), section labels and the drag-drop insertion line.
//!
//! [`group`] draws whichever the style asks for around the caller's body
//! and keeps the group's view state (the selected tab, whether it is
//! collapsed) in the context under its id.
//!
//! **Pro.** A 26 pt `tab_strip` strip (top corners 3, all four when
//! collapsed) of edge-to-edge tabs in Inter 11.5, label plus 22 wide (at
//! least 40): the active tab `card` with `text`, a hovered one `hover` at
//! 40% with `text_dim`, the rest bare in `text_faint`. A three-line panel
//! menu button sits 14 pt from the strip's right. The body is `card` with
//! bottom corners 3 and an 8 pt margin; 2 pt follow the group.
//!
//! **Studio.** A `card` with a 1 pt `card_border` at `radius`, margins 8
//! at the sides, 6 at the top and 10 at the bottom (6 collapsed), and 6 pt
//! after it. Its 24 pt header holds the pills (Inter Medium 12.5, label
//! plus 20, at least 44, 2 pt apart; the selected pill `hover` with a 1 pt
//! `field_border` inside, a hovered one `hover` at 60%, all in `text_dim`
//! but the selected one) and, 4 pt after them, a 22 pt ellipsis button.
//! 6 pt separate the header from the body.
//!
//! In both, dragging the strip (or the card's top 22 pt) reports a drag for
//! the dock to move the group, and double-clicking a tab or the strip
//! collapses the group to its header. Overflow follows [`crate::tabs::fit`].
//! [`stack`] columns groups so those drags reorder them: the insertion line
//! shows where a group would land, and the drop hands back the new order.

use crate::button::IconButton;
use crate::chrome::{self, Chrome};
use design::family::style::PanelGroups;
use crate::icons::Icon;
use crate::tabs;
use egui::emath::GuiRounding;
use egui::{
    Color32, CornerRadius, FontFamily, FontId, Frame, Id, Margin, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};

/// Pro strip height, tab text, padding and minimum (§3.10).
pub const STRIP: f32 = 26.0;
const PRO_SIZE: f32 = 11.5;
const PRO_PAD: f32 = 22.0;
const PRO_MIN: f32 = 40.0;
const PRO_RADIUS: u8 = 3;
const PRO_HOVER_ALPHA: f32 = 0.4;
const PRO_MARGIN: i8 = 8;
const PRO_AFTER: f32 = 2.0;

/// The Pro panel menu ("hamburger"): hit box, centre from the strip's
/// right, line length and pitch (§3.10).
const MENU_BOX: egui::Vec2 = vec2(20.0, 18.0);
const MENU_FROM_RIGHT: f32 = 14.0;
const MENU_LINE: f32 = 10.0;
const MENU_PITCH: f32 = 3.5;

/// Studio card (§3.11).
const CARD_MARGIN: Margin = Margin { left: 8, right: 8, top: 6, bottom: 10 };
const CARD_MARGIN_COLLAPSED: i8 = 6;
const CARD_AFTER: f32 = 6.0;
pub const HEADER: f32 = 24.0;
const HEADER_AFTER: f32 = 6.0;
const DRAG_BAND: f32 = 22.0;
const PILL_SIZE: f32 = 12.5;
const PILL_PAD: f32 = 20.0;
const PILL_MIN: f32 = 44.0;
const PILL_GAP: f32 = 2.0;
const PILL_HOVER_ALPHA: f32 = 0.6;
const ELLIPSIS: f32 = 22.0;
const ELLIPSIS_GAP: f32 = 4.0;

/// Section label size, Inter Medium (§2.1).
const SECTION_SIZE: f32 = 11.5;

/// The drag-drop insertion line's width and end insets (§3.22).
const DROP_LINE: f32 = 2.0;
const DROP_INSET_VERTICAL: f32 = 2.0;
const DROP_INSET_HORIZONTAL: f32 = 4.0;

/// A group's view state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupState {
    pub selected: usize,
    pub collapsed: bool,
}

/// What a group did this frame.
pub struct GroupResponse<R> {
    /// The body's value; `None` while collapsed.
    pub inner: Option<R>,
    pub state: GroupState,
    /// The panel menu (Pro) or ellipsis (Studio) button.
    pub menu: Response,
    /// The drag handle: the strip background, or the card's top band.
    pub handle: Response,
}

/// A dock group of `tabs` around `body`, which is given the selected tab.
pub fn group<R>(ui: &mut Ui, id_salt: impl egui::AsIdSalt, tabs: &[&str], body: impl FnOnce(&mut Ui, usize) -> R) -> GroupResponse<R> {
    let id = ui.make_persistent_id(id_salt);
    let mut state: GroupState = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    state.selected = state.selected.min(tabs.len().saturating_sub(1));
    let chrome = Chrome::of(ui.ctx());
    let out = match chrome.style.panel_groups {
        PanelGroups::TabStrip => pro_group(ui, id, &chrome, tabs, &mut state, body),
        PanelGroups::Cards => card_group(ui, id, &chrome, tabs, &mut state, body),
    };
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
fn widths(ui: &Ui, tabs: &[&str], font: &FontId, pad: f32, minimum: f32, room: f32, selected: usize) -> tabs::Fit {
    let naturals: Vec<f32> = tabs
        .iter()
        .map(|t| ui.painter().layout_no_wrap((*t).to_owned(), font.clone(), Color32::PLACEHOLDER).size().x + pad)
        .collect();
    tabs::fit(&naturals, minimum, room, Some(selected))
}

fn pro_group<R>(
    ui: &mut Ui,
    id: Id,
    chrome: &Chrome,
    tabs: &[&str],
    state: &mut GroupState,
    body: impl FnOnce(&mut Ui, usize) -> R,
) -> GroupResponse<R> {
    let p = &chrome.palette;
    let width = ui.available_width();
    let (strip, handle) = ui.allocate_exact_size(vec2(width, STRIP), Sense::click_and_drag());
    if handle.double_clicked() {
        state.collapsed = !state.collapsed;
    }
    let r = PRO_RADIUS;
    let strip_corners = if state.collapsed { CornerRadius::same(r) } else { CornerRadius { nw: r, ne: r, sw: 0, se: 0 } };
    ui.painter().rect_filled(strip, strip_corners, p.tab_strip);

    let menu_rect = Rect::from_center_size(pos2(strip.right() - MENU_FROM_RIGHT, strip.center().y), MENU_BOX);
    let font = FontId::new(PRO_SIZE, FontFamily::Proportional);
    let fit = widths(ui, tabs, &font, PRO_PAD, PRO_MIN, menu_rect.left() - strip.left(), state.selected);
    let mut x = strip.left();
    let mut hidden = Vec::new();
    for (index, (name, width)) in tabs.iter().zip(&fit.widths).enumerate() {
        let Some(width) = *width else {
            hidden.push((index, (*name).to_owned()));
            continue;
        };
        let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(width, STRIP));
        x += width;
        let response = ui.interact(rect, id.with(("tab", index)), Sense::click());
        let active = index == state.selected && !state.collapsed;
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), active, name));
        let (fill, ink) = if active {
            (Some(p.card), p.text)
        } else if response.hovered() {
            (Some(p.hover.gamma_multiply(PRO_HOVER_ALPHA)), p.text_dim)
        } else {
            (None, p.text_faint)
        };
        if let Some(fill) = fill {
            let corners = if index == 0 { CornerRadius { nw: r, ..CornerRadius::ZERO } } else { CornerRadius::ZERO };
            ui.painter().rect_filled(rect, corners, fill);
        }
        let (galley, cut) = tabs::label(ui, name, font.clone(), width, PRO_PAD);
        let at = (rect.center() - galley.size() / 2.0).round_to_pixels(ui.pixels_per_point());
        ui.painter().galley(at, galley, ink);
        let response = if cut { response.on_hover_text(*name) } else { response };
        tab_input(&response, index, state);
    }
    if fit.overflows() {
        let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(tabs::CHEVRON, STRIP));
        if let Some(index) = tabs::chevron(ui, id.with("overflow"), rect, &hidden) {
            state.selected = index;
        }
    }
    let menu = ui.interact(menu_rect, id.with("menu"), Sense::click());
    menu.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), crate::strings::own("panel-menu")));
    let ink = if menu.hovered() { p.text } else { p.text_faint };
    for k in [-1.0, 0.0, 1.0] {
        let y = ui.painter().round_to_pixel_center(menu_rect.center().y + k * MENU_PITCH);
        let half = MENU_LINE / 2.0;
        ui.painter().hline(menu_rect.center().x - half..=menu_rect.center().x + half, y, Stroke::new(1.0, ink));
    }

    let inner = (!state.collapsed).then(|| {
        Frame::new()
            .fill(p.card)
            .corner_radius(CornerRadius { nw: 0, ne: 0, sw: r, se: r })
            .inner_margin(Margin::same(PRO_MARGIN))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                body(ui, state.selected)
            })
            .inner
    });
    ui.add_space(PRO_AFTER);
    GroupResponse { inner, state: *state, menu, handle }
}

fn card_group<R>(
    ui: &mut Ui,
    id: Id,
    chrome: &Chrome,
    tabs: &[&str],
    state: &mut GroupState,
    body: impl FnOnce(&mut Ui, usize) -> R,
) -> GroupResponse<R> {
    let p = chrome.palette;
    let margin = if state.collapsed { Margin { bottom: CARD_MARGIN_COLLAPSED, ..CARD_MARGIN } } else { CARD_MARGIN };
    let frame = Frame::new()
        .fill(p.card)
        .stroke(if chrome.style.bevels { Stroke::NONE } else { Stroke::new(1.0, p.card_border) })
        .corner_radius(chrome.metrics.radius)
        .inner_margin(margin);
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let width = ui.available_width();
        let (header, _) = ui.allocate_exact_size(vec2(width, HEADER), Sense::hover());
        let card_top = header.top() - f32::from(margin.top);
        let band = Rect::from_x_y_ranges(header.x_range(), card_top..=card_top + DRAG_BAND);
        let handle = ui.interact(band, id.with("handle"), Sense::click_and_drag());
        if handle.double_clicked() {
            state.collapsed = !state.collapsed;
        }
        let font = crate::fonts::bound(ui.ctx(), crate::fonts::medium(PILL_SIZE));
        let room = width - ELLIPSIS - ELLIPSIS_GAP;
        let fit = widths(ui, tabs, &font, PILL_PAD + PILL_GAP, PILL_MIN + PILL_GAP, room, state.selected);
        let mut x = header.left();
        let mut hidden = Vec::new();
        for (index, (name, width)) in tabs.iter().zip(&fit.widths).enumerate() {
            let Some(width) = *width else {
                hidden.push((index, (*name).to_owned()));
                continue;
            };
            let rect = Rect::from_min_size(pos2(x, header.top()), vec2(width - PILL_GAP, HEADER));
            x += width;
            let response = ui.interact(rect, id.with(("pill", index)), Sense::click());
            let selected = index == state.selected;
            response.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), selected, name));
            pill(ui, chrome, rect, selected, response.hovered());
            let (galley, cut) = tabs::label(ui, name, font.clone(), rect.width(), PILL_PAD);
            let ink = if selected { p.text } else { p.text_dim };
            let at = (rect.center() - galley.size() / 2.0).round_to_pixels(ui.pixels_per_point());
            ui.painter().galley(at, galley, ink);
            let response = if cut { response.on_hover_text(*name) } else { response };
            tab_input(&response, index, state);
        }
        if fit.overflows() {
            let rect = Rect::from_min_size(pos2(x, header.top()), vec2(tabs::CHEVRON, HEADER));
            if let Some(index) = tabs::chevron(ui, id.with("overflow"), rect, &hidden) {
                state.selected = index;
            }
        }
        let ellipsis = Rect::from_min_size(pos2(header.right() - ELLIPSIS, header.center().y - ELLIPSIS / 2.0), vec2(ELLIPSIS, ELLIPSIS));
        let menu = ui.put(ellipsis, IconButton::new(Icon::Ellipsis).size(ELLIPSIS).tooltip(crate::strings::own("panel-menu")));
        let inner = (!state.collapsed).then(|| {
            ui.add_space(HEADER_AFTER);
            body(ui, state.selected)
        });
        (inner, menu, handle)
    });
    if chrome.style.bevels {
        chrome::bevel(ui.painter(), shown.response.rect, true, &p);
    }
    ui.add_space(CARD_AFTER);
    let (inner, menu, handle) = shown.inner;
    GroupResponse { inner, state: *state, menu, handle }
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
    extents.iter().position(|r| y < r.center()).unwrap_or(extents.len())
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
        let shown = ui.scope_builder(egui::UiBuilder::new().id(id.with(group)), |ui| show(ui, group));
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
    (handle.drag_stopped() && before != from && before != from + 1).then(|| moved(order, from, before))
}

/// A Studio pill's fill and outline (§3.11).
fn pill(ui: &Ui, chrome: &Chrome, rect: Rect, selected: bool, hovered: bool) {
    let (p, painter) = (&chrome.palette, ui.painter());
    let radius = chrome.metrics.radius_sm;
    if selected {
        painter.rect_filled(rect, radius, p.hover);
        if chrome.style.bevels {
            chrome::bevel(painter, rect, true, p);
        } else {
            painter.rect_stroke(rect, radius, Stroke::new(1.0, p.field_border), StrokeKind::Inside);
        }
    } else if hovered {
        painter.rect_filled(rect, radius, p.hover.gamma_multiply(PILL_HOVER_ALPHA));
    }
}

/// A section label: Inter Medium 11.5 in `text_faint` (§3.22).
pub fn section_label(ui: &mut Ui, text: &str) -> Response {
    let font = crate::fonts::bound(ui.ctx(), crate::fonts::medium(SECTION_SIZE));
    let ink = Chrome::of(ui.ctx()).palette.text_faint;
    ui.label(egui::RichText::new(text).font(font).color(ink))
}

/// The drag-drop insertion line from `a` to `b` (§3.22): 2 pt `accent`,
/// inset 2 pt at the ends of a vertical line and 4 pt of a horizontal one.
pub fn drop_line(ui: &Ui, a: Pos2, b: Pos2) {
    let vertical = (b.x - a.x).abs() < (b.y - a.y).abs();
    let inset = if vertical { DROP_INSET_VERTICAL } else { DROP_INSET_HORIZONTAL };
    let along = (b - a).normalized() * inset;
    let accent = Chrome::of(ui.ctx()).palette.accent;
    ui.painter().line_segment([a + along, b - along], Stroke::new(DROP_LINE, accent));
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
