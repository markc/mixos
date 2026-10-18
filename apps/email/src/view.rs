// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The egui rendering of the state. [`view`] reads the state and returns
//! what the person did as [`UiEvent`]s; the shell applies them after the
//! frame, so drawing never mutates state and a snapshot needs no network.
//!
//! The window is built from the toolkit's chrome: the title bar, a status
//! bar, the account tree (each account with its mailboxes, calendars and
//! address books), the centre pane (the selected account's list for the
//! mode, or why there is none), the reader panel (the open email, event or
//! contact), and the compose, About and Shortcuts dialogs. Accounts are
//! configured in Prefs, not here: the centre pane offers to open it.

pub mod account;
pub mod calendar;
pub mod compose;
pub mod contacts;
pub mod form;
pub mod mail;
pub mod tree;

use toolkit::command::Origin;
use toolkit::dialog::{self, Choice};
use toolkit::titlebar::{self, Control};
use toolkit::{Icon, Registry, Strings, bars, panel};

use crate::state::{AccountView, Dialog, Mode, Node, Phase, Source, State, TreeKey};

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Command(&'static str, Origin),
    /// Switch the centre pane's surface.
    Mode(Mode),
    /// A tree row was clicked: select what it stands for.
    Select(Node),
    /// A tree row's disclosure: expand (`true`) or collapse it.
    Expand(Node, bool),
    /// A key press while the tree has the keyboard.
    TreeKey(TreeKey),
    /// Open an account's email (also marks it read through the state).
    OpenEmail(String, String),
    /// The account's message list reported this (a row picked or
    /// activated, a column dragged).
    MailTable(String, toolkit::datatable::TableEvent),
    SelectEvent(String, usize),
    SelectContact(String, usize),
    ContactFilter(String, String),
    Compose,
    Reply,
    /// The compose dialog's From account.
    ComposeAccount(String),
    ComposeField(crate::state::ComposeField, String),
    /// Cancel or Escape on the compose: it closes, or (something typed)
    /// the person is asked first.
    CloseCompose,
    /// The answer to the close or quit question over a draft.
    ConfirmDraft(crate::state::DraftChoice),
    /// The answer to the offer of a kept draft.
    RestoreDraft(crate::state::DraftChoice),
    /// Take the account's new address as the stale draft's From.
    ComposeUseNewFrom,
    SendCompose,
    /// Connect an account again.
    Retry(String),
    /// Open Prefs on its Email section.
    OpenPrefs,
    /// An event or contact form's field.
    FormField(crate::state::FormField, String),
    /// The form's answer: confirm, save, or (a move) the mailbox chosen.
    FormSubmit(Option<String>),
    FormCancel,
    SplitPlaces(f32),
    SplitReader(f32),
    CloseDialog,
    /// The title bar's light/dark toggle: the mode to switch to.
    SetMode(design::Mode),
    /// Show or hide the open email's attachments popup.
    AttachmentsOpen(bool),
    /// Save the open email's attachment (its part id) to the downloads.
    SaveAttachment(String),
    /// Open the open email's attachment with the desktop's application.
    OpenAttachment(String),
    /// Move the completion highlight up (-1) or down (1).
    SuggestMove(i32),
    /// Take a completion: this row, or (`None`) the highlighted one.
    SuggestAccept(Option<usize>),
    /// Close the completions.
    SuggestClose,
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
            icon: Icon::SquarePen,
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
    if state.accounts.is_empty() {
        egui::CentralPanel::default()
            .frame(bars::dock_frame(ui.ctx()))
            .show(ui, |ui| account::none(ui, &mut events));
    } else {
        window(ui, state, commands, strings, stroke, &mut events);
    }
    compose::dialog(ui, state, &mut events);
    compose::confirm(ui, state, &mut events);
    compose::restore(ui, state, &mut events);
    form::dialog(ui, state, &mut events);
    if let Some(dialog) = state.ui.dialog {
        let title = match dialog {
            Dialog::About => "about",
            Dialog::Shortcuts => "shortcuts",
        };
        let id = egui::Id::new("dialog").with(title);
        let shown = dialog::Dialog::new(id, crate::label(title))
            .buttons(vec![Choice::new(
                crate::label("done"),
                dialog::Role::Default,
            )])
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

/// The status bar: the current account's connection and name, the last
/// word, and a note while the accounts are read-only.
fn status(ui: &mut egui::Ui, state: &State, stroke: f32) {
    bars::status(ui, |ui| {
        let current = state.current();
        let (icon, colour) = match current.map(|v| &v.phase) {
            Some(Phase::Ready) => (Icon::Plug, ui.visuals().weak_text_color()),
            _ => (Icon::Unplug, ui.visuals().error_fg_color),
        };
        ui.add(toolkit::icons::image(icon, stroke, ICON, colour));
        let mut line = current.map(|v| v.entry.name.clone()).unwrap_or_default();
        let mut push = |text: &str| {
            if !line.is_empty() {
                line.push_str(" — ");
            }
            line.push_str(text);
        };
        if !state.status.is_empty() {
            push(&state.status);
        }
        if state.source == Source::Missing
            && !state.status.contains(&crate::label("settings-missing"))
        {
            push(&crate::label("settings-missing"));
        }
        ui.small(&line);
    });
}

/// The window with accounts: the tree, the centre pane, the reader.
fn window(
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
        .show(ui, |ui| tree::show(ui, state, stroke, events));
    let split = left.response.rect.width() / total.max(1.0);
    if (split - state.ui.places).abs() > 0.005 {
        events.push(UiEvent::SplitPlaces(split));
    }
    let ready = state.current().filter(|v| v.phase == Phase::Ready);
    if let Some(view) = ready {
        let remaining = ui.available_width();
        let right = egui::Panel::right("reader")
            .resizable(true)
            .default_size(remaining * state.ui.reader)
            .frame(bars::dock_frame(ui.ctx()))
            .show(ui, |ui| {
                reader(ui, state, view, commands, strings, stroke, events)
            });
        let split = right.response.rect.width() / remaining.max(1.0);
        if (split - state.ui.reader).abs() > 0.005 {
            events.push(UiEvent::SplitReader(split));
        }
    }
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| match state.current() {
            Some(view) if view.phase == Phase::Ready => {
                centre(ui, state, view, commands, strings, stroke, events)
            }
            Some(view) => account::pane(ui, view, events),
            None => account::none(ui, events),
        });
}

/// The centre pane: the current mode's list for the current account.
fn centre(
    ui: &mut egui::Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    match state.mode {
        Mode::Mail => mail::list(ui, state, view, commands, strings, stroke, events),
        Mode::Calendar => calendar::list(ui, view, stroke, events),
        Mode::Contacts => contacts::list(ui, view, stroke, events),
    }
}

/// The reader panel: the open email, event or contact.
fn reader(
    ui: &mut egui::Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let title = crate::label("reader");
    panel::group(ui, "reader", &[title.as_str()], |ui, _| match state.mode {
        Mode::Mail => mail::reader(ui, state, view, commands, strings, stroke, events),
        Mode::Calendar => calendar::reader(ui, view),
        Mode::Contacts => contacts::reader(ui, view),
    });
}
