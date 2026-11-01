// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! One setting row and its control, in COSMIC's geometry (spec 4,
//! "Setting row" and "Control set"): padding 4 by 16, content at least 32
//! tall, the control right-aligned 16 from the card edge, or under the
//! title for the wide ones (accent, mode, segmented, image choices).
//! Every control is named for agents with its row's title, so
//! `prefs.ui.click {label}` reaches it as a hand would.
use super::{Look, description, lit, named, set, title};
use crate::view::UiEvent;
use crate::{label, label_with};
use design::cosmic_tables::{DARK_PALETTE, LIGHT_PALETTE, SWATCH_NAMES};
use egui::{
    Align, Color32, Layout, Margin, Rect, Response, RichText, Role, Sense, Stroke, StrokeKind, Ui,
    Vec2, Widget, WidgetInfo, pos2, vec2,
};
use preferences::Engine;
use preferences::live::ListEdit;
use preferences::pages::{self, ChoiceStyle, Control, Lit, Opt, Page, Row};
use serde_json::{Value, json};
use toolkit::{Icon, icons};

/// The resolved light or dark mode the store's values give.
pub fn resolved_dark(engine: &Engine) -> bool {
    let look = <settings::view::Look as settings::view::View>::of(&engine.store.values);
    matches!(look.mode().0, settings::resolve::values::Mode::Dark)
}

/// COSMIC's standard button: 24 tall (compact), pill, button colour.
pub fn standard_button<'a>(text: &'a str, look: &'a Look) -> impl Widget + 'a {
    named_button(text, text, look)
}

/// A standard button named `name` for agents (one of many `Add`s).
pub fn named_button<'a>(text: &'a str, name: &'a str, look: &'a Look) -> impl Widget + 'a {
    move |ui: &mut Ui| {
        let height = match look.density {
            design::cosmic_tables::Density::Compact => 24.0,
            design::cosmic_tables::Density::Standard => 32.0,
            design::cosmic_tables::Density::Spacious => 48.0,
        };
        let galley = ui.painter().layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(14.0),
            look.text,
        );
        let size = vec2(galley.size().x + 2.0 * 12.0, height);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let fill = if !ui.is_enabled() {
            look.button.gamma_multiply(0.5)
        } else if response.hovered() {
            look.button_hover
        } else {
            look.button
        };
        ui.painter()
            .rect_filled(rect, Look::radius(look.r_xl), fill);
        let ink = if ui.is_enabled() {
            look.text
        } else {
            look.text.gamma_multiply(0.65)
        };
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, ink);
        focus_ring(ui, &response, rect, look.r_xl, look);
        response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), name));
        response
    }
}

fn focus_ring(ui: &Ui, response: &Response, rect: Rect, radius: f32, look: &Look) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(1.0),
            Look::radius(radius),
            Stroke::new(1.0, look.accent),
            StrokeKind::Outside,
        );
    }
}

/// COSMIC's toggle: a 48 by 24 pill, the knob 20, accent when on.
fn switch(ui: &mut Ui, on: bool, look: &Look, name: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(48.0, 24.0), Sense::click());
    let alpha = if ui.is_enabled() { 1.0 } else { 0.5 };
    let track = if on { look.accent } else { look.track_off };
    ui.painter()
        .rect_filled(rect, Look::radius(look.r_xl), track.gamma_multiply(alpha));
    let x = if on {
        rect.max.x - 12.0
    } else {
        rect.min.x + 12.0
    };
    ui.painter().circle_filled(
        pos2(x, rect.center().y),
        10.0,
        look.knob.gamma_multiply(alpha),
    );
    if response.hovered() && ui.is_enabled() {
        ui.painter().rect_filled(
            rect,
            Look::radius(look.r_xl),
            Color32::from_white_alpha(if look.dark { 26 } else { 0 }),
        );
    }
    focus_ring(ui, &response, rect, look.r_xl, look);
    response.widget_info(|| WidgetInfo::selected(Role::Switch, ui.is_enabled(), on, name));
    response
}

/// The label column, laid out before it is drawn so the row knows its
/// height: the title, the description, and the row's own reason when it
/// alone is off.
struct Labels {
    title: std::sync::Arc<egui::Galley>,
    description: Option<std::sync::Arc<egui::Galley>>,
    reason: Option<std::sync::Arc<egui::Galley>>,
}

impl Labels {
    fn new(
        ui: &Ui,
        look: &Look,
        title: &str,
        description: Option<&str>,
        reason: Option<&str>,
        width: f32,
    ) -> Self {
        let lay = |text: &str, size: f32, width: f32| {
            ui.painter().layout(
                text.to_owned(),
                egui::FontId::proportional(size),
                look.text,
                width.max(1.0),
            )
        };
        Self {
            title: lay(title, 14.0, width),
            description: description
                .filter(|d| !d.is_empty())
                .map(|d| lay(d, 12.0, width)),
            reason: reason.map(|r| lay(r, 12.0, width - 16.0)),
        }
    }

    fn height(&self) -> f32 {
        let gap = 2.0;
        self.title.size().y
            + self.description.as_ref().map_or(0.0, |g| gap + g.size().y)
            + self.reason.as_ref().map_or(0.0, |g| gap + g.size().y)
    }

    fn paint(self, ui: &Ui, look: &Look, top_left: egui::Pos2) {
        let mut at = top_left;
        let height = self.title.size().y;
        ui.painter().galley(at, self.title, look.text);
        at.y += height + 2.0;
        if let Some(g) = self.description {
            let height = g.size().y;
            ui.painter().galley(at, g, look.text);
            at.y += height + 2.0;
        }
        if let Some(g) = self.reason {
            toolkit::button::paint_icon(
                ui,
                Icon::CircleAlert,
                at + vec2(6.0, 8.0),
                12.0,
                look.warning,
            );
            ui.painter().galley(at + vec2(16.0, 0.0), g, look.text);
        }
    }
}

