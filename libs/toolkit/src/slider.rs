// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sliders (chrome specification §3.18).
//!
//! A slider is an 18 pt row, the full width it is given (at least 60). Its
//! track is inset 7 pt each side: 3 pt high in `field_border` at radius 2
//! with the part left of the knob in `text_dim` (Classic: `accent`), or
//! 5 pt high for a gradient track. The knob is a 6 pt `#ECECEC` disc with
//! a 1 pt `#282828` ring in Pro, a 7 pt white disc in Studio, and a 10 x 16
//! raised `card` block in Classic; round knobs cast a black shadow (90/255,
//! half a point larger, 1 pt down). Hovered or dragged, a 2 pt `accent_soft`
//! halo of radius 9.5 rings the knob.
//!
//! Two optional gestures, for dialogs: a double-click (within 0.5 s and
//! 6 pt, nothing else between, counted the Windows way, so a third click
//! starts over) resets the value, and each wheel notch steps it (x10 with
//! Shift).
//!
//! A focused slider steps with Left and Right (x10 with Shift), and takes
//! AccessKit Increment, Decrement and SetValue, advertising its bounds and
//! step, so an agent or a screen reader adjusts it without a pointer.
//!
//! [`row`] is the slider row: the label in `text_dim` at the left and a
//! 74 pt value field at the right, the slider below, and 4 pt after.

use crate::chrome::{self, Chrome};
use design::family::style::{Knob, SliderFill};
use crate::field::{self, ValueField};
use egui::emath::GuiRounding;
use egui::accesskit::{Action, ActionData};
use egui::{
    Color32, Event, EventFilter, Key, Mesh, Pos2, Rect, Response, Sense, Shape, Stroke, TextStyle, Ui, Vec2, Widget,
    WidgetInfo, pos2, vec2,
};
use std::ops::RangeInclusive;

/// Row height, least width and the track's inset (§3.18).
pub const HEIGHT: f32 = 18.0;
const MIN_WIDTH: f32 = 60.0;
const INSET: f32 = 7.0;

/// Track heights and radius (§3.18, §2.4).
const PLAIN_TRACK: f32 = 3.0;
const GRADIENT_TRACK: f32 = 5.0;
const TRACK_RADIUS: f32 = 2.0;

/// Knobs (§3.18): Pro's fill and ring are literals of the specification,
/// not roles.
const KNOB_PRO: f32 = 6.0;
const KNOB_STUDIO: f32 = 7.0;
const KNOB_PRO_FILL: Color32 = Color32::from_rgb(0xEC, 0xEC, 0xEC);
const KNOB_PRO_RING: Color32 = Color32::from_rgb(0x28, 0x28, 0x28);
const KNOB_STUDIO_FILL: Color32 = Color32::WHITE;
const KNOB_CLASSIC: Vec2 = vec2(10.0, 16.0);
const SHADOW: Color32 = Color32::from_black_alpha(90);
const HALO_RADIUS: f32 = 9.5;
const HALO_WIDTH: f32 = 2.0;

/// The double-click window (§3.18), and how far apart its two clicks may
/// be (undetermined, §5.4: egui's own click tolerance, 6 pt).
const DOUBLE_CLICK: f64 = 0.5;
const DOUBLE_CLICK_DISTANCE: f32 = 6.0;

/// A press held longer than this is no click (egui's own limit).
const LONG_PRESS: f64 = 0.8;

/// The keyboard step as a share of the range when the caller sets none
/// (undetermined, §5.4: a hundredth), and Shift's multiple (as the wheel).
const KEY_STEP_SHARE: f64 = 0.01;
const BIG_STEP: f64 = 10.0;

/// The gap after a slider row (§3.18) and its value field's width.
const ROW_GAP: f32 = 4.0;

/// A horizontal slider.
#[must_use = "add it with `ui.add`"]
pub struct Slider<'a> {
    value: &'a mut f64,
    range: RangeInclusive<f64>,
    gradient: Option<Vec<Color32>>,
    reset: Option<f64>,
    wheel_step: Option<f64>,
    step: Option<f64>,
    label: String,
}

impl<'a> Slider<'a> {
    pub fn new(value: &'a mut f64, range: RangeInclusive<f64>) -> Self {
        Self { value, range, gradient: None, reset: None, wheel_step: None, step: None, label: String::new() }
    }

