// SPDX-License-Identifier: MIT OR Apache-2.0
//! The egui rendering of the engine's state. [`view`] reads the engine and
//! returns what the person did as [`UiEvent`]s; the shell applies them after
//! the frame, so drawing never mutates state and a snapshot needs no Bus.
use crate::label;
use egui::{Align, Layout, RichText, ScrollArea, TextEdit, Ui};
use inspector::{Dialog, Engine, Row, RowKind};
use toolkit::{Icon, Registry, Strings, icons};

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Command(&'static str),
    Filter(String),
    Toggle(String),
    Select(Row),
    Body(String),
    Split(f32),
    CloseDialog,
}

const ICON: f32 = 14.0;

/// Draw the whole window into `ui`.
pub fn view(ui: &mut Ui, engine: &Engine, commands: &Registry<Engine>, strings: &Strings, stroke: f32) -> Vec<UiEvent> {
    let mut events = Vec::new();
    let fired = toolkit::titlebar::show(ui, &label("title"), Some(Icon::Server), stroke, commands, engine, strings);
    events.extend(fired.into_iter().map(UiEvent::Command));
    egui::Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            let (icon, colour) = if engine.connected {
                (Icon::Plug, ui.visuals().weak_text_color())
            } else {
                (Icon::Unplug, ui.visuals().error_fg_color)
            };
            ui.add(icons::image(icon, stroke, ICON, colour));
            ui.small(&engine.status);
        });
    });
    let total = ui.available_width();
    let left = egui::Panel::left("services")
        .resizable(true)
        .default_size(total * engine.ui.split)
        .show(ui, |ui| services(ui, engine, stroke, &mut events));
    let split = left.response.rect.width() / total.max(1.0);
    if (split - engine.ui.split).abs() > 0.005 {
        events.push(UiEvent::Split(split));
    }
    egui::CentralPanel::default().show(ui, |ui| details(ui, engine, commands, strings, stroke, &mut events));
    if let Some(dialog) = engine.ui.dialog {
        let modal = egui::Modal::new(egui::Id::new("dialog")).show(ui.ctx(), |ui| {
            let (title, body) = match dialog {
                Dialog::About => ("about", "about-body"),
                Dialog::Shortcuts => ("shortcuts", "shortcut-body"),
            };
            ui.set_max_width(360.0);
            ui.heading(label(title));
            ui.label(label(body));
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| ui.button(label("done")).clicked()).inner
        });
        if modal.inner || modal.should_close() {
            events.push(UiEvent::CloseDialog);
        }
    }
    toolkit::titlebar::edges(ui);
    events
}

fn services(ui: &mut Ui, engine: &Engine, stroke: f32, events: &mut Vec<UiEvent>) {
    ui.heading(label("services"));
    let mut filter = engine.ui.filter.clone();
    let search = ui.add(TextEdit::singleline(&mut filter).hint_text(label("search")).desired_width(f32::INFINITY));
    if search.changed() {
        events.push(UiEvent::Filter(filter));
    }
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for row in engine.tree() {
            tree_row(ui, engine, &row, stroke, events);
        }
    });
}

fn tree_row(ui: &mut Ui, engine: &Engine, row: &Row, stroke: f32, events: &mut Vec<UiEvent>) {
    let colour = ui.visuals().text_color();
    ui.horizontal(|ui| {
        if row.children.is_empty() {
            ui.add_space(ICON + ui.spacing().item_spacing.x);
        } else {
            let chevron = if row.expanded { Icon::ChevronDown } else { Icon::ChevronRight };
            if ui.add(egui::Button::image(icons::image(chevron, stroke, ICON, colour)).frame(false)).clicked() {
                events.push(UiEvent::Toggle(row.key.clone()));
            }
        }
        let text = match &row.kind {
            RowKind::Service(name) | RowKind::Peer(name) => RichText::new(name),
            RowKind::Verb(target) => RichText::new(&target.verb).monospace(),
            RowKind::Error(_) => RichText::new(label("descriptions-failed")).color(ui.visuals().error_fg_color),
            RowKind::Peers => RichText::new(label("peers")),
            RowKind::NoPeers => RichText::new(label("no-peers")).weak(),
        };
        if let RowKind::Service(_) = row.kind {
            ui.add(icons::image(Icon::Server, stroke, ICON, ui.visuals().weak_text_color()));
        }
        let selected = engine.ui.row_key.as_deref() == Some(row.key.as_str());
        if ui.selectable_label(selected, text).clicked() {
            events.push(UiEvent::Select(row.clone()));
        }
    });
    if row.expanded {
        ui.indent(&row.key, |ui| {
            for child in &row.children {
                tree_row(ui, engine, child, stroke, events);
            }
        });
    }
}

/// The selected verb's description, or what the selected row is.
fn description(engine: &Engine) -> String {
    let verb = engine.ui.selected.as_ref().and_then(|s| engine.snapshot.verb(s).map(|v| (s, v)));
    if let Some((selection, verb)) = verb {
        let args = if verb.args.is_empty() { label("unspecified") } else { verb.args.clone() };
        let read_only = label(match verb.read_only {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        });
        let description =
            if verb.description.is_empty() { label("description-unavailable") } else { verb.description.clone() };
        return format!(
            "{}  {}\n{}: {args}\n{}: {read_only}\n\n{description}",
            selection.service,
            selection.verb,
            label("arguments"),
            label("read-only")
        );
    }
    let row = engine.ui.row_key.as_ref().and_then(|key| find(&engine.tree(), key));
    match row.map(|r| r.kind) {
        Some(RowKind::Error(error)) => error,
        Some(RowKind::Service(name)) => name,
        Some(RowKind::Peer(name)) => format!("{}: {name}", label("peer-membership")),
        _ => label("select"),
    }
}

fn find(rows: &[Row], key: &str) -> Option<Row> {
    rows.iter().find_map(|r| if r.key == key { Some(r.clone()) } else { find(&r.children, key) })
}

fn details(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    ui.heading(label("details"));
    let rows = ui.text_style_height(&egui::TextStyle::Body) * 6.0;
    ScrollArea::vertical().id_salt("description").max_height(rows).show(ui, |ui| {
        ui.label(description(engine));
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(label("body"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for id in ["bus.call", "edit.clear"] {
                let Some(command) = commands.get(id) else { continue };
                let icon = command.icon.map(|i| icons::image(i, stroke, ICON, ui.visuals().text_color()));
                let text = strings.get(command.label);
                let button = match icon {
                    Some(icon) => egui::Button::image_and_text(icon, text),
                    None => egui::Button::new(text),
                };
                if ui.add_enabled((command.enabled)(engine), button).clicked() {
                    events.push(UiEvent::Command(command.id));
                }
            }
        });
    });
    let mut body = engine.ui.body.clone();
    let editor = TextEdit::multiline(&mut body)
        .code_editor()
        .desired_rows(7)
        .desired_width(f32::INFINITY)
        .hint_text(label("body"))
        .interactive(!engine.calling() && engine.ui.dialog.is_none());
    if ui.add(editor).changed() {
        events.push(UiEvent::Body(body));
    }
    ui.label(label("reply"));
    ScrollArea::vertical().id_salt("reply").auto_shrink(false).show(ui, |ui| {
        let mut reply = engine.reply.as_str();
        ui.add(TextEdit::multiline(&mut reply).code_editor().desired_width(f32::INFINITY));
    });
}
