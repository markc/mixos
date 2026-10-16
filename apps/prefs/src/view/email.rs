// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! App Settings → Email: the account list beside the selected account's
//! form, its password and connection, then the mail options. The values
//! are settingsd's as last read; the form is the person's typing until
//! Save. Each list action and option applies at once.
use super::{Numeric, UiEvent, choices, command_button, number};
use crate::{label, label_with};
use egui::{Align, Layout, RichText, Ui};
use preferences::Engine;
use preferences::appearance::Availability as Settings;
use preferences::email::{
    AccountEntry, Change, Email, Field, Form, PAGE_SIZE_MAX, PAGE_SIZE_MIN, READER_FONT_MAX,
    EmailTab, READER_FONT_MIN, READER_FONT_STEP, ReaderTheme, ReplyPosition, Secret, Service,
};
use toolkit::command::Origin;
use serde_json::json;
use toolkit::toggle::Toggle;
use toolkit::{Registry, Strings, bars, field, panel};

/// The account list's width beside the form.
const LIST_WIDTH: f32 = 220.0;
/// The widest a form field grows.
const FIELD_WIDTH: f32 = 380.0;
/// The form's label column, shared by the password row so they line up.
const LABEL_WIDTH: f32 = 84.0;
const GRID_GAP: [f32; 2] = [18.0, 10.0];
/// The signature field's height, in lines.
const SIGNATURE_ROWS: usize = 4;

pub fn section(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    // One group whose tabs are the sub-sections. The engine holds the tab
    // (prefs.section name=email tab=…): the strip is told which it shows,
    // and a click is the tab's command.
    let titles: Vec<String> = EmailTab::ALL.iter().map(|t| label(t.label())).collect();
    let tabs: Vec<&str> = titles.iter().map(String::as_str).collect();
    let tab = engine.ui.email_tab;
    let selected = EmailTab::ALL.iter().position(|t| *t == tab).unwrap_or(0);
    let id = ui.make_persistent_id("email");
    ui.data_mut(|d| {
        d.insert_temp(
            id,
            panel::GroupState {
                selected,
                collapsed: false,
            },
        )
    });
    let shown = panel::group(ui, "email", &tabs, |ui, _| {
        tab_body(ui, engine, commands, strings, tab, events)
    });
    if shown.state.selected != selected
        && let Some(picked) = EmailTab::ALL.get(shown.state.selected)
    {
        events.push(UiEvent::Command(picked.command(), Origin::Ui));
    }
}

/// One tab: Accounts (the list beside the form), Mail (the list options
/// and Reset email settings), Reader, Compose.
fn tab_body(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    tab: EmailTab,
    events: &mut Vec<UiEvent>,
) {
    ui.horizontal(|ui| {
        if tab == EmailTab::Accounts {
            for id in [
                "email.account.add",
                "email.account.up",
                "email.account.down",
                "email.account.default",
                "email.account.remove",
            ] {
                command_button(ui, engine, commands, strings, id, false, events);
            }
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
    match tab {
        EmailTab::Accounts => accounts_tab(ui, engine, commands, strings, email, events),
        EmailTab::Mail => {
            mail_options(ui, engine, email, events);
            bars::hairline(ui);
            ui.horizontal(|ui| {
                command_button(ui, engine, commands, strings, "email.reset", false, events);
                ui.add(egui::Label::new(RichText::new(label("email-note")).weak()).wrap());
            });
        }
        EmailTab::Reader => reader(ui, engine, commands, strings, email, events),
        EmailTab::Compose => compose(ui, engine, email, events),
    }
}

/// The account list beside the selected account's form.
fn accounts_tab(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    email: &Email,
    events: &mut Vec<UiEvent>,
) {
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
                    panel::section_label(ui, &label(key));
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
                    panel::section_label(ui, &label("email-enabled"));
                    let mut on = saved.enabled;
                    if ui
                        .add(Toggle::new(&mut on, label("email-enabled-note")))
                        .changed()
                    {
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
    }
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
            panel::section_label(ui, &label("email-password"));
            ui.vertical(|ui| {
                let mut typed = engine.email.password.expose().to_owned();
                let response = ui.add_enabled(
                    engine.email.task.is_none(),
                    egui::TextEdit::singleline(&mut typed)
                        .password(true)
                        .hint_text(label("email-password-hint"))
                        .desired_width(width)
                        .min_size(egui::vec2(0.0, field::HEIGHT))
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
    if let Some((text, failed)) = &engine.email.outcome {
        let text = if *failed {
            RichText::new(text).color(ui.visuals().error_fg_color)
        } else {
            RichText::new(text).weak()
        };
        ui.add(egui::Label::new(text).wrap());
    } else if let Some(task) = &engine.email.task {
        let key = match task.stage {
            preferences::email::Stage::Sending => "email-task-sending",
            preferences::email::Stage::Launching => "email-launching",
            preferences::email::Stage::Waiting => "email-waiting",
        };
        ui.label(RichText::new(label(key)).weak());
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
            let mut read = mail.mark_read_on_open;
            if ui
                .add(Toggle::new(&mut read, label("email-mark-read")))
                .changed()
            {
                events.push(UiEvent::EmailChange(Change::MarkReadOnOpen(read)));
            }
            let mut confirm = mail.confirm_delete;
            if ui
                .add(Toggle::new(&mut confirm, label("email-confirm-delete")))
                .changed()
            {
                events.push(UiEvent::EmailChange(Change::ConfirmDelete(confirm)));
            }
        });
    });
}

/// The reader's text size: typed or dragged (each edit one apply, on a
/// half-point step), a line set at that size, and back to the default.
fn reader(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    email: &Email,
    events: &mut Vec<UiEvent>,
) {
    let current = email.reader.font_size;
    ui.add_enabled_ui(engine.can_edit_email(), |ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            let size = Numeric {
                key: "email-reader-font",
                title: label("email-reader-font"),
                unit: "pt",
                places: 1,
                slider: true,
            };
            if let Some(value) = number(ui, size, current, READER_FONT_MIN..=READER_FONT_MAX) {
                let stepped = (value / READER_FONT_STEP).round() * READER_FONT_STEP;
                if stepped != current {
                    events.push(UiEvent::EmailChange(Change::ReaderFontSize(stepped)));
                }
            }
        });
    });
    ui.add_enabled_ui(engine.can_edit_email(), |ui| {
        panel::section_label(ui, &label("email-reader-theme"));
        let themes = [
            (ReaderTheme::Auto, label("email-reader-theme-auto")),
            (ReaderTheme::Light, label("email-reader-theme-light")),
            (ReaderTheme::Dark, label("email-reader-theme-dark")),
        ];
        if let Some(theme) = choices(ui, email.reader.theme, &themes) {
            events.push(UiEvent::EmailChange(Change::Path(
                "email.reader.theme".into(),
                json!(theme),
            )));
        }
    });
    // The preview is what the reader will show: its size, on its colours.
    let palette = reader_palette(ui.ctx(), engine, email.reader.theme);
    egui::Frame::new()
        .fill(palette.card)
        .stroke(egui::Stroke::new(1.0, palette.card_border))
        .corner_radius(toolkit::chrome::Chrome::of(ui.ctx()).metrics.radius_sm)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let text = RichText::new(label("email-reader-preview"))
                .size(current as f32)
                .color(palette.text);
            ui.add(egui::Label::new(text).wrap());
        });
    ui.horizontal(|ui| {
        command_button(
            ui,
            engine,
            commands,
            strings,
            "email.reader.reset",
            false,
            events,
        );
        let note = label_with(
            "email-reader-default",
            &[(
                "size",
                &format_size(preferences::email::READER_FONT_DEFAULT),
            )],
        );
        ui.label(RichText::new(note).weak());
    });
}

