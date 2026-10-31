// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Agents branch's Details: the New session form, a session's header,
//! its actions and config options, its transcript and the composer. The
//! state and the folding of events into items are the engine's
//! (`inspector::agents`); this only draws them and reports what was asked.
use crate::{label, label_with, view::UiEvent};
use egui::{Align, Key, Layout, RichText, ScrollArea, TextEdit, Ui};
use inspector::{Engine, agents::Item};
use serde_json::Value;
use toolkit::{button::PushButton, panel};

/// A session row in the tree: its name, then its one badge.
pub fn row_text(engine: &Engine, session: &str) -> RichText {
    let badge = engine
        .agents
        .sessions
        .iter()
        .find(|row| row.session == session)
        .map(|row| row.badge().to_owned())
        .unwrap_or_default();
    let name = engine.session_label(session);
    if badge.is_empty() {
        return RichText::new(name);
    }
    let text = RichText::new(format!(
        "{name}  · {}",
        label(&format!("agents-badge-{badge}"))
    ));
    if badge == "question" {
        text.strong()
    } else {
        text
    }
}

pub fn view(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    let agents = &engine.agents;
    if engine.ui.row_key.as_deref() == Some("agents:new") {
        new_session(ui, engine, events);
        return;
    }
    let Some(row) = agents.selected_row().filter(|_| {
        engine.ui.row_key.as_deref()
            == agents
                .selected
                .as_ref()
                .map(|s| format!("agent:{s}"))
                .as_deref()
    }) else {
        ui.label(label("agents-intro"));
        error_line(ui, engine);
        return;
    };
    panel::section_label(ui, &engine.session_label(&row.session));
    egui::Grid::new("agent-session")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label(RichText::new(label("agents-status")).weak());
            ui.label(label(&format!("agents-badge-{}", row.badge())));
            ui.end_row();
            ui.label(RichText::new(label("agents-cwd")).weak());
            ui.label(RichText::new(&row.cwd).monospace());
            ui.end_row();
            for option in selects(&agents.config_options) {
                ui.label(RichText::new(&option.name).weak());
                let mut chosen = option.current.clone();
                egui::ComboBox::from_id_salt(("agent-option", &option.id))
                    .selected_text(option.label_of(&option.current))
                    .show_ui(ui, |ui| {
                        for (value, name) in &option.values {
                            if toolkit::button::selectable(ui, chosen == *value, name).clicked() {
                                chosen = value.clone();
                            }
                        }
                    });
                if chosen != option.current && row.live() {
                    events.push(UiEvent::AgentSet(option.id.clone(), chosen));
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        let running = row.status == "running";
        if running
            && ui
                .add(PushButton::secondary(label("agents-cancel")))
                .clicked()
        {
            events.push(UiEvent::AgentAction("cancel"));
        }
        if row.live()
            && ui
                .add(PushButton::secondary(label("agents-close")))
                .clicked()
        {
            events.push(UiEvent::AgentAction("close"));
        }
        if !row.live()
            && ui
                .add(PushButton::primary(label("agents-resume")))
                .clicked()
        {
            events.push(UiEvent::AgentAction("resume"));
        }
    });
    error_line(ui, engine);
    ui.separator();
    panel::section_label(ui, &label("agents-transcript"));
    let transcript = &agents.transcript;
    if transcript.evicted {
        ui.label(RichText::new(label("agents-evicted")).weak());
    }
    if transcript.gap {
        ui.label(RichText::new(label("agents-gap")).weak());
    }
    // The composer is pinned below the transcript, never inside it.
    let composer = if row.live() {
        ui.text_style_height(&egui::TextStyle::Body) * 5.0 + 36.0
    } else {
        0.0
    };
    let items = agents.items();
    let height = (ui.available_height() - composer).max(60.0);
    if items.is_empty() {
        ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
            ui.label(
                RichText::new(label(if transcript.filling {
                    "agents-filling"
                } else {
                    "agents-empty"
                }))
                .weak(),
            );
        });
    } else {
        // The transcript sticks to the newest line unless the person scrolls up.
        ScrollArea::vertical()
            .id_salt(("agent-transcript", &row.session))
            .auto_shrink([false, false])
            .max_height(height)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (index, item) in items.iter().enumerate() {
                    item_view(ui, index, item);
                }
            });
    }
    if row.live() {
        composer_view(ui, engine, events);
    }
}

fn error_line(ui: &mut Ui, engine: &Engine) {
    if let Some(error) = &engine.agents.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
}

/// The composer: a prompt for the selected session, Ctrl+Enter sends. It is
/// disabled while a turn runs (a second prompt would be refused).
fn composer_view(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    let ready = engine.agents_can_send();
    ui.separator();
    let mut draft = engine.agents.draft.clone();
    let response = ui.add_enabled(
        ready,
        TextEdit::multiline(&mut draft)
            .id_salt("agent-composer")
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .hint_text(label(if ready {
                "agents-composer"
            } else {
                "agents-composer-busy"
            })),
    );
    if response.changed() {
        events.push(UiEvent::AgentDraft(draft.clone()));
    }
    let shortcut =
        response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && i.modifiers.command);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let send = ui.add_enabled(
            ready && !draft.trim().is_empty(),
            PushButton::primary(label("agents-send")),
        );
        if send.clicked() || (shortcut && ready && !draft.trim().is_empty()) {
            events.push(UiEvent::AgentSend);
        }
    });
}

