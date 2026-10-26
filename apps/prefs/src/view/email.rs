// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Apps → Email: the account list beside the selected account's
//! form, its password and connection, then the mail options. The values
//! are settingsd's as last read; the form is the person's typing until
//! Save. Each list action and option applies at once.
use super::{Numeric, UiEvent, command_button, field_label, group_heading, number, row, toggle};
use crate::{label, label_with};
use egui::{Align, Layout, RichText, Ui};
use preferences::Engine;
use preferences::appearance::Availability as Settings;
use preferences::email::{
    AccountEntry, Change, Email, Field, Form, PAGE_SIZE_MAX, PAGE_SIZE_MIN, ReplyPosition, Secret,
    Service,
};
use toolkit::{Registry, Strings, bars, field, panel};

/// The account list's width beside the form.
const LIST_WIDTH: f32 = 220.0;
/// The widest a form field grows.
const FIELD_WIDTH: f32 = 380.0;
/// The form's label column, shared by the password row so they line up.
const LABEL_WIDTH: f32 = super::LABEL_WIDTH;
const GRID_GAP: [f32; 2] = [24.0, 10.0];
/// The signature field's height, in lines.
const SIGNATURE_ROWS: usize = 4;

pub fn section(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    // One group: a dock group fills the height left, so a second one
    // would start below the fold.
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            for (id, primary) in [
                ("email.account.add", false),
                ("email.account.up", false),
                ("email.account.down", false),
                ("email.account.default", false),
                ("email.account.remove", false),
            ] {
                command_button(ui, engine, commands, strings, id, primary, events);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                command_button(ui, engine, commands, strings, "view.refresh", false, events);
            });
        });
        bars::hairline(ui);
        if engine.look.settings == Settings::Missing {
            ui.add(egui::Label::new(RichText::new(label("look-missing")).weak()).wrap());
            return;
        }
        let Some(email) = engine.email() else {
            ui.label(RichText::new(label("email-reading")).weak());
            return;
        };
        // The divider is painted once the row's height is known: a
        // separator would claim all the height left.
        let row = ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(LIST_WIDTH);
                accounts(ui, engine, email, events);
            });
            ui.add_space(ui.spacing().item_spacing.x * 2.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                match &engine.email.form {
                    Some(form) => account(ui, engine, commands, strings, form, events),
                    None => {
                        let key = if email.accounts.is_empty() {
                            "email-empty"
                        } else {
                            "email-select"
                        };
                        ui.add(egui::Label::new(RichText::new(label(key)).weak()).wrap());
                        task_feedback(ui, engine);
                    }
                }
            });
        });
        let rect = row.response.rect;
        // Midway across the gap: item spacing either side of the space.
        let x = rect.left() + LIST_WIDTH + ui.spacing().item_spacing.x * 2.0;
        ui.painter().vline(
            x,
            rect.y_range(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        group_heading(ui, "email-mail", false);
        mail_options(ui, engine, email, events);
        super::text::section(
            ui,
            engine,
            &[
                preferences::text::Surface::EmailReader,
                preferences::text::Surface::EmailComposer,
            ],
            events,
        );
        ui.horizontal(|ui| {
            command_button(ui, engine, commands, strings, "email.reset", false, events);
            ui.add(egui::Label::new(RichText::new(label("email-note")).weak()).wrap());
        });
    });
}

/// The accounts in tree order, each with its address and what marks it:
/// the default, disabled.
fn accounts(ui: &mut Ui, engine: &Engine, email: &Email, events: &mut Vec<UiEvent>) {
    panel::section_label(ui, &label("email-accounts"));
    if email.accounts.is_empty() {
        ui.label(RichText::new(label("email-no-accounts")).weak());
    }
    for account in &email.accounts {
        let selected = engine.ui.email_account.as_deref() == Some(account.id.as_str());
        let name = if account.enabled {
            RichText::new(&account.name)
        } else {
            RichText::new(&account.name).weak()
        };
        if toolkit::selectable(ui, selected, name).clicked() && !selected {
            events.push(UiEvent::EmailSelect(account.id.clone()));
        }
        let mut marks = vec![account.email.clone()];
        if email.default_account.as_deref() == Some(account.id.as_str()) {
            marks.push(label("email-default"));
        }
        if !account.enabled {
            marks.push(label("email-disabled"));
        }
        ui.label(RichText::new(marks.join(" · ")).small().weak());
        ui.add_space(ui.spacing().item_spacing.y);
    }
    if engine.email.form.as_ref().is_some_and(|f| f.id.is_none()) {
        toolkit::selectable(ui, true, label("email-new-account"));
    }
}

