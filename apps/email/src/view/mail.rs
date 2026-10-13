// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The mail surface: a mailbox's list in the centre pane, the open email
//! in the reader, a mailbox row in the places panel.

use egui::{RichText, ScrollArea, Ui};
use jmap::{Email, Mailbox};
use toolkit::button::PushButton;
use toolkit::command::Origin;
use toolkit::{Icon, Registry, Strings, icons, panel};

use super::UiEvent;
use crate::state::State;

/// The centre pane's email list.
pub fn list(
    ui: &mut Ui,
    state: &State,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let title = state
        .mail
        .selected_mailbox()
        .map(|mailbox| mailbox.name.clone())
        .unwrap_or_else(|| crate::label("mail"));
    let shown = state.mail.emails.len();
    let count = if state.mail.total > 0 {
        crate::label_with(
            "count-of",
            &[
                ("shown", &shown.to_string()),
                ("total", &state.mail.total.to_string()),
            ],
        )
    } else {
        String::new()
    };
    panel::group(ui, "emails", &[title.as_str(), count.as_str()], |ui, _| {
        // Reply works from the open email; the button sits where the reader
        // is not (a person reading in the reader panel replies from the
        // menu or the shortcut).
        ui.horizontal(|ui| {
            let Some(command) = commands.get("mail.reply") else {
                return;
            };
            let text = strings.get(command.label);
            let response = ui.add_enabled(
                (command.enabled)(state) && state.mail.open.is_some(),
                PushButton::secondary(text),
            );
            if response
                .on_hover_text(strings.get("reply"))
                .clicked()
            {
                events.push(UiEvent::Command(command.id, Origin::Ui));
            }
        });
        if state.mail.busy {
            ui.weak(crate::label("loading"));
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if state.mail.emails.is_empty() && !state.mail.busy {
                ui.weak(crate::label("empty"));
            }
            for email in &state.mail.emails {
                row(ui, email, state, stroke, events);
            }
        });
    });
}

/// One email row: sender, subject, date; unread rows are strong.
fn row(ui: &mut Ui, email: &Email, state: &State, stroke: f32, events: &mut Vec<UiEvent>) {
    let selected = state
        .mail
        .open
        .as_ref()
        .is_some_and(|open| open.id == email.id);
    let strong = |text: String| {
        if email.is_read() {
            RichText::new(text)
        } else {
            RichText::new(text).strong()
        }
    };
    egui::Sides::new().show(
        ui,
        |ui| {
            if toolkit::selectable(ui, selected, strong(email.from_display())).clicked() {
                events.push(UiEvent::OpenEmail(email.id.clone()));
            }
        },
        |ui| {
            let date = if email.is_read() {
                RichText::new(email.date_short()).weak()
            } else {
                RichText::new(email.date_short()).strong()
            };
            ui.label(date);
        },
    );
    egui::Sides::new().show(
        ui,
        |ui| {
            let subject = email.subject.clone().unwrap_or_default();
            let text = if email.is_read() {
                RichText::new(subject).weak()
            } else {
                RichText::new(subject).strong()
            };
            ui.label(text);
        },
        |ui| {
            if email.has_attachment == Some(true) {
                let weak = ui.visuals().weak_text_color();
                ui.add(icons::image(Icon::Paperclip, stroke, 12.0, weak));
            }
        },
    );
}

/// A mailbox row in the places panel, with its unread count.
pub fn place_row(ui: &mut Ui, mailbox: &Mailbox, state: &State, stroke: f32, events: &mut Vec<UiEvent>) {
    let selected = state.mail.selected.as_deref() == Some(mailbox.id.as_str());
    let icon = match mailbox.role.as_deref() {
        Some("inbox") => Icon::Inbox,
        Some("sent") => Icon::Send,
        Some("drafts") => Icon::FilePen,
        Some("trash") => Icon::Trash2,
        Some("junk") | Some("spam") => Icon::ShieldAlert,
        Some("archive") | Some("all") => Icon::Archive,
        _ => Icon::Folder,
    };
    let weak = ui.visuals().weak_text_color();
    egui::Sides::new().show(
        ui,
        |ui| {
            ui.add(icons::image(icon, stroke, 12.0, weak));
            let text = if selected {
                RichText::new(&mailbox.name).strong()
            } else {
                RichText::new(&mailbox.name)
            };
            if toolkit::selectable(ui, selected, text).clicked() {
                events.push(UiEvent::SelectMailbox(mailbox.id.clone()));
            }
        },
        |ui| {
            if mailbox.unread_emails > 0 {
                ui.label(RichText::new(mailbox.unread_emails.to_string()).strong());
            }
        },
    );
}

/// The reader: the open email's headers and body.
pub fn reader(ui: &mut Ui, state: &State) {
    let Some(email) = &state.mail.open else {
        ui.weak(crate::label("select-email"));
        return;
    };
    panel::section_label(ui, email.subject.as_deref().unwrap_or(""));
    ui.label(
        RichText::new(crate::label_with(
            "from-line",
            &[("from", &email.from_display())],
        ))
        .weak(),
    );
    let to = email
        .to
        .as_ref()
        .map(|to| {
            to.iter()
                .map(|a| a.email.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if !to.is_empty() {
        ui.label(RichText::new(crate::label_with("to-line", &[("to", &to)])).weak());
    }
    ui.label(RichText::new(email.date_short()).weak());
    toolkit::bars::hairline(ui);
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        match email.text_body_value() {
            Some(body) => {
                ui.add(egui::Label::new(body).wrap());
            }
            None => match email.html_body_value() {
                // No HTML rendering in this slice: the source, escaped by
                // the label, with a note.
                Some(html) => {
                    ui.weak(crate::label("html-source"));
                    ui.add(egui::Label::new(html).wrap().monospace());
                }
                None => ui.weak(crate::label("no-body")),
            },
        }
    });
}
