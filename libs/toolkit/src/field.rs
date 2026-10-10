// SPDX-License-Identifier: MIT OR Apache-2.0
//! Input fields (chrome specification §3.14–3.15): numeric value fields,
//! single-line text fields and the search field.
//!
//! **Value fields** are 24 pt boxes in `field` with a 1 pt `field_border`
//! inside (Classic: a sunken bevel instead), the number in JetBrains Mono 12
//! right-aligned 4 pt in, and an optional unit in JetBrains Mono 11
//! `text_faint` in a 16 pt reserve at the right. At rest the box scrubs:
//! dragging moves the value 0.5 per point (0.01 for ranges of 10 or less),
//! and a click opens the inner editor, which has no fill or stroke of its
//! own. Up and Down step by 1 (x10 with Shift, x0.1 with Ctrl/Cmd) after
//! rounding to the step, so 55.4 + 1 is 56. A plain number applies as it is
//! typed; arithmetic (`+ - * /`, `− × ÷`, parentheses) applies on Enter, Tab
//! or click-away, and Escape abandons the edit.
//!
//! **Text fields** are egui's single-line text edit, which the chrome style
//! already draws to §3.15: `field`, `radius_sm`, a 1 pt `field_border` at
//! rest and on hover, and a 1 pt `accent_text` (the selection stroke) while
//! focused. The search field adds a leading Lucide search icon.

use crate::button::paint_icon;
use crate::chrome::{self, Chrome};
use crate::icons::Icon;
use egui::emath::GuiRounding;
use egui::{
    Align, EventFilter, FontFamily, FontId, Key, Margin, Rect, Response, Sense, Stroke, StrokeKind,
    TextEdit, Ui, UiBuilder, Widget, WidgetInfo, pos2, vec2,
};
use std::ops::RangeInclusive;

/// Field height (§3.14) and the default width of a value field (the slider
/// row's field, §3.18).
pub const HEIGHT: f32 = 24.0;
pub const VALUE_WIDTH: f32 = 74.0;

/// Number and unit sizes, JetBrains Mono (§2.1).
const NUMBER_SIZE: f32 = 12.0;
const UNIT_SIZE: f32 = 11.0;

/// The number's inset (§3.14).
const INSET: Margin = Margin::symmetric(4, 2);

/// The unit's reserve at the right, and its right inset inside it (§3.14).
const UNIT_RESERVE: f32 = 16.0;
const UNIT_INSET: f32 = 6.0;
const UNIT_GAP: f32 = 2.0;

/// Scrub rates per point of drag (§3.14).
const SCRUB: f64 = 0.5;
const FINE_SCRUB: f64 = 0.01;

/// A range this wide or narrower is fine: two decimals, the fine scrub.
const FINE_SPAN: f64 = 10.0;

/// The search field's icon size, and the room it takes before the text.
/// Undetermined by the specification (§5.4): the icon matches the
/// title-bar link's 14 pt, with the field's own 4 pt inset either side.
const SEARCH_ICON: f32 = 14.0;
const SEARCH_LEAD: i8 = 4 + 14 + 4;

/// Whether `range` takes two decimals and the fine scrub.
pub fn is_fine(range: &RangeInclusive<f64>) -> bool {
    range.end() - range.start() <= FINE_SPAN
}