/// The width a control takes beside its labels.
fn control_width(ui: &Ui, engine: &Engine, row: &Row) -> f32 {
    match &row.control {
        Control::Toggle { .. }
        | Control::Flag { .. }
        | Control::Live { .. }
        | Control::AutoMode
        | Control::HintAccent => 48.0,
        Control::Colour { key, auto_link } => {
            if *auto_link && engine.store.text(key).as_deref() == Some("auto") {
                text_width(ui, &label("colour-auto"), 14.0) + 28.0
            } else {
                48.0
            }
        }
        Control::Spin { .. } => 96.0,
        Control::NumberEntry { .. } => 80.0,
        Control::Choice { .. } => {
            let labels: Vec<String> = engine.choices(row).into_iter().map(|o| o.1).collect();
            dropdown_width(ui, &labels)
        }
        Control::Slider { .. } | Control::Steps { .. } => 360.0,
        Control::Pick { key, .. } => {
            let text = engine
                .store
                .text(key)
                .unwrap_or_else(|| label("pick-default"));
            text_width(ui, &text, 14.0) + 28.0
        }
        Control::Page { .. } | Control::Drawer { .. } => 16.0,
        Control::Reset { .. } => 160.0,
        Control::Outputs { .. } | Control::Preview { .. } | Control::Gallery { .. } => 0.0,
        Control::Info => row
            .value_from
            .and_then(|src| engine.live_value(src))
            .map_or(0.0, |v| text_width(ui, v, 14.0)),
        Control::Accent
        | Control::Scheme
        | Control::Mode
        | Control::Ordered { .. }
        | Control::Append { .. } => 0.0,
    }
}

fn text_width(ui: &Ui, text: &str, size: f32) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(size),
            Color32::WHITE,
        )
        .size()
        .x
}

/// A row dropdown's width: its longest value, padding 8 and the caret.
fn dropdown_width(ui: &Ui, labels: &[String]) -> f32 {
    labels
        .iter()
        .map(|l| text_width(ui, l, 14.0))
        .fold(0.0_f32, f32::max)
        + 8.0
        + 16.0
        + 9.0
}

/// Whether a control sits under its title rather than beside it.
fn stacked(control: &Control) -> bool {
    matches!(
        control,
        Control::Accent
            | Control::Scheme
            | Control::Mode
            | Control::Outputs { .. }
            | Control::Preview { .. }
            | Control::Gallery { .. }
            | Control::Choice {
                style: ChoiceStyle::Segmented | ChoiceStyle::Images,
                ..
            }
    )
}

/// Whether the whole row is a list button (go-next).
fn goes(control: &Control) -> bool {
    matches!(
        control,
        Control::Page { .. } | Control::Drawer { .. } | Control::Pick { .. }
    )
}

/// One row: labels, control, the disabled line, and the landing ring.
/// `quiet` drops the row's own reason: its section already said it.
#[allow(clippy::too_many_arguments)]
pub fn row(
    ui: &mut Ui,
    engine: &Engine,
    page: &Page,
    row: &Row,
    quiet: bool,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let enabled = engine.row_blocked(page, row).is_none();
    let name = title(page.id, row.id);
    let desc = match row.control {
        Control::Info => description(page.id, row.id).or_else(|| Some(String::new())),
        _ => description(page.id, row.id),
    };
    let reason = engine.row_reason(page, row).filter(|_| !quiet);
    let pad = look.row_pad();
    let response = egui::Frame::NONE
        .inner_margin(Margin {
            left: pad as i8,
            right: pad as i8,
            top: 4,
            bottom: 4,
        })
        .show(ui, |ui| {
            let width = ui.available_width();
            ui.set_width(width);
            let cw = control_width(ui, engine, row);
            // The flex rule: below 200 for the labels the control stacks
            // under them (spec 4, "Setting row").
            let beside = !stacked(&row.control) && width - cw - look.xs >= 200.0;
            // The mode picker carries its own captions (spec 4).
            let titled = !matches!(row.control, Control::Mode) || reason.is_some();
            if beside {
                let labels = Labels::new(
                    ui,
                    look,
                    &name,
                    desc.as_deref(),
                    reason.as_deref(),
                    width - cw - look.xs,
                );
                let label_height = labels.height();
                let height = label_height.max(32.0);
                let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
                labels.paint(
                    ui,
                    look,
                    pos2(rect.min.x, rect.center().y - label_height / 2.0),
                );
                let slot = Rect::from_min_max(
                    pos2(rect.max.x - cw.max(16.0) - look.xs, rect.min.y),
                    rect.max,
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(slot)
                        .layout(Layout::right_to_left(Align::Center)),
                    |ui| {
                        ui.add_enabled_ui(enabled, |ui| {
                            control(ui, engine, page, row, &name, look, events)
                        });
                    },
                );
            } else {
                if titled {
                    let labels =
                        Labels::new(ui, look, &name, desc.as_deref(), reason.as_deref(), width);
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(width, labels.height()), Sense::hover());
                    labels.paint(ui, look, rect.min);
                    ui.add_space(look.xs);
                }
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(enabled, |ui| {
                        control(ui, engine, page, row, &name, look, events)
                    });
                });
            }
        })
        .response;
    if goes(&row.control) {
        let click = ui.interact(
            response.rect,
            ui.scope_id().with(("go", row.id)),
            Sense::click(),
        );
        let click = named(click, Role::Link, enabled, &name);
        if click.clicked() && enabled {
            go(row, events);
        }
        if click.hovered() && enabled {
            ui.painter().rect_filled(
                response.rect,
                0.0,
                Color32::from_white_alpha(if look.dark { 8 } else { 0 }),
            );
        }
        focus_ring(ui, &click, response.rect, look.r_s, look);
    }
    if engine.nav.focus == Some((page.id, row.id)) {
        response.scroll_to_me(Some(Align::Center));
        ui.painter().rect_stroke(
            response.rect.shrink(1.0),
            Look::radius(look.r_s),
            Stroke::new(1.0, look.accent),
            StrokeKind::Inside,
        );
        events.push(UiEvent::FocusDone);
    }
}

/// A row drawn on the page without a card or labels (a plain section):
/// its control alone, the full column wide.
pub fn bare(
    ui: &mut Ui,
    engine: &Engine,
    page: &Page,
    row: &Row,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let enabled = engine.row_blocked(page, row).is_none();
    let name = title(page.id, row.id);
    let response = ui
        .vertical(|ui| {
            ui.set_width(ui.available_width());
            ui.add_enabled_ui(enabled, |ui| {
                control(ui, engine, page, row, &name, look, events)
            });
        })
        .response;
    if engine.nav.focus == Some((page.id, row.id)) {
        response.scroll_to_me(Some(Align::Center));
        events.push(UiEvent::FocusDone);
    }
}

