// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! On/off controls (chrome specification §3.16): the checkbox, the Studio
//! switch, and [`Toggle`], the style's on/off control: the checkbox in Pro
//! (every Pro on/off toggle is one) and the switch otherwise.
//!
//! **Checkbox.** A 14 pt box at radius 2 and its label 6 pt after in
//! `text_dim`; the label is part of the click target. Off: `field` with a
//! 1.5 pt outline inside, `text_faint` (`text_dim` on hover). On: `accent`
//! with a white 1.8 pt tick.
//!
//! **Switch.** A 30 x 17 fully round track, `accent` on and `field_border`
//! off (Classic: a sunken bevel box in `accent` or `field`), and a white
//! knob of radius 6.5 whose centre travels from 8.5 pt in on the left to
//! 8.5 pt in on the right over egui's animation time. The label follows,
//! `text` on and `text_dim` off.

use crate::chrome::{self, Chrome};
use design::family::style::{Switch as SwitchStyle, Toggle as ToggleStyle};
use egui::emath::GuiRounding;
use egui::{
    Color32, Rect, Response, Sense, Stroke, StrokeKind, TextStyle, Ui, Vec2, Widget, WidgetInfo,
    WidgetType, pos2, vec2,
};

/// A checkbox.
#[must_use = "add it with `ui.add`"]
pub struct Checkbox<'a> {
    checked: &'a mut bool,
    label: String,
}

impl<'a> Checkbox<'a> {
    pub fn new(checked: &'a mut bool, label: impl Into<String>) -> Self {
        Self {
            checked,
            label: label.into(),
        }
    }
}

/// The control's rect and its label galley, laid out in one row of the
/// interact height with the label `LABEL_GAP` after `control`.
fn row(
    ui: &mut Ui,
    control: Vec2,
    label: &str,
    checked: bool,
) -> (Rect, Response, Option<std::sync::Arc<egui::Galley>>) {
    let galley = (!label.is_empty()).then(|| {
        let font = TextStyle::Body.resolve(ui.style());
        ui.painter()
            .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER)
    });
    let width = control.x
        + galley.as_ref().map_or(0.0, |g| {
            crate::chrome::Chrome::of(ui.ctx()).style.toggle_label_gap as f32 + g.size().x
        });
    let height = ui.spacing().interact_size.y.max(control.y);
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if response.clicked() {
        response.mark_changed();
    }
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), checked, label)
    });
    let control = Rect::from_min_size(
        pos2(rect.left(), rect.center().y - control.y / 2.0),
        control,
    );
    (
        control.round_to_pixels(ui.pixels_per_point()),
        response,
        galley,
    )
}

fn paint_label(ui: &Ui, control: Rect, galley: Option<std::sync::Arc<egui::Galley>>, ink: Color32) {
    if let Some(galley) = galley {
        let at = pos2(
            control.right() + crate::chrome::Chrome::of(ui.ctx()).style.toggle_label_gap as f32,
            control.center().y - galley.size().y / 2.0,
        );
        ui.painter()
            .galley(at.round_to_pixels(ui.pixels_per_point()), galley, ink);
    }
}

/// The tick inside a checkbox `b` (§3.16): from 3 pt right of the
/// left-middle (0.5 down), to 1 pt left of the bottom-centre (3.5 up), to
/// 3 pt in from the top-right (3.5 down).
pub fn tick(b: Rect) -> [egui::Pos2; 3] {
    [
        pos2(b.left() + 3.0, b.center().y + 0.5),
        pos2(b.center().x - 1.0, b.bottom() - 3.5),
        pos2(b.right() - 3.0, b.top() + 3.5),
    ]
}

impl Widget for Checkbox<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (b, response, galley) = row(
            ui,
            Vec2::splat(crate::chrome::Chrome::of(ui.ctx()).style.checkbox_box as f32),
            &self.label,
            *self.checked,
        );
        if response.clicked() {
            *self.checked = !*self.checked;
        }
        if ui.is_rect_visible(response.rect) {
            let p = Chrome::of(ui.ctx()).palette;
            let outline = if response.hovered() {
                p.text_dim
            } else {
                p.text_faint
            };
            paint_box(ui.painter(), &p, b, *self.checked, outline);
            paint_label(ui, b, galley, p.text_dim);
        }
        response
    }
}

/// A checkbox's box `b` as [`Checkbox`] draws it, with `outline` round an
/// unchecked one: also the read-only mark of a Markdown task.
pub fn paint_box(
    painter: &egui::Painter,
    p: &chrome::Palette,
    b: Rect,
    checked: bool,
    outline: Color32,
) {
    if checked {
        painter.rect_filled(
            b,
            Chrome::of(painter.ctx()).style.toggle_box_radius as f32,
            p.accent,
        );
        painter.add(egui::Shape::line(
            tick(b).to_vec(),
            Stroke::new(
                Chrome::of(painter.ctx()).style.toggle_tick as f32,
                crate::style::token_colour(Chrome::of(painter.ctx()).style.toggle_mark),
            ),
        ));
    } else {
        painter.rect_filled(
            b,
            Chrome::of(painter.ctx()).style.toggle_box_radius as f32,
            p.field,
        );
        painter.rect_stroke(
            b,
            Chrome::of(painter.ctx()).style.toggle_box_radius as f32,
            Stroke::new(
                Chrome::of(painter.ctx()).style.toggle_outline as f32,
                outline,
            ),
            StrokeKind::Inside,
        );
    }
}

