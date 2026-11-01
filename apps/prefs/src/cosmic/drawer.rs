// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The context drawer at the right of the content column (spec 4,
//! "Dialogs"): a page's own drawer (Frosted glass, Icons and toolkit
//! theming, Window shadow and corners), the colour picker of a swatch row,
//! or the list of a pick row (fonts, icon themes). A colour applies when
//! the pointer lets go of the picker, or on Enter in the hex field; the
//! list applies on a click and stays open, as COSMIC's font drawer does.
use super::{Look, card, divider, section_card, set};
use crate::label;
use crate::view::UiEvent;
use egui::{Align, Color32, Layout, RichText, Role, Sense, Ui, WidgetInfo, vec2};
use preferences::Engine;
use preferences::pages::{self, Control, Page, Row};
use serde_json::json;
use toolkit::{Icon, icons};

pub fn show(
    ui: &mut Ui,
    engine: &Engine,
    page: &Page,
    open: &'static str,
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let rect = ui.max_rect().shrink2(vec2(look.l, look.s));
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    let ui = &mut ui;
    let row = page.row(open);
    let title = match (page.drawer(open), row) {
        (Some(_), _) => label(&pages::drawer_title(page.id, open)),
        (None, Some(row)) => super::title(page.id, row.id),
        (None, None) => String::new(),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(&title).font(super::heading(ui, 20.0)).color(look.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close = ui.add(
                toolkit::button::IconButton::new(Icon::X)
                    .size(16.0)
                    .tooltip(label("drawer-close")),
            );
            if close.clicked() {
                events.push(UiEvent::Drawer(None));
            }
        });
    });
    ui.add_space(look.m);
    if let Some(drawer) = page.drawer(open) {
        egui::ScrollArea::vertical()
            .id_salt(("drawer", page.id, open))
            .show(ui, |ui| {
                for (index, section) in drawer.sections.iter().enumerate() {
                    let rows: Vec<&Row> = section
                        .rows
                        .iter()
                        .filter(|r| r.show.holds(&engine.store))
                        .collect();
                    if rows.is_empty() {
                        continue;
                    }
                    if index > 0 {
                        ui.add_space(look.m);
                    }
                    section_card(ui, engine, page, section, &rows, look, events);
                }
            });
        return;
    }
    let Some(row) = row else {
        return;
    };
    if let Some(why) = engine.row_blocked(page, row) {
        super::notice(ui, look, &why);
    }
    ui.add_enabled_ui(engine.row_blocked(page, row).is_none(), |ui| {
        match row.control {
            Control::Colour { key } => colour(ui, engine, key, look, events),
            Control::Accent => colour(ui, engine, "appearance.accent", look, events),
            Control::Pick { key, list } => {
                pick(ui, engine, key, &super::pick_list(ui, list), look, events)
            }
            _ => {}
        }
    });
}

/// The word a colour key resets to.
fn auto_word(key: &str) -> &'static str {
    if key == "window.hint_colour" {
        "accent"
    } else {
        "auto"
    }
}

fn colour(ui: &mut Ui, engine: &Engine, key: &'static str, look: &Look, events: &mut Vec<UiEvent>) {
    let alpha = matches!(
        engine.store.catalog.get(key).map(|i| &i.kind),
        Some(preferences::catalog::Kind::Colour { alpha: true, .. })
    );
    let stored = engine.store.text(key).as_deref().and_then(super::parse_hex);
    let id = ui.scope_id().with(("picking", key));
    let mut picking: Color32 = ui
        .data(|d| d.get_temp(id))
        .or(stored)
        .unwrap_or(look.accent);
    let picker = egui::widgets::color_picker::color_picker_color32(
        ui,
        &mut picking,
        if alpha {
            egui::widgets::color_picker::Alpha::OnlyBlend
        } else {
            egui::widgets::color_picker::Alpha::Opaque
        },
    );
    if picker {
        ui.data_mut(|d| d.insert_temp(id, picking));
    }
    let released = ui.input(|i| i.pointer.any_released());
    if released
        && let Some(held) = ui.data(|d| d.get_temp::<Color32>(id))
        && Some(held) != stored
    {
        ui.data_mut(|d| d.remove_temp::<Color32>(id));
        set(events, key, super::hex(held, alpha));
    }
    ui.add_space(look.s);
    let hex_id = ui.scope_id().with(("hex", key));
    let mut text: String = ui
        .data(|d| d.get_temp(hex_id))
        .unwrap_or_else(|| engine.store.text(key).unwrap_or_default());
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label("colour-hex"))
                .size(12.0)
                .color(look.text),
        );
        let edit = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(120.0)
                .id_salt(hex_id),
        );
        if edit.changed() {
            ui.data_mut(|d| d.insert_temp(hex_id, text.clone()));
        }
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            ui.data_mut(|d| d.remove_temp::<String>(hex_id));
            match super::parse_hex(text.trim()) {
                Some(c) => set(events, key, super::hex(c, alpha)),
                None => events.push(UiEvent::Problem(label("colour-hex-invalid"))),
            }
        }
    });
    ui.add_space(look.m);
    let reset = label(if auto_word(key) == "accent" {
        "colour-reset-accent"
    } else {
        "colour-reset-auto"
    });
    if ui
        .add(super::controls::standard_button(&reset, look))
        .clicked()
    {
        ui.data_mut(|d| {
            d.remove_temp::<Color32>(id);
            d.remove_temp::<String>(hex_id);
        });
        set(events, key, json!(auto_word(key)));
    }
}

fn pick(
    ui: &mut Ui,
    engine: &Engine,
    key: &'static str,
    names: &[String],
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    let filter_id = ui.scope_id().with(("pick-filter", key));
    let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    if ui
        .add(
            egui::TextEdit::singleline(&mut filter)
                .hint_text(label("search-hint"))
                .desired_width(ui.available_width()),
        )
        .changed()
    {
        ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
    }
    ui.add_space(look.s);
    let current = engine.store.text(key);
    let nullable = engine
        .store
        .catalog
        .get(key)
        .is_some_and(|i| i.default.as_ref().is_none_or(serde_json::Value::is_null));
    let query = filter.to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt(("pick", key))
        .show(ui, |ui| {
            card(ui, look, |ui| {
                let mut first = true;
                let mut entry =
                    |ui: &mut Ui, text: &str, value: serde_json::Value, selected: bool| {
                        if !first {
                            divider(ui, look);
                        }
                        first = false;
                        let (rect, response) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
                        ui.painter().text(
                            rect.left_center() + vec2(look.m, 0.0),
                            egui::Align2::LEFT_CENTER,
                            text,
                            egui::FontId::proportional(14.0),
                            look.text,
                        );
                        if selected {
                            toolkit::button::paint_icon(
                                ui,
                                Icon::Check,
                                rect.right_center() - vec2(look.m + 8.0, 0.0),
                                16.0,
                                look.accent,
                            );
                        }
                        response.widget_info(|| {
                            WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), selected, text)
                        });
                        if response.clicked() && !selected {
                            set(events, key, value);
                        }
                    };
                if nullable && query.is_empty() {
                    entry(
                        ui,
                        &label("pick-default"),
                        serde_json::Value::Null,
                        current.is_none(),
                    );
                }
                for name in names.iter().filter(|n| n.to_lowercase().contains(&query)) {
                    entry(
                        ui,
                        name,
                        json!(name),
                        current.as_deref() == Some(name.as_str()),
                    );
                }
            });
            if names.is_empty() {
                ui.add(icons::image_of(
                    ui.ctx(),
                    Icon::LoaderCircle,
                    16.0,
                    look.icon,
                ));
            }
        });
}
