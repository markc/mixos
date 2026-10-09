// SPDX-License-Identifier: MIT OR Apache-2.0
//! Buttons (chrome specification §3.8, §3.13): square icon buttons for bars,
//! the title bar and the icon rail; the default-action ("primary") and
//! secondary push buttons of dialogs; and a frameless text link.
//!
//! Icon button states (§3.8):
//!
//! | state | fill | outline | icon tint |
//! |---|---|---|---|
//! | rest | none | none | `icon` |
//! | hover | `hover` | none | `text` |
//! | selected | `accent_soft` | 1 pt `accent_border` inside | `accent_text` |
//!
//! The rail variant is `card` with a 1 pt `card_border` and `text` when on,
//! `hover` and `text` on hover, and `text_faint` at rest.
//!
//! Push buttons are Inter Medium 13, label plus 28 wide (or the caller's
//! minimum). Pro draws them 28 high and fully round; Studio and Classic 30
//! high at `radius_sm`, and Classic bevels them instead of outlining. A
//! push button shows a 2 pt `accent` focus ring only while it has keyboard
//! focus: egui never focuses a button on click.
//!
//! Disabled buttons need nothing here: a disabled `Ui` paints at
//! `disabled_alpha` (50%, §1.4).

use crate::chrome::{self, Chrome, Grammar};
use crate::icons::{self, Icon};
use egui::emath::GuiRounding;
use egui::{
    Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, Widget, WidgetInfo, WidgetType, pos2,
    vec2,
};

/// An icon button's side in the title bar (§3.1, §3.8).
pub const ICON_BUTTON: f32 = 28.0;

/// The icon's share of an icon button's side, rounded to whole points (§3.8).
const ICON_SHARE: f32 = 0.52;

/// Push-button text size, Inter Medium (§2.1).
pub const PUSH_SIZE: f32 = 13.0;

/// Push-button width beyond its label (§3.13: 14 pt each side).
const PUSH_PADDING: f32 = 28.0;

/// Primary hover and pressed opacities (§1.4): Pro, then Studio and Classic.
const PRIMARY_ALPHA_PRO: [f32; 2] = [0.9, 0.8];
const PRIMARY_ALPHA: [f32; 2] = [0.93, 0.85];

/// Width of the Pro secondary outline (§2.3).
const PRO_OUTLINE: f32 = 1.5;

/// Focus ring width and its distance outside the button (§2.3, §3.13).
pub const FOCUS_RING: f32 = 2.0;
pub const FOCUS_OFFSET: f32 = 2.0;

/// The title-bar link's icon and text sizes (§3.1).
const LINK_ICON: f32 = 14.0;
const LINK_SIZE: f32 = 12.0;

/// From the link's icon to its label. Undetermined by the specification
/// (§5.4); 4 pt matches the evidence's spacing by eye.
const LINK_GAP: f32 = 4.0;

/// Which family of icon button (§3.8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconKind {
    /// Title bar and options bar.
    #[default]
    Bar,
    /// The icon rail's toggles.
    Rail,
}

/// A square button showing one Lucide icon.
#[must_use = "add it with `ui.add`"]
pub struct IconButton {
    icon: Icon,
    size: f32,
    selected: bool,
    kind: IconKind,
    tooltip: Option<String>,
}

impl IconButton {
    pub fn new(icon: Icon) -> Self {
        Self { icon, size: ICON_BUTTON, selected: false, kind: IconKind::Bar, tooltip: None }
    }

    /// The box's side; the icon is 52% of it.
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Toggled on.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// The icon rail's colours.
    pub fn rail(mut self) -> Self {
        self.kind = IconKind::Rail;
        self
    }

    /// The tooltip, which is also the button's accessible name.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
}