/// `value` as a value field shows it: two decimals in a fine range, else
/// no decimals for a whole number and at most two otherwise ("100",
/// "12.5", never "100.0").
pub fn format(value: f64, fine: bool) -> String {
    if fine {
        return format!("{value:.2}");
    }
    if value.fract() == 0.0 {
        return format!("{value:.0}");
    }
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// `value` stepped by `step` in `direction` (+1 or -1), rounded to the
/// step first (§3.14: 55.4 + 1 = 56).
pub fn step(value: f64, step: f64, direction: f64) -> f64 {
    // In whole steps, so float noise (0.1 * 3) never accumulates.
    ((value / step).round() + direction) * step
}

/// `text` as a plain finite number ("NaN" and "inf" parse as `f64` but are
/// refused, keeping the previous value).
pub fn plain(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The value of an arithmetic expression: numbers, `+ - * /` and their
/// typographic forms `− × ÷`, unary minus and parentheses, with the usual
/// precedence. `None` when it does not parse or divides by zero.
pub fn evaluate(text: &str) -> Option<f64> {
    let tokens: Vec<char> = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '−' => '-',
            '×' => '*',
            '÷' => '/',
            other => other,
        })
        .collect();
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
    };
    let value = parser.sum()?;
    (parser.at == tokens.len() && value.is_finite()).then_some(value)
}

struct Parser<'a> {
    tokens: &'a [char],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.tokens.get(self.at).copied()
    }

    fn sum(&mut self) -> Option<f64> {
        let mut value = self.product()?;
        while let Some(op @ ('+' | '-')) = self.peek() {
            self.at += 1;
            let rhs = self.product()?;
            value = if op == '+' { value + rhs } else { value - rhs };
        }
        Some(value)
    }

    fn product(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        while let Some(op @ ('*' | '/')) = self.peek() {
            self.at += 1;
            let rhs = self.unary()?;
            if op == '/' && rhs == 0.0 {
                return None;
            }
            value = if op == '*' { value * rhs } else { value / rhs };
        }
        Some(value)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek()? {
            '-' => {
                self.at += 1;
                self.unary().map(|v| -v)
            }
            '+' => {
                self.at += 1;
                self.unary()
            }
            '(' => {
                self.at += 1;
                let value = self.sum()?;
                if self.peek() != Some(')') {
                    return None;
                }
                self.at += 1;
                Some(value)
            }
            _ => {
                let start = self.at;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
                    self.at += 1;
                }
                self.tokens[start..self.at]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .ok()
            }
        }
    }
}

/// What a value field remembers between frames.
#[derive(Clone, Debug, Default)]
struct Edit {
    /// The editor's text while it is open.
    text: Option<String>,
    /// The value when the current drag began.
    drag_from: f64,
}

/// A numeric value field.
#[must_use = "add it with `ui.add`"]
pub struct ValueField<'a> {
    value: &'a mut f64,
    range: RangeInclusive<f64>,
    unit: Option<&'a str>,
    width: f32,
}

impl<'a> ValueField<'a> {
    pub fn new(value: &'a mut f64, range: RangeInclusive<f64>) -> Self {
        Self {
            value,
            range,
            unit: None,
            width: VALUE_WIDTH,
        }
    }