/// The selected account's form, Save and Revert, its password and
/// connection.
fn account(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    form: &Form,
    events: &mut Vec<UiEvent>,
) {
    let saved = engine.email_account();
    if saved.is_none() {
        ui.strong(label("email-new-account"));
    }
    let width = (ui.available_width() - LABEL_WIDTH - GRID_GAP[0]).min(FIELD_WIDTH);
    ui.add_enabled_ui(engine.can_edit_email(), |ui| {
        egui::Grid::new("email-form")
            .num_columns(2)
            .min_col_width(LABEL_WIDTH)
            .spacing(GRID_GAP)
            .show(ui, |ui| {
                for (field, key, hint) in [
                    (Field::Name, "email-name", "email-name-hint"),
                    (Field::Url, "email-url", "email-url-hint"),
                    (Field::Email, "email-address", "email-address-hint"),
                    (Field::CaPath, "email-ca", "email-ca-hint"),
                    (Field::Signature, "email-signature", "email-signature-hint"),
                ] {
                    field_label(ui, &label(key));
                    ui.vertical(|ui| {
                        let mut text = form.get(field).to_owned();
                        let response = if field == Field::Signature {
                            ui.add(
                                egui::TextEdit::multiline(&mut text)
                                    .hint_text(label(hint))
                                    .desired_rows(SIGNATURE_ROWS)
                                    .desired_width(width),
                            )
                        } else {
                            field::text(ui, &mut text, &label(hint), width)
                        };
                        if response.changed() {
                            events.push(UiEvent::EmailField(field, text));
                        }
                        if let Some(refusal) = form.errors.get(&field) {
                            let text = RichText::new(label(refusal))
                                .small()
                                .color(ui.visuals().error_fg_color);
                            ui.add(egui::Label::new(text).wrap());
                        }
                    });
                    ui.end_row();
                }
                if let Some(saved) = saved {
                    field_label(ui, &label("email-enabled"));
                    let mut on = saved.enabled;
                    if toggle(ui, &mut on, "email-enabled-note") {
                        events.push(UiEvent::EmailChange(Change::Enable(saved.id.clone(), on)));
                    }
                    ui.end_row();
                }
            });
    });
    ui.horizontal(|ui| {
        command_button(
            ui,
            engine,
            commands,
            strings,
            "email.account.save",
            true,
            events,
        );
        command_button(
            ui,
            engine,
            commands,
            strings,
            "email.account.revert",
            false,
            events,
        );
    });
    if let Some(saved) = saved {
        bars::hairline(ui);
        password(ui, engine, commands, strings, saved, width, events);
        task_feedback(ui, engine);
        identities(ui, engine, width, events);
    } else {
        task_feedback(ui, engine);
    }
}