/// The New session form: an agent, a working directory under its roots.
fn new_session(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    let agents = &engine.agents;
    panel::section_label(ui, &label("agents-new"));
    let instances: Vec<&Value> = agents.instances["agents"]
        .as_array()
        .map(|list| list.iter().collect())
        .unwrap_or_default();
    let chosen = instances
        .iter()
        .find(|i| i["agent"].as_str() == Some(agents.new_agent.as_str()));
    egui::Grid::new("agent-new").num_columns(2).show(ui, |ui| {
        ui.label(RichText::new(label("agents-agent")).weak());
        let mut agent = agents.new_agent.clone();
        egui::ComboBox::from_id_salt("agent-new-agent")
            .selected_text(if agent.is_empty() {
                label("agents-choose")
            } else {
                agent.clone()
            })
            .show_ui(ui, |ui| {
                for instance in &instances {
                    let name = instance["agent"].as_str().unwrap_or_default();
                    if toolkit::button::selectable(ui, agent == name, name).clicked() {
                        agent = name.to_owned();
                    }
                }
            });
        if agent != agents.new_agent {
            events.push(UiEvent::AgentNewAgent(agent));
        }
        ui.end_row();
        ui.label(RichText::new(label("agents-cwd")).weak());
        let mut cwd = agents.new_cwd.clone();
        let roots = chosen
            .and_then(|i| i["roots"].as_array())
            .map(|roots| {
                roots
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        if ui
            .add(
                TextEdit::singleline(&mut cwd)
                    .id_salt("agent-new-cwd")
                    .desired_width(f32::INFINITY)
                    .hint_text(roots),
            )
            .changed()
        {
            events.push(UiEvent::AgentNewCwd(cwd));
        }
        ui.end_row();
    });
    let live = agents.instances["live"].as_u64().unwrap_or_default();
    let cap = agents.instances["max_live"].as_u64().unwrap_or_default();
    ui.label(
        RichText::new(label_with(
            "agents-live",
            &[("live", &live.to_string()), ("cap", &cap.to_string())],
        ))
        .weak(),
    );
    let ready = !engine.agents_at_cap()
        && !agents.new_agent.trim().is_empty()
        && !agents.new_cwd.trim().is_empty();
    if ui
        .add_enabled(ready, PushButton::primary(label("agents-start")))
        .clicked()
    {
        events.push(UiEvent::AgentStart);
    }
    error_line(ui, engine);
}

/// One select-type config option the session reported (model, effort, …).
struct Select {
    id: String,
    name: String,
    current: String,
    values: Vec<(String, String)>,
}

impl Select {
    fn label_of(&self, value: &str) -> String {
        self.values
            .iter()
            .find(|(v, _)| v == value)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| value.to_owned())
    }
}

/// The session's select options; grouped options are flattened.
fn selects(config: &Value) -> Vec<Select> {
    fn values(options: &Value, out: &mut Vec<(String, String)>) {
        for option in options.as_array().into_iter().flatten() {
            if let Some(value) = option["value"].as_str() {
                let name = option["name"].as_str().unwrap_or(value);
                out.push((value.to_owned(), name.to_owned()));
            } else {
                values(&option["options"], out);
            }
        }
    }
    config
        .as_array()
        .into_iter()
        .flatten()
        .filter(|option| option["type"] == "select")
        .filter_map(|option| {
            let mut list = Vec::new();
            values(&option["options"], &mut list);
            Some(Select {
                id: option["id"].as_str()?.to_owned(),
                name: option["name"]
                    .as_str()
                    .or_else(|| option["id"].as_str())?
                    .to_owned(),
                current: option["currentValue"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                values: list,
            })
        })
        .collect()
}

fn item_view(ui: &mut Ui, index: usize, item: &Item) {
    match item {
        Item::Prompt { text } => {
            ui.add_space(6.0);
            ui.label(RichText::new(label("agents-you")).strong());
            ui.label(text);
        }
        Item::Message { text } => {
            ui.add_space(4.0);
            ui.label(text);
        }
        Item::Thought { text } => {
            // Several thoughts in one transcript: each needs its own id.
            egui::CollapsingHeader::new(RichText::new(label("agents-thought")).weak())
                .id_salt(("agent-thought", index))
                .show(ui, |ui| {
                    ui.label(RichText::new(text).weak().italics());
                });
        }
        Item::Tool {
            id,
            title,
            tool,
            status,
            raw,
        } => {
            let name = if !title.is_empty() {
                title.as_str()
            } else if !tool.is_empty() {
                tool.as_str()
            } else {
                "tool"
            };
            let head = format!("{name}  [{status}]");
            egui::CollapsingHeader::new(RichText::new(head).monospace())
                .id_salt(("agent-tool", id))
                .show(ui, |ui| {
                    let text = serde_json::to_string_pretty(raw).unwrap_or_default();
                    ui.label(RichText::new(text).monospace().small());
                });
        }
        Item::Plan { entries } => {
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                for entry in entries {
                    ui.label(RichText::new(entry).small());
                }
            });
        }
        Item::Stop { reason } => {
            ui.label(
                RichText::new(label_with("agents-stop", &[("reason", reason)]))
                    .weak()
                    .small(),
            );
            ui.separator();
        }
        Item::Error { message } => {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
        Item::Note { kind, text } => {
            ui.label(RichText::new(format!("{kind} {text}")).weak().small());
        }
    }
}
