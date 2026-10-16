// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The mail surface: an account's mailbox list in the centre pane and the
//! open email in the reader (the mailboxes themselves are the tree's).

use egui::{RichText, ScrollArea, Ui};
use jmap::Email;
use toolkit::button::PushButton;
use toolkit::command::Origin;
use toolkit::{Icon, Registry, Strings, icons, panel};

use super::UiEvent;
use crate::state::{AccountView, State};

/// The centre pane's email list.
pub fn list(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let title = view
        .mail
        .selected_mailbox()
        .map(|mailbox| mailbox.name.clone())
        .unwrap_or_else(|| crate::label("mail"));
    let shown = view.mail.emails.len();
    let count = if view.mail.total > 0 {
        crate::label_with(
            "count-of",
            &[
                ("shown", &shown.to_string()),
                ("total", &view.mail.total.to_string()),
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
            let response = ui.add_enabled((command.enabled)(state), PushButton::secondary(text));
            if response.on_hover_text(strings.get("reply")).clicked() {
                events.push(UiEvent::Command(command.id, Origin::Ui));
            }
        });
        if view.mail.busy {
            ui.weak(crate::label("loading"));
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if view.mail.emails.is_empty() && !view.mail.busy {
                ui.weak(crate::label("empty"));
            }
            for email in &view.mail.emails {
                row(ui, email, view, stroke, events);
            }
        });
    });
}

/// One email row: sender, subject, date; unread rows are strong.
fn row(ui: &mut Ui, email: &Email, view: &AccountView, stroke: f32, events: &mut Vec<UiEvent>) {
    let selected = view
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
                events.push(UiEvent::OpenEmail(view.id().to_owned(), email.id.clone()));
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

/// The reader: the open email's headers and body.
pub fn reader(ui: &mut Ui, view: &AccountView) {
    let Some(email) = &view.mail.open else {
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
                    ui.add(egui::Label::new(egui::RichText::new(html).monospace()).wrap());
                }
                None => {
                    ui.weak(crate::label("no-body"));
                }
            },
        }
    });
}
