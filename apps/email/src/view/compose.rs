// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The compose dialog: a modal over the whole window with the address
//! fields, the subject, the body, and Send/Cancel. Send runs the state's
//! recipe (create in Sent, submit); a failure keeps the dialog open with
//! the error so nothing typed is lost.

use egui::{TextEdit, Ui};
use toolkit::dialog::{self, Choice};
use toolkit::field;

use super::UiEvent;
use crate::state::{ComposeField, ComposeState, State};

/// The compose dialog, when a draft is open.
pub fn dialog(ui: &mut Ui, state: &State, events: &mut Vec<UiEvent>) {
    let Some(compose) = &state.compose else {
        return;
    };
    let id = egui::Id::new("compose");
    let mut send = Choice::new(crate::label("send"), dialog::Role::Default);
    send.enabled = state.compose_sendable();
    let choices = vec![
        send,
        Choice::new(crate::label("cancel"), dialog::Role::Cancel),
    ];
    let send_index = 0; // choices[0] is Send; the cancel closes below.
    let shown = dialog::Dialog::new(id, crate::label("compose-title"))
        .buttons(choices)
        .show(ui.ctx(), |ui| {
            form(ui, compose, state, events);
        });
    if shown.chosen == Some(send_index) {
        events.push(UiEvent::SendCompose);
        dialog::Dialog::reset(ui.ctx(), id);
    } else if shown.chosen.is_some() || shown.dismissed {
        dialog::Dialog::reset(ui.ctx(), id);
        events.push(UiEvent::CloseCompose);
    }
}

/// The dialog's fields. Each edit is one event; the state owns the draft.
fn form(ui: &mut Ui, compose: &ComposeState, state: &State, events: &mut Vec<UiEvent>) {
    let width = ui.available_width();
    let line =
        |ui: &mut Ui, label: &str, field: ComposeField, value: &str, events: &mut Vec<UiEvent>| {
            let mut buffer = value.to_owned();
            let response = field::text(ui, &mut buffer, &crate::label(label), width);
            if response.changed() {
                events.push(UiEvent::ComposeField(field, buffer));
            }
        };
    // While a send is in flight every field is read-only: the dialog shows
    // exactly what is being sent.
    ui.add_enabled_ui(!compose.busy, |ui| {
        line(ui, "to", ComposeField::To, &compose.to_text, events);
        line(ui, "cc", ComposeField::Cc, &compose.cc_text, events);
        line(ui, "bcc", ComposeField::Bcc, &compose.bcc_text, events);
        line(
            ui,
            "subject",
            ComposeField::Subject,
            &compose.draft.subject,
            events,
        );
        let mut body = compose.draft.body.clone();
        let editor = TextEdit::multiline(&mut body)
            .desired_rows(8)
            .desired_width(f32::INFINITY)
            .hint_text(crate::label("body"));
        if ui.add(editor).changed() {
            events.push(UiEvent::ComposeField(ComposeField::Body, body));
        }
    });
    if let Some(error) = &compose.error {
        ui.label(
            egui::RichText::new(crate::label_with("send-failed", &[("error", error)]))
                .color(ui.visuals().error_fg_color),
        );
    }
    if compose.busy {
        ui.weak(crate::label("sending"));
    }
    let _ = state;
}