    /// One arrow key or AccessKit Increment moves the value by `step`
    /// (default a hundredth of the range).
    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    /// A 5 pt gradient track through `stops`, left to right (hue and similar).
    pub fn gradient(mut self, stops: Vec<Color32>) -> Self {
        self.gradient = Some(stops);
        self
    }

    /// A double-click sets `value`.
    pub fn reset_to(mut self, value: f64) -> Self {
        self.reset = Some(value);
        self
    }

    /// Each wheel notch moves the value by `step` (x10 with Shift).
    pub fn wheel(mut self, step: f64) -> Self {
        self.wheel_step = Some(step);
        self
    }

    /// The accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

/// Where a pointer at `x` puts the value: the track spans `track`.
pub fn value_at(track: Rect, x: f32, range: &RangeInclusive<f64>) -> f64 {
    let t = f64::from(((x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0));
    range.start() + t * (range.end() - range.start())
}

/// The value's position along `track`.
pub fn x_of(track: Rect, value: f64, range: &RangeInclusive<f64>) -> f32 {
    let span = range.end() - range.start();
    let t = if span == 0.0 { 0.0 } else { ((value - range.start()) / span).clamp(0.0, 1.0) };
    egui::lerp(track.x_range(), t as f32)
}

/// One input to a slider's [`Gesture`], in the order it arrived.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    /// The primary button went down over the slider.
    Press(Pos2),
    Release(Pos2),
    Move(Pos2),
    /// Anything else that adjusts the slider between clicks: a wheel turn,
    /// a key or an AccessKit step or value.
    Interrupt,
}

/// What an input does to the value, applied in order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// The knob follows the pointer to this x.
    Follow(f32),
    /// A double click: the value goes back to its default.
    Reset,
}

/// A press waiting for its release.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    at: Pos2,
    time: f64,
    /// The pointer went beyond the click slop: a drag, not a click.
    moved: bool,
}

/// One slider's pointer gesture, kept across frames and fed every input in
/// the order it arrives, so frames can split or coalesce events freely: a
/// press held, then its release; clicks counted the Windows way (§3.18),
/// the second click within 0.5 s and 6 pt of the first, with nothing
/// between, being a double click and the next starting over.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gesture {
    pending: Option<Pending>,
    /// The last click, while it can still begin a double click.
    last_click: Option<(f64, Pos2)>,
}

impl Gesture {
    /// Feed `input`, arriving at `time`; push what it does to `out`.
    pub fn feed(&mut self, time: f64, input: Input, out: &mut Vec<Effect>) {
        match input {
            Input::Press(at) => {
                self.pending = Some(Pending { at, time, moved: false });
                out.push(Effect::Follow(at.x));
            }
            Input::Move(to) => {
                if let Some(pending) = &mut self.pending {
                    out.push(Effect::Follow(to.x));
                    if pending.at.distance(to) > DOUBLE_CLICK_DISTANCE {
                        pending.moved = true;
                        self.last_click = None;
                    }
                }
            }
            Input::Release(at) => {
                let Some(pending) = self.pending.take() else { return };
                out.push(Effect::Follow(at.x));
                let click =
                    !pending.moved && pending.at.distance(at) <= DOUBLE_CLICK_DISTANCE && time - pending.time <= LONG_PRESS;
                if !click {
                    self.last_click = None;
                    return;
                }
                match self.last_click {
                    Some((last, from)) if time - last <= DOUBLE_CLICK && from.distance(at) <= DOUBLE_CLICK_DISTANCE => {
                        self.last_click = None;
                        out.push(Effect::Reset);
                    }
                    _ => self.last_click = Some((time, at)),
                }
            }
            Input::Interrupt => self.last_click = None,
        }
    }
}

