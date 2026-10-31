// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! One setting row and its control, in COSMIC's geometry (spec 4,
//! "Setting row" and "Control set"): padding 4 by 16, content at least 32
//! tall, the control right-aligned 16 from the card edge, or under the
//! title for the wide ones (accent, mode, segmented, image choices).
//! Every control is named for agents with its row's title, so
//! `prefs.ui.click {label}` reaches it as a hand would.
use super::{Look, description, lit, named, set, title};
use crate::label;
use crate::view::UiEvent;
use design::cosmic_tables::{DARK_PALETTE, LIGHT_PALETTE, SWATCH_NAMES};
use egui::{
    Align, Color32, Layout, Margin, Rect, Response, RichText, Role, Sense, Stroke, StrokeKind, Ui,
    Vec2, Widget, WidgetInfo, pos2, vec2,
};
use preferences::Engine;
use preferences::pages::{ChoiceStyle, Control, Lit, Opt, Page, Row};
use serde_json::{Value, json};
use toolkit::{Icon, icons};

/// The resolved light or dark mode the store's values give.
pub fn resolved_dark(engine: &Engine) -> bool {
    let look = <settings::view::Look as settings::view::View>::of(&engine.store.values);
    look.mode().0 == design::Mode::Dark
}

/// COSMIC's standard button: 24 tall (compact), pill, button colour.
pub fn standard_button<'a>(text: &'a str, look: &'a Look) -> impl Widget + 'a {
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
        ui.painter().rect_filled(rect, Look::radius(look.r_xl), fill);
        let ink = if ui.is_enabled() {
            look.text
        } else {
            look.text.gamma_multiply(0.65)
        };
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, ink);
        focus_ring(ui, &response, rect, look.r_xl, look);
        response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), text));
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
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), 10.0, look.knob.gamma_multiply(alpha));
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

/// The label column: the title, the description, and why the row is off.
fn labels(ui: &mut Ui, look: &Look, title: &str, description: Option<&str>, blocked: Option<&str>) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.add(egui::Label::new(RichText::new(title).size(14.0).color(look.text)).wrap());
        if let Some(d) = description {
            ui.add(egui::Label::new(RichText::new(d).size(12.0).color(look.text)).wrap());
        }
        if let Some(why) = blocked {
            ui.horizontal_wrapped(|ui| {
                ui.add(icons::image_of(ui.ctx(), Icon::CircleAlert, 12.0, look.warning));
                ui.add(egui::Label::new(RichText::new(why).size(12.0).color(look.text)).wrap());
            });
        }
    });
}

