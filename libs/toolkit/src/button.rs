// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
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

use crate::chrome::{self, Chrome};
use crate::icons::{self, Icon};
use design::family::style::{PushShape, SecondaryButton};
use egui::emath::GuiRounding;
use egui::{
    Color32, CornerRadius, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2, Widget,
    WidgetInfo, pos2, vec2,
};

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
    size: Option<f32>,
    selected: bool,
    kind: IconKind,
    tooltip: Option<String>,
}

impl IconButton {
    pub fn new(icon: Icon) -> Self {
        Self {
            icon,
            size: None,
            selected: false,
            kind: IconKind::Bar,
            tooltip: None,
        }
    }

    /// The box's side; the icon is 52% of it.
    pub fn size(mut self, size: f32) -> Self {
        self.size = Some(size);
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
        let Self {
            icon,
            size,
            selected,
            kind,
            tooltip,
        } = self;
        let size = size.unwrap_or(Chrome::of(ui.ctx()).style.icon_button as f32);
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
        let name = tooltip.clone().unwrap_or_else(|| icon.name().to_owned());
        response
            .widget_info(|| WidgetInfo::selected(Role::Button, ui.is_enabled(), selected, &name));
        if ui.is_rect_visible(rect) {
            let chrome = Chrome::of(ui.ctx());
            let p = &chrome.palette;
            let hot = response.hovered() || response.is_pointer_button_down_on();
            // A pressed icon button is undetermined (§5.4): it keeps the
            // hover look, as caption buttons do before their press fill.
            let (fill, outline, tint) = match kind {
                IconKind::Bar if selected => {
                    (Some(p.accent_soft), Some(p.accent_border), p.accent_text)
                }
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
                painter.rect_stroke(
                    rect,
                    radius,
                    Stroke::new(Chrome::of(ui.ctx()).style.hairline_width as f32, outline),
                    StrokeKind::Inside,
                );
            }
            paint_icon(
                ui,
                icon,
                rect.center(),
                // Density changes the control's bounds, never its glyph.
                (size / crate::ui_scale::of(ui.ctx()).density as f32
                    * crate::chrome::Chrome::of(ui.ctx()).style.button_icon_share as f32)
                    .round(),
                tint,
            );
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
    let rect =
        Rect::from_center_size(centre, Vec2::splat(side)).round_to_pixels(ui.pixels_per_point());
    icons::image_of(ui.ctx(), icon, side, tint).paint_at(ui, rect);
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
        Self {
            label: label.into(),
            kind: Kind::Primary,
            min_width: 0.0,
        }
    }

    pub fn secondary(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            kind: Kind::Secondary,
            min_width: 0.0,
        }
    }

    /// At least `width` wide.
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }
}

/// A push button's corner radius: fully round for a pill (Pro), else
/// `radius_sm`.
fn push_radius(chrome: &Chrome, height: f32) -> f32 {
    match chrome.style.push_shape {
        PushShape::Pill => height / 2.0,
        PushShape::Rounded => f32::from(chrome.metrics.radius_sm),
    }
}

impl Widget for PushButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let chrome = Chrome::of(ui.ctx());
        let (style, p) = (&chrome.style, &chrome.palette);
        let font = crate::fonts::bound(
            ui.ctx(),
            crate::fonts::medium(crate::chrome::Chrome::of(ui.ctx()).style.push_label_size as f32),
        );
        let galley = ui
            .painter()
            .layout_no_wrap(self.label.clone(), font, Color32::PLACEHOLDER);
        let height = chrome.metrics.push_height.max(galley.size().y);
        let size = vec2(
            (galley.size().x
                + crate::chrome::Chrome::of(ui.ctx())
                    .style
                    .button_push_padding as f32)
                .max(self.min_width),
            height,
        );
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), &self.label));
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
                // Primary hover and pressed opacities (§1.4).
                let [hover, press] = chrome.metrics.primary_alpha;
                let alpha = if pressed {
                    press
                } else if hovered {
                    hover
                } else {
                    1.0
                };
                painter.rect_filled(rect, corner, p.primary_bg.gamma_multiply(alpha));
                p.primary_text
            }
            Kind::Secondary if style.secondary_button == SecondaryButton::Outline => {
                if hovered || pressed {
                    painter.rect_filled(rect, corner, p.hover);
                }
                let outline = if hovered || pressed {
                    p.text
                } else {
                    p.text_dim
                };
                painter.rect_stroke(
                    rect,
                    corner,
                    Stroke::new(
                        crate::chrome::Chrome::of(ui.ctx()).style.button_pro_outline as f32,
                        outline,
                    ),
                    StrokeKind::Inside,
                );
                p.text
            }
            Kind::Secondary => {
                // "85% when pressed" is read as the hover fill at 85%, the
                // primary's pressed rule (§3.13; undetermined, §5.4).
                let fill = if pressed {
                    p.hover.gamma_multiply(chrome.metrics.primary_alpha[1])
                } else if hovered {
                    p.hover
                } else {
                    p.field
                };
                painter.rect_filled(rect, corner, fill);
                if !style.bevels {
                    painter.rect_stroke(
                        rect,
                        corner,
                        Stroke::new(
                            Chrome::of(ui.ctx()).style.hairline_width as f32,
                            p.field_border,
                        ),
                        StrokeKind::Inside,
                    );
                }
                p.text
            }
        };
        if style.bevels {
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
    let ring = rect.expand(crate::chrome::Chrome::of(ui.ctx()).style.focus_offset as f32);
    let corner = CornerRadius::same(
        (radius + crate::chrome::Chrome::of(ui.ctx()).style.focus_offset as f32).round() as u8,
    );
    ui.painter().rect_stroke(
        ring,
        corner,
        Stroke::new(
            crate::chrome::Chrome::of(ui.ctx()).style.focus_ring as f32,
            chrome.palette.accent,
        ),
        StrokeKind::Inside,
    );
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
        Self {
            icon,
            label: label.into(),
        }
    }

    /// The width it takes in a row.
    pub fn width(ui: &Ui, label: &str) -> f32 {
        let font = egui::FontId::proportional(
            crate::chrome::Chrome::of(ui.ctx()).style.button_link_size as f32,
        );
        crate::chrome::Chrome::of(ui.ctx()).style.button_link_icon as f32
            + crate::chrome::Chrome::of(ui.ctx()).style.link_gap as f32
            + ui.painter()
                .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER)
                .size()
                .x
    }
}

