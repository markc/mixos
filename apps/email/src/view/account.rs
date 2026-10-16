// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The centre pane when there is no list to show: no accounts at all, or
//! the selected account connecting, failed, waiting for a password, or
//! turned off. Accounts are configured in Prefs, so each of these offers to
//! open it on its Email section; a failed account also offers a retry.

use egui::Ui;
use toolkit::button::PushButton;
use toolkit::panel;

use super::UiEvent;
use crate::state::{AccountView, Phase};

/// The Open Email settings button.
fn settings_button(ui: &mut Ui, primary: bool, events: &mut Vec<UiEvent>) {
    let text = crate::label("open-settings");
    let button = if primary {
        PushButton::primary(text)
    } else {
        PushButton::secondary(text)
    };
    if ui.add(button).clicked() {
        events.push(UiEvent::OpenPrefs);
    }
}

/// No account is configured yet.
pub fn none(ui: &mut Ui, events: &mut Vec<UiEvent>) {
    let title = crate::label("no-accounts-title");
    panel::group(ui, "accounts", &[title.as_str()], |ui, _| {
        ui.add(egui::Label::new(crate::label("no-accounts-body")).wrap());
        ui.horizontal(|ui| settings_button(ui, true, events));
    });
}

/// The selected account, when it has nothing to list.
pub fn pane(ui: &mut Ui, view: &AccountView, events: &mut Vec<UiEvent>) {
    let title = view.entry.name.clone();
    panel::group(ui, "account", &[title.as_str()], |ui, _| match &view.phase {
        Phase::Connecting => {
            ui.label(crate::label_with(
                "connecting-account",
                &[("email", &view.entry.email)],
            ));
        }
        Phase::Failed(error) => {
            ui.label(
                egui::RichText::new(crate::label_with("account-failed", &[("error", error)]))
                    .color(ui.visuals().error_fg_color),
            );
            ui.horizontal(|ui| {
                if ui.add(PushButton::primary(crate::label("retry"))).clicked() {
                    events.push(UiEvent::Retry(view.entry.id.clone()));
                }
                settings_button(ui, false, events);
            });
        }
        Phase::NoPassword => {
            ui.label(
                egui::RichText::new(crate::label_with(
                    "account-no-password",
                    &[("email", &view.entry.email)],
                ))
                .color(ui.visuals().error_fg_color),
            );
            ui.horizontal(|ui| settings_button(ui, true, events));
        }
        Phase::Disabled => {
            ui.weak(crate::label("account-disabled"));
            ui.horizontal(|ui| settings_button(ui, false, events));
        }
        // The list owns the centre pane once connected.
        Phase::Ready => {}
    });
}
