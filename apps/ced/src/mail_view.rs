// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::{app::App, label, view::UiEvent};
use toolkit::header_form as form;

fn field_id(ctx: &egui::Context, tab: u64, key: &str) -> egui::Id {
    egui::Id::unique((ctx.viewport_id(), "ced.mail.field", tab, key))
}

pub fn header_focused(ctx: &egui::Context, app: &App) -> bool {
    let Some(tab) = app.active_tab().filter(|tab| tab.mail.is_some()) else { return false; };
    let focused = ctx.memory(|memory| memory.focused());
    ["mail-from", "mail-to", "mail-cc", "mail-bcc", "mail-reply-to", "mail-subject"].iter()
        .any(|key| focused == Some(field_id(ctx, tab.id, key)))
}

pub fn strip(ui: &mut egui::Ui, app: &App, events: &mut Vec<UiEvent>) {
    let Some(tab) = app.active_tab() else { return; };
    if let Some(notice) = &tab.document_notice { ui.weak(notice); }
    let Some(mail) = &tab.mail else { return; };
    let column = form::column(ui, ["mail-from", "mail-to", "mail-cc", "mail-bcc", "mail-reply-to", "mail-subject", "mail-in-reply-to", "mail-references", "mail-attachments"].map(label));
    let mut headers = mail.headers.clone();
    let before = headers.clone();
    ui.push_id((ui.ctx().viewport_id(), "ced.mail", tab.id), |ui| {
        ui.add_enabled_ui(!mail.sending && !mail.saving, |ui| {
            for (key, buffer, shown) in [
                ("mail-from", &mut headers.from, true), ("mail-to", &mut headers.to, true),
                ("mail-cc", &mut headers.cc, mail.show_cc), ("mail-bcc", &mut headers.bcc, mail.show_bcc),
                ("mail-reply-to", &mut headers.reply_to, mail.show_reply_to),
                ("mail-subject", &mut headers.subject, true),
            ] {
                if !shown { continue; }
                let field_id = field_id(ui.ctx(), tab.id, key);
                form::row(ui, &label(key), column, |ui, labelled| {
                    if key == "mail-to" {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (field, icon, selected) in [("reply-to", toolkit::Icon::Reply, mail.show_reply_to), ("bcc", toolkit::Icon::EyeOff, mail.show_bcc), ("cc", toolkit::Icon::Copy, mail.show_cc)] {
                                if ui.add(toolkit::button::IconButton::new(icon).size(toolkit::field::height_of(ui.ctx())).selected(selected).tooltip(label(&format!("mail-show-{field}")))).clicked() { events.push(UiEvent::MailRow(tab.id, field)); }
                            }
                            let width = ui.available_width();
                            form::text(ui, field_id, labelled, buffer, width);
                        });
                    } else {
                        form::text(ui, field_id, labelled, buffer, ui.available_width());
                    }
                });
            }
            if let Some(value) = &headers.in_reply_to {
                form::row(ui, &label("mail-in-reply-to"), column, |ui, _| { ui.label(value); });
            }
            if !headers.references.is_empty() {
                form::row(ui, &label("mail-references"), column, |ui, _| { ui.label(headers.references.join(" ")); });
            }
            let mut remove = None;
            form::row(ui, &label("mail-attachments"), column, |ui, _| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button(label("mail-attach")).clicked() { events.push(UiEvent::MailAttach(tab.id)); }
                    for (index, path) in headers.attachments.iter().enumerate() {
                        if form::attachment_chip(ui, index, path, &path.display().to_string(), false, toolkit::field::height_of(ui.ctx()), &label("mail-remove")) { remove = Some(index); }
                    }
                });
            });
            if let Some(index) = remove { headers.attachments.remove(index); }
        });
    });
    if headers != before { events.push(UiEvent::MailHeaders(tab.id, headers)); }
    if let Some(notice) = &mail.notice {
        ui.weak(if mail.sending { label("mail-sending") } else {
            match notice.as_str() { "Sent" => label("mail-sent"), "Saved" => label("mail-saved"), _ => notice.clone() }
        });
    }
}

pub fn chooser(ctx: &egui::Context, app: &mut App, events: &mut Vec<UiEvent>) {
    let Some((tab, chooser)) = &mut app.mail_chooser else { return; };
    if let Some(outcome) = chooser.show(ctx) {
        if let toolkit::file_chooser::Outcome::Picked(paths) = outcome
            && let Some(mail) = app.ctl.tabs().iter().find(|t| t.id == *tab).and_then(|t| t.mail.as_ref()) {
                let mut headers = mail.headers.clone();
                headers.attachments.extend(paths);
                events.push(UiEvent::MailHeaders(*tab, headers));
            }
        app.mail_chooser = None;
    }
}
