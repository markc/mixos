// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
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
use toolkit::command::Origin;
use toolkit::dialog::{self, Choice};
use toolkit::titlebar::{self, Action};
use toolkit::{Icon, Registry, Strings, bars, field, icons, panel, tooltip};

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Macro(String, serde_json::Value),
    Dock(bool),
    Pause(bool),
    TrafficFilter(String),
    Command(&'static str, Origin),
    CloseMacros,
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
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    // The icon bar: refresh, then the standard tail.
    let fired = titlebar::show_bar(
        ui,
        &label("title"),
        Some(Icon::Server),
        commands,
        engine,
        strings,
        &[Action::new("file.refresh", Icon::RefreshCw)],
    );
    // The toggle goes to the mode opposite the one this frame drew, so a
    // click reported twice cannot flip it back.
    let mut mode_set = false;
    for (id, origin) in fired {
        if id == "view.mode" {
            if !mode_set {
                commands.note(id, origin);
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
            events.push(UiEvent::Command(id, origin));
        }
    }
    bars::status(ui, |ui| {
        let (icon, colour) = if engine.connected {
            (Icon::Plug, ui.visuals().weak_text_color())
        } else {
            (Icon::Unplug, ui.visuals().error_fg_color)
        };
        ui.add(icons::image_of(ui.ctx(), icon, ICON, colour));
        ui.small(&engine.status);
    });
    let total = ui.available_width();
    egui::Panel::bottom("traffic").resizable(true).default_size(if engine.workshop.dock_open { 180.0 } else { 28.0 })
        .frame(bars::dock_frame(ui.ctx())).show(ui, |ui| traffic(ui, engine, &mut events));
    let left = egui::Panel::left("services")
        .resizable(true)
        .default_size(total * engine.ui.split)
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| services(ui, engine, &mut events));
    let split = left.response.rect.width() / total.max(1.0);
    if (split - engine.ui.split).abs() > 0.005 {
        events.push(UiEvent::Split(split));
    }
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| details(ui, engine, commands, strings, &mut events));
    if let Some(dialog) = engine.ui.dialog {
        let title = match dialog {
            Dialog::About => "about",
            Dialog::Shortcuts => "shortcuts",
        };
        let id = egui::Id::new("dialog").with(title);
        // The shortcuts in force, from the registry: the profile can rebind them.
        let shown = dialog::Dialog::new(id, label(title))
            .buttons(vec![Choice::new(label("done"), dialog::Role::Default)])
            .show(ui.ctx(), |ui| match dialog {
                Dialog::About => {
                    ui.add(egui::Label::new(label("about-body")).wrap());
                }
                Dialog::Shortcuts => commands.shortcut_grid(ui, strings),
            });
        if shown.chosen.is_some() || shown.dismissed {
            // Each opening starts centred again.
            dialog::Dialog::reset(ui.ctx(), id);
            events.push(UiEvent::CloseDialog);
        }
    }
    if toolkit::macros::show_app(ui.ctx(), "workshop", engine.ui.macros_open) {
        events.push(UiEvent::CloseMacros);
    }
    toolkit::titlebar::edges(ui);
    events
}

fn services(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
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
                tree_row(ui, engine, &row, events);
            }
        });
    });
}

/// The text a tree row shows.
fn row_name(engine: &Engine, row: &Row) -> String {
    match &row.kind {
        RowKind::MacroGroup(title) | RowKind::Macro { title, .. } => title.clone(),
        RowKind::Service(name) => engine.service_label(name),
        RowKind::Peer(name) => name.clone(),
        RowKind::Node(name) => engine.node_label(name),
        RowKind::Verb(target) => target.verb.clone(),
        RowKind::Error(_) => label("descriptions-failed"),
        RowKind::Peers => label("peers"),
        RowKind::NoPeers => label("no-peers"),
    }
}

