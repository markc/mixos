// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The centre pane before there is a connection: the setup form (first
//! run, or an edited account), the connecting note, and the failure pane
//! with its retry. The account's URL, address, password and optional CA
//! bundle path are the whole surface; nothing else is configurable.

use egui::Ui;
use toolkit::button::PushButton;
use toolkit::field;
use toolkit::panel;

use super::UiEvent;
use crate::state::{Phase, SetupField, State};

/// The first-run / edit form.
pub fn form(ui: &mut Ui, state: &State, stroke: f32, events: &mut Vec<UiEvent>) {
    let _ = stroke;
    let title = crate::label("setup-title");
    panel::group(ui, "setup", &[title.as_str()], |ui, _| {
        ui.add(egui::Label::new(crate::label("setup-body")).wrap());
        let width = ui.available_width();
        let mut line = |ui: &mut Ui,
                        label: &str,
                        setup: SetupField,
                        value: &str,
                        secret: bool,
                        events: &mut Vec<UiEvent>| {
            let mut buffer = value.to_owned();
            let response = if secret {
                ui.add(
                    egui::TextEdit::singleline(&mut buffer)
                        .password(true)
                        .hint_text(crate::label(label))
                        .desired_width(width),
                )
            } else {
                field::text(ui, &mut buffer, &crate::label(label), width)
            };
            if response.changed() {
                events.push(UiEvent::SetupField(setup, buffer));
            }
        };
        line(ui, "setup-url", SetupField::Url, &state.setup.url, false, events);
        line(ui, "setup-email", SetupField::Email, &state.setup.email, false, events);
        line(ui, "setup-password", SetupField::Password, &state.setup.password, true, events);
        line(ui, "setup-ca", SetupField::CaPath, &state.setup.ca_path, false, events);
        if let Some(error) = &state.setup.error {
            ui.label(
                egui::RichText::new(crate::label_with(
                    "setup-failed",
                    &[("error", error)],
                ))
                .color(ui.visuals().error_fg_color),
            );
        }
        ui.horizontal(|ui| {
            let connect = if state.setup.busy {
                PushButton::secondary(crate::label("connecting"))
            } else {
                PushButton::primary(crate::label("setup-connect"))
            };
            if ui
                .add_enabled(!state.setup.busy, connect)
                .clicked()
            {
                events.push(UiEvent::SetupConnect);
            }
        });
    });
}

/// The connecting note.
pub fn connecting(ui: &mut Ui, state: &State) {
    let title = crate::label("setup-title");
    panel::group(ui, "setup", &[title.as_str()], |ui, _| {
        ui.label(crate::label_with(
            "connecting-account",
            &[("email", &state.setup.email)],
        ));
    });
}

/// The failure pane: the error, a retry, and a way back to the form.
pub fn failed(ui: &mut Ui, state: &State, stroke: f32, events: &mut Vec<UiEvent>) {
    let _ = stroke;
    let error = match &state.phase {
        Phase::Failed(error) => error.clone(),
        _ => String::new(),
    };
    let title = crate::label("setup-title");
    panel::group(ui, "setup", &[title.as_str()], |ui, _| {
        ui.label(
            egui::RichText::new(crate::label_with(
                "setup-failed",
                &[("error", &error)],
            ))
            .color(ui.visuals().error_fg_color),
        );
        ui.horizontal(|ui| {
            if ui
                .add(PushButton::primary(crate::label("retry")))
                .clicked()
            {
                events.push(UiEvent::Retry);
            }
            if ui
                .add(PushButton::secondary(crate::label("setup-edit")))
                .clicked()
            {
                events.push(UiEvent::SetupEdit);
            }
        });
    });
}