/// The display selector (spec 1 §4.1): the "All displays" heading while
/// `all` holds, else one tab per connected output when there are two or
/// more, the selected one underlined in the accent.
fn outputs(ui: &mut Ui, engine: &Engine, all: &str, look: &Look, events: &mut Vec<UiEvent>) {
    let heading = |ui: &mut Ui, text: &str| {
        let galley =
            ui.painter()
                .layout_no_wrap(text.to_owned(), super::heading(ui, 14.0), look.text);
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 21.0), Sense::hover());
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, look.text);
        response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
    };
    if engine.store.flag(all) {
        heading(ui, &label("opt-all-displays"));
        return;
    }
    let outputs: Vec<(String, String)> = engine
        .live_options("outputs")
        .iter()
        .filter_map(|(v, text)| v.as_str().map(|v| (v.to_owned(), text.clone())))
        .filter(|(v, _)| v != "all")
        .collect();
    if outputs.len() < 2 {
        return;
    }
    let selected = engine.selected_output();
    let widths: Vec<f32> = outputs
        .iter()
        .map(|(_, text)| text_width(ui, text, 14.0) + 2.0 * look.s)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + look.xs * (outputs.len() - 1) as f32;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let mut x = row.center().x - total / 2.0;
    for ((connector, text), width) in outputs.iter().zip(widths) {
        let rect = Rect::from_min_size(pos2(x, row.min.y), vec2(width, 32.0));
        x += width + look.xs;
        let on = selected.as_deref() == Some(connector.as_str());
        let response = ui.interact(
            rect,
            ui.scope_id().with(("output", connector)),
            Sense::click(),
        );
        if response.hovered() && ui.is_enabled() {
            ui.painter()
                .rect_filled(rect, Look::radius(look.r_s), look.card_hover);
        }
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(14.0),
            look.text,
        );
        if on {
            ui.painter().hline(
                rect.x_range(),
                rect.max.y - 1.0,
                Stroke::new(2.0, look.accent),
            );
        }
        response.widget_info(|| WidgetInfo::selected(Role::Tab, ui.is_enabled(), on, text));
        if response.clicked() && !on {
            events.push(UiEvent::Output(Some(connector.clone())));
        }
    }
}

fn go(row: &Row, events: &mut Vec<UiEvent>) {
    match row.control {
        Control::Page { page } => events.push(UiEvent::Page(page, None)),
        Control::Drawer { drawer } => events.push(UiEvent::Drawer(Some(drawer))),
        Control::Pick { .. } => events.push(UiEvent::Drawer(Some(row.id))),
        _ => {}
    }
}

/// One option of a radio choice: a row of its own (spec 4, "Radio").
#[allow(clippy::too_many_arguments)]
pub fn radio_row(
    ui: &mut Ui,
    engine: &Engine,
    page: &Page,
    row: &Row,
    key: &'static str,
    option: &(Value, String),
    blocked: Option<&str>,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let value = engine.store.value(key);
    let selected = value.as_ref() == Some(&option.0);
    let text = option.1.clone();
    let pad = look.row_pad() as i8;
    let response = egui::Frame::NONE
        .inner_margin(Margin {
            left: pad,
            right: pad,
            top: 4,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.set_min_height(32.0);
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add_enabled_ui(blocked.is_none(), |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
                    let c = rect.center();
                    if selected {
                        ui.painter().circle_filled(c, 8.0, look.accent);
                        ui.painter().circle_filled(c, 3.0, look.on_accent);
                    } else {
                        ui.painter().circle_filled(c, 8.0, look.field);
                        ui.painter()
                            .circle_stroke(c, 7.5, Stroke::new(1.0, look.swatch_border));
                    }
                    response.widget_info(|| {
                        WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, &text)
                    });
                    if response.clicked() && !selected {
                        set(events, key, option.0.clone());
                    }
                });
                ui.add_space(look.xs);
                // The option's one-line description under its label.
                match engine.choice_description(row, &option.0) {
                    Some(d) => {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new(&text).size(14.0).color(look.text));
                            ui.label(RichText::new(d).size(12.0).color(look.text));
                        });
                    }
                    None => {
                        ui.label(RichText::new(&text).size(14.0).color(look.text));
                    }
                }
            });
        })
        .response;
    if blocked.is_none() {
        let whole = ui.interact(
            response.rect,
            ui.scope_id().with(("radio", key, text.as_str())),
            Sense::click(),
        );
        if whole.clicked() && !selected {
            set(events, key, option.0.clone());
        }
    }
    if engine.nav.focus == Some((page.id, row.id)) && selected {
        response.scroll_to_me(Some(Align::Center));
        events.push(UiEvent::FocusDone);
    }
}

