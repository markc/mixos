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
use crate::state::{AccountView, State, Transfer};

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
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let area = ui.max_rect();
    toolbar(ui, state, view, commands, strings, stroke, events);
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
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
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
        // The paperclip and its count, only when there are attachments.
        let attached = view
            .mail
            .open
            .as_ref()
            .and_then(|e| e.attachments.as_ref())
            .filter(|list| !list.is_empty());
        if let (Some(list), Some(email)) = (attached, view.mail.open.as_ref()) {
            toolkit::bars::divider(ui, DIVIDER);
            let count = list.len().to_string();
            let clip = ui
                .add(Link::new(Icon::Paperclip, count.as_str()))
                .on_hover_text(crate::label_with("attachment-count", &[("count", &count)]));
            if clip.clicked() {
                events.push(UiEvent::Command("mail.attachments", Origin::Ui));
            }
            attachments_popup(ui, &clip, view, email, stroke, events);
        }
    });
}

/// The type icon for an attachment of type `mime`.
fn type_icon(mime: Option<&str>) -> Icon {
    let mime = mime.unwrap_or("").to_ascii_lowercase();
    match mime.split('/').next().unwrap_or("") {
        "image" => Icon::FileImage,
        "audio" => Icon::FileMusic,
        "video" => Icon::FileVideoCamera,
        "text" if mime.starts_with("text/csv") => Icon::FileSpreadsheet,
        "text" => Icon::FileText,
        _ if mime.contains("zip") || mime.contains("tar") || mime.contains("compressed") => {
            Icon::FileArchive
        }
        _ if mime.contains("spreadsheet") || mime.contains("excel") => Icon::FileSpreadsheet,
        _ if mime.contains("pdf") || mime.contains("document") || mime.contains("word") => {
            Icon::FileText
        }
        _ => Icon::File,
    }
}

/// The attachments popup under the paperclip: each attachment's type,
/// name (elided, whole on hover) and size, with Save and Open; Save All
/// at the foot. A part that could not be decoded is greyed with a note;
/// a program or script offers Save only.
fn attachments_popup(
    ui: &Ui,
    clip: &egui::Response,
    view: &AccountView,
    email: &Email,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let open = view.mail.attachments_open;
    let shown = egui::Popup::from_response(clip)
        .id(clip.id.with("attachments"))
        .open(open)
        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
        .show(|ui| {
            let body = ui.text_style_height(&egui::TextStyle::Body);
            let name_width = body * 16.0;
            ui.set_min_width(body * 26.0);
            ui.label(RichText::new(crate::label("attachments-title")).strong());
            let weak = ui.visuals().weak_text_color();
            for (index, a) in email.attachments.iter().flatten().enumerate() {
                let name = crate::attachments::file_name(
                    a.name.as_deref(),
                    index + 1,
                    a.content_type.as_deref(),
                );
                let part = a.part_id.clone().unwrap_or_default();
                let usable = a.downloadable();
                ui.horizontal(|ui| {
                    let ink = if usable {
                        ui.visuals().text_color()
                    } else {
                        weak
                    };
                    ui.add(icons::image(
                        type_icon(a.content_type.as_deref()),
                        stroke,
                        14.0,
                        ink,
                    ));
                    // Fixed name and size columns, so the actions line up.
                    let height = ui.spacing().interact_size.y;
                    ui.allocate_ui(egui::vec2(name_width, height), |ui| {
                        ui.set_min_width(name_width);
                        let label = RichText::new(&name).color(ink);
                        ui.add(egui::Label::new(label).truncate())
                            .on_hover_text(a.name.as_deref().unwrap_or(&name));
                    });
                    let size = RichText::new(crate::attachments::human_size(a.size)).weak();
                    ui.allocate_ui_with_layout(
                        egui::vec2(body * 4.0, height),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(body * 4.0);
                            ui.label(size);
                        },
                    );
                    if !usable {
                        ui.label(
                            RichText::new(crate::label("attachment-undecodable"))
                                .weak()
                                .small(),
                        );
                        return;
                    }
                    if ui
                        .add(Link::new(Icon::Download, crate::label("save")))
                        .clicked()
                    {
                        events.push(UiEvent::SaveAttachment(part.clone()));
                    }
                    let save_only =
                        crate::attachments::refuses_open(&name, a.content_type.as_deref());
                    let opened = ui.add_enabled(
                        !save_only,
                        Link::new(Icon::ExternalLink, crate::label("open")),
                    );
                    let opened = if save_only {
                        opened.on_disabled_hover_text(crate::label("attachment-save-only"))
                    } else {
                        opened
                    };
                    if opened.clicked() {
                        events.push(UiEvent::OpenAttachment(part.clone()));
                    }
                });
                if let Some(transfer) = view.mail.transfers.get(&part) {
                    let note = match transfer {
                        Transfer::Saving => crate::label("attachment-saving"),
                        Transfer::Opening => crate::label("attachment-opening"),
                        Transfer::Saved(path) => {
                            crate::label_with("attachment-saved", &[("path", path)])
                        }
                        Transfer::Opened(path) => {
                            crate::label_with("attachment-opened", &[("path", path)])
                        }
                        Transfer::Failed(error) => {
                            crate::label_with("attachment-failed", &[("error", error)])
                        }
                    };
                    ui.label(RichText::new(note).weak().small());
                }
            }
            ui.separator();
            let any = email.attachments.iter().flatten().any(|a| a.downloadable());
            if ui
                .add_enabled(any, Link::new(Icon::Download, crate::label("save-all")))
                .clicked()
            {
                events.push(UiEvent::Command("mail.save_attachments", Origin::Ui));
            }
        });
    // A click outside the popup and the paperclip closes it.
    if open
        && let Some(shown) = shown
        && ui.input(|i| i.pointer.any_click())
        && let Some(at) = ui.input(|i| i.pointer.interact_pos())
        && !shown.response.rect.contains(at)
        && !clip.rect.contains(at)
    {
        events.push(UiEvent::AttachmentsOpen(false));
    }
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