impl Widget for IconButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let Self { icon, size, selected, kind, tooltip } = self;
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
        let name = tooltip.clone().unwrap_or_else(|| icon.name().to_owned());
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), selected, &name));
        if ui.is_rect_visible(rect) {
            let chrome = Chrome::of(ui.ctx());
            let p = &chrome.palette;
            let hot = response.hovered() || response.is_pointer_button_down_on();
            // A pressed icon button is undetermined (§5.4): it keeps the
            // hover look, as caption buttons do before their press fill.
            let (fill, outline, tint) = match kind {
                IconKind::Bar if selected => (Some(p.accent_soft), Some(p.accent_border), p.accent_text),
                IconKind::Bar if hot => (Some(p.hover), None, p.text),
                IconKind::Bar => (None, None, p.icon),
                IconKind::Rail if selected => (Some(p.card), Some(p.card_border), p.text),
                IconKind::Rail if hot => (Some(p.hover), None, p.text),
                IconKind::Rail => (None, None, p.text_faint),
            };
            let radius = chrome.metrics.radius_sm;
            let painter = ui.painter();
            if let Some(fill) = fill {
                painter.rect_filled(rect, radius, fill);
            }
            if let Some(outline) = outline {
                painter.rect_stroke(rect, radius, Stroke::new(1.0, outline), StrokeKind::Inside);
            }
            paint_icon(ui, icon, rect.center(), (size * ICON_SHARE).round(), tint);
        }
        match tooltip {
            Some(text) => response.on_hover_text(text),
            None => response,
        }
    }
}

/// `icon`, `side` points square, centred on `centre` and tinted `tint`, in
/// the context's icon stroke.
pub fn paint_icon(ui: &Ui, icon: Icon, centre: egui::Pos2, side: f32, tint: Color32) {
    let rect = Rect::from_center_size(centre, Vec2::splat(side)).round_to_pixels(ui.pixels_per_point());
    icons::image(icon, icons::stroke_of(ui.ctx()), side, tint).paint_at(ui, rect);
}

/// The two push-button kinds (§3.13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The default action: `primary_bg` with `primary_text` ink.
    Primary,
    Secondary,
}

/// A dialog push button.
#[must_use = "add it with `ui.add`"]
pub struct PushButton {
    label: String,
    kind: Kind,
    min_width: f32,
}

impl PushButton {
    pub fn primary(label: impl Into<String>) -> Self {
        Self { label: label.into(), kind: Kind::Primary, min_width: 0.0 }
    }

    pub fn secondary(label: impl Into<String>) -> Self {
        Self { label: label.into(), kind: Kind::Secondary, min_width: 0.0 }
    }

    /// At least `width` wide.
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }
}

/// A push button's height in `grammar` (§3.13: 28 Pro, 30 otherwise).
pub fn push_height(grammar: Grammar) -> f32 {
    if grammar.is_pro() { 28.0 } else { 30.0 }
}

/// A push button's corner radius: fully round in Pro, else `radius_sm`.
fn push_radius(chrome: &Chrome, height: f32) -> f32 {
    if chrome.grammar.is_pro() { height / 2.0 } else { f32::from(chrome.metrics.radius_sm) }
}

