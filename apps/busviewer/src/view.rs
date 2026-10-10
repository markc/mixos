// SPDX-License-Identifier: MIT OR Apache-2.0
//! The egui rendering of the engine's state. [`view`] reads the engine and
//! returns what the person did as [`UiEvent`]s; the shell applies them after
//! the frame, so drawing never mutates state and a snapshot needs no Bus.
//!
//! The window is built from the toolkit's chrome components: the title bar,
//! a status bar, two docked panel groups ("Services on this node" with its
//! search field and tree, "Details" with the verb's description, the JSON
//! body and the reply), push buttons for the body's commands with
//! "Label (Shortcut)" tooltips, and the About and Shortcuts dialogs as
//! modal dialogs.
use crate::{label, label_with};
use egui::{Align, Layout, RichText, ScrollArea, TextEdit, Ui};
use inspector::{Dialog, Engine, Row, RowKind};
use toolkit::button::PushButton;
use toolkit::dialog::{self, Choice};
use toolkit::titlebar::{self, Control};
use toolkit::{Icon, Registry, Strings, bars, field, icons, panel, tooltip};

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
    /// The services filter took the keyboard, as `view.search` asked.
    FilterFocused,
    /// The title bar's light/dark toggle: invert the session mode or not.
    SetMode(design::Mode),
}

const ICON: f32 = 14.0;

/// Draw the whole window into `ui`.
pub fn view(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    stroke: f32,
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    // The right-hand group (§3.1): search the services, flip light and
    // dark, and the keyboard shortcuts as the link.
    let dark = ui.visuals().dark_mode;
    let mut controls = [
        Control::Icon {
            command: "view.search",
            icon: Icon::Search,
            selected: false,
        },
        Control::Icon {
            command: "view.mode",
            icon: titlebar::theme_icon(dark),
            selected: false,
        },
        Control::Link {
            command: "help.shortcuts",
            icon: Icon::Keyboard,
        },
    ];
    let fired = titlebar::show_with(
        ui,
        &label("title"),
        Some(Icon::Server),
        stroke,
        commands,
        engine,
        strings,
        &mut controls,
    );
    // The toggle goes to the mode opposite the one this frame drew, so a
    // click reported twice cannot flip it back.
    let mut mode_set = false;
    for id in fired {
        if id == "view.mode" {
            if !mode_set {
                let shown = engine.effective_theme().1;
                let target = if shown == design::Mode::Dark {
                    design::Mode::Light
                } else {
                    design::Mode::Dark
                };
                events.push(UiEvent::SetMode(target));
                mode_set = true;
            }
        } else {
            events.push(UiEvent::Command(id));
        }
    }
    bars::status(ui, |ui| {
        let (icon, colour) = if engine.connected {
            (Icon::Plug, ui.visuals().weak_text_color())
        } else {
            (Icon::Unplug, ui.visuals().error_fg_color)
        };
        ui.add(icons::image(icon, stroke, ICON, colour));
        ui.small(&engine.status);
    });
    let total = ui.available_width();
    let left = egui::Panel::left("services")
        .resizable(true)
        .default_size(total * engine.ui.split)
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| services(ui, engine, stroke, &mut events));
    let split = left.response.rect.width() / total.max(1.0);
    if (split - engine.ui.split).abs() > 0.005 {
        events.push(UiEvent::Split(split));
    }
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| details(ui, engine, commands, strings, &mut events));
    if let Some(dialog) = engine.ui.dialog {
        let (title, body) = match dialog {
            Dialog::About => ("about", "about-body"),
            Dialog::Shortcuts => ("shortcuts", "shortcut-body"),
        };
        let id = egui::Id::new("dialog").with(title);
        let shown = dialog::Dialog::new(id, label(title))
            .buttons(vec![Choice::new(label("done"), dialog::Role::Default)])
            .show(ui.ctx(), |ui| ui.add(egui::Label::new(label(body)).wrap()));
        if shown.chosen.is_some() || shown.dismissed {
            // Each opening starts centred again.
            dialog::Dialog::reset(ui.ctx(), id);
            events.push(UiEvent::CloseDialog);
        }
    }
    toolkit::titlebar::edges(ui);
    events
}

fn services(ui: &mut Ui, engine: &Engine, stroke: f32, events: &mut Vec<UiEvent>) {
    let title = label("services");
    panel::group(ui, "services", &[title.as_str()], |ui, _| {
        let mut filter = engine.ui.filter.clone();
        let search = field::search(ui, &mut filter, &label("search"), ui.available_width());
        if search.changed() {
            events.push(UiEvent::Filter(filter));
        }
        if engine.ui.focus_filter {
            search.request_focus();
            events.push(UiEvent::FilterFocused);
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for row in engine.tree() {
                tree_row(ui, engine, &row, stroke, events);
            }
        });
    });
}

