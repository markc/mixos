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
pub mod identities;
pub mod mail;
pub mod tree;

use toolkit::action::Origin;
use toolkit::dialog::{self, Choice};
use toolkit::titlebar::{self, Action};
use toolkit::{Icon, Registry, Strings, bars, panel};

use crate::state::{AccountView, Dialog, Mode, Node, Pane, Phase, Source, State, TreeKey};
use crate::view::mail::LIST;
use egui::{Key, Modifiers};
use toolkit::datatable;

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Command(&'static str, Origin),
    CloseMacros,
    /// Switch the centre pane's surface.
    Mode(Mode),
    /// A tree row was clicked: select what it stands for.
    Select(Node),
    /// A tree row's disclosure: expand (`true`) or collapse it.
    Expand(Node, bool),
    /// A key press while the tree has the keyboard.
    TreeKey(TreeKey),
    /// The pane that has the keyboard now: Tab moved it, or a click did.
    PaneFocused(crate::state::Pane),
    /// The reader's message body in light, while the window is dark.
    SetReaderLight(bool),
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
    ComposeEdit(editor::Event),
    /// The To row's Cc, Bcc or Reply-To button: show or hide that row.
    ComposeShow(crate::state::ComposeField),
    /// Cancel or Escape on the compose: it closes, or (something typed)
    /// the person is asked first.
    CloseCompose,
    /// The answer to the close or quit question over a draft.
    ConfirmDraft(crate::state::DraftChoice),
    /// The answer to the offer of a kept draft.
    RestoreDraft(crate::state::DraftChoice),
    /// Take the account's new address as the stale draft's From.
    ComposeUseNewFrom,
    /// Put a reply's quote above or below the person's text.
    ComposeReplyPosition(crate::quote::Position),
    /// Send the open draft as one of an account's identities (the From list).
    ComposeIdentity {
        account: String,
        identity: String,
    },
    /// The identities dialog: open for an account, pick, edit, add, remove,
    /// save or close.
    OpenIdentities(String),
    IdentityEdit(crate::state::IdentityField, String),
    IdentitySelect(usize),
    IdentityAdd,
    IdentityRemove,
    IdentitySave,
    CloseIdentities,
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
    /// Escape while "Select all N"'s ids load: stop, changing nothing.
    CancelFetch,
}

const ICON: f32 = 14.0;