/// The colours the reader draws a message on: the window's for Follow
/// window, else the light or dark variant of the scheme the window shows,
/// from its design tokens (compiled once per scheme and mode).
fn reader_palette(
    ctx: &egui::Context,
    engine: &Engine,
    theme: ReaderTheme,
) -> toolkit::chrome::Palette {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    let mode = match theme {
        ReaderTheme::Auto => return toolkit::chrome::Chrome::of(ctx).palette,
        ReaderTheme::Light => design::Mode::Light,
        ReaderTheme::Dark => design::Mode::Dark,
    };
    type Cache = Mutex<HashMap<(design::Scheme, design::Mode), toolkit::chrome::Palette>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let scheme = engine.effective_theme().0;
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *cache.entry((scheme, mode)).or_insert_with(|| {
        let theme = toolkit::Theme::for_context(design::DesignContext {
            scheme,
            mode,
            ..design::DesignContext::default()
        });
        toolkit::chrome::Chrome::for_theme(&theme).palette
    })
}

/// Quote on reply, where the reply goes, autosave: each applies at once.
fn compose(ui: &mut Ui, engine: &Engine, email: &Email, events: &mut Vec<UiEvent>) {
    let compose = &email.compose;
    let path = |name: &str| format!("email.compose.{name}");
    ui.add_enabled_ui(engine.can_edit_email(), |ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            let mut quote = compose.quote_on_reply;
            if ui
                .add(Toggle::new(&mut quote, label("email-quote-on-reply")))
                .changed()
            {
                events.push(UiEvent::EmailChange(Change::Path(
                    path("quote_on_reply"),
                    json!(quote),
                )));
            }
            // Where the reply goes only matters when the original is quoted.
            ui.add_enabled_ui(compose.quote_on_reply, |ui| {
                panel::section_label(ui, &label("email-reply-position"));
                let positions = [
                    (ReplyPosition::Above, label("email-reply-above")),
                    (ReplyPosition::Below, label("email-reply-below")),
                ];
                if let Some(position) = choices(ui, compose.reply_position, &positions) {
                    events.push(UiEvent::EmailChange(Change::Path(
                        path("reply_position"),
                        json!(position),
                    )));
                }
            });
            let mut autosave = compose.autosave;
            if ui
                .add(Toggle::new(&mut autosave, label("email-autosave")))
                .changed()
            {
                events.push(UiEvent::EmailChange(Change::Path(
                    path("autosave"),
                    json!(autosave),
                )));
            }
        });
    });
}

/// A size in points as the panel writes it: 15, 15.5.
fn format_size(size: f64) -> String {
    if size.fract() == 0.0 {
        format!("{size:.0}")
    } else {
        format!("{size:.1}")
    }
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
    let id = egui::Id::new("dialog").with(salt);
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