impl Widget for Slider<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let Self { value, range, gradient, reset, wheel_step, step, label } = self;
        let width = ui.available_width().max(MIN_WIDTH);
        let (rect, mut response) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click_and_drag());
        let track_x = rect.x_range().shrink(INSET);
        let before = *value;
        let clamp = |v: f64| v.clamp(*range.start(), *range.end());
        let span = Rect::from_x_y_ranges(track_x, rect.y_range());

        let step = step.unwrap_or_else(|| (range.end() - range.start()) * KEY_STEP_SHARE);
        let id = response.id;
        let enabled = ui.is_enabled();
        let focused = enabled && response.has_focus();
        if focused {
            let lock = EventFilter { horizontal_arrows: true, ..EventFilter::default() };
            ui.memory_mut(|m| m.set_focus_lock_filter(id, lock));
        }
        // Every input this frame, in the order it arrived, through one
        // gesture kept across frames. Each changes the value at once: the
        // knob follows a press, a drag and a release; a double click resets;
        // a wheel notch, a key or an AccessKit step or value adjusts, and
        // ends any double click in progress. A disabled slider takes none.
        let mut gesture: Gesture = ui.data(|d| d.get_temp(id)).unwrap_or_default();
        if !enabled {
            // Disabled mid-gesture: drop the press and the last click, so
            // nothing left over follows the pointer once enabled again.
            gesture = Gesture::default();
        }
        let wheel = wheel_step.filter(|_| enabled);
        let hovered = response.hovered();
        // A press counts only where the slider is visible (its clip, as in a
        // scroll area) and where egui's own hit test makes this slider the
        // click target, not a control drawn over it: being the drag
        // candidate under a button is not enough.
        let visible = rect.intersect(ui.clip_rect());
        let click_target = ui.ctx().viewport(|v| v.hits.click.is_some_and(|w| w.id == id));
        let owned = click_target || response.clicked();
        let layer = ui.layer_id();
        let ctx = ui.ctx().clone();
        let over_us = |pos: Pos2| owned && visible.contains(pos) && ctx.layer_id_at(pos).is_none_or(|l| l == layer);
        let mut stepped_wheel = false;
        let (time, events, shift) = ui.input(|i| (i.time, i.events.clone(), i.modifiers.shift));
        let mut effects = Vec::new();
        for event in &events {
            if !enabled {
                break;
            }
            let adjust = |amount: f64, effects: &mut Vec<Effect>, gesture: &mut Gesture, value: &mut f64| {
                gesture.feed(time, Input::Interrupt, effects);
                *value = clamp(*value + amount);
            };
            match event {
                Event::PointerMoved(to) => gesture.feed(time, Input::Move(*to), &mut effects),
                Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } if over_us(*pos) => {
                    gesture.feed(time, Input::Press(*pos), &mut effects);
                }
                Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, .. } => {
                    gesture.feed(time, Input::Release(*pos), &mut effects);
                }
                Event::MouseWheel { delta, .. } if hovered && delta.y != 0.0 => {
                    let notch = f64::from(delta.y.signum()) * if shift { 10.0 } else { 1.0 };
                    stepped_wheel |= wheel.is_some();
                    adjust(wheel.map_or(0.0, |per| notch * per), &mut effects, &mut gesture, value);
                }
                Event::Key { key: key @ (Key::ArrowLeft | Key::ArrowRight), pressed: true, modifiers, .. } if focused => {
                    let size = if modifiers.shift { BIG_STEP } else { 1.0 };
                    let sign = if *key == Key::ArrowRight { 1.0 } else { -1.0 };
                    adjust(sign * size * step, &mut effects, &mut gesture, value);
                }
                Event::AccessKitActionRequest(request) if request.target_node == id.accesskit_id() => match request.action {
                    Action::Increment => adjust(step, &mut effects, &mut gesture, value),
                    Action::Decrement => adjust(-step, &mut effects, &mut gesture, value),
                    Action::SetValue => {
                        if let Some(ActionData::NumericValue(target)) = request.data
                            && target.is_finite()
                        {
                            gesture.feed(time, Input::Interrupt, &mut effects);
                            *value = clamp(target);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
            // This event's effects, now, before the next event's.
            for effect in effects.drain(..) {
                match effect {
                    Effect::Follow(x) => *value = value_at(span, x, &range),
                    Effect::Reset => {
                        if let Some(reset) = reset {
                            *value = clamp(reset);
                        }
                    }
                }
            }
        }
        ui.data_mut(|d| d.insert_temp(id, gesture));
        if stepped_wheel {
            // The slider took the wheel: the area around it does not scroll.
            ui.input_mut(|i| {
                i.events.retain(|e| !matches!(e, Event::MouseWheel { .. }));
                i.smooth_scroll_delta = Vec2::ZERO;
            });
        }
        if *value != before {
            response.mark_changed();
        }
        response.widget_info(|| WidgetInfo::slider(enabled, *value, &label));
        let current = *value;
        ui.ctx().accesskit_node_builder(id, |builder| {
            builder.set_min_numeric_value(*range.start());
            builder.set_max_numeric_value(*range.end());
            builder.set_numeric_value_step(step);
            if !enabled {
                return;
            }
            builder.add_action(Action::SetValue);
            if current < *range.end() {
                builder.add_action(Action::Increment);
            }
            if current > *range.start() {
                builder.add_action(Action::Decrement);
            }
        });

        if ui.is_rect_visible(rect) {
            let chrome = Chrome::of(ui.ctx());
            let p = &chrome.palette;
            let painter = ui.painter();
            let ppp = ui.pixels_per_point();
            let cy = rect.center().y;
            let knob_x = x_of(span, *value, &range);
            match &gradient {
                Some(stops) => {
                    let track = Rect::from_x_y_ranges(track_x, cy - GRADIENT_TRACK / 2.0..=cy + GRADIENT_TRACK / 2.0);
                    painter.add(Shape::mesh(gradient_mesh(track.round_to_pixels(ppp), stops)));
                }
                None => {
                    let track = Rect::from_x_y_ranges(track_x, cy - PLAIN_TRACK / 2.0..=cy + PLAIN_TRACK / 2.0);
                    painter.rect_filled(track, TRACK_RADIUS, p.field_border);
                    let fill = match chrome.style.slider_fill {
                        SliderFill::Accent => p.accent,
                        SliderFill::TextDim => p.text_dim,
                    };
                    let filled = Rect::from_min_max(track.min, pos2(knob_x, track.max.y));
                    painter.rect_filled(filled, TRACK_RADIUS, fill);
                }
            }
            let centre = pos2(knob_x, cy);
            if chrome.style.knob == Knob::Block {
                let knob = Rect::from_center_size(centre, KNOB_CLASSIC).round_to_pixels(ppp);
                painter.rect_filled(knob, 0.0, p.card);
                chrome::bevel(painter, knob, true, p);
            } else {
                let (radius, fill, ring) = if chrome.style.knob == Knob::Ringed {
                    (KNOB_PRO, KNOB_PRO_FILL, Some(KNOB_PRO_RING))
                } else {
                    (KNOB_STUDIO, KNOB_STUDIO_FILL, None)
                };
                if response.hovered() || response.dragged() {
                    painter.circle_stroke(centre, HALO_RADIUS, Stroke::new(HALO_WIDTH, p.accent_soft));
                }
                painter.circle_filled(centre + vec2(0.0, 1.0), radius + 0.5, SHADOW);
                painter.circle_filled(centre, radius, fill);
                if let Some(ring) = ring {
                    painter.circle_stroke(centre, radius, Stroke::new(1.0, ring));
                }
            }
        }
        response
    }
}