/// Draw the whole window into `ui`.
pub fn view(
    ui: &mut egui::Ui,
    state: &State,
    commands: &Registry<State>,
    strings: &Strings,
    palette: &editor::Palette,
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    // The icon bar: a new mail, then the standard tail.
    let fired = titlebar::show_bar(
        ui,
        &crate::label("title"),
        Some(Icon::Mail),
        commands,
        state,
        strings,
        &[Action::new("mail.compose", Icon::SquarePen)],
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
    status(ui, state);
    if state.accounts.is_empty() {
        egui::CentralPanel::default()
            .frame(bars::dock_frame(ui.ctx()))
            .show(ui, |ui| account::none(ui, &mut events));
    } else {
        window(ui, state, commands, strings, &mut events);
    }
    compose::dialog(ui, state, palette, &mut events);
    compose::confirm(ui, state, &mut events);
    identities::dialog(ui, state, &mut events);
    compose::restore(ui, state, &mut events);
    form::dialog(ui, state, &mut events);
    if let Some(dialog) = state.ui.dialog {
        let title = match dialog {
            Dialog::About => "about",
            Dialog::Shortcuts => "shortcuts",
        };
        let id = egui::Id::unique("dialog").with(title);
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
    if toolkit::macros::show_app(ui.ctx(), "email", state.ui.macros_open) {
        events.push(UiEvent::CloseMacros);
    }
    toolkit::titlebar::edges(ui);
    events
}

/// The connection word for an account's phase, as the status bar shows it.
pub fn connection(phase: &Phase) -> String {
    crate::label(match phase {
        Phase::Ready => "connected",
        Phase::Connecting => "connecting",
        Phase::Failed(_) | Phase::NoPassword | Phase::Disabled => "not-connected",
    })
}

/// The status bar: the current account's name and its connection (from
/// that account's own phase, never another account's news), then the last
/// operation's word, and a note while the accounts are read-only.
fn status(ui: &mut egui::Ui, state: &State) {
    bars::status(ui, |ui| {
        let current = state.current();
        let (icon, colour) = match current.map(|v| &v.phase) {
            Some(Phase::Ready) => (Icon::Plug, ui.visuals().weak_text_color()),
            _ => (Icon::Unplug, ui.visuals().error_fg_color),
        };
        ui.add(toolkit::icons::image_of(ui.ctx(), icon, ICON, colour));
        let mut line = current
            .map(|v| format!("{} — {}", v.entry.name, connection(&v.phase)))
            .unwrap_or_default();
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

/// A pane's frame. The pane that has the keyboard wears the focus ring the
/// term app draws: the card border a quarter of the way to the accent.
fn pane_frame(ctx: &egui::Context, active: bool) -> egui::Frame {
    // Every pane has the same 1 pt border, transparent when it is not the
    // active one, so the ring appearing never moves the layout.
    let ring = if active {
        let palette = &toolkit::chrome::Chrome::of(ctx).palette;
        palette
            .card_border
            .lerp_to_gamma(palette.accent, FOCUS_RING)
    } else {
        egui::Color32::TRANSPARENT
    };
    bars::dock_frame(ctx).stroke(egui::Stroke::new(1.0, ring))
}

/// The focus ring's share of the accent, as the term app's default.
const FOCUS_RING: f32 = 0.25;

/// Tab and Shift+Tab move the keyboard between the panes, in order: the tree,
/// the list, then the reader. A click on a pane moves the state to it too.
/// Tab is taken only while no text field or dialog has the keyboard.
fn keyboard_panes(ui: &egui::Ui, state: &State, reader: bool, events: &mut Vec<UiEvent>) {
    let ctx = ui.ctx();
    let mut panes = vec![Pane::Tree, Pane::List];
    if reader {
        panes.push(Pane::Reader);
    }
    // Tab is the panes' whenever no text field or dialog is typing: the
    // window takes the keyboard from the mouse hovering over it, so no click
    // is needed first.
    let dialog = state.compose.is_some() || state.identity_editor.is_some();
    if !ctx.text_edit_focused() && !dialog {
        let back = ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Tab));
        let forward = !back && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab));
        if back || forward {
            let at = panes.iter().position(|p| *p == state.pane).unwrap_or(0);
            let next = if forward {
                panes[(at + 1) % panes.len()]
            } else {
                panes[(at + panes.len() - 1) % panes.len()]
            };
            match next {
                Pane::Tree => tree::focus(ctx),
                Pane::List => datatable::focus(ctx, LIST),
                Pane::Reader => mail::focus_reader(ctx),
            }
            events.push(UiEvent::PaneFocused(next));
            return;
        }
    }
    let now = [
        (tree::is_focused(ctx), Pane::Tree),
        (datatable::is_focused(ctx, LIST), Pane::List),
        (reader && mail::reader_focused(ctx), Pane::Reader),
    ]
    .into_iter()
    .find_map(|(on, pane)| on.then_some(pane));
    if let Some(now) = now
        && now != state.pane
    {
        events.push(UiEvent::PaneFocused(now));
    }
}

/// The window with accounts: the tree, the centre pane, the reader.
fn window(
    ui: &mut egui::Ui,
    state: &State,
    commands: &Registry<State>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let total = ui.available_width();
    let reader_shown = state.current().is_some_and(|v| v.phase == Phase::Ready);
    keyboard_panes(ui, state, reader_shown, events);
    let left = egui::Panel::left("places")
        .resizable(true)
        .default_size(total * state.ui.places)
        .frame(pane_frame(ui.ctx(), state.pane == Pane::Tree))
        .show(ui, |ui| tree::show(ui, state, events));
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
            .frame(pane_frame(ui.ctx(), state.pane == Pane::Reader))
            .show(ui, |ui| reader(ui, state, view, commands, strings, events));
        let split = right.response.rect.width() / remaining.max(1.0);
        if (split - state.ui.reader).abs() > 0.005 {
            events.push(UiEvent::SplitReader(split));
        }
    }
    egui::CentralPanel::default()
        .frame(pane_frame(ui.ctx(), state.pane == Pane::List))
        .show(ui, |ui| match state.current() {
            Some(view) if view.phase == Phase::Ready => {
                centre(ui, state, view, commands, strings, events)
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
    events: &mut Vec<UiEvent>,
) {
    match state.mode {
        Mode::Mail => mail::list(ui, state, view, commands, strings, events),
        Mode::Calendar => calendar::list(ui, view, events),
        Mode::Contacts => contacts::list(ui, view, events),
    }
}

/// The reader panel: the open email, event or contact.
fn reader(
    ui: &mut egui::Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    // The mail reader is its own panel: its header holds the actions and the
    // panel menu. The other modes have a single-tab panel.
    if state.mode == Mode::Mail {
        mail::panel(ui, state, view, commands, strings, events);
        return;
    }
    let title = crate::label("reader");
    panel::group(ui, "reader", &[title.as_str()], |ui, _| match state.mode {
        Mode::Calendar => calendar::reader(ui, view),
        Mode::Contacts => contacts::reader(ui, view),
        Mode::Mail => {}
    });
}

/// The reader's options menu, open from the panel's three dots: a click on
/// them opens or closes it, and a click anywhere else closes it.
pub fn reader_menu(
    ui: &mut egui::Ui,
    state: &State,
    button: &egui::Response,
    events: &mut Vec<UiEvent>,
) {
    let id = egui::Id::unique("reader-menu-open");
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if button.clicked() {
        open = !open;
    }
    if open {
        let area = egui::Area::new(id.with("area"))
            .order(egui::Order::Foreground)
            .fixed_pos(button.rect.left_bottom())
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let mut light = state.settings.reader.light;
                    let item = toolkit::toggle::Checkbox::new(
                        &mut light,
                        crate::label("reader-light-content"),
                    );
                    if ui.add(item).changed() {
                        events.push(UiEvent::SetReaderLight(light));
                    }
                });
            });
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        let at = ui.input(|i| i.pointer.interact_pos());
        let inside = at.is_some_and(|p| area.response.rect.contains(p) || button.rect.contains(p));
        if pressed && !inside {
            open = false;
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, open));
}