/// The text a tree row shows.
fn row_name(row: &Row) -> String {
    match &row.kind {
        RowKind::Service(name) | RowKind::Peer(name) => name.clone(),
        RowKind::Verb(target) => target.verb.clone(),
        RowKind::Error(_) => label("descriptions-failed"),
        RowKind::Peers => label("peers"),
        RowKind::NoPeers => label("no-peers"),
    }
}

fn tree_row(ui: &mut Ui, engine: &Engine, row: &Row, stroke: f32, events: &mut Vec<UiEvent>) {
    let colour = ui.visuals().text_color();
    ui.horizontal(|ui| {
        if row.children.is_empty() {
            ui.add_space(ICON + ui.spacing().item_spacing.x);
        } else {
            let chevron = if row.expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            };
            let toggle = ui
                .add(egui::Button::image(icons::image(chevron, stroke, ICON, colour)).frame(false));
            // An image button has no text: name it, with its row's name, for
            // screen readers and click-by-label.
            let name = label_with(
                if row.expanded { "collapse" } else { "expand" },
                &[("name", &row_name(row))],
            );
            toggle.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &name)
            });
            if toggle.clicked() {
                events.push(UiEvent::Toggle(row.key.clone()));
            }
        }
        let text = match &row.kind {
            RowKind::Service(name) | RowKind::Peer(name) => RichText::new(name),
            RowKind::Verb(target) => RichText::new(&target.verb).monospace(),
            RowKind::Error(_) => {
                RichText::new(label("descriptions-failed")).color(ui.visuals().error_fg_color)
            }
            RowKind::Peers => RichText::new(label("peers")),
            RowKind::NoPeers => RichText::new(label("no-peers")).weak(),
        };
        if let RowKind::Service(_) = row.kind {
            ui.add(icons::image(
                Icon::Server,
                stroke,
                ICON,
                ui.visuals().weak_text_color(),
            ));
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
    let verb = engine
        .ui
        .selected
        .as_ref()
        .and_then(|s| engine.snapshot.verb(s).map(|v| (s, v)));
    if let Some((selection, verb)) = verb {
        let args = if verb.args.is_empty() {
            label("unspecified")
        } else {
            verb.args.clone()
        };
        let read_only = label(match verb.read_only {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        });
        let description = if verb.description.is_empty() {
            label("description-unavailable")
        } else {
            verb.description.clone()
        };
        return format!(
            "{}  {}\n{}: {args}\n{}: {read_only}\n\n{description}",
            selection.service,
            selection.verb,
            label("arguments"),
            label("read-only")
        );
    }
    let row = engine
        .ui
        .row_key
        .as_ref()
        .and_then(|key| find(&engine.tree(), key));
    match row.map(|r| r.kind) {
        Some(RowKind::Error(error)) => error,
        Some(RowKind::Service(name)) => name,
        Some(RowKind::Peer(name)) => format!("{}: {name}", label("peer-membership")),
        _ => label("select"),
    }
}

fn find(rows: &[Row], key: &str) -> Option<Row> {
    rows.iter().find_map(|r| {
        if r.key == key {
            Some(r.clone())
        } else {
            find(&r.children, key)
        }
    })
}

fn details(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let title = label("details");
    panel::group(ui, "details", &[title.as_str()], |ui, _| {
        let rows = ui.text_style_height(&egui::TextStyle::Body) * 6.0;
        ScrollArea::vertical()
            .id_salt("description")
            .max_height(rows)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(description(engine));
            });
        bars::hairline(ui);
        ui.horizontal(|ui| {
            panel::section_label(ui, &label("body"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Call is the default action; Clear the secondary one.
                for id in ["bus.call", "edit.clear"] {
                    let Some(command) = commands.get(id) else {
                        continue;
                    };
                    let text = strings.get(command.label);
                    let button = if id == "bus.call" {
                        PushButton::primary(text)
                    } else {
                        PushButton::secondary(text)
                    };
                    let response = ui.add_enabled((command.enabled)(engine), button);
                    if response
                        .on_hover_text(tooltip::for_command(ui.ctx(), command, strings))
                        .clicked()
                    {
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
        panel::section_label(ui, &label("reply"));
        ScrollArea::vertical()
            .id_salt("reply")
            .auto_shrink(false)
            .show(ui, |ui| {
                let mut reply = engine.reply.as_str();
                ui.add(
                    TextEdit::multiline(&mut reply)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
    });
}