/// Whether a control sits under its title rather than beside it.
fn stacked(control: &Control) -> bool {
    matches!(
        control,
        Control::Accent
            | Control::Scheme
            | Control::Mode
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
pub fn row(
    ui: &mut Ui,
    engine: &Engine,
    page: &Page,
    row: &Row,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let blocked = engine.row_blocked(page, row);
    let enabled = blocked.is_none();
    let name = title(page.id, row.id);
    let desc = match row.control {
        Control::Info => description(page.id, row.id).or_else(|| Some(String::new())),
        _ => description(page.id, row.id),
    };
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
            if stacked(&row.control) {
                ui.vertical(|ui| {
                    labels(ui, look, &name, desc.as_deref(), blocked.as_deref());
                    ui.add_space(look.xs);
                    ui.add_enabled_ui(enabled, |ui| {
                        control(ui, engine, page, row, &name, look, events)
                    });
                });
            } else {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(enabled, |ui| {
                        control(ui, engine, page, row, &name, look, events)
                    });
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        labels(
                            ui,
                            look,
                            &name,
                            desc.as_deref().filter(|d| !d.is_empty()),
                            blocked.as_deref(),
                        )
                    });
                });
            }
        })
        .response;
    if goes(&row.control) {
        let click = ui.interact(response.rect, ui.id().with(("go", row.id)), Sense::click());
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
    if engine.nav.focus == Some(row.id) {
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
    option: &Opt,
    blocked: Option<&str>,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let value = engine.store.value(key);
    let selected = option.value.is(value.as_ref());
    let text = label(option.label);
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
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
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
                        set(events, key, lit(option.value));
                    }
                });
                ui.add_space(look.xs);
                ui.label(RichText::new(&text).size(14.0).color(look.text));
            });
        })
        .response;
    if blocked.is_none() {
        let whole = ui.interact(response.rect, ui.id().with(("radio", key, option.label)), Sense::click());
        if whole.clicked() && !selected {
            set(events, key, lit(option.value));
        }
    }
    if engine.nav.focus == Some(row.id) && selected {
        response.scroll_to_me(Some(Align::Center));
        events.push(UiEvent::FocusDone);
    }
    let _ = page;
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
            options,
        } => dropdown(ui, store.value(key), key, options, name, look, events),
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
            let shown = store.number(key).unwrap_or(*min) * scale;
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
            // A value off the steps shows as the middle one (COSMIC's Custom).
            let index = steps
                .iter()
                .position(|s| s.is(value.as_ref()))
                .unwrap_or(steps.len() / 2) as f64;
            let last = (steps.len().max(1) - 1) as f64;
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
                let i = (v.round() as usize).min(steps.len() - 1);
                set(events, key, lit(steps[i]));
            }
        }
        Control::Spin {
            key,
            min,
            max,
            step,
        } => {
            let value = store.number(key).unwrap_or(*min as f64) as i64;
            ui.spacing_mut().item_spacing.x = 4.0;
            let plus = ui.add_enabled(
                value < *max,
                toolkit::button::IconButton::new(Icon::Plus).size(16.0),
            );
            let plus = named(plus, Role::Button, value < *max, &format!("{name} +"));
            ui.label(RichText::new(value.to_string()).size(14.0).color(look.value));
            let minus = ui.add_enabled(
                value > *min,
                toolkit::button::IconButton::new(Icon::Minus).size(16.0),
            );
            let minus = named(minus, Role::Button, value > *min, &format!("{name} −"));
            if plus.clicked() {
                set(events, key, json!((value + step).min(*max)));
            }
            if minus.clicked() {
                set(events, key, json!((value - step).max(*min)));
            }
        }
        Control::Colour { key } => {
            let colour = store.text(key).as_deref().and_then(super::parse_hex);
            let (rect, response) = ui.allocate_exact_size(vec2(48.0, 24.0), Sense::click());
            match colour {
                Some(c) => ui.painter().rect_filled(rect, Look::radius(look.r_xs), c),
                None => {
                    ui.painter()
                        .rect_filled(rect, Look::radius(look.r_xs), look.field);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label("colour-auto"),
                        egui::FontId::proportional(11.0),
                        look.text,
                    );
                }
            }
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
                let resolved = if resolved_dark(engine) { "dark" } else { "light" };
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
            let current = store
                .text(key)
                .unwrap_or_else(|| label("pick-default"));
            ui.label(RichText::new(current).size(14.0).color(look.value));
        }
        Control::Page { .. } | Control::Drawer { .. } => chevron(ui, look),
        Control::Info => {}
        Control::Reset { keys } => {
            if ui.add(standard_button(name, look)).clicked() {
                events.push(UiEvent::Reset(keys));
            }
        }
    }
}

fn chevron(ui: &mut Ui, look: &Look) {
    ui.add(icons::image_of(ui.ctx(), Icon::ChevronRight, 16.0, look.icon));
}

