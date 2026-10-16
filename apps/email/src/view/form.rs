// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The forms over the window: confirm a delete (when the settings ask
//! for it), choose a move's mailbox, and the New Event and New Contact
//! forms. Each edit is one event; the state owns the form and decides what
//! is sent.

use egui::Ui;
use toolkit::dialog::{self, Choice};
use toolkit::field;

use super::UiEvent;
use crate::state::{FolderAction, FolderForm, Form, FormField, State};

/// The open form, when there is one.
pub fn dialog(ui: &mut Ui, state: &State, events: &mut Vec<UiEvent>) {
    let Some(form) = &state.form else {
        return;
    };
    let (key, title, confirm) = match form {
        Form::Delete { .. } => (
            "form-delete",
            crate::label("delete-title"),
            Some(crate::label("delete")),
        ),
        Form::Move { .. } => ("form-move", crate::label("move-title"), None),
        Form::Event(_) => (
            "form-event",
            crate::label("new-event-title"),
            Some(crate::label("save")),
        ),
        Form::Contact(_) => (
            "form-contact",
            crate::label("new-contact-title"),
            Some(crate::label("save")),
        ),
        Form::Folder(f) => {
            let (title, confirm) = match f.action {
                FolderAction::New { .. } => ("folder-new-title", "folder-create"),
                FolderAction::Rename { .. } => ("folder-rename-title", "folder-rename-confirm"),
                FolderAction::Delete { .. } => ("folder-delete-title", "folder-delete-confirm"),
                FolderAction::Empty { .. } => ("folder-empty-title", "folder-empty-confirm"),
            };
            ("form-folder", crate::label(title), Some(crate::label(confirm)))
        }
    };
    // A delete or an empty is undone by nothing: Cancel is the default.
    let destructive = matches!(
        form,
        Form::Folder(FolderForm {
            action: FolderAction::Delete { .. } | FolderAction::Empty { .. },
            ..
        })
    );
    let id = egui::Id::new(key);
    let busy = match form {
        Form::Event(f) => f.busy,
        Form::Contact(f) => f.busy,
        _ => false,
    };
    let mut choices = Vec::new();
    let (go, stop) = if destructive {
        (dialog::Role::Other, dialog::Role::Default)
    } else {
        (dialog::Role::Default, dialog::Role::Cancel)
    };
    if let Some(confirm) = confirm {
        let mut button = Choice::new(confirm, go);
        button.enabled = !busy;
        choices.push(button);
    }
    choices.push(Choice::new(crate::label("cancel"), stop));
    let confirmable = choices.len() == 2;
    let shown = dialog::Dialog::new(id, title)
        .buttons(choices)
        .show(ui.ctx(), |ui| body(ui, state, form, events));
    if confirmable && shown.chosen == Some(0) {
        dialog::Dialog::reset(ui.ctx(), id);
        events.push(UiEvent::FormSubmit(None));
    } else if shown.chosen.is_some() || shown.dismissed {
        dialog::Dialog::reset(ui.ctx(), id);
        events.push(UiEvent::FormCancel);
    } else if events
        .iter()
        .any(|e| matches!(e, UiEvent::FormSubmit(Some(_))))
    {
        // A move's mailbox was chosen in the body.
        dialog::Dialog::reset(ui.ctx(), id);
    }
}

/// One labelled field of a form grid: its name, then the field with `hint`
/// as its placeholder.
fn line(
    ui: &mut Ui,
    label: &str,
    hint: &str,
    field: FormField,
    value: &str,
    events: &mut Vec<UiEvent>,
) {
    ui.label(crate::label(label));
    let mut buffer = value.to_owned();
    if field::text(ui, &mut buffer, hint, 300.0_f32.max(ui.available_width())).changed() {
        events.push(UiEvent::FormField(field, buffer));
    }
    ui.end_row();
}

fn body(ui: &mut Ui, state: &State, form: &Form, events: &mut Vec<UiEvent>) {
    if let Some(note) = &state.form_note {
        ui.label(egui::RichText::new(note).color(ui.visuals().error_fg_color));
    }
    let optional = crate::label("optional");
    let grid = |ui: &mut Ui, salt: &str, add: &mut dyn FnMut(&mut Ui)| {
        egui::Grid::new(salt).num_columns(2).show(ui, |ui| add(ui));
    };
    let error = |ui: &mut Ui, error: &Option<String>| {
        if let Some(error) = error {
            ui.label(egui::RichText::new(error).color(ui.visuals().error_fg_color));
        }
    };
    match form {
        Form::Delete { ids, .. } => {
            ui.label(crate::label_with(
                "delete-body",
                &[("count", &ids.len().to_string())],
            ));
        }
        Form::Move { account, .. } => {
            ui.label(crate::label("move-body"));
            let Some(view) = state.account(account) else {
                return;
            };
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for mailbox in &view.mail.mailboxes {
                        if view.mail.selected.as_deref() == Some(mailbox.id.as_str()) {
                            continue;
                        }
                        if toolkit::selectable(ui, false, &mailbox.name).clicked() {
                            events.push(UiEvent::FormSubmit(Some(mailbox.id.clone())));
                        }
                    }
                });
        }
        Form::Event(f) => {
            let time = crate::label("event-time-hint");
            ui.add_enabled_ui(!f.busy, |ui| {
                grid(ui, "event-form", &mut |ui| {
                    line(ui, "event-title", "", FormField::Title, &f.title, events);
                    line(ui, "event-start", &time, FormField::Start, &f.start, events);
                    line(ui, "event-end", &time, FormField::End, &f.end, events);
                    let location = &f.location;
                    line(
                        ui,
                        "event-location",
                        &optional,
                        FormField::Location,
                        location,
                        events,
                    );
                });
            });
            error(ui, &f.error);
        }
        Form::Folder(f) => {
            match f.action {
                FolderAction::New { .. } | FolderAction::Rename { .. } => {
                    grid(ui, "folder-form", &mut |ui| {
                        line(ui, "folder-name", "", FormField::FolderName, &f.name, events);
                    });
                }
                FolderAction::Delete { .. } | FolderAction::Empty { .. } => {
                    let key = if matches!(f.action, FolderAction::Delete { .. }) {
                        "folder-delete-body"
                    } else {
                        "folder-empty-body"
                    };
                    ui.label(crate::label_with(
                        key,
                        &[("name", &f.name), ("count", &f.count.to_string())],
                    ));
                }
            }
            error(ui, &f.error);
        }
        Form::Contact(f) => {
            ui.add_enabled_ui(!f.busy, |ui| {
                grid(ui, "contact-form", &mut |ui| {
                    line(ui, "contact-name", "", FormField::Name, &f.name, events);
                    line(
                        ui,
                        "contact-email",
                        &optional,
                        FormField::Email,
                        &f.email,
                        events,
                    );
                    line(
                        ui,
                        "contact-phone",
                        &optional,
                        FormField::Phone,
                        &f.phone,
                        events,
                    );
                    let org = &f.organization;
                    line(
                        ui,
                        "contact-organization",
                        &optional,
                        FormField::Organization,
                        org,
                        events,
                    );
                });
            });
            error(ui, &f.error);
        }
    }
}
