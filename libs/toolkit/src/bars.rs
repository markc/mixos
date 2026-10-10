// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's bars (chrome specification §2.6, §3.9): the options bar
//! under the title bar, the status bar, the tool bar and the icon rail, all
//! in the `chrome` role, with their `separator` edges, and the dividers
//! drawn inside bars.
//!
//! | bar | Pro | Studio / Classic | edges |
//! |---|---|---|---|
//! | options bar (height) | 36 | 42 | top and bottom |
//! | status bar (height) | 24 | 30 | top |
//! | tool bar (width) | 40 | 50 | right (Pro only) |
//! | icon rail (width) | 36 | 44 | one 6 pt left of its left edge |
//!
//! Each is an egui panel of exact size, so call them in egui's panel order:
//! after the title bar and before the central panel.

use crate::chrome::Chrome;
use egui::{Align, Frame, InnerResponse, Layout, Margin, Panel, Rect, Sense, Stroke, Ui, pos2, vec2};

/// A bar's size along its short side, from the style (§2.6:
/// [`crate::chrome::Metrics::bars`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sizes {
    pub options: f32,
    pub status: f32,
    pub tool: f32,
    /// A tool-bar button and the tool bar's side margin.
    pub tool_button: f32,
    pub tool_margin: f32,
    pub rail: f32,
    pub rail_button: f32,
}

/// The inset of a horizontal bar's contents from its ends. Undetermined by
/// the specification (§5.4); the item spacing keeps the first control off
/// the window edge.
const BAR_INSET: i8 = 8;

/// The gap between stacked panel control rows (§2.2).
pub const ROW_GAP: f32 = 4.0;

/// The rail's separator stands this far left of the rail (§2.6).
const RAIL_RULE: f32 = 6.0;

/// A vertical divider's slot width (§3.9), and its height in the options
/// bar.
pub const DIVIDER_SLOT: f32 = 9.0;
pub const DIVIDER_HEIGHT: f32 = 22.0;

fn frame(ui: &Ui, margin: Margin) -> Frame {
    Frame::new().fill(Chrome::of(ui.ctx()).palette.chrome).inner_margin(margin)
}

fn rule(ui: &Ui, from: egui::Pos2, to: egui::Pos2) {
    let colour = Chrome::of(ui.ctx()).palette.separator;
    // A layer painter: the rail's rule lies outside the rail.
    let painter = ui.ctx().layer_painter(ui.layer_id());
    let stroke = Stroke::new(1.0, colour);
    if from.y == to.y {
        painter.hline(from.x..=to.x, painter.round_to_pixel_center(from.y), stroke);
    } else {
        painter.vline(painter.round_to_pixel_center(from.x), from.y..=to.y, stroke);
    }
}

/// The options bar: a row of controls, centred vertically.
pub fn options<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let size = Chrome::of(ui.ctx()).metrics.bars.options;
    let frame = frame(ui, Margin::symmetric(BAR_INSET, 0));
    Panel::top("toolkit-options-bar").exact_size(size).show_separator_line(false).frame(frame).show(ui, |ui| {
        let r = ui.max_rect().expand2(vec2(f32::from(BAR_INSET), 0.0));
        rule(ui, pos2(r.left(), r.top() + 0.5), pos2(r.right(), r.top() + 0.5));
        rule(ui, pos2(r.left(), r.bottom() - 0.5), pos2(r.right(), r.bottom() - 0.5));
        ui.with_layout(Layout::left_to_right(Align::Center), contents).inner
    })
}

/// The status bar: a row of controls, centred vertically.
pub fn status<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let size = Chrome::of(ui.ctx()).metrics.bars.status;
    let frame = frame(ui, Margin::symmetric(BAR_INSET, 0));
    Panel::bottom("toolkit-status-bar").exact_size(size).show_separator_line(false).frame(frame).show(ui, |ui| {
        let r = ui.max_rect().expand2(vec2(f32::from(BAR_INSET), 0.0));
        rule(ui, pos2(r.left(), r.top() + 0.5), pos2(r.right(), r.top() + 0.5));
        ui.with_layout(Layout::left_to_right(Align::Center), contents).inner
    })
}