/// The dropdown: the value text and a caret, no background (spec 4).
fn dropdown(
    ui: &mut Ui,
    value: Option<Value>,
    key: &'static str,
    options: &[Opt],
    name: &str,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let mut labels: Vec<String> = options.iter().map(|o| label(o.label)).collect();
    let mut index = options.iter().position(|o| o.value.is(value.as_ref()));
    // A stored value outside the options (a connector) still shows.
    if index.is_none()
        && let Some(text) = value.as_ref().and_then(Value::as_str)
    {
        labels.push(text.to_owned());
        index = Some(labels.len() - 1);
    }
    let mut selected = index.unwrap_or(0);
    let before = selected;
    let width = labels
        .iter()
        .map(|l| {
            ui.painter()
                .layout_no_wrap(l.clone(), egui::FontId::proportional(14.0), look.value)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
        + 48.0;
    let response = toolkit::combo::show(ui, ("choice", key), &mut selected, &labels, Some(width));
    named(response, Role::ComboBox, ui.is_enabled(), name);
    if selected != before && selected < options.len() {
        set(events, key, lit(options[selected].value));
    }
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
        let r = Rect::from_min_size(pos2(rect.min.x + each * i as f32, rect.min.y), vec2(each, 32.0));
        let selected = option.value.is(value.as_ref());
        let response = ui.interact(r, ui.id().with(("seg", key, i)), Sense::click());
        if selected {
            ui.painter()
                .rect_filled(r.shrink(1.0), Look::radius(look.r_xl), look.sidebar_selected);
        }
        let text = label(option.label);
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            if selected { format!("✓ {text}") } else { text.clone() },
            egui::FontId::proportional(14.0),
            look.text,
        );
        response.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, &text));
        if response.clicked() && !selected && ui.is_enabled() {
            set(events, key, lit(option.value));
        }
    }
}

/// Image buttons with captions (roundness): each illustration is a little
/// window drawn with that option's corner.
fn images(
    ui: &mut Ui,
    value: Option<Value>,
    key: &'static str,
    options: &[Opt],
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = look.xs;
        for option in options {
            let selected = option.value.is(value.as_ref());
            let text = label(option.label);
            let radius = match option.value {
                Lit::S("round") => 16.0,
                Lit::S("slightly_round") => 6.0,
                _ => 0.0,
            };
            ui.vertical(|ui| {
                let (rect, response) = ui.allocate_exact_size(vec2(191.0_f32.min(ui.available_width()), 100.0), Sense::click());
                ui.painter().rect_filled(rect, Look::radius(look.r_s), look.window);
                let window = Rect::from_min_size(rect.min + vec2(32.0, 24.0), vec2(rect.width() - 64.0, 60.0));
                ui.painter().rect_filled(window, Look::radius(radius), look.card);
                ui.painter().rect_filled(
                    Rect::from_min_size(window.min + vec2(12.0, 14.0), vec2(48.0, 18.0)),
                    Look::radius(radius.min(9.0)),
                    look.accent,
                );
                if selected {
                    ui.painter().rect_stroke(rect, Look::radius(look.r_s), Stroke::new(2.0, look.accent), StrokeKind::Inside);
                }
                focus_ring(ui, &response, rect, look.r_s, look);
                response.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, &text));
                if response.clicked() && !selected {
                    set(events, key, lit(option.value));
                }
                ui.vertical_centered(|ui| {
                    ui.set_width(rect.width());
                    ui.label(RichText::new(&text).size(14.0).color(look.text));
                });
            });
        }
    });
}

/// The Dark and Light image buttons: little windows in each mode's
/// palette, the active one bordered in the accent.
fn mode(ui: &mut Ui, engine: &Engine, look: &Look, events: &mut Vec<UiEvent>) {
    let dark_now = resolved_dark(engine);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = look.m;
        for (dark, value, key) in [(true, "dark", "opt-dark"), (false, "light", "opt-light")] {
            let palette = if dark { &DARK_PALETTE } else { &LIGHT_PALETTE };
            let bg = super::rgba(palette.gray_1);
            let card = super::rgba(palette.neutral[if dark { 2 } else { 1 }]);
            let text = label(key);
            ui.vertical(|ui| {
                let width = 191.0_f32.min((ui.available_width() - look.m) / 2.0).max(96.0);
                let (rect, response) = ui.allocate_exact_size(vec2(width, 100.0), Sense::click());
                ui.painter().rect_filled(rect, Look::radius(look.r_s), bg);
                let window = Rect::from_min_size(rect.min + vec2(20.0, 18.0), vec2(rect.width() - 40.0, 64.0));
                ui.painter().rect_filled(window, Look::radius(look.r_s), card);
                ui.painter().rect_filled(
                    Rect::from_min_size(window.min + vec2(10.0, 10.0), vec2(36.0, 10.0)),
                    Look::radius(look.r_xs),
                    look.accent,
                );
                let selected = dark == dark_now;
                if selected {
                    ui.painter().rect_stroke(rect, Look::radius(look.r_s), Stroke::new(2.0, look.accent), StrokeKind::Inside);
                }
                focus_ring(ui, &response, rect, look.r_s, look);
                response.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, &text));
                if response.clicked() && engine.store.text("appearance.mode").as_deref() != Some(value) {
                    set(events, "appearance.mode", json!(value));
                }
                ui.vertical_centered(|ui| {
                    ui.set_width(width);
                    ui.label(RichText::new(&text).size(14.0).color(look.text));
                });
            });
        }
    });
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
                ui.painter().rect_stroke(rect, Look::radius(look.r_s), Stroke::new(1.0, look.swatch_border), StrokeKind::Inside);
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
    ui.painter().rect_filled(rect, Look::radius(look.r_s), colour);
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
    response.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, name));
    response
}

