// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The identities dialog: one account's sender identities, a list and the
//! selected identity's fields. Save hands the list to the settings rules;
//! a refused list stays open with the reason.

use egui::{TextEdit, Ui};
use toolkit::dialog::{self, Choice};
use toolkit::field;

use super::UiEvent;
use crate::state::{IdentityEditor, IdentityField, State};

/// Show the dialog while an identities editor is open.
pub fn dialog(ui: &mut Ui, state: &State, events: &mut Vec<UiEvent>) {
    let Some(editor) = &state.identity_editor else {
        return;
    };
    let id = egui::Id::new("identities");
    let choices = vec![
        Choice::new(crate::label("identity-save"), dialog::Role::Default),
        Choice::new(crate::label("identity-close"), dialog::Role::Cancel),
    ];
    let shown = dialog::Dialog::new(id, crate::label("identities-title"))
        .buttons(choices)
        .show(ui.ctx(), |ui| {
            form(ui, editor, events);
        });
    if shown.chosen == Some(0) {
        events.push(UiEvent::IdentitySave);
    } else if shown.chosen.is_some() || shown.dismissed {
        dialog::Dialog::reset(ui.ctx(), id);
        events.push(UiEvent::CloseIdentities);
    }
}

fn form(ui: &mut Ui, editor: &IdentityEditor, events: &mut Vec<UiEvent>) {
    let width = ui.available_width();
    // The list: one row per identity, the selected one marked. Add and
    // Remove work on the selection.
    ui.horizontal_wrapped(|ui| {
        for (index, row) in editor.rows.iter().enumerate() {
            let label = if row.name.is_empty() {
                row.id.clone()
            } else {
                format!("{} <{}>", row.name, row.address)
            };
            let selected = index == editor.selected;
            if toolkit::selectable(ui, selected, label).clicked() {
                events.push(UiEvent::IdentitySelect(index));
            }
        }
    });
    ui.horizontal(|ui| {
        if ui
            .add(toolkit::button::PushButton::secondary(crate::label(
                "identity-add",
            )))
            .clicked()
        {
            events.push(UiEvent::IdentityAdd);
        }
        if ui
            .add_enabled(
                !editor.rows.is_empty(),
                toolkit::button::PushButton::secondary(crate::label("identity-remove")),
            )
            .clicked()
        {
            events.push(UiEvent::IdentityRemove);
        }
    });
    ui.add_space(8.0);
    if let Some(row) = editor.rows.get(editor.selected) {
        text(
            ui,
            "identity-name",
            IdentityField::Name,
            &row.name,
            width,
            events,
        );
        text(
            ui,
            "identity-address",
            IdentityField::Address,
            &row.address,
            width,
            events,
        );
        text(
            ui,
            "identity-reply-to",
            IdentityField::ReplyTo,
            row.reply_to.as_deref().unwrap_or(""),
            width,
            events,
        );
        let mut signature = row.signature.clone();
        let signature_edit = TextEdit::multiline(&mut signature)
            .desired_rows(4)
            .desired_width(f32::INFINITY)
            .hint_text(crate::label("identity-signature"));
        if ui.add(signature_edit).changed() {
            events.push(UiEvent::IdentityEdit(IdentityField::Signature, signature));
        }
    }
    if let Some(error) = &editor.error {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(crate::label_with(
                "identity-refused",
                &[("error", error.as_str())],
            ))
            .color(ui.visuals().warn_fg_color),
        );
    }
}

fn text(
    ui: &mut Ui,
    label: &str,
    field: IdentityField,
    value: &str,
    width: f32,
    events: &mut Vec<UiEvent>,
) {
    let mut buffer = value.to_owned();
    let response = field::text(ui, &mut buffer, &crate::label(label), width);
    if response.changed() {
        events.push(UiEvent::IdentityEdit(field, buffer));
    }
}
