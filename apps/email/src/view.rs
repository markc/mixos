// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The egui rendering of the state. [`view`] reads the state and returns
//! what the person did as [`UiEvent`]s; the shell applies them after the
//! frame, so drawing never mutates state and a snapshot needs no network.
//!
//! The window is built from the toolkit's chrome: the title bar, a status
//! bar, the places panel (mailboxes, calendars, address books), the centre
//! pane (the setup form until connected, then the mode's list), the reader
//! panel (the open email, event or contact), and the compose, About and
//! Shortcuts dialogs.

pub mod calendar;
pub mod compose;
pub mod contacts;
pub mod mail;
pub mod setup;

use toolkit::command::Origin;
use toolkit::dialog::{self, Choice};
use toolkit::titlebar::{self, Control};
use toolkit::{Icon, Registry, Strings, bars, panel};

use crate::state::{Dialog, Mode, Phase, State};

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Command(&'static str, Origin),
    /// Switch the centre pane's surface.
    Mode(Mode),
    SelectMailbox(String),
    /// Open an email (also marks it read through the state).
    OpenEmail(String),
    SelectCalendar(String),
    SelectEvent(usize),
    SelectBook(String),
    SelectContact(usize),
    ContactFilter(String),
    Compose,
    Reply,
    ComposeField(crate::state::ComposeField, String),
    CloseCompose,
    SendCompose,
    SetupField(crate::state::SetupField, String),
    SetupConnect,
    /// Retry the saved account's connect.
    Retry,
    /// Edit the account again (from the failure pane).
    SetupEdit,
    SplitPlaces(f32),
    SplitReader(f32),
    CloseDialog,
    /// The title bar's light/dark toggle: the mode to switch to.
    SetMode(design::Mode),
}

const ICON: f32 = 14.0;

/// Draw the whole window into `ui`.
pub fn view(
    ui: &mut egui::Ui,
    state: &State,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    let dark = ui.visuals().dark_mode;
    // The right-hand group: a new mail, light and dark, the shortcuts link.
    let mut controls = [
        Control::Icon {
            command: "mail.compose",
            icon: Icon::PenSquare,
            selected: false,
        },
        Control::Icon {
            command: "view.mode",
            icon: titlebar::theme_icon(dark),
            selected: false,
        },
        Control::Link {
            command: "help.shortcuts",
            icon: Icon::Keyboard,
        },
    ];
    let fired = titlebar::show_with(
        ui,
        &crate::label("title"),
        Some(Icon::Mail),
        stroke,
        commands,
        state,
        strings,
        &mut controls,
    );
    let mut mode_set = false;
    for (id, origin) in fired {
        if id == "view.mode" {
            // The toggle goes to the mode opposite the one this frame drew,
            // so a click reported twice cannot flip it back.
            if !mode_set {
                commands.note(id, origin);
                let target = match state.effective_mode() {
                    design::Mode::Dark => design::Mode::Light,
                    design::Mode::Light => design::Mode::Dark,
                };
                events.push(UiEvent::SetMode(target));
                mode_set = true;
            }
        } else {
            events.push(UiEvent::Command(id, origin));
        }
    }
    status(ui, state, stroke);
    match &state.phase {
        Phase::Setup => setup::form(ui, state, stroke, &mut events),
        Phase::Connecting => setup::connecting(ui, state),
        Phase::Failed(_) => setup::failed(ui, state, stroke, &mut events),
        Phase::Ready => ready(ui, state, commands, strings, stroke, &mut events),
    }
    compose::dialog(ui, state, &mut events);
    if let Some(dialog) = state.ui.dialog {
        let title = match dialog {
            Dialog::About => "about",
            Dialog::Shortcuts => "shortcuts",
        };
        let id = egui::Id::new("dialog").with(title);
        let shown = dialog::Dialog::new(id, crate::label(title))
            .buttons(vec![Choice::new(crate::label("done"), dialog::Role::Default)])
            .show(ui.ctx(), |ui| match dialog {
                Dialog::About => {
                    ui.add(egui::Label::new(crate::label("about-body")).wrap());
                }
                Dialog::Shortcuts => commands.shortcut_grid(ui, strings),
            });
        if shown.chosen.is_some() || shown.dismissed {
            dialog::Dialog::reset(ui.ctx(), id);
            events.push(UiEvent::CloseDialog);
        }
    }
    toolkit::titlebar::edges(ui);
    events
}