    /// A unit suffix such as "%" or "px".
    pub fn unit(mut self, unit: &'a str) -> Self {
        self.unit = Some(unit);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

impl Widget for ValueField<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let Self {
            value,
            range,
            unit,
            width,
        } = self;
        let chrome = Chrome::of(ui.ctx());
        let p = chrome.palette;
        let fine = is_fine(&range);
        // One id for the resting box and its editor, as egui's DragValue
        // has: the field edits while it has keyboard focus.
        let id = ui.next_auto_id();
        let (_, rect) = ui.allocate_space(vec2(width, HEIGHT));
        let editing = ui.memory(|m| m.has_focus(id));
        let mut edit: Edit = ui.data(|d| d.get_temp(id)).unwrap_or_default();
        let before = *value;
        let clamp = |v: f64| v.clamp(*range.start(), *range.end());

        // Up and Down step while editing.
        if editing {
            let lock = EventFilter {
                vertical_arrows: true,
                ..EventFilter::default()
            };
            ui.memory_mut(|m| m.set_focus_lock_filter(id, lock));
            let steps = ui.input_mut(|i| {
                let mut steps = Vec::new();
                i.events.retain(|event| {
                    let egui::Event::Key {
                        key: key @ (Key::ArrowUp | Key::ArrowDown),
                        pressed: true,
                        modifiers,
                        ..
                    } = event
                    else {
                        return true;
                    };
                    let size = if modifiers.shift {
                        10.0
                    } else if modifiers.command {
                        0.1
                    } else {
                        1.0
                    };
                    steps.push((size, if *key == Key::ArrowUp { 1.0 } else { -1.0 }));
                    false
                });
                steps
            });
            for (size, direction) in steps {
                *value = clamp(step(*value, size, direction));
                edit.text = Some(format(*value, fine));
            }
        }

        let number_font = FontId::new(NUMBER_SIZE, FontFamily::Monospace);
        let unit_font = FontId::new(UNIT_SIZE, FontFamily::Monospace);
        let unit_galley = unit.map(|u| {
            ui.painter()
                .layout_no_wrap(u.to_owned(), unit_font, p.text_faint)
        });
        // The 16 pt reserve holds a one-character unit; a longer one ("px")
        // widens it to keep UNIT_GAP before the unit (undetermined, §5.4).
        let reserve = unit_galley.as_ref().map_or(0.0, |g| {
            UNIT_RESERVE.max(UNIT_INSET + g.size().x + UNIT_GAP)
        });
        let inner = Rect::from_min_max(rect.min, pos2(rect.right() - reserve, rect.bottom()));
        let painter = ui.painter_at(rect.expand(1.0));
        painter.rect_filled(rect, chrome.metrics.radius_sm, p.field);
        // Both modes take the same ids from the parent (the box's auto id and
        // one child cell), so opening the editor never renumbers the widgets
        // drawn after the field.
        let layout = egui::Layout::centered_and_justified(egui::Direction::TopDown);
        let mut cell = ui.new_child(
            UiBuilder::new()
                .id(id.with("cell"))
                .max_rect(inner)
                .layout(layout),
        );
        let escaped = ui.input(|i| i.key_pressed(Key::Escape));
        let mut response;
        if editing {
            let mut text = edit.text.take().unwrap_or_else(|| format(*value, fine));
            let output = TextEdit::singleline(&mut text)
                .id(id)
                .frame(egui::Frame::NONE)
                .margin(INSET)
                .font(number_font.clone())
                .horizontal_align(Align::RIGHT)
                .vertical_align(Align::Center)
                .desired_width(inner.width() - INSET.sum().x)
                .min_size(inner.size());
            response = cell.add(output);
            if response.changed()
                && let Some(number) = plain(&text)
            {
                *value = clamp(number);
            }
            if response.lost_focus() {
                if !escaped && let Some(result) = evaluate(&text) {
                    *value = clamp(result);
                }
            } else {
                edit.text = Some(text);
            }
        } else {
            // Focus can leave before the editor sees it go (to a widget drawn
            // earlier in the frame): what was typed still applies.
            if let Some(text) = edit.text.take()
                && !escaped
                && let Some(result) = evaluate(&text)
            {
                *value = clamp(result);
            }
            response = ui.interact(rect, id, Sense::click_and_drag());
            // Scrubbing from the resting box; a click opens the editor.
            if response.drag_started() {
                edit.drag_from = *value;
            }
            if response.dragged() {
                let travel = ui.input(|i| {
                    i.pointer
                        .press_origin()
                        .zip(i.pointer.interact_pos())
                        .map(|(a, b)| b.x - a.x)
                });
                let rate = if fine { FINE_SCRUB } else { SCRUB };
                *value = clamp(
                    ((edit.drag_from + f64::from(travel.unwrap_or(0.0)) * rate) / rate).round()
                        * rate,
                );
            }
            if response.clicked() {
                ui.memory_mut(|m| m.request_focus(id));
            }
            let galley = painter.layout_no_wrap(format(*value, fine), number_font, p.text);
            let x = inner.right() - f32::from(INSET.right) - galley.size().x;
            let at = pos2(x, rect.center().y - galley.size().y / 2.0)
                .round_to_pixels(ui.pixels_per_point());
            painter.galley(at, galley, p.text);
            response = response.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        }
        if let Some(galley) = unit_galley {
            let x = rect.right() - UNIT_INSET - galley.size().x;
            let at = pos2(x, rect.center().y - galley.size().y / 2.0)
                .round_to_pixels(ui.pixels_per_point());
            painter.galley(at, galley, p.text_faint);
        }
        if chrome.style.bevels {
            chrome::bevel(&painter, rect, false, &p);
        } else if editing || chrome.style.outlines {
            // An open editor shows focus as a text field does (§3.15).
            let border = if editing {
                p.accent_text
            } else {
                p.field_border
            };
            painter.rect_stroke(
                rect,
                chrome.metrics.radius_sm,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );
        }
        ui.data_mut(|d| d.insert_temp(id, edit));
        if *value != before {
            response.mark_changed();
        }
        if !editing {
            response.widget_info(|| WidgetInfo::drag_value(ui.is_enabled(), *value));
        }
        response
    }
}