/// The tool bar at the window's left: a column of tool buttons.
pub fn tools<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let chrome = Chrome::of(ui.ctx());
    let sizes = chrome.metrics.bars;
    let margin = Margin::symmetric(sizes.tool_margin as i8, sizes.tool_margin as i8);
    let frame = frame(ui, margin);
    Panel::left("toolkit-tool-bar").exact_size(sizes.tool).resizable(false).show_separator_line(false).frame(frame).show(ui, |ui| {
        if chrome.style.tool_bar_rule {
            let r = ui.max_rect().expand(sizes.tool_margin);
            rule(ui, pos2(r.right() - 0.5, r.top()), pos2(r.right() - 0.5, r.bottom()));
        }
        ui.with_layout(Layout::top_down(Align::Center), contents).inner
    })
}

/// The icon rail at the window's right: a column of rail toggles.
pub fn rail<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let sizes = Chrome::of(ui.ctx()).metrics.bars;
    let inset = ((sizes.rail - sizes.rail_button) / 2.0) as i8;
    let frame = frame(ui, Margin::symmetric(inset, inset));
    Panel::right("toolkit-rail").exact_size(sizes.rail).resizable(false).show_separator_line(false).frame(frame).show(ui, |ui| {
        let r = ui.max_rect().expand(f32::from(inset));
        rule(ui, pos2(r.left() - RAIL_RULE, r.top()), pos2(r.left() - RAIL_RULE, r.bottom()));
        ui.with_layout(Layout::top_down(Align::Center), contents).inner
    })
}

/// The right dock behind the panel groups: `dock` fill, the style's width
/// by default (290 pt Pro, 300 otherwise), resizable from 250 to 520, with
/// the style's inner margin (2 pt Pro, 8 otherwise; §2.6).
pub fn dock<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let width = Chrome::of(ui.ctx()).metrics.dock_width;
    Panel::right("toolkit-dock")
        .default_size(width)
        .size_range(250.0..=520.0)
        .resizable(true)
        .show_separator_line(false)
        .frame(dock_frame(ui.ctx()))
        .show(ui, |ui| {
            // Stacked panel rows are 4 pt apart, not egui's 6 (§2.2).
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            contents(ui)
        })
}

/// The dock's frame, for any panel that holds panel groups: `dock` with the
/// style's margin (§2.6).
pub fn dock_frame(ctx: &egui::Context) -> Frame {
    let chrome = Chrome::of(ctx);
    Frame::new().fill(chrome.palette.dock).inner_margin(Margin::same(chrome.metrics.dock_margin as i8))
}

/// A vertical divider in a bar: a 9 pt slot with a centred 1 pt
/// `separator` line `height` tall ([`DIVIDER_HEIGHT`] in the options bar).
pub fn divider(ui: &mut Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(DIVIDER_SLOT, height), Sense::hover());
    let x = ui.painter().round_to_pixel_center(rect.center().x);
    ui.painter().vline(x, rect.y_range(), Stroke::new(1.0, Chrome::of(ui.ctx()).palette.separator));
}

/// A full-width 1 pt `separator` hairline.
pub fn hairline(ui: &mut Ui) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    let y = ui.painter().round_to_pixel_center(rect.center().y);
    ui.painter().hline(rect.x_range(), y, Stroke::new(1.0, Chrome::of(ui.ctx()).palette.separator));
    rect
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;
    use design::{DesignContext, Mode, Scheme};

    #[test]
    fn bars_take_the_style_sizes() {
        let sizes = |scheme| Chrome::for_theme(&Theme::for_context(DesignContext { scheme, mode: Mode::Dark, ..DesignContext::default() })).metrics.bars;
        let (pro, studio) = (sizes(Scheme::Pro), sizes(Scheme::Studio));
        assert_eq!((pro.options, pro.status, pro.tool, pro.rail), (36.0, 24.0, 40.0, 36.0));
        assert_eq!((studio.options, studio.status, studio.tool, studio.rail), (42.0, 30.0, 50.0, 44.0));
        // A button plus its two side margins fills the tool bar.
        assert_eq!(pro.tool_button + 2.0 * pro.tool_margin, pro.tool);
        assert_eq!(studio.tool_button + 2.0 * studio.tool_margin, studio.tool);
    }
}