/// The account's identity list: its rows, the selected identity's fields,
/// and Add, Remove, Save and Revert. Save sends the whole list.
fn identities(ui: &mut Ui, engine: &Engine, width: f32, events: &mut Vec<UiEvent>) {
    let draft = engine.identity_draft();
    bars::hairline(ui);
    panel::section_label(ui, &label("email-identities"));
    ui.horizontal_wrapped(|ui| {
        for (index, row) in draft.rows.iter().enumerate() {
            let text = if row.name.is_empty() {
                row.id.clone()
            } else {
                format!("{} <{}>", row.name, row.address)
            };
            if toolkit::selectable(ui, index == draft.selected, text).clicked() {
                events.push(UiEvent::EmailIdentitySelect(index));
            }
        }
    });
    ui.horizontal(|ui| {
        let add = toolkit::button::PushButton::secondary(label("email-identity-add"));
        if ui.add(add).clicked() {
            events.push(UiEvent::EmailIdentityAdd);
        }
        let remove = toolkit::button::PushButton::secondary(label("email-identity-remove"));
        if ui.add_enabled(!draft.rows.is_empty(), remove).clicked() {
            events.push(UiEvent::EmailIdentityRemove);
        }
    });
    if let Some(row) = draft.rows.get(draft.selected) {
        use preferences::email::IdentityField;
        for (field, key, mut value) in [
            (IdentityField::Name, "email-identity-name", row.name.clone()),
            (
                IdentityField::Address,
                "email-identity-address",
                row.address.clone(),
            ),
            (
                IdentityField::ReplyTo,
                "email-identity-reply-to",
                row.reply_to.clone().unwrap_or_default(),
            ),
            (
                IdentityField::Signature,
                "email-identity-signature",
                row.signature.clone(),
            ),
        ] {
            super::row(ui, key, &label(key), |ui| {
                let response = if field == IdentityField::Signature {
                    ui.add(
                        egui::TextEdit::multiline(&mut value)
                            .desired_rows(SIGNATURE_ROWS)
                            .desired_width(width),
                    )
                } else {
                    ui.add(field::text_edit(&mut value, &label(key), width))
                };
                if response.changed() {
                    events.push(UiEvent::EmailIdentityEdit(field, value));
                }
            });
        }
    }
    if let Some(error) = &draft.error {
        let text = match error {
            preferences::email::Refusal::Key(key) => label(key),
            preferences::email::Refusal::Settings(text) => text.clone(),
            _ => label("email-identity-refused"),
        };
        ui.add(
            egui::Label::new(
                RichText::new(text)
                    .small()
                    .color(ui.visuals().error_fg_color),
            )
            .wrap(),
        );
    }
    let pending = engine.email.identities.is_some();
    ui.horizontal(|ui| {
        let save = toolkit::button::PushButton::primary(label("email-identity-save"));
        if ui.add_enabled(pending, save).clicked() {
            events.push(UiEvent::EmailIdentitySave);
        }
        let revert = toolkit::button::PushButton::secondary(label("email-identity-revert"));
        if ui.add_enabled(pending, revert).clicked() {
            events.push(UiEvent::EmailIdentityRevert);
        }
    });
}

/// The write-only password field, whether one is set, and Connect now.
fn password(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    saved: &AccountEntry,
    width: f32,
    events: &mut Vec<UiEvent>,
) {
    egui::Grid::new("email-password")
        .num_columns(2)
        .min_col_width(LABEL_WIDTH)
        .spacing(GRID_GAP)
        .show(ui, |ui| {
            field_label(ui, &label("email-password"));
            ui.vertical(|ui| {
                let mut typed = engine.email.password.expose().to_owned();
                let response = ui.add_enabled(
                    engine.email.task.is_none(),
                    egui::TextEdit::singleline(&mut typed)
                        .password(true)
                        .hint_text(label("email-password-hint"))
                        .desired_width(width)
                        .min_size(egui::vec2(0.0, field::height_of(ui.ctx())))
                        .vertical_align(Align::Center),
                );
                if response.changed() {
                    events.push(UiEvent::EmailPassword(Secret::new(typed)));
                }
                ui.label(
                    RichText::new(password_state(engine, &saved.id))
                        .small()
                        .weak(),
                );
            });
            ui.end_row();
        });
    ui.horizontal(|ui| {
        command_button(
            ui,
            engine,
            commands,
            strings,
            "email.password.set",
            true,
            events,
        );
        command_button(
            ui,
            engine,
            commands,
            strings,
            "email.account.connect",
            false,
            events,
        );
        command_button(
            ui,
            engine,
            commands,
            strings,
            "email.password.forget",
            false,
            events,
        );
    });
}

fn task_feedback(ui: &mut Ui, engine: &Engine) {
    if let Some((text, failed)) = &engine.email.outcome {
        let text = if *failed {
            RichText::new(text).color(ui.visuals().error_fg_color)
        } else {
            RichText::new(text).weak()
        };
        ui.add(egui::Label::new(text).wrap());
    }
}

/// Whether Email holds a password for `id`, as it last said.
fn password_state(engine: &Engine, id: &str) -> String {
    match (engine.email.service, engine.email.passwords.get(id)) {
        (Service::Running, Some(true)) => label("email-password-is-set"),
        (Service::Running, Some(false)) => label("email-password-not-set"),
        (Service::Running, None) => label("email-password-unknown"),
        (Service::Absent, _) => label("email-password-not-running"),
        (Service::Unsupported, _) => label("email-password-unsupported"),
        (Service::Unknown, _) => label("email-password-checking"),
    }
}