fn control(
    ui: &mut Ui,
    engine: &Engine,
    page: &Page,
    row: &Row,
    name: &str,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let store = &engine.store;
    match &row.control {
        Control::Toggle { key } => {
            let on = store.flag(key);
            if switch(ui, on, look, name).clicked() {
                set(events, key, json!(!on));
            }
        }
        Control::Flag { key, on, off } => {
            let is_on = on.is(store.value(key).as_ref());
            if switch(ui, is_on, look, name).clicked() {
                set(events, key, lit(if is_on { *off } else { *on }));
            }
        }
        Control::Live { .. } => {
            let _ = switch(ui, false, look, name);
        }
        Control::Choice {
            key,
            style: ChoiceStyle::Dropdown,
            ..
        } => {
            let value = store.value(key);
            let unlisted = value.as_ref().and_then(|v| engine.unlisted_label(row, v));
            dropdown(
                ui,
                value,
                unlisted,
                key,
                &engine.choices(row),
                name,
                look,
                events,
            )
        }
        Control::Choice {
            key,
            style: ChoiceStyle::Segmented,
            options,
        } => segmented(ui, store.value(key), key, options, look, events),
        Control::Choice {
            key,
            style: ChoiceStyle::Images,
            options,
        } => images(ui, store.value(key), key, options, look, events),
        Control::Choice { .. } => {}
        Control::Slider {
            key,
            min,
            max,
            step,
            scale,
            show_value,
            start,
            end,
        } => {
            let (min, max) = engine.bounds(key, *min, *max);
            let shown = store.number(key).filter(|v| v.is_finite()).unwrap_or(min) * scale;
            let range = (min * scale, max * scale);
            if let Some(v) = slider(
                ui,
                (page.id, row.id),
                shown,
                range,
                step * scale,
                *show_value,
                *start,
                *end,
                name,
                look,
            ) {
                let stored = v / scale;
                let stored = if *step > 0.0 {
                    (stored / step).round() * step
                } else {
                    stored
                };
                // No float drift past the step (0.15000000000000002).
                let stored = ((stored * 1e6).round() / 1e6).clamp(min, max);
                set(events, key, json!(stored));
            }
        }
        Control::Steps {
            key,
            steps,
            start,
            end,
        } => {
            let value = store.value(key);
            let steps = engine.steps(key, steps);
            if steps.is_empty() {
                return;
            }
            // A value off the steps shows as the middle one (COSMIC's Custom).
            let index = steps
                .iter()
                .position(|s| value.as_ref().and_then(Value::as_f64) == s.as_f64())
                .unwrap_or(steps.len() / 2) as f64;
            let last = (steps.len() - 1) as f64;
            if let Some(v) = slider(
                ui,
                (page.id, row.id),
                index,
                (0.0, last),
                1.0,
                false,
                Some(start),
                Some(end),
                name,
                look,
            ) {
                let i = (v.round().max(0.0) as usize).min(steps.len() - 1);
                set(events, key, steps[i].clone());
            }
        }
        Control::Spin {
            key,
            min,
            max,
            step,
        } => {
            let (min, max) = engine.bounds(key, *min as f64, *max as f64);
            let (min, max) = (min as i64, max as i64);
            let (min, max) = (&min, &max);
            let value = store
                .number(key)
                .filter(|v| v.is_finite())
                .map_or(*min, |v| (v.round() as i64).clamp(*min, *max));
            ui.spacing_mut().item_spacing.x = 4.0;
            let plus = icon_button(ui, Icon::Plus, value < *max, &format!("{name} +"), look);
            ui.label(
                RichText::new(value.to_string())
                    .size(14.0)
                    .color(look.value),
            );
            let minus = icon_button(ui, Icon::Minus, value > *min, &format!("{name} −"), look);
            if plus.clicked() {
                set(events, key, json!((value + step).min(*max)));
            }
            if minus.clicked() {
                set(events, key, json!((value - step).max(*min)));
            }
        }
        Control::NumberEntry { key, min, max } => {
            number_entry(
                ui,
                engine,
                (page.id, row.id),
                (key, *min, *max),
                name,
                events,
            );
        }
        Control::Colour { key, auto_link } => {
            let stored = store.text(key);
            if *auto_link && stored.as_deref() == Some("auto") {
                // "Auto ›": the row is a go-next to the colour drawer.
                let (rect, response) = ui.allocate_exact_size(
                    vec2(text_width(ui, &label("colour-auto"), 14.0) + 28.0, 24.0),
                    Sense::click(),
                );
                toolkit::button::paint_icon(
                    ui,
                    Icon::ChevronRight,
                    pos2(rect.max.x - 8.0, rect.center().y),
                    16.0,
                    look.icon,
                );
                ui.painter().text(
                    pos2(rect.max.x - 20.0, rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    label("colour-auto"),
                    egui::FontId::proportional(14.0),
                    look.value,
                );
                let response = named(response, Role::Link, ui.is_enabled(), name);
                if response.clicked() {
                    events.push(UiEvent::Drawer(Some(row.id)));
                }
                return;
            }
            // The effective colour: under auto (or the accent word), the
            // one the derived theme draws.
            let colour = stored
                .as_deref()
                .and_then(super::parse_hex)
                .unwrap_or(match *key {
                    "appearance.window_bg" => look.window,
                    "appearance.container_bg" => look.card,
                    "appearance.text_tint" => look.text,
                    "appearance.control_tint" => look.control,
                    _ => look.accent,
                });
            let (rect, response) = ui.allocate_exact_size(vec2(48.0, 24.0), Sense::click());
            ui.painter()
                .rect_filled(rect, Look::radius(look.r_xs), colour);
            ui.painter().rect_stroke(
                rect,
                Look::radius(look.r_xs),
                Stroke::new(1.0, look.swatch_border),
                StrokeKind::Inside,
            );
            focus_ring(ui, &response, rect, look.r_xs, look);
            let response = named(response, Role::Button, ui.is_enabled(), name);
            if response.clicked() {
                events.push(UiEvent::Drawer(Some(row.id)));
            }
        }
        Control::Accent => accent(ui, engine, row, look, events),
        Control::Scheme => scheme(ui, engine, look, events),
        Control::Mode => mode(ui, engine, look, events),
        Control::AutoMode => {
            let auto = store.text("appearance.mode").as_deref() == Some("auto");
            if switch(ui, auto, look, name).clicked() {
                let resolved = if resolved_dark(engine) {
                    "dark"
                } else {
                    "light"
                };
                set(
                    events,
                    "appearance.mode",
                    json!(if auto { resolved } else { "auto" }),
                );
            }
        }
        Control::HintAccent => {
            let accent = store.text("window.hint_colour").as_deref() == Some("accent");
            if switch(ui, accent, look, name).clicked() {
                // Off copies the current accent in, as COSMIC does.
                let value = if accent {
                    super::hex(look.accent, false)
                } else {
                    json!("accent")
                };
                set(events, "window.hint_colour", value);
            }
        }
        Control::Pick { key, .. } => {
            chevron(ui, look);
            let current = store.text(key).unwrap_or_else(|| label("pick-default"));
            ui.label(RichText::new(current).size(14.0).color(look.value));
        }
        Control::Page { .. } | Control::Drawer { .. } => chevron(ui, look),
        // Drawn by the section: one row per entry, or the drawer's list.
        // A readout: the live value trailing the row, as body text.
        Control::Info => {
            if let Some(value) = row.value_from.and_then(|src| engine.live_value(src)) {
                ui.label(RichText::new(value).size(14.0).color(look.value));
            }
        }
        Control::Outputs { all } => outputs(ui, engine, all, look, events),
        Control::Preview { image, colour, fit } => {
            super::gallery::preview(ui, engine, (image, colour, fit), name, look);
        }
        Control::Gallery { image, colour } => {
            super::gallery::chooser(ui, engine, row.id, (image, colour), look, events)
        }
        Control::Ordered { .. } | Control::Append { .. } => {}
        Control::Reset { keys } => {
            if ui.add(standard_button(name, look)).clicked() {
                events.push(UiEvent::Reset(keys));
            }
        }
    }
}

/// An inline number field (`Control::NumberEntry`): the stored value until it
/// is edited. The text is written when editing ends (Enter or unfocus); text
/// that is not a number stays in the field and is not stored.
fn number_entry(
    ui: &mut Ui,
    engine: &Engine,
    salt: (&str, &str),
    (key, min, max): (&'static str, i64, i64),
    name: &str,
    events: &mut Vec<UiEvent>,
) {
    let (min, max) = engine.bounds(key, min as f64, max as f64);
    let (min, max) = (min as i64, max as i64);
    let stored = engine
        .store
        .number(key)
        .filter(|v| v.is_finite())
        .map_or(min, |v| (v.round() as i64).clamp(min, max));
    let id = ui.scope_id().with(("number", salt));
    let editing: Option<String> = ui.data(|d| d.get_temp(id));
    let mut text = editing.unwrap_or_else(|| stored.to_string());
    let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(80.0));
    let response = named(response, Role::TextInput, ui.is_enabled(), name);
    if response.lost_focus() {
        let parsed = text.trim().parse::<i64>();
        match parsed {
            Ok(value) => {
                ui.data_mut(|d| d.remove::<String>(id));
                set(events, key, json!(value.clamp(min, max)));
            }
            Err(_) => {
                ui.data_mut(|d| d.insert_temp(id, text));
            }
        }
    } else if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    }
}