impl Widget for Link {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = egui::FontId::proportional(
            crate::chrome::Chrome::of(ui.ctx()).style.button_link_size as f32,
        );
        let galley = ui
            .painter()
            .layout_no_wrap(self.label.clone(), font, Color32::PLACEHOLDER);
        let size = vec2(
            crate::chrome::Chrome::of(ui.ctx()).style.button_link_icon as f32
                + crate::chrome::Chrome::of(ui.ctx()).style.link_gap as f32
                + galley.size().x,
            ui.spacing().interact_size.y,
        );
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| WidgetInfo::labeled(Role::Link, ui.is_enabled(), &self.label));
        if ui.is_rect_visible(rect) {
            let p = Chrome::of(ui.ctx()).palette;
            let ink = if response.hovered() {
                p.text
            } else {
                p.text_dim
            };
            paint_icon(
                ui,
                self.icon,
                pos2(
                    rect.left()
                        + crate::chrome::Chrome::of(ui.ctx()).style.button_link_icon as f32 / 2.0,
                    rect.center().y,
                ),
                crate::chrome::Chrome::of(ui.ctx()).style.button_link_icon as f32,
                ink,
            );
            let at = pos2(
                rect.left()
                    + crate::chrome::Chrome::of(ui.ctx()).style.button_link_icon as f32
                    + crate::chrome::Chrome::of(ui.ctx()).style.link_gap as f32,
                rect.center().y - galley.size().y / 2.0,
            );
            ui.painter()
                .galley(at.round_to_pixels(ui.pixels_per_point()), galley, ink);
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// A selectable label (list rows, chips, tabs, combo items) that keeps its
/// place on hover. Use it instead of `Ui::selectable_label`.
///
/// egui lays out a frame's stroke inside its margin, and `button_style`
/// takes the stroke width back off the padding so a framed widget is the
/// same size in every state. `selectable_label` draws no frame while it is
/// inactive and unselected, though, and keeps the reduced padding: with
/// the chrome's 1 pt stroke at rest, every such row grew by the stroke on
/// hover and its text moved right and down. Here the frame is always laid
/// out, with a transparent fill and a transparent 1 pt border at rest, so
/// hover only changes colours.
pub fn selectable(ui: &mut Ui, selected: bool, text: impl Into<egui::WidgetText>) -> Response {
    let button = egui::Button::selectable(selected, text).frame_when_inactive(true);
    if selected {
        return ui.add(button);
    }
    let rest = ui.visuals().widgets.inactive;
    {
        let inactive = &mut ui.visuals_mut().widgets.inactive;
        inactive.weak_bg_fill = Color32::TRANSPARENT;
        inactive.bg_stroke.color = Color32::TRANSPARENT;
    }
    let response = ui.add(button);
    ui.visuals_mut().widgets.inactive = rest;
    response
}

/// The installed value of the former `ICON_BUTTON` constant.
pub fn icon_button_of(ctx: &egui::Context) -> f32 {
    crate::chrome::Chrome::of(ctx).style.icon_button as f32
}

/// The installed value of the former `PUSH_SIZE` constant.
pub fn push_size_of(ctx: &egui::Context) -> f32 {
    crate::chrome::Chrome::of(ctx).style.push_label_size as f32
}

/// The installed value of the former `FOCUS_RING` constant.
pub fn focus_ring_of(ctx: &egui::Context) -> f32 {
    crate::chrome::Chrome::of(ctx).style.focus_ring as f32
}

/// The installed value of the former `FOCUS_OFFSET` constant.
pub fn focus_offset_of(ctx: &egui::Context) -> f32 {
    crate::chrome::Chrome::of(ctx).style.focus_offset as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adds one widget and returns its response.
    type Add = fn(&mut Ui) -> Response;

    /// A context styled for `context`.
    fn styled(context: design::DesignContext) -> egui::Context {
        let ctx = egui::Context::default();
        crate::install(&ctx, &crate::Theme::for_context(context));
        ctx
    }

    /// The rect a widget takes at rest and while hovered, in the default
    /// design.
    fn rest_and_hover(add: Add) -> (Rect, Rect) {
        rest_and_hover_in(&styled(design::DesignContext::default()), add)
    }

    /// The rect a widget takes at rest and while hovered in `ctx`. The
    /// pointer leaves first, so a context can measure one widget after
    /// another.
    fn rest_and_hover_in(ctx: &egui::Context, add: Add) -> (Rect, Rect) {
        let frame = |ctx: &egui::Context, input: egui::RawInput| {
            let mut rect = Rect::NOTHING;
            let mut output = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| rect = add(ui).rect);
            });
            output.textures_delta.clear();
            rect
        };
        let away = egui::RawInput {
            events: vec![egui::Event::PointerGone],
            ..Default::default()
        };
        frame(ctx, away.clone());
        let rest = frame(ctx, away);
        let hover = egui::RawInput {
            events: vec![egui::Event::PointerMoved(rest.center())],
            ..Default::default()
        };
        frame(ctx, hover.clone());
        let hovered = frame(ctx, hover);
        (rest, hovered)
    }

    #[test]
    fn a_selectable_keeps_its_place_on_hover() {
        let (rest, hovered) = rest_and_hover(|ui| selectable(ui, false, "Applications"));
        assert_eq!(rest, hovered);
        // Selected rows are framed in every state already.
        let (rest, hovered) = rest_and_hover(|ui| selectable(ui, true, "Applications"));
        assert_eq!(rest, hovered);
    }

    #[test]
    fn every_widget_keeps_its_place_on_hover() {
        use design::{DesignContext, Mode, Scheme};
        let widgets: [(&str, Add); 12] = [
            ("button", |ui| ui.button("Find next")),
            ("small_button", |ui| ui.small_button("Dismiss")),
            ("frameless image", |ui| {
                let image = icons::image_of(ui.ctx(), Icon::Ellipsis, 16.0, Color32::PLACEHOLDER);
                ui.add(egui::Button::image(image).frame(false))
            }),
            ("selectable", |ui| selectable(ui, false, "Applications")),
            ("icon", |ui| ui.add(IconButton::new(Icon::Ellipsis))),
            ("rail icon", |ui| {
                ui.add(IconButton::new(Icon::Ellipsis).rail())
            }),
            ("primary", |ui| ui.add(PushButton::primary("Apply"))),
            ("secondary", |ui| ui.add(PushButton::secondary("Cancel"))),
            ("label", |ui| ui.label("Published")),
            ("weak wrapped label", |ui| {
                ui.add(egui::Label::new(egui::RichText::new("Select an app").weak()).wrap())
            }),
            ("label with a tooltip", |ui| {
                ui.label("Update")
                    .on_hover_text("A newer release is published")
            }),
            ("section label", |ui| {
                crate::panel::section_label(ui, "Scheme")
            }),
        ];
        let mut moved = Vec::new();
        for scheme in Scheme::ALL {
            for mode in [Mode::Light, Mode::Dark] {
                let ctx = styled(DesignContext {
                    scheme,
                    mode,
                    ..DesignContext::default()
                });
                for (name, add) in widgets {
                    let (rest, hovered) = rest_and_hover_in(&ctx, add);
                    if rest != hovered {
                        moved.push(format!(
                            "{name} in {scheme:?} {mode:?}: {rest:?} -> {hovered:?}"
                        ));
                    }
                }
            }
        }
        assert!(moved.is_empty(), "moved on hover:\n{}", moved.join("\n"));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn egui_selectable_label_keeps_its_place_on_hover() {
        // egui 0.37 fixes the former hover-dependent size change.
        let (rest, hovered) = rest_and_hover(|ui| ui.selectable_label(false, "Applications"));
        assert_eq!(rest, hovered);
    }

    #[test]
    fn push_buttons_follow_the_style() {
        use design::{DesignContext, Scheme};
        let height = |scheme| {
            Chrome::for_theme(&crate::Theme::for_context(DesignContext {
                scheme,
                ..DesignContext::default()
            }))
            .metrics
            .push_height
        };
        assert_eq!(
            (
                height(Scheme::Pro),
                height(Scheme::Studio),
                height(Scheme::Classic)
            ),
            (28.0, 30.0, 30.0)
        );
    }

    #[test]
    fn the_icon_is_about_half_the_box() {
        assert_eq!(
            (crate::style::default_tokens().icon_button as f32
                * crate::style::default_tokens().button_icon_share as f32)
                .round(),
            15.0
        );
        assert_eq!(
            (22.0 * crate::style::default_tokens().button_icon_share as f32).round(),
            11.0
        );
    }
}