/// Page size, mark read on open, confirm before delete: each applies at once.
fn mail_options(ui: &mut Ui, engine: &Engine, email: &Email, events: &mut Vec<UiEvent>) {
    ui.add_enabled_ui(engine.can_edit_email(), |ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            let mail = &email.mail;
            let size = Numeric {
                key: "email-page-size",
                title: label("email-page-size"),
                unit: "",
                places: 0,
                slider: true,
            };
            let range = f64::from(PAGE_SIZE_MIN)..=f64::from(PAGE_SIZE_MAX);
            if let Some(value) = number(ui, size, f64::from(mail.page_size), range) {
                events.push(UiEvent::EmailChange(Change::PageSize(value as u32)));
            }
            row(ui, "email-mark-read-row", &label("email-mark-read"), |ui| {
                let mut read = mail.mark_read_on_open;
                if toggle(ui, &mut read, "email-mark-read") {
                    events.push(UiEvent::EmailChange(Change::MarkReadOnOpen(read)));
                }
            });
            row(
                ui,
                "email-confirm-delete-row",
                &label("email-confirm-delete"),
                |ui| {
                    let mut confirm = mail.confirm_delete;
                    if toggle(ui, &mut confirm, "email-confirm-delete") {
                        events.push(UiEvent::EmailChange(Change::ConfirmDelete(confirm)));
                    }
                },
            );
            row(
                ui,
                "email-quote-on-reply-row",
                &label("email-quote-on-reply"),
                |ui| {
                    let mut quote = mail.quote_on_reply;
                    if toggle(ui, &mut quote, "email-quote-on-reply") {
                        events.push(UiEvent::EmailChange(Change::QuoteOnReply(quote)));
                    }
                },
            );
            let mut position = mail.reply_position;
            row(
                ui,
                "email-reply-position",
                &label("email-reply-position"),
                |ui| {
                    ui.horizontal(|ui| {
                        let above = ui.radio_value(
                            &mut position,
                            ReplyPosition::Above,
                            label("email-reply-above"),
                        );
                        let below = ui.radio_value(
                            &mut position,
                            ReplyPosition::Below,
                            label("email-reply-below"),
                        );
                        if (above.changed() || below.changed()) && position != mail.reply_position {
                            events.push(UiEvent::EmailChange(Change::ReplyPosition(position)));
                        }
                    });
                },
            );
        });
    });
}

/// The remove and reset confirmations: Cancel is the default, so Enter
/// never removes or resets.
pub fn confirmation(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    let (salt, title, body, confirm) = match &engine.ui.dialog {
        Some(preferences::Dialog::RemoveAccount(id)) => {
            let name = engine
                .email()
                .and_then(|e| e.account(id))
                .map_or_else(|| id.clone(), |a| a.name.clone());
            let args = [("account", name.as_str())];
            (
                "remove-account",
                label_with("email-remove-title", &args),
                label_with("email-remove-body", &args),
                label("remove-confirm"),
            )
        }
        Some(preferences::Dialog::ForgetPassword(id)) => {
            let name = engine
                .email()
                .and_then(|e| e.account(id))
                .map_or_else(|| id.clone(), |a| a.name.clone());
            let args = [("account", name.as_str())];
            (
                "forget-password",
                label_with("email-forget-title", &args),
                label_with("email-forget-body", &args),
                label("email-forget-confirm"),
            )
        }
        Some(preferences::Dialog::ResetEmail) => (
            "reset-email",
            label("email-reset-title"),
            label("email-reset-body"),
            label("email-reset-confirm"),
        ),
        _ => return,
    };
    let id = egui::Id::unique("dialog").with(salt);
    let shown = toolkit::dialog::Dialog::new(id, title)
        .buttons(vec![
            toolkit::dialog::Choice::new(confirm, toolkit::dialog::Role::Other),
            toolkit::dialog::Choice::new(label("cancel"), toolkit::dialog::Role::Default),
        ])
        .show(ui.ctx(), |ui| ui.add(egui::Label::new(body).wrap()));
    if shown.chosen.is_some() || shown.dismissed {
        toolkit::dialog::Dialog::reset(ui.ctx(), id);
        events.push(if shown.chosen == Some(0) {
            UiEvent::ConfirmEmail
        } else {
            UiEvent::CloseDialog
        });
    }
}