fn chevron(ui: &mut Ui, look: &Look) {
    ui.add(icons::image_of(
        ui.ctx(),
        Icon::ChevronRight,
        16.0,
        look.icon,
    ));
}

/// A 24 px icon button with a 16 px icon, circular, drawn disabled at its
/// bound (spec 4 [C4], [C13]).
fn icon_button(ui: &mut Ui, icon: Icon, enabled: bool, name: &str, look: &Look) -> Response {
    let enabled = enabled && ui.is_enabled();
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(24.0), sense);
    if response.hovered() && enabled {
        ui.painter()
            .circle_filled(rect.center(), 12.0, look.button_hover);
    }
    let ink = if enabled {
        look.value
    } else {
        look.value.gamma_multiply(0.5)
    };
    toolkit::button::paint_icon(ui, icon, rect.center(), 16.0, ink);
    focus_ring(ui, &response, rect, look.r_xl, look);
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, name));
    response
}

/// The dropdown: the value text and a 16 px caret, no fill and no border
/// (spec 4 [C2]); the menu it opens keeps its background and radius m.
/// A stored value the menu does not offer shows as `unlisted`.
#[allow(clippy::too_many_arguments)]
fn dropdown(
    ui: &mut Ui,
    value: Option<Value>,
    unlisted: Option<String>,
    key: &'static str,
    options: &[(Value, String)],
    name: &str,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let labels: Vec<String> = options.iter().map(|o| o.1.clone()).collect();
    // An unset value is the option that reads null (Never).
    let stored = value.clone().unwrap_or(Value::Null);
    let index = options.iter().position(|o| o.0 == stored);
    // A stored value outside the options (a connector) still shows.
    let shown = match (index, unlisted, value.as_ref().and_then(Value::as_str)) {
        (Some(i), _, _) => labels[i].clone(),
        (None, Some(text), _) => text,
        (None, None, Some(text)) => text.to_owned(),
        (None, None, None) => String::new(),
    };

    let width = dropdown_width(ui, &labels).max(text_width(ui, &shown, 14.0) + 33.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    let ink = if ui.is_enabled() {
        look.value
    } else {
        look.value.gamma_multiply(0.65)
    };
    toolkit::button::paint_icon(
        ui,
        Icon::ChevronDown,
        pos2(rect.max.x - 9.0 - 8.0, rect.center().y),
        16.0,
        ink,
    );
    ui.painter().text(
        pos2(rect.max.x - 9.0 - 16.0 - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        &shown,
        egui::FontId::proportional(14.0),
        ink,
    );
    focus_ring(ui, &response, rect, look.r_s, look);
    response.widget_info(|| WidgetInfo::selected(Role::ComboBox, ui.is_enabled(), false, name));
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(width);
        for (i, option) in options.iter().enumerate() {
            let selected = index == Some(i);
            if toolkit::selectable(ui, selected, &labels[i]).clicked() && !selected {
                set(events, key, option.0.clone());
            }
        }
    });
}

/// Equal segments the full row wide; the selected one tinted with a check.
fn segmented(
    ui: &mut Ui,
    value: Option<Value>,
    key: &'static str,
    options: &[Opt],
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, Look::radius(look.r_xl), look.field);
    let each = width / options.len().max(1) as f32;
    for (i, option) in options.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(rect.min.x + each * i as f32, rect.min.y),
            vec2(each, 32.0),
        );
        let selected = option.value.is(value.as_ref());
        let response = ui.interact(r, ui.scope_id().with(("seg", key, i)), Sense::click());
        if selected {
            ui.painter().rect_filled(
                r.shrink(1.0),
                Look::radius(look.r_xl),
                look.sidebar_selected,
            );
        }
        let text = label(option.label);
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            if selected {
                format!("✓ {text}")
            } else {
                text.clone()
            },
            egui::FontId::proportional(14.0),
            look.text,
        );
        response.widget_info(|| {
            WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, &text)
        });
        if response.clicked() && !selected && ui.is_enabled() {
            set(events, key, lit(option.value));
        }
    }
}

