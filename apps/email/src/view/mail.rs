// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The mail surface: an account's mailbox list in the centre pane and the
//! open email in the reader (the mailboxes themselves are the tree's).

use egui::{RichText, ScrollArea, Ui};
use jmap::Email;
use toolkit::button::{IconButton, Link};
use toolkit::command::{Origin, label_text};
use toolkit::markdown::{self, Document};
use toolkit::{Icon, Registry, Strings, icons, panel};

use super::UiEvent;
use crate::reader::{self, Format};
use crate::state::{AccountView, State};

/// The centre pane's email list.
pub fn list(
    ui: &mut Ui,
    _state: &State,
    view: &AccountView,
    _commands: &Registry<State>,
    _strings: &Strings,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    // Reply, Reply All and Forward live on the open email: the reader's
    // toolbar and the Message menu.
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

/// The reader toolbar's split Reply menu: the commands that pick quoting.
const REPLY_OPTIONS: [&str; 4] = [
    "mail.reply_quoted",
    "mail.reply_unquoted",
    "mail.reply_all_quoted",
    "mail.reply_all_unquoted",
];

/// The divider's height between the toolbar's groups.
const DIVIDER: f32 = 16.0;

/// The reader: a toolbar of what to do with the open email, then its
/// subject, headers and body. A text body is shown as Markdown when it is
/// (or the person asked for it, View › Message), else as the plain text it
/// is. A right-click anywhere in it opens the Message menu.
pub fn reader(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let area = ui.max_rect();
    toolbar(ui, state, commands, strings, events);
    toolkit::bars::hairline(ui);
    if let Some(email) = &view.mail.open {
        message(ui, view, email);
    } else {
        ui.weak(crate::label("select-email"));
    }
    let menu = ui.id().with("reader-menu");
    if let Some(id) = commands.context_menu_over(ui, area, menu, "message", state, strings) {
        events.push(UiEvent::Command(id, Origin::Menu));
    }
}

/// Reply (with its quoting menu), Reply All, Forward, then Delete apart.
/// Each is its registry command: same label, enablement and shortcut.
fn toolbar(
    ui: &mut Ui,
    state: &State,
    commands: &Registry<State>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    ui.horizontal(|ui| {
        let link = |ui: &mut Ui, events: &mut Vec<UiEvent>, id: &'static str, icon: Icon| {
            let Some(command) = commands.get(id) else {
                return;
            };
            let key = commands.label_of(id, state).unwrap_or(command.label);
            let text = label_text(strings, key);
            let mut response = ui.add_enabled((command.enabled)(state), Link::new(icon, text));
            if let Some(chord) = command.shortcut {
                response = response.on_hover_text(ui.ctx().format_shortcut(&chord));
            }
            if response.clicked() {
                events.push(UiEvent::Command(id, Origin::Ui));
            }
        };
        link(ui, events, "mail.reply", Icon::Reply);
        let replying = commands
            .get("mail.reply")
            .is_some_and(|c| (c.enabled)(state));
        let more = ui.add_enabled(
            replying,
            IconButton::new(Icon::ChevronDown)
                .size(ui.spacing().interact_size.y * 0.8)
                .tooltip(crate::label("reply-options")),
        );
        egui::Popup::menu(&more).show(|ui| {
            for id in REPLY_OPTIONS {
                let Some(command) = commands.get(id) else {
                    continue;
                };
                let button = egui::Button::new(label_text(strings, command.label));
                if ui.add_enabled((command.enabled)(state), button).clicked() {
                    events.push(UiEvent::Command(id, Origin::Ui));
                }
            }
        });
        link(ui, events, "mail.reply_all", Icon::ReplyAll);
        link(ui, events, "mail.forward", Icon::Forward);
        toolkit::bars::divider(ui, DIVIDER);
        link(ui, events, "mail.delete", Icon::Trash);
    });
}

/// The open email: subject first and largest, then From, To and the date,
/// then the body.
fn message(ui: &mut Ui, view: &AccountView, email: &Email) {
    let reader = &view.mail.reader;
    // The cache is the open email's once it has loaded; until then (or for
    // an email put in place without opening), detect afresh.
    let format = if reader.is_for(email) {
        reader.format()
    } else {
        reader::detect(email)
    };
    ui.add(
        egui::Label::new(RichText::new(email.subject.as_deref().unwrap_or("")).heading()).wrap(),
    );
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
            Some(body) if format == Format::Markdown => {
                let parsed;
                let document = match reader.document(email) {
                    Some(document) => document,
                    None => {
                        parsed = Document::parse(body);
                        &parsed
                    }
                };
                markdown::show(ui, document);
            }
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
