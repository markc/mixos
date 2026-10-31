// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Agents branch's Details: a session's header and its transcript. The
//! state and the folding of events into items are the engine's
//! (`inspector::agents`); this only draws them.
use crate::{label, label_with};
use egui::{Align, Layout, RichText, ScrollArea, Ui};
use inspector::{Engine, agents::Item};
use toolkit::panel;

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

pub fn view(ui: &mut Ui, engine: &Engine) {
    let agents = &engine.agents;
    let Some(row) = agents.selected_row().filter(|_| {
        engine.ui.row_key.as_deref()
            == agents
                .selected
                .as_ref()
                .map(|s| format!("agent:{s}"))
                .as_deref()
    }) else {
        ui.label(label("agents-intro"));
        if let Some(error) = &agents.error {
            ui.label(RichText::new(error).weak());
        }
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
        });
    ui.separator();
    panel::section_label(ui, &label("agents-transcript"));
    let transcript = &agents.transcript;
    if transcript.evicted {
        ui.label(RichText::new(label("agents-evicted")).weak());
    }
    let items = agents.items();
    if items.is_empty() {
        ui.label(
            RichText::new(label(if transcript.filling {
                "agents-filling"
            } else {
                "agents-empty"
            }))
            .weak(),
        );
        return;
    }
    // The transcript sticks to the newest line unless the person scrolls up.
    ScrollArea::vertical()
        .id_salt(("agent-transcript", &row.session))
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (index, item) in items.iter().enumerate() {
                item_view(ui, index, item);
            }
        });
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