/// Image buttons with captions (roundness): three fixed cells of the image
/// width, each caption centred under its image on one line (Fable P6).
fn images(
    ui: &mut Ui,
    value: Option<Value>,
    key: &'static str,
    options: &[Opt],
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let count = options.len().max(1) as f32;
    let gap = look.xs;
    // Below three cells' width the images shrink to fit, never clip.
    let cell = 191.0_f32
        .min((ui.available_width() - gap * (count - 1.0)) / count)
        .max(1.0);
    let total = cell * count + gap * (count - 1.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 100.0 + 30.0), Sense::hover());
    let left = row.center().x - total / 2.0;
    for (i, option) in options.iter().enumerate() {
        let selected = option.value.is(value.as_ref());
        let text = label(option.label);
        let radius = match option.value {
            Lit::S("round") => 16.0,
            Lit::S("slightly_round") => 6.0,
            _ => 0.0,
        };
        let rect = Rect::from_min_size(
            pos2(left + i as f32 * (cell + gap), row.min.y),
            vec2(cell, 100.0),
        );
        let response = ui.interact(rect, ui.scope_id().with(("image", key, i)), Sense::click());
        ui.painter()
            .rect_filled(rect, Look::radius(look.r_s), look.window);
        let window =
            Rect::from_min_size(rect.min + vec2(cell * 0.17, 24.0), vec2(cell * 0.66, 60.0));
        ui.painter()
            .rect_filled(window, Look::radius(radius), look.card);
        ui.painter().rect_filled(
            Rect::from_min_size(window.min + vec2(12.0, 14.0), vec2(48.0, 18.0)),
            Look::radius(radius.min(9.0)),
            look.accent,
        );
        if selected {
            ui.painter().rect_stroke(
                rect,
                Look::radius(look.r_s),
                Stroke::new(2.0, look.accent),
                StrokeKind::Inside,
            );
        }
        focus_ring(ui, &response, rect, look.r_s, look);
        response.widget_info(|| {
            WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, &text)
        });
        if response.clicked() && !selected && ui.is_enabled() {
            set(events, key, lit(option.value));
        }
        ui.painter().text(
            pos2(rect.center().x, rect.max.y + 8.0),
            egui::Align2::CENTER_TOP,
            &text,
            egui::FontId::proportional(14.0),
            look.text,
        );
    }
}

/// The Dark and Light image buttons: little windows in each mode's
/// palette, the active one bordered in the accent.
fn mode(ui: &mut Ui, engine: &Engine, look: &Look, events: &mut Vec<UiEvent>) {
    let dark_now = resolved_dark(engine);
    // Two 191 px cards in a 478 px row, centred (spec 1 §4.2.1).
    // Narrower than two cards and their gap, both shrink (the drawer open
    // at 1024): never clipped.
    let gap = (look.xl * 3.0).min(ui.available_width() * 0.1);
    let width = 191.0_f32.min((ui.available_width() - gap) / 2.0).max(1.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 100.0 + 30.0), Sense::hover());
    let left = row.center().x - width - gap / 2.0;
    {
        for (index, (dark, value, key)) in
            [(true, "dark", "opt-dark"), (false, "light", "opt-light")]
                .into_iter()
                .enumerate()
        {
            let palette = if dark { &DARK_PALETTE } else { &LIGHT_PALETTE };
            let bg = super::rgba(palette.gray_1);
            let card = super::rgba(palette.neutral[if dark { 2 } else { 1 }]);
            let text = label(key);
            {
                let rect = Rect::from_min_size(
                    pos2(left + index as f32 * (width + gap), row.min.y),
                    vec2(width, 100.0),
                );
                let response =
                    ui.interact(rect, ui.scope_id().with(("mode", value)), Sense::click());
                ui.painter().rect_filled(rect, Look::radius(look.r_s), bg);
                let inset = (rect.width() * 0.1).min(20.0);
                let window = Rect::from_min_size(
                    rect.min + vec2(inset, 18.0),
                    vec2(rect.width() - 2.0 * inset, 64.0),
                );
                ui.painter()
                    .rect_filled(window, Look::radius(look.r_s), card);
                ui.painter().rect_filled(
                    Rect::from_min_size(window.min + vec2(10.0, 10.0), vec2(36.0, 10.0)),
                    Look::radius(look.r_xs),
                    look.accent,
                );
                let selected = dark == dark_now;
                if selected {
                    ui.painter().rect_stroke(
                        rect,
                        Look::radius(look.r_s),
                        Stroke::new(2.0, look.accent),
                        StrokeKind::Inside,
                    );
                }
                focus_ring(ui, &response, rect, look.r_s, look);
                response.widget_info(|| {
                    WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, &text)
                });
                if response.clicked()
                    && engine.store.text("appearance.mode").as_deref() != Some(value)
                {
                    set(events, "appearance.mode", json!(value));
                }
                ui.painter().text(
                    pos2(rect.center().x, rect.max.y + 8.0),
                    egui::Align2::CENTER_TOP,
                    &text,
                    egui::FontId::proportional(14.0),
                    look.text,
                );
            }
        }
    }
}

/// The nine accent swatches of the resolved mode and a custom one.
fn accent(ui: &mut Ui, engine: &Engine, row: &Row, look: &Look, events: &mut Vec<UiEvent>) {
    let palette = if resolved_dark(engine) {
        &DARK_PALETTE
    } else {
        &LIGHT_PALETTE
    };
    let value = engine
        .store
        .text("appearance.accent")
        .unwrap_or_else(|| "auto".into());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(look.m, look.m);
        for name in SWATCH_NAMES {
            let Some(colour) = palette.swatch(name) else {
                continue;
            };
            let selected = value == name || (value == "auto" && name == "blue");
            let text = label(&format!("accent-{}", name.replace('_', "-")));
            let response = swatch(ui, super::rgba(colour), selected, &text, look);
            if response.clicked() && value != name {
                set(events, "appearance.accent", json!(name));
            }
        }
        let custom = super::parse_hex(&value);
        let text = label("accent-custom");
        let response = match custom {
            Some(colour) => swatch(ui, colour, true, &text, look),
            None => {
                let (rect, response) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
                ui.painter().rect_stroke(
                    rect,
                    Look::radius(look.r_s),
                    Stroke::new(1.0, look.swatch_border),
                    StrokeKind::Inside,
                );
                toolkit::button::paint_icon(ui, Icon::Plus, rect.center(), 16.0, look.icon);
                response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), &text));
                response
            }
        };
        if response.clicked() {
            events.push(UiEvent::Drawer(Some(row.id)));
        }
    });
}