/// A single-line text field (§3.15), `width` wide and one control high.
pub fn text(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    ui.add(
        TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(width)
            .min_size(vec2(0.0, HEIGHT))
            .vertical_align(Align::Center),
    )
}

/// A search field: a text field led by the search icon in `text_faint`.
pub fn search(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    let margin = Margin {
        left: SEARCH_LEAD,
        right: INSET.right,
        top: INSET.top,
        bottom: INSET.bottom,
    };
    let edit = TextEdit::singleline(text)
        .hint_text(hint)
        // egui's desired width is the whole field, margins included.
        .desired_width(width)
        .margin(margin)
        .min_size(vec2(0.0, HEIGHT))
        .vertical_align(Align::Center);
    let response = ui.add(edit);
    let tint = Chrome::of(ui.ctx()).palette.text_faint;
    let centre = pos2(
        response.rect.left() + f32::from(INSET.left) + SEARCH_ICON / 2.0,
        response.rect.center().y,
    );
    paint_icon(ui, Icon::Search, centre, SEARCH_ICON, tint);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_show_without_needless_decimals() {
        assert_eq!(format(100.0, false), "100");
        assert_eq!(format(12.5, false), "12.5");
        assert_eq!(format(12.25, false), "12.25");
        assert_eq!(format(0.5, true), "0.50");
        assert!(is_fine(&(0.0..=10.0)) && !is_fine(&(0.0..=100.0)));
    }

    #[test]
    fn steps_round_to_the_step_first() {
        assert_eq!(step(55.4, 1.0, 1.0), 56.0);
        assert_eq!(step(55.6, 1.0, -1.0), 55.0);
        assert_eq!(step(55.0, 1.0, 1.0), 56.0);
        assert_eq!(step(50.0, 10.0, -1.0), 40.0);
        assert!((step(0.2, 0.1, 1.0) - 0.3).abs() < 1e-9);
    }

    #[test]
    fn arithmetic_takes_both_spellings_and_precedence() {
        assert_eq!(evaluate("100"), Some(100.0));
        assert_eq!(evaluate("10 + 5 × 2"), Some(20.0));
        assert_eq!(evaluate("(10 + 5) * 2"), Some(30.0));
        assert_eq!(evaluate("90 ÷ 3 − 5"), Some(25.0));
        assert_eq!(evaluate("-4/2"), Some(-2.0));
        assert_eq!(evaluate("1/0"), None);
        assert_eq!(evaluate("2 +"), None);
        assert_eq!(evaluate("abc"), None);
    }

    #[test]
    fn non_finite_plain_input_is_refused() {
        assert_eq!(plain(" 12.5 "), Some(12.5));
        for text in ["NaN", "nan", "inf", "-infinity", "1e999"] {
            assert_eq!(plain(text), None, "{text}");
        }
        assert_eq!(evaluate("NaN"), None);
    }
}