impl Widget for PushButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let chrome = Chrome::of(ui.ctx());
        let (grammar, p) = (chrome.grammar, &chrome.palette);
        let font = crate::fonts::bound(ui.ctx(), crate::fonts::medium(PUSH_SIZE));
        let galley = ui.painter().layout_no_wrap(self.label.clone(), font, Color32::PLACEHOLDER);
        let height = push_height(grammar);
        let size = vec2((galley.size().x + PUSH_PADDING).max(self.min_width), height);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), &self.label));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let pressed = response.is_pointer_button_down_on();
        let hovered = response.hovered();
        let radius = push_radius(&chrome, height);
        let corner = CornerRadius::same(radius.round() as u8);
        let painter = ui.painter();
        let ink = match self.kind {
            Kind::Primary => {
                let [hover, press] = if grammar.is_pro() { PRIMARY_ALPHA_PRO } else { PRIMARY_ALPHA };
                let alpha = if pressed { press } else if hovered { hover } else { 1.0 };
                painter.rect_filled(rect, corner, p.primary_bg.gamma_multiply(alpha));
                p.primary_text
            }
            Kind::Secondary if grammar.is_pro() => {
                if hovered || pressed {
                    painter.rect_filled(rect, corner, p.hover);
                }
                let outline = if hovered || pressed { p.text } else { p.text_dim };
                painter.rect_stroke(rect, corner, Stroke::new(PRO_OUTLINE, outline), StrokeKind::Inside);
                p.text
            }
            Kind::Secondary => {
                // "85% when pressed" is read as the hover fill at 85%, the
                // primary's pressed rule (§3.13; undetermined, §5.4).
                let fill = if pressed {
                    p.hover.gamma_multiply(PRIMARY_ALPHA[1])
                } else if hovered {
                    p.hover
                } else {
                    p.field
                };
                painter.rect_filled(rect, corner, fill);
                if !grammar.has_bevel() {
                    painter.rect_stroke(rect, corner, Stroke::new(1.0, p.field_border), StrokeKind::Inside);
                }
                p.text
            }
        };
        if grammar.has_bevel() {
            chrome::bevel(painter, rect, !pressed, p);
        }
        let at = (rect.center() - galley.size() / 2.0).round_to_pixels(ui.pixels_per_point());
        painter.galley(at, galley, ink);
        if response.has_focus() {
            focus_ring(ui, rect, radius, &chrome);
        }
        response
    }
}

/// The keyboard focus ring round `rect` of corner radius `radius` (§3.13):
/// 2 pt `accent`, 2 pt outside, its radius grown to match.
pub fn focus_ring(ui: &Ui, rect: Rect, radius: f32, chrome: &Chrome) {
    let ring = rect.expand(FOCUS_OFFSET);
    let corner = CornerRadius::same((radius + FOCUS_OFFSET).round() as u8);
    ui.painter().rect_stroke(ring, corner, Stroke::new(FOCUS_RING, chrome.palette.accent), StrokeKind::Inside);
}

/// A frameless link: a 14 pt icon and a 12 pt `text_dim` label, `text` on
/// hover (the hover ink is undetermined, §5.4).
#[must_use = "add it with `ui.add`"]
pub struct Link {
    icon: Icon,
    label: String,
}

impl Link {
    pub fn new(icon: Icon, label: impl Into<String>) -> Self {
        Self { icon, label: label.into() }
    }

    /// The width it takes in a row.
    pub fn width(ui: &Ui, label: &str) -> f32 {
        let font = egui::FontId::proportional(LINK_SIZE);
        LINK_ICON + LINK_GAP + ui.painter().layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER).size().x
    }
}

impl Widget for Link {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = egui::FontId::proportional(LINK_SIZE);
        let galley = ui.painter().layout_no_wrap(self.label.clone(), font, Color32::PLACEHOLDER);
        let size = vec2(LINK_ICON + LINK_GAP + galley.size().x, ui.spacing().interact_size.y);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Link, ui.is_enabled(), &self.label));
        if ui.is_rect_visible(rect) {
            let p = Chrome::of(ui.ctx()).palette;
            let ink = if response.hovered() { p.text } else { p.text_dim };
            paint_icon(ui, self.icon, pos2(rect.left() + LINK_ICON / 2.0, rect.center().y), LINK_ICON, ink);
            let at = pos2(rect.left() + LINK_ICON + LINK_GAP, rect.center().y - galley.size().y / 2.0);
            ui.painter().galley(at.round_to_pixels(ui.pixels_per_point()), galley, ink);
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_buttons_follow_the_grammar() {
        assert_eq!(push_height(Grammar::Pro), 28.0);
        assert_eq!(push_height(Grammar::Studio), 30.0);
        assert_eq!(push_height(Grammar::Classic), 30.0);
    }

    #[test]
    fn the_icon_is_about_half_the_box() {
        assert_eq!((ICON_BUTTON * ICON_SHARE).round(), 15.0);
        assert_eq!((22.0 * ICON_SHARE).round(), 11.0);
    }
}