/// The colour schemes as chips, the derived MixOS one first (Fable).
fn scheme(ui: &mut Ui, engine: &Engine, look: &Look, events: &mut Vec<UiEvent>) {
    let current = engine.store.text("appearance.scheme").unwrap_or_else(|| "mixos".into());
    let names = match engine.store.catalog.get("appearance.scheme").map(|i| &i.kind) {
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
            let galley = ui.painter().layout_no_wrap(text.clone(), egui::FontId::proportional(13.0), look.text);
            let size = vec2(galley.size().x + 40.0, 28.0);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            let fill = if selected { look.sidebar_selected } else { look.field };
            ui.painter().rect_filled(rect, Look::radius(look.r_xl), fill);
            let chip = Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), Vec2::splat(14.0));
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
            ui.painter().galley(pos2(rect.min.x + 30.0, rect.center().y - galley.size().y / 2.0), galley, look.text);
            if selected {
                ui.painter().rect_stroke(rect, Look::radius(look.r_xl), Stroke::new(1.0, look.accent), StrokeKind::Inside);
            }
            focus_ring(ui, &response, rect, look.r_xl, look);
            response.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, &text));
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
    let id = ui.id().with(("slider", salt));
    let held: Option<f64> = ui.data(|d| d.get_temp(id));
    let shown = held.unwrap_or(value);
    let mut out = None;
    ui.spacing_mut().item_spacing.x = look.xs;
    if let Some(end) = end {
        ui.label(RichText::new(label(end)).size(14.0).color(look.text));
    }
    let width = 250.0_f32.min(ui.available_width() * 0.5).max(120.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click_and_drag());
    let span = (range.1 - range.0).max(f64::EPSILON);
    let rail = Rect::from_center_size(rect.center(), vec2(rect.width() - 20.0, 4.0));
    let t = ((shown - range.0) / span).clamp(0.0, 1.0) as f32;
    let x = rail.min.x + t * rail.width();
    let alpha = if ui.is_enabled() { 1.0 } else { 0.5 };
    ui.painter()
        .rect_filled(rail, Look::radius(look.r_xs), look.track_off.gamma_multiply(alpha));
    ui.painter().rect_filled(
        Rect::from_min_max(rail.min, pos2(x, rail.max.y)),
        Look::radius(look.r_xs),
        look.accent.gamma_multiply(alpha),
    );
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), 10.0, look.accent.gamma_multiply(alpha));
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
    if response.drag_stopped() {
        out = ui.data_mut(|d| d.remove_temp::<f64>(id));
    } else if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        out = Some(at(pos));
    }
    response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), shown, name));
    if let Some(start) = start {
        ui.label(RichText::new(label(start)).size(14.0).color(look.text));
    }
    if show_value {
        ui.label(RichText::new(format!("{shown:.0}")).size(14.0).color(look.text));
    }
    out.filter(|v| (v - value).abs() > f64::EPSILON)
}