/// A switch.
#[must_use = "add it with `ui.add`"]
pub struct Switch<'a> {
    on: &'a mut bool,
    label: String,
}

impl<'a> Switch<'a> {
    pub fn new(on: &'a mut bool, label: impl Into<String>) -> Self {
        Self {
            on,
            label: label.into(),
        }
    }
}

/// The knob centre's x along `track` at animation position `t` (0 off, 1 on).
pub fn knob_x(track: Rect, t: f32) -> f32 {
    knob_x_with_inset(
        track,
        t,
        crate::style::default_tokens().toggle_knob_inset as f32,
    )
}

fn knob_x_with_inset(track: Rect, t: f32, inset: f32) -> f32 {
    egui::lerp(track.left() + inset..=track.right() - inset, t)
}

impl Widget for Switch<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (track, response, galley) = row(
            ui,
            vec2(
                crate::chrome::Chrome::of(ui.ctx()).style.switch_track_w as f32,
                crate::chrome::Chrome::of(ui.ctx()).style.switch_track_h as f32,
            ),
            &self.label,
            *self.on,
        );
        if response.clicked() {
            *self.on = !*self.on;
        }
        if ui.is_rect_visible(response.rect) {
            let chrome = Chrome::of(ui.ctx());
            let p = &chrome.palette;
            let t = ui.ctx().animate_bool_responsive(response.id, *self.on);
            let painter = ui.painter();
            let centre = pos2(
                knob_x_with_inset(
                    track,
                    t,
                    Chrome::of(ui.ctx()).style.toggle_knob_inset as f32,
                ),
                track.center().y,
            );
            if chrome.style.switch == SwitchStyle::Block {
                // Classic's knob is undetermined (§5.4): a raised `card`
                // block the knob's diameter square, as its slider knob is.
                painter.rect_filled(
                    track,
                    Chrome::of(ui.ctx()).style.square_radius,
                    if *self.on { p.accent } else { p.field },
                );
                chrome::bevel(painter, track, false, p);
                let knob = Rect::from_center_size(
                    centre,
                    Vec2::splat(2.0 * crate::chrome::Chrome::of(ui.ctx()).style.toggle_knob as f32),
                )
                .round_to_pixels(ui.pixels_per_point());
                painter.rect_filled(knob, Chrome::of(ui.ctx()).style.square_radius, p.card);
                chrome::bevel(painter, knob, true, p);
            } else {
                painter.rect_filled(
                    track,
                    vec2(
                        crate::chrome::Chrome::of(ui.ctx()).style.switch_track_w as f32,
                        crate::chrome::Chrome::of(ui.ctx()).style.switch_track_h as f32,
                    )
                    .y / 2.0,
                    if *self.on { p.accent } else { p.field_border },
                );
                painter.circle_filled(
                    centre,
                    crate::chrome::Chrome::of(ui.ctx()).style.toggle_knob as f32,
                    crate::style::token_colour(
                        crate::chrome::Chrome::of(ui.ctx()).style.toggle_mark,
                    ),
                );
            }
            paint_label(
                ui,
                track,
                galley,
                if *self.on { p.text } else { p.text_dim },
            );
        }
        response
    }
}

/// The style's on/off toggle: a [`Checkbox`] (Pro) or a [`Switch`].
#[must_use = "add it with `ui.add`"]
pub struct Toggle<'a> {
    on: &'a mut bool,
    label: String,
}

impl<'a> Toggle<'a> {
    pub fn new(on: &'a mut bool, label: impl Into<String>) -> Self {
        Self {
            on,
            label: label.into(),
        }
    }
}

impl Widget for Toggle<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        match Chrome::of(ui.ctx()).style.toggle {
            ToggleStyle::Checkbox => ui.add(Checkbox::new(self.on, self.label)),
            ToggleStyle::Switch => ui.add(Switch::new(self.on, self.label)),
        }
    }
}

/// The installed value of the former `BOX` constant.
pub fn box_of(ctx: &egui::Context) -> f32 {
    crate::chrome::Chrome::of(ctx).style.checkbox_box as f32
}

/// The installed value of the former `TRACK` constant.
pub fn track_of(ctx: &egui::Context) -> Vec2 {
    vec2(
        crate::chrome::Chrome::of(ctx).style.switch_track_w as f32,
        crate::chrome::Chrome::of(ctx).style.switch_track_h as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tick_follows_the_specified_path() {
        let b = Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::splat(crate::style::default_tokens().checkbox_box as f32),
        );
        assert_eq!(tick(b), [pos2(3.0, 7.5), pos2(6.0, 10.5), pos2(11.0, 3.5)]);
    }

    #[test]
    fn the_knob_travels_between_its_insets() {
        let track = Rect::from_min_size(
            pos2(10.0, 0.0),
            vec2(
                crate::style::default_tokens().switch_track_w as f32,
                crate::style::default_tokens().switch_track_h as f32,
            ),
        );
        assert_eq!(knob_x(track, 0.0), 18.5);
        assert_eq!(knob_x(track, 1.0), 31.5);
    }
}
