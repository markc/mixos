// SPDX-License-Identifier: MIT OR Apache-2.0
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

/// The checkbox's box, its radius and the gap before any label (§3.16).
pub const BOX: f32 = 14.0;
const BOX_RADIUS: f32 = 2.0;
const LABEL_GAP: f32 = 6.0;

/// Outline and tick widths (§2.3).
const OUTLINE: f32 = 1.5;
const TICK: f32 = 1.8;

/// The tick and the knob are white in every theme (§3.16).
const MARK: Color32 = Color32::WHITE;

/// The switch's track, knob radius and the knob centre's inset (§3.16).
pub const TRACK: Vec2 = vec2(30.0, 17.0);
const KNOB: f32 = 6.5;
const KNOB_INSET: f32 = 8.5;

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
    let width = control.x + galley.as_ref().map_or(0.0, |g| LABEL_GAP + g.size().x);
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
            control.right() + LABEL_GAP,
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
        let (b, response, galley) = row(ui, Vec2::splat(BOX), &self.label, *self.checked);
        if response.clicked() {
            *self.checked = !*self.checked;
        }
        if ui.is_rect_visible(response.rect) {
            let p = Chrome::of(ui.ctx()).palette;
            let painter = ui.painter();
            if *self.checked {
                painter.rect_filled(b, BOX_RADIUS, p.accent);
                painter.add(egui::Shape::line(tick(b).to_vec(), Stroke::new(TICK, MARK)));
            } else {
                let outline = if response.hovered() {
                    p.text_dim
                } else {
                    p.text_faint
                };
                painter.rect_filled(b, BOX_RADIUS, p.field);
                painter.rect_stroke(
                    b,
                    BOX_RADIUS,
                    Stroke::new(OUTLINE, outline),
                    StrokeKind::Inside,
                );
            }
            paint_label(ui, b, galley, p.text_dim);
        }
        response
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
    egui::lerp(track.left() + KNOB_INSET..=track.right() - KNOB_INSET, t)
}

impl Widget for Switch<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (track, response, galley) = row(ui, TRACK, &self.label, *self.on);
        if response.clicked() {
            *self.on = !*self.on;
        }
        if ui.is_rect_visible(response.rect) {
            let chrome = Chrome::of(ui.ctx());
            let p = &chrome.palette;
            let t = ui.ctx().animate_bool_responsive(response.id, *self.on);
            let painter = ui.painter();
            let centre = pos2(knob_x(track, t), track.center().y);
            if chrome.style.switch == SwitchStyle::Block {
                // Classic's knob is undetermined (§5.4): a raised `card`
                // block the knob's diameter square, as its slider knob is.
                painter.rect_filled(track, 0.0, if *self.on { p.accent } else { p.field });
                chrome::bevel(painter, track, false, p);
                let knob = Rect::from_center_size(centre, Vec2::splat(2.0 * KNOB))
                    .round_to_pixels(ui.pixels_per_point());
                painter.rect_filled(knob, 0.0, p.card);
                chrome::bevel(painter, knob, true, p);
            } else {
                painter.rect_filled(
                    track,
                    TRACK.y / 2.0,
                    if *self.on { p.accent } else { p.field_border },
                );
                painter.circle_filled(centre, KNOB, MARK);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tick_follows_the_specified_path() {
        let b = Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(BOX));
        assert_eq!(tick(b), [pos2(3.0, 7.5), pos2(6.0, 10.5), pos2(11.0, 3.5)]);
    }

    #[test]
    fn the_knob_travels_between_its_insets() {
        let track = Rect::from_min_size(pos2(10.0, 0.0), TRACK);
        assert_eq!(knob_x(track, 0.0), 18.5);
        assert_eq!(knob_x(track, 1.0), 31.5);
    }
}