/// A horizontal gradient through evenly spaced `stops` over `rect`.
fn gradient_mesh(rect: Rect, stops: &[Color32]) -> Mesh {
    let mut mesh = Mesh::default();
    let n = stops.len();
    if n == 0 {
        return mesh;
    }
    for (i, colour) in stops.iter().enumerate() {
        let t = if n == 1 { 0.0 } else { i as f32 / (n - 1) as f32 };
        let x = egui::lerp(rect.x_range(), t);
        mesh.colored_vertex(pos2(x, rect.top()), *colour);
        mesh.colored_vertex(pos2(x, rect.bottom()), *colour);
        if i > 0 {
            let at = (2 * i) as u32;
            mesh.add_triangle(at - 2, at - 1, at);
            mesh.add_triangle(at - 1, at, at + 1);
        }
    }
    mesh
}

/// The slider row: `label` and a value field on one line, the slider below.
/// Returns the slider's response, changed when either part changed the value.
pub fn row(ui: &mut Ui, label: &str, value: &mut f64, range: RangeInclusive<f64>, unit: Option<&str>) -> Response {
    let dim = Chrome::of(ui.ctx()).palette.text_dim;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(dim).text_style(TextStyle::Body));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut field = ValueField::new(&mut *value, range.clone()).width(field::VALUE_WIDTH);
            if let Some(unit) = unit {
                field = field.unit(unit);
            }
            changed |= ui.add(field).changed();
        });
    });
    let mut response = ui.add(Slider::new(value, range).label(label));
    ui.add_space(ROW_GAP);
    if changed {
        response.mark_changed();
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointer_maps_onto_the_range_and_back() {
        let track = Rect::from_min_max(pos2(7.0, 0.0), pos2(107.0, 18.0));
        assert_eq!(value_at(track, 57.0, &(0.0..=200.0)), 100.0);
        assert_eq!(value_at(track, -50.0, &(0.0..=200.0)), 0.0);
        assert_eq!(x_of(track, 50.0, &(0.0..=200.0)), 32.0);
    }

    const A: Pos2 = pos2(20.0, 9.0);
    const B: Pos2 = pos2(160.0, 9.0);

    /// `(time, input)` in order: what a gesture from rest does.
    fn run(inputs: &[(f64, Input)]) -> Vec<Effect> {
        let mut gesture = Gesture::default();
        let mut out = Vec::new();
        for (time, input) in inputs {
            gesture.feed(*time, *input, &mut out);
        }
        out
    }

    /// A press and release at `at`, at `time`.
    fn click(time: f64, at: Pos2) -> [(f64, Input); 2] {
        [(time, Input::Press(at)), (time, Input::Release(at))]
    }

    fn resets(effects: &[Effect]) -> usize {
        effects.iter().filter(|e| **e == Effect::Reset).count()
    }

    #[test]
    fn a_press_in_one_frame_and_its_release_in_the_next_is_a_click() {
        let inputs = [(0.0, Input::Press(A)), (0.1, Input::Release(A))];
        assert_eq!(run(&inputs), [Effect::Follow(A.x), Effect::Follow(A.x)]);
        let double = [inputs.as_slice(), &click(0.2, A)].concat();
        assert_eq!(resets(&run(&double)), 1, "and pairs with the next click");
    }

    /// sol confirmation 1: frame A has the first press; frame B its release
    /// and then a whole second click. The release ends the first click, so
    /// the second is a double click.
    #[test]
    fn a_click_spanning_frames_pairs_with_a_whole_click_after_it() {
        let inputs = [vec![(0.0, Input::Press(A)), (0.1, Input::Release(A))], click(0.1, A).to_vec()].concat();
        let effects = run(&inputs);
        assert_eq!(effects.last(), Some(&Effect::Reset), "{effects:?}");
        assert_eq!(resets(&effects), 1);
    }

    /// sol confirmation 2: one click, then two coalesced: click 2 resets,
    /// click 3 is kept (its own place, its own tracking), so a fourth pairs
    /// with it.
    #[test]
    fn coalesced_clicks_after_a_click_each_count_and_a_fourth_pairs() {
        let three = [click(0.0, A).to_vec(), click(0.2, A).to_vec(), click(0.2, A).to_vec()].concat();
        let effects = run(&three);
        assert_eq!(resets(&effects), 1, "click 2 resets");
        let after_reset = effects.iter().position(|e| *e == Effect::Reset).unwrap();
        assert!(effects[after_reset + 1..].contains(&Effect::Follow(A.x)), "click 3 still moves the knob after the reset");
        assert_eq!(effects.last(), Some(&Effect::Follow(A.x)), "click 3 is not overwritten by a late reset");
        let four = [three, click(0.4, A).to_vec()].concat();
        assert_eq!(resets(&run(&four)), 2, "the fourth pairs with the third");
    }

    #[test]
    fn distant_clicks_coalesced_in_one_frame_only_move_the_knob() {
        let effects = run(&[click(0.0, A), click(0.0, B)].concat());
        assert_eq!(resets(&effects), 0);
        assert_eq!(effects.last(), Some(&Effect::Follow(B.x)), "the value is the second click's");
    }

    #[test]
    fn a_wheel_turn_between_clicks_is_no_double_click() {
        let effects = run(&[click(0.0, A).to_vec(), vec![(0.1, Input::Interrupt)], click(0.2, A).to_vec()].concat());
        assert_eq!(resets(&effects), 0);
    }

    #[test]
    fn a_third_click_starts_over_and_slow_or_dragged_clicks_never_pair() {
        let triple = [click(0.0, A), click(0.1, A), click(0.2, A)].concat();
        assert_eq!(resets(&run(&triple)), 1, "the second resets; the third starts over");
        assert_eq!(resets(&run(&[click(0.0, A), click(0.7, A)].concat())), 0, "over 0.5 s apart");
        let dragged = [
            vec![(0.0, Input::Press(A)), (0.0, Input::Move(B)), (0.1, Input::Move(A)), (0.1, Input::Release(A))],
            click(0.2, A).to_vec(),
        ]
        .concat();
        assert_eq!(resets(&run(&dragged)), 0, "a drag is no click");
        let long = [vec![(0.0, Input::Press(A)), (1.0, Input::Release(A))], click(1.1, A).to_vec()].concat();
        assert_eq!(resets(&run(&long)), 0, "a long press is no click");
    }
}