fn swatch(ui: &mut Ui, colour: Color32, selected: bool, name: &str, look: &Look) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::click());
    ui.painter()
        .rect_filled(rect, Look::radius(look.r_s), colour);
    if selected {
        toolkit::button::paint_icon(
            ui,
            Icon::Check,
            pos2(rect.min.x + 12.0, rect.max.y - 12.0),
            16.0,
            Color32::WHITE,
        );
    }
    focus_ring(ui, &response, rect, look.r_s, look);
    response
        .widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, name));
    response
}

/// The colour schemes as chips, the derived MixOS one first (Fable).
fn scheme(ui: &mut Ui, engine: &Engine, look: &Look, events: &mut Vec<UiEvent>) {
    let current = engine
        .store
        .text("appearance.scheme")
        .unwrap_or_else(|| "mixos".into());
    let names = match engine
        .store
        .catalog
        .get("appearance.scheme")
        .map(|i| &i.kind)
    {
        Some(preferences::catalog::Kind::Enum(names)) => names.clone(),
        _ => vec!["mixos".into()],
    };
    let mode = if resolved_dark(engine) {
        design::Mode::Dark
    } else {
        design::Mode::Light
    };
    let mut names: Vec<String> = names;
    if let Some(at) = names.iter().position(|n| n == "mixos") {
        let mixos = names.remove(at);
        names.insert(0, mixos);
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(look.xs, look.xs);
        for name in names {
            let selected = name == current;
            let text = label(&format!("scheme-{name}"));
            let galley = ui.painter().layout_no_wrap(
                text.clone(),
                egui::FontId::proportional(13.0),
                look.text,
            );
            // Never wider than the row the chips wrap in.
            let size = vec2((galley.size().x + 40.0).min(ui.max_rect().width()), 28.0);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            let fill = if selected {
                look.sidebar_selected
            } else {
                look.field
            };
            ui.painter()
                .rect_filled(rect, Look::radius(look.r_xl), fill);
            let chip =
                Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), Vec2::splat(14.0));
            if let Some(scheme) = design::Scheme::from_name(&name)
                && let Some((fill, ink)) = crate::view::scheme_swatch(scheme, mode)
            {
                ui.painter().rect_filled(chip, 3.0, fill);
                if let Some(ink) = ink {
                    ui.painter().rect_filled(
                        Rect::from_min_max(pos2(chip.center().x, chip.min.y), chip.max),
                        3.0,
                        ink,
                    );
                }
            }
            ui.painter().galley(
                pos2(rect.min.x + 30.0, rect.center().y - galley.size().y / 2.0),
                galley,
                look.text,
            );
            if selected {
                ui.painter().rect_stroke(
                    rect,
                    Look::radius(look.r_xl),
                    Stroke::new(1.0, look.accent),
                    StrokeKind::Inside,
                );
            }
            focus_ring(ui, &response, rect, look.r_xl, look);
            response.widget_info(|| {
                WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, &text)
            });
            if response.clicked() && !selected {
                set(events, "appearance.scheme", json!(name));
            }
        }
    });
}

/// COSMIC's slider: a 4 px rail, a 20 px handle, optional value or end
/// labels; written once, on release or a click.
#[allow(clippy::too_many_arguments)]
fn slider(
    ui: &mut Ui,
    salt: (&str, &str),
    value: f64,
    range: (f64, f64),
    step: f64,
    show_value: bool,
    start: Option<&str>,
    end: Option<&str>,
    name: &str,
    look: &Look,
) -> Option<f64> {
    let id = ui.scope_id().with(("slider", salt));
    let held: Option<f64> = ui.data(|d| d.get_temp(id));
    let shown = held.unwrap_or(value);
    let mut out = None;
    ui.spacing_mut().item_spacing.x = look.xs;
    // Read left to right: the value, the start label, the rail, the end
    // label, whichever way the row lays it out.
    let rtl = ui.layout().prefer_right_to_left();
    let text = |ui: &mut Ui, s: String| {
        ui.label(RichText::new(s).size(14.0).color(look.text));
    };
    let value_text = show_value.then(|| format!("{shown:.0}"));
    if rtl {
        if let Some(end) = end {
            text(ui, label(end));
        }
    } else {
        if let Some(v) = &value_text {
            text(ui, v.clone());
        }
        if let Some(start) = start {
            text(ui, label(start));
        }
    }
    // Beside its labels the rail is COSMIC's 250; stacked under them it
    // spans the row, its end label at the card padding.
    let width = if rtl {
        250.0_f32.min(ui.available_width() * 0.5).max(120.0)
    } else {
        let end_width = end.map_or(0.0, |e| text_width(ui, &label(e), 14.0) + look.xs);
        (ui.available_width() - end_width).max(60.0)
    };

    let (rect, response) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click_and_drag());
    let span = (range.1 - range.0).max(f64::EPSILON);
    let rail = Rect::from_center_size(rect.center(), vec2(rect.width() - 20.0, 4.0));
    let t = ((shown - range.0) / span).clamp(0.0, 1.0) as f32;
    let x = rail.min.x + t * rail.width();
    let alpha = if ui.is_enabled() { 1.0 } else { 0.5 };
    ui.painter().rect_filled(
        rail,
        Look::radius(look.r_xs),
        look.track_off.gamma_multiply(alpha),
    );
    ui.painter().rect_filled(
        Rect::from_min_max(rail.min, pos2(x, rail.max.y)),
        Look::radius(look.r_xs),
        look.accent.gamma_multiply(alpha),
    );
    ui.painter().circle_filled(
        pos2(x, rect.center().y),
        10.0,
        look.accent.gamma_multiply(alpha),
    );
    focus_ring(ui, &response, rect, look.r_m, look);
    let at = |pos: egui::Pos2| {
        let t = ((pos.x - rail.min.x) / rail.width()).clamp(0.0, 1.0) as f64;
        let v = range.0 + t * span;
        if step > 0.0 {
            range.0 + ((v - range.0) / step).round() * step
        } else {
            v
        }
    };
    if (response.dragged() || response.drag_started())
        && let Some(pos) = response.interact_pointer_pos()
    {
        ui.data_mut(|d| d.insert_temp(id, at(pos)));
    }
    // Arrow keys step a focused slider (the Role::Slider promise).
    if response.has_focus() {
        let unit = if step > 0.0 { step } else { span / 20.0 };
        let delta = ui.input(|i| {
            if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp) {
                unit
            } else if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown) {
                -unit
            } else {
                0.0
            }
        });
        if delta != 0.0 {
            out = Some((value + delta).clamp(range.0, range.1));
        }
    }
    if response.drag_stopped() {
        out = ui.data_mut(|d| d.remove_temp::<f64>(id));
    } else if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        out = Some(at(pos));
    }
    response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), shown, name));
    if rtl {
        if let Some(start) = start {
            text(ui, label(start));
        }
        if let Some(v) = value_text {
            text(ui, v);
        }
    } else if let Some(end) = end {
        text(ui, label(end));
    }
    out.filter(|v| (v - value).abs() > f64::EPSILON)
}