fn tree_row(ui: &mut Ui, engine: &Engine, row: &Row, events: &mut Vec<UiEvent>) {
    let colour = ui.visuals().text_color();
    ui.horizontal(|ui| {
        // A diagnostic belongs to the row, not to the panel's minimum width.
        // Keep the complete reason available when the row is hovered.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        if row.children.is_empty() {
            ui.add_space(ICON + ui.spacing().item_spacing.x);
        } else {
            let chevron = if row.expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            };
            let toggle = ui.add(
                egui::Button::image(icons::image_of(ui.ctx(), chevron, ICON, colour)).frame(false),
            );
            // An image button has no text: name it, with its row's name, for
            // screen readers and click-by-label.
            let name = label_with(
                if row.expanded { "collapse" } else { "expand" },
                &[("name", &row_name(engine, row))],
            );
            toggle.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &name)
            });
            if toggle.clicked() {
                events.push(UiEvent::Toggle(row.key.clone()));
            }
        }
        let text = match &row.kind {
            RowKind::MacroGroup(title) | RowKind::Macro { title, .. } => RichText::new(title),
            RowKind::Service(name) => RichText::new(engine.service_label(name)),
            RowKind::Peer(name) => RichText::new(name),
            RowKind::Node(name) => RichText::new(engine.node_label(name)),
            RowKind::Verb(target) => RichText::new(&target.verb).monospace(),
            RowKind::Error(_) => {
                RichText::new(label("descriptions-failed")).color(ui.visuals().error_fg_color)
            }
            RowKind::Peers => RichText::new(label("peers")),
            RowKind::NoPeers => RichText::new(label("no-peers")).weak(),
        };
        if let RowKind::Service(_) = row.kind {
            ui.add(icons::image_of(
                ui.ctx(),
                Icon::Server,
                ICON,
                ui.visuals().weak_text_color(),
            ));
        }
        let selected = engine.ui.row_key.as_deref() == Some(row.key.as_str());
        if toolkit::selectable(ui, selected, text)
            .on_hover_text(row_name(engine, row))
            .clicked()
        {
            events.push(UiEvent::Select(row.clone()));
        }
    });
    if row.expanded {
        ui.indent(&row.key, |ui| {
            for child in &row.children {
                tree_row(ui, engine, child, events);
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
        Some(RowKind::Service(name)) => engine
            .snapshot
            .identities
            .get(&name)
            .and_then(|info| serde_json::to_string_pretty(info).ok())
            .unwrap_or(name),
        Some(RowKind::Node(name)) => engine
            .snapshot
            .nodes
            .iter()
            .find(|node| node.name == name)
            .and_then(|node| serde_json::to_string_pretty(&node.info).ok())
            .unwrap_or(name),
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
    if engine.workshop.selected.is_some() {
        macro_details(ui, engine, events);
        return;
    }
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
                        events.push(UiEvent::Command(command.id, Origin::Ui));
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

fn traffic(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    ui.horizontal(|ui| {
        if ui.selectable_label(engine.workshop.dock_open, label("traffic")).clicked() {
            events.push(UiEvent::Dock(!engine.workshop.dock_open));
        }
        let mut paused = engine.workshop.paused;
        if ui.checkbox(&mut paused, label("traffic-pause")).changed() { events.push(UiEvent::Pause(paused)); }
        let mut filter = engine.workshop.filter.clone();
        if ui.add(TextEdit::singleline(&mut filter).hint_text(label("traffic-filter")).desired_width(160.0)).changed() {
            events.push(UiEvent::TrafficFilter(filter));
        }
        ui.small(format!("{} · {} {}", engine.workshop.observe, engine.workshop.dropped, label("traffic-dropped")));
    });
    if !engine.workshop.dock_open { return; }
    ScrollArea::vertical().id_salt("traffic-rows").show(ui, |ui| {
        ui.monospace(label("traffic-columns"));
        for (index, row) in engine.workshop.rows(100, &engine.workshop.filter).iter().enumerate() {
            let text = |key: &str| row[key].as_str().map(str::to_owned).unwrap_or_else(|| if row[key].is_null() { String::new() } else { row[key].to_string() });
            let title = format!("{}  {}  {}  {}  {}  {}  {}", text("ts"), text("direction"), text("from"), text("service"), text("verb"), text("rc"), text("ms"));
            egui::CollapsingHeader::new(RichText::new(title).monospace()).id_salt(index).show(ui, |ui| {
                ui.monospace(serde_json::to_string_pretty(row).unwrap_or_default());
            });
        }
    });
}

fn macro_details(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    let Some((app, id)) = &engine.workshop.selected else { return; };
    let document = &engine.workshop.document;
    let entry = engine.workshop.catalogues.get(app).and_then(|c| c["macros"].as_array())
        .and_then(|entries| entries.iter().find(|entry| entry["id"] == *id));
    ui.heading(entry.and_then(|entry| entry["title"].as_str()).unwrap_or(id));
    ui.small(format!("{} · {}", document["tier"].as_str().unwrap_or_default(), document["path"].as_str().unwrap_or_default()));
    if let Some(entry) = entry { ui.small(format!("{} {}", label("macro-shadows"), entry["shadows"])); }
    type Document = std::sync::Arc<std::sync::Mutex<editor::local::Document>>;
    let key = egui::Id::new(("workshop-document", app, id, document["text"].as_str().unwrap_or_default()));
    let doc: Option<Document> = ui.ctx().data_mut(|data| {
        if let Some(doc) = data.get_temp::<Document>(key) { return Some(doc); }
        let doc = editor::local::Document::new(document["text"].as_str().unwrap_or_default(), "mix").ok()?;
        let doc = std::sync::Arc::new(std::sync::Mutex::new(doc));
        data.insert_temp(key, doc.clone());
        Some(doc)
    });
    let Some(doc) = doc else { return; };
    let mut doc = doc.lock().unwrap();
    let palette = ui.ctx().data(|data| data.get_temp::<editor::Palette>(egui::Id::new("toolkit.editor-palette")))
        .unwrap_or_else(|| editor::Palette::from_theme(&toolkit::Theme::embedded()));
    ui.allocate_ui(egui::vec2(ui.available_width(), (ui.available_height() - 150.0).max(100.0)), |ui| {
        doc.show(ui, &palette, &editor::View::default());
    });
    let mut target = ui.ctx().data_mut(|data| data.get_temp::<String>(key.with("target")).unwrap_or_default());
    ui.horizontal_wrapped(|ui| {
        if ui.button(label("macro-save")).clicked() {
            events.push(UiEvent::Macro("macros.put".into(), serde_json::json!({"text":doc.contents()})));
        }
        if app == "global" {
            let targets = entry.and_then(|entry| entry.get("targets")).unwrap_or(&serde_json::Value::Null);
            let apps: Vec<String> = if targets == "any" {
                engine.workshop.catalogues.iter().filter(|(app, c)| *app != "global" && c["running"] == true).map(|(app, _)| app.clone()).collect()
            } else if targets.is_null() { engine.workshop.catalogues.keys().filter(|app| *app != "global").cloned().collect() }
            else { targets.as_array().into_iter().flatten().filter_map(|app| app.as_str().map(str::to_owned)).collect() };
            egui::ComboBox::from_id_salt("macro-target").selected_text(if target.is_empty() { label("macro-target") } else { target.clone() }).show_ui(ui, |ui| {
                for app in apps { ui.selectable_value(&mut target, app.clone(), app); }
            });
        }
        if ui.button(label("macro-run")).clicked() {
            let has_target = entry.is_some_and(|entry| !entry["targets"].is_null());
            let args = if target.is_empty() || !has_target { serde_json::json!({}) } else { serde_json::json!({"target":target}) };
            events.push(UiEvent::Macro("macros.run".into(), args));
        }
        if ui.button(label("macro-stop")).clicked() { events.push(UiEvent::Macro("macros.stop".into(), serde_json::json!({}))); }
        for (operation, label_key, args) in [
            ("macros.copy", "macro-copy", serde_json::json!({})),
            ("macros.move", "macro-move", if app == "global" { serde_json::json!({"tier":"user-app","destination":target}) } else { serde_json::json!({"tier":"user-global"}) }),
            ("macros.ced", "macro-ced", serde_json::json!({})),
            ("macros.dopus", "macro-dopus", serde_json::json!({})),
        ] {
            if ui.button(label(label_key)).clicked() { events.push(UiEvent::Macro(operation.into(), args)); }
        }
        if ui.button(label("macro-delete")).clicked() { ui.ctx().data_mut(|data| data.insert_temp(key.with("delete"), true)); }
    });
    ui.ctx().data_mut(|data| data.insert_temp(key.with("target"), target));
    if ui.ctx().data(|data| data.get_temp::<bool>(key.with("delete")).unwrap_or(false)) {
        ui.horizontal(|ui| {
            ui.label(label("macro-delete-confirm"));
            if ui.button(label("macro-delete")).clicked() {
                events.push(UiEvent::Macro("macros.remove".into(), serde_json::json!({})));
                ui.ctx().data_mut(|data| data.insert_temp(key.with("delete"), false));
            }
            if ui.button(label("macro-cancel")).clicked() { ui.ctx().data_mut(|data| data.insert_temp(key.with("delete"), false)); }
        });
    }
    ui.monospace(&engine.workshop.message);
}