/// The status bar: the connection, the account, the last word.
fn status(ui: &mut egui::Ui, state: &State, stroke: f32) {
    bars::status(ui, |ui| {
        let (icon, colour) = match state.phase {
            Phase::Ready => (Icon::Plug, ui.visuals().weak_text_color()),
            _ => (Icon::Unplug, ui.visuals().error_fg_color()),
        };
        ui.add(toolkit::icons::image(icon, stroke, ICON, colour));
        let account = state
            .session
            .as_ref()
            .map(|s| s.account_name.as_str())
            .unwrap_or_default();
        let mut line = account.to_owned();
        if !state.status.is_empty() {
            if !line.is_empty() {
                line.push_str(" — ");
            }
            line.push_str(&state.status);
        }
        ui.small(&line);
    });
}

/// The connected window: places, the mode's centre pane, the reader.
fn ready(
    ui: &mut egui::Ui,
    state: &State,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let total = ui.available_width();
    let left = egui::Panel::left("places")
        .resizable(true)
        .default_size(total * state.ui.places)
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| places(ui, state, stroke, events));
    let split = left.response.rect.width() / total.max(1.0);
    if (split - state.ui.places).abs() > 0.005 {
        events.push(UiEvent::SplitPlaces(split));
    }
    let remaining = ui.available_width();
    let right = egui::Panel::right("reader")
        .resizable(true)
        .default_size(remaining * state.ui.reader)
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| reader(ui, state, stroke, events));
    let split = right.response.rect.width() / remaining.max(1.0);
    if (split - state.ui.reader).abs() > 0.005 {
        events.push(UiEvent::SplitReader(split));
    }
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| centre(ui, state, commands, strings, stroke, events));
}

/// The centre pane: the current mode's list.
fn centre(
    ui: &mut egui::Ui,
    state: &State,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    match state.mode {
        Mode::Mail => mail::list(ui, state, commands, strings, stroke, events),
        Mode::Calendar => calendar::list(ui, state, stroke, events),
        Mode::Contacts => contacts::list(ui, state, stroke, events),
    }
}

/// The places panel: mailboxes, calendars, address books.
fn places(ui: &mut egui::Ui, state: &State, stroke: f32, events: &mut Vec<UiEvent>) {
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let title = crate::label("mail");
            panel::group(ui, "mailboxes", &[title.as_str()], |ui, _| {
                for mailbox in &state.mail.mailboxes {
                    mail::place_row(ui, mailbox, state, stroke, events);
                }
                if state.mail.mailboxes.is_empty() {
                    ui.weak(crate::label("empty"));
                }
            });
            let title = crate::label("calendar");
            panel::group(ui, "calendars", &[title.as_str()], |ui, _| {
                for calendar in &state.calendars.calendars {
                    calendar::place_row(ui, calendar, state, stroke, events);
                }
                if state.calendars.calendars.is_empty() {
                    ui.weak(crate::label("empty"));
                }
            });
            let title = crate::label("contacts");
            panel::group(ui, "books", &[title.as_str()], |ui, _| {
                for book in &state.contacts.books {
                    contacts::place_row(ui, book, state, stroke, events);
                }
                if state.contacts.books.is_empty() {
                    ui.weak(crate::label("empty"));
                }
            });
        });
}

/// The reader panel: the open email, event or contact.
fn reader(ui: &mut egui::Ui, state: &State, stroke: f32, events: &mut Vec<UiEvent>) {
    let title = crate::label("reader");
    panel::group(ui, "reader", &[title.as_str()], |ui, _| {
        match state.mode {
            Mode::Mail => mail::reader(ui, state),
            Mode::Calendar => calendar::reader(ui, state),
            Mode::Contacts => contacts::reader(ui, state),
        }
    });
}