/// One entry of an ordered list (spec 1 §4.9.4, Input Sources): its label
/// and a "view more" button whose menu moves it up or down or removes it.
/// A move that changes nothing, or a removal below the catalogue's least
/// count, is drawn disabled.
#[allow(clippy::too_many_arguments)]
pub fn ordered_entry(
    ui: &mut Ui,
    engine: &Engine,
    key: &'static str,
    entries: &[Value],
    index: usize,
    text: &str,
    enabled: bool,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let (min, _) = engine.list_bounds(key);
    let pad = look.row_pad() as i8;
    egui::Frame::NONE
        .inner_margin(Margin::symmetric(pad, 4))
        .show(ui, |ui| {
            ui.set_min_height(32.0);
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let more = label_with("list-more", &[("entry", text)]);
                    let response = ui
                        .add_enabled_ui(enabled, |ui| {
                            icon_button(ui, Icon::EllipsisVertical, true, &more, look)
                        })
                        .inner;
                    egui::Popup::menu(&response).show(|ui| {
                        ui.set_min_width(160.0);
                        let edits = [
                            (ListEdit::Up(index), "list-move-up", index > 0),
                            (
                                ListEdit::Down(index),
                                "list-move-down",
                                index + 1 < entries.len(),
                            ),
                        ];
                        for (edit, id, can) in edits {
                            menu_edit(ui, key, entries, edit, &label(id), can, events);
                        }
                        ui.separator();
                        menu_edit(
                            ui,
                            key,
                            entries,
                            ListEdit::Remove(index),
                            &label("list-remove"),
                            entries.len() > min,
                            events,
                        );
                    });
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.label(RichText::new(text).size(14.0).color(look.text));
                    });
                });
            });
        });
}

fn menu_edit(
    ui: &mut Ui,
    key: &'static str,
    entries: &[Value],
    edit: ListEdit,
    text: &str,
    enabled: bool,
    events: &mut Vec<UiEvent>,
) {
    let clicked = ui
        .add_enabled_ui(enabled, |ui| toolkit::selectable(ui, false, text))
        .inner
        .clicked();
    if clicked && let Some(list) = edit.apply(entries) {
        set(events, key, Value::Array(list));
        ui.close();
    }
}

/// The values a list key can take, from a live source (the Add input
/// source drawer): a search field, a switch for the `more` source, then one
/// row per value with Add, or Added once it is in the list. A value that
/// refines another (a layout's variant) is indented under it. Rows out of
/// view are laid out but not drawn.
#[allow(clippy::too_many_arguments)]
pub fn append(
    ui: &mut Ui,
    engine: &Engine,
    page: &Page,
    row: &Row,
    key: &'static str,
    source: &'static str,
    more: Option<&'static str>,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let enabled = engine.row_blocked(page, row).is_none();
    let entries = engine.entries(key);
    let (_, max) = engine.list_bounds(key);
    let full = entries.len() >= max;
    let filter_id = ui.scope_id().with(("append-filter", key));
    let more_id = ui.scope_id().with(("append-more", key));
    let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    let search = ui.add(
        egui::TextEdit::singleline(&mut filter)
            .hint_text(label("search-hint"))
            .desired_width(ui.available_width()),
    );
    if search.changed() {
        ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
    }
    ui.add_space(look.s);
    let mut extended: bool = ui.data(|d| d.get_temp(more_id)).unwrap_or(false);
    if more.is_some() {
        let name = label(&format!("{}-more", pages::row_title(page.id, row.id)));
        super::card(ui, look, |ui| {
            egui::Frame::NONE
                .inner_margin(Margin::symmetric(look.row_pad() as i8, 4))
                .show(ui, |ui| {
                    ui.set_min_height(32.0);
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if switch(ui, extended, look, &name).clicked() {
                                extended = !extended;
                                ui.data_mut(|d| d.insert_temp(more_id, extended));
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                ui.label(RichText::new(&name).size(14.0).color(look.text));
                            });
                        });
                    });
                });
        });
        ui.add_space(look.s);
    }
    let query = filter.trim().to_lowercase();
    let mut values: Vec<&(Value, String)> = engine.live_options(source).iter().collect();
    if extended && let Some(more) = more {
        values.extend(engine.live_options(more));
    }
    values.retain(|(_, text)| text.to_lowercase().contains(&query));
    super::card(ui, look, |ui| {
        if values.is_empty() {
            super::plain_row(ui, look, &label("list-empty"), None);
            return;
        }
        let (add, added) = (label("list-add"), label("list-added"));
        for (index, (value, text)) in values.into_iter().enumerate() {
            if index > 0 {
                super::divider(ui, look);
            }
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let refines = value
                .as_array()
                .and_then(|parts| parts.last())
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty());
            let indent = if refines { look.l } else { 0.0 };
            ui.painter().text(
                rect.left_center() + vec2(look.m + indent, 0.0),
                egui::Align2::LEFT_CENTER,
                text,
                egui::FontId::proportional(14.0),
                look.text,
            );
            let have = entries.contains(value);
            let slot = Rect::from_min_max(
                pos2(rect.max.x - 160.0, rect.min.y),
                rect.max - vec2(look.m, 0.0),
            );
            let word = if have { &added } else { &add };
            let name = format!("{word} {text}");
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(slot)
                    .layout(Layout::right_to_left(Align::Center)),
                |ui| {
                    let response =
                        ui.add_enabled(enabled && !have && !full, named_button(word, &name, look));
                    if response.clicked()
                        && let Some(list) = ListEdit::Append(value.clone()).apply(&entries)
                    {
                        set(events, key, Value::Array(list));
                    }
                },
            );
        }
    });
}
