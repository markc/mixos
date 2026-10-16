// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The engine: everything the window shows, and every rule that changes
//! it. [`State::apply_ui`] takes a frame's [`view::UiEvent`]s, and
//! [`State::apply`] takes the shell's [`shell::Event`] completions; both
//! mutate the state and queue [`work::Work`] for the shell to perform
//! (`take_work`). Drawing never mutates; a snapshot needs no network.
//!
//! The state stays UI-pure: the connected [`jmap::Client`] lives in the
//! shell, and the events carry only what the state needs to know.

use jmap::{Account, AddressBook, Calendar, CalendarEvent, Composed, Contact, Email, Mailbox};
use serde::{Deserialize, Serialize};

use crate::work::Work;

/// Which surface the centre pane shows.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Mail,
    Calendar,
    Contacts,
}

/// Where the app stands with its server.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// No account yet: the setup form owns the centre pane.
    Setup,
    /// A connect is in flight.
    Connecting,
    /// Connected; the session summary is held.
    Ready,
    /// The last connect failed; the error owns the centre pane.
    Failed(String),
}

/// What the shell knows about the connected account.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub account_name: String,
    pub account_id: String,
}

/// The first-run / re-connect form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetupForm {
    pub url: String,
    pub email: String,
    pub password: String,
    /// A path to a PEM bundle to trust, for a self-issued certificate.
    pub ca_path: String,
    pub error: Option<String>,
    pub busy: bool,
}

/// One field of the setup form.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SetupField {
    Url,
    Email,
    Password,
    CaPath,
}

/// One field of the compose form.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ComposeField {
    To,
    Cc,
    Bcc,
    Subject,
    Body,
}

/// The mail surface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailState {
    pub mailboxes: Vec<Mailbox>,
    pub selected: Option<String>,
    pub emails: Vec<Email>,
    pub total: u64,
    pub open: Option<Email>,
    pub busy: bool,
}

impl MailState {
    /// The selected mailbox, when it is still in the list.
    pub fn selected_mailbox(&self) -> Option<&Mailbox> {
        self.selected
            .as_deref()
            .and_then(|id| self.mailboxes.iter().find(|m| m.id == id))
    }

    /// The mailbox with a role, preferring the inbox.
    pub fn by_role(&self, role: &str) -> Option<&Mailbox> {
        self.mailboxes
            .iter()
            .find(|m| m.role.as_deref() == Some(role))
    }
}

/// The calendar surface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalendarState {
    pub calendars: Vec<Calendar>,
    pub selected: Option<String>,
    pub events: Vec<CalendarEvent>,
    pub open: Option<usize>,
    pub busy: bool,
}

/// The contacts surface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContactState {
    pub books: Vec<AddressBook>,
    pub selected: Option<String>,
    pub contacts: Vec<Contact>,
    pub open: Option<usize>,
    pub filter: String,
    pub busy: bool,
}

/// A compose dialog in flight.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposeState {
    pub draft: Composed,
    /// The recipient fields as typed. The draft's lists are parsed from
    /// them, so a separator the user just typed is not lost.
    pub to_text: String,
    pub cc_text: String,
    pub bcc_text: String,
    pub busy: bool,
    pub error: Option<String>,
    /// The send this compose is waiting on: only its completion closes it.
    pub sending: Option<u64>,
}

impl ComposeState {
    pub fn new(draft: Composed) -> Self {
        Self {
            to_text: draft.to.join(", "),
            cc_text: draft.cc.join(", "),
            bcc_text: draft.bcc.join(", "),
            draft,
            busy: false,
            error: None,
            sending: None,
        }
    }
}

/// The latest request of each kind whose answer can be superseded. A
/// completion carries the ticket of its request; an older one is dropped.
#[derive(Debug, Default)]
struct Tickets {
    page: u64,
    open: u64,
    contacts: u64,
    send: u64,
}

/// The modal dialogs (the setup form is the centre pane, not a dialog).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    About,
    Shortcuts,
}

/// The window's own theme choice: `None` on an axis follows the session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemeChoice {
    pub scheme: Option<design::Scheme>,
    pub mode: Option<design::Mode>,
    pub style: Option<Option<design::Style>>,
    pub decorations: Option<design::Decorations>,
    pub captions: Option<design::CaptionSide>,
}

/// The remembered window shape (persisted) plus the open dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct UiState {
    pub places: f32,
    pub reader: f32,
    pub dialog: Option<Dialog>,
    pub theme: ThemeChoice,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            places: 0.18,
            reader: 0.42,
            dialog: None,
            theme: ThemeChoice::default(),
        }
    }
}

/// Everything the window shows.
pub struct State {
    pub phase: Phase,
    /// The account that last connected (or was loaded): the one saved.
    account: Option<Account>,
    /// A setup form's account while it connects. It replaces `account` only
    /// when the connect succeeds, so a failed edit is never persisted.
    candidate: Option<Account>,
    pub session: Option<SessionInfo>,
    pub mode: Mode,
    pub setup: SetupForm,
    pub mail: MailState,
    pub calendars: CalendarState,
    pub contacts: ContactState,
    pub compose: Option<ComposeState>,
    pub ui: UiState,
    /// The last thing the status bar should say.
    pub status: String,
    /// A command asked the window to close; the shell performs it.
    pub exiting: bool,
    /// The session theme's mode, as the shell installed it (the choice's
    /// `None` mode follows this).
    session_mode: design::Mode,
    work: Vec<Work>,
    tickets: Tickets,
    /// Whether the persisted config (account, shape) has changed.
    config_dirty: bool,
}

impl State {
    /// The state a launch starts in: connecting with the saved account, or
    /// at the setup form without one. The mode is the persisted one.
    pub fn new(account: Option<Account>, ui: UiState, mode: Mode) -> Self {
        let (phase, work) = match &account {
            Some(account) => (Phase::Connecting, vec![Work::Connect(account.clone())]),
            None => (Phase::Setup, Vec::new()),
        };
        let mut setup = SetupForm::default();
        if let Some(account) = &account {
            setup.url = account.url.clone();
            setup.email = account.email.clone();
        }
        Self {
            phase,
            account,
            candidate: None,
            session: None,
            mode,
            setup,
            mail: MailState::default(),
            calendars: CalendarState::default(),
            contacts: ContactState::default(),
            compose: None,
            ui,
            status: String::new(),
            exiting: false,
            session_mode: design::Mode::Dark,
            work,
            tickets: Tickets::default(),
            config_dirty: false,
        }
    }

    /// The saved account, when there is one.
    pub fn account(&self) -> Option<&Account> {
        self.account.as_ref()
    }

    /// Whether the persisted config should be rewritten.
    pub fn config_dirty(&self) -> bool {
        self.config_dirty
    }

    /// The shell saved the config.
    pub fn config_saved(&mut self) {
        self.config_dirty = false;
    }

    /// The work this state has queued; the shell drains it after each
    /// event and each frame.
    pub fn take_work(&mut self) -> Vec<Work> {
        std::mem::take(&mut self.work)
    }

    fn queue(&mut self, work: Work) {
        self.work.push(work);
    }

    /// Apply one shell completion.
    pub fn apply(&mut self, event: crate::shell::Event) {
        use crate::shell::Event;
        match event {
            Event::Delivery(..) | Event::Look(..) | Event::LookRetry(_) => {}
            Event::Connected(Ok(info)) => {
                if let Some(account) = self.candidate.take() {
                    self.account = Some(account);
                }
                self.phase = Phase::Ready;
                self.session = Some(info);
                self.setup.busy = false;
                self.setup.error = None;
                self.config_dirty = true;
                self.status = crate::label("connected");
                self.queue(Work::FetchMailboxes);
                self.queue(Work::FetchCalendars);
                self.queue(Work::FetchBooks);
            }
            Event::Connected(Err(error)) => {
                let error = error.to_string();
                // A failed candidate is dropped: the saved account stays.
                self.candidate = None;
                if self.setup.busy || self.account.is_none() {
                    // First connect or an edited account: back to the form.
                    self.setup.busy = false;
                    self.setup.error = Some(error.clone());
                    self.phase = Phase::Setup;
                } else {
                    self.phase = Phase::Failed(error.clone());
                }
                self.status = error;
            }
            Event::Mailboxes(Ok(mailboxes)) => {
                self.mail.busy = false;
                self.mail.mailboxes = mailboxes;
                let selected = self
                    .mail
                    .selected
                    .clone()
                    .filter(|id| self.mail.mailboxes.iter().any(|m| m.id == *id))
                    .or_else(|| {
                        self.mail
                            .by_role("inbox")
                            .map(|m| m.id.clone())
                            .or_else(|| self.mail.mailboxes.first().map(|m| m.id.clone()))
                    });
                // A refresh of the same mailbox (new counts after a mark-read)
                // keeps the page and the open email; only a change reselects.
                if let Some(id) = selected
                    && self.mail.selected.as_deref() != Some(id.as_str())
                {
                    self.select_mailbox(id);
                }
            }
            Event::Mailboxes(Err(error)) => self.failed_now(&error),
            Event::Page(ticket, _) if ticket != self.tickets.page => {}
            Event::Page(_, Ok(page)) => {
                self.mail.busy = false;
                self.mail.emails = page.emails;
                self.mail.total = page.total;
            }
            Event::Page(_, Err(error)) => {
                self.mail.busy = false;
                self.failed_now(&error);
            }
            Event::Opened(ticket, _) if ticket != self.tickets.open => {}
            Event::Opened(_, result) => match *result {
                Ok(email) => self.mail.open = Some(email),
                Err(ref error) => self.failed_now(error),
            },
            Event::Marked(Ok(())) => self.queue(Work::FetchMailboxes),
            Event::Marked(Err(error)) => self.failed_now(&error),
            Event::Sent(ticket, result) => {
                // Only the compose that sent closes or shows the error; a
                // send whose compose was cancelled leaves a newer one alone.
                let mine = self
                    .compose
                    .as_ref()
                    .is_some_and(|compose| compose.sending == Some(ticket));
                match result {
                    Ok(()) => {
                        if mine {
                            self.compose = None;
                        }
                        self.status = crate::label("sent");
                        // The Sent mailbox and its counts just changed.
                        self.queue(Work::FetchMailboxes);
                        if let Some(mailbox) = self.mail.selected_mailbox() {
                            let id = mailbox.id.clone();
                            self.fetch_emails(id);
                        }
                    }
                    Err(error) => {
                        let error = error.to_string();
                        if mine && let Some(compose) = &mut self.compose {
                            compose.busy = false;
                            compose.sending = None;
                            compose.error = Some(error.clone());
                        }
                        self.status = error;
                    }
                }
            }
            Event::Calendars(Ok(calendars)) => {
                self.calendars.busy = false;
                self.calendars.calendars = calendars;
                let selected = self
                    .calendars
                    .selected
                    .clone()
                    .filter(|id| self.calendars.calendars.iter().any(|c| c.id == *id))
                    .or_else(|| self.calendars.calendars.first().map(|c| c.id.clone()));
                if let Some(id) = selected {
                    self.select_calendar(id);
                }
            }
            Event::Calendars(Err(error)) => self.failed_now(&error),
            Event::Events(Ok(events)) => {
                self.calendars.busy = false;
                self.calendars.events = events;
                self.calendars.open = None;
            }
            Event::Events(Err(error)) => {
                self.calendars.busy = false;
                self.failed_now(&error);
            }
            Event::Books(Ok(books)) => {
                self.contacts.busy = false;
                self.contacts.books = books;
                let selected = self
                    .contacts
                    .selected
                    .clone()
                    .filter(|id| self.contacts.books.iter().any(|b| b.id == *id))
                    .or_else(|| self.contacts.books.first().map(|b| b.id.clone()));
                if let Some(id) = selected {
                    self.select_book(id);
                }
            }
            Event::Books(Err(error)) => self.failed_now(&error),
            Event::Contacts(ticket, _) if ticket != self.tickets.contacts => {}
            Event::Contacts(_, Ok(contacts)) => {
                self.contacts.busy = false;
                self.contacts.contacts = contacts;
                self.contacts.open = None;
            }
            Event::Contacts(_, Err(error)) => {
                self.contacts.busy = false;
                self.failed_now(&error);
            }
        }
    }

    /// A background failure the surfaces should read off the status line.
    fn failed_now(&mut self, error: &jmap::Error) {
        self.status = error.to_string();
    }

    /// Select a mailbox and fetch its first page.
    pub fn select_mailbox(&mut self, id: String) {
        self.mail.selected = Some(id.clone());
        self.mail.open = None;
        // An open still in flight belongs to the old mailbox.
        self.tickets.open += 1;
        self.mail.busy = true;
        self.fetch_emails(id);
    }

    /// Queue a page fetch; only the newest one's answer is applied.
    fn fetch_emails(&mut self, mailbox: String) {
        self.tickets.page += 1;
        let ticket = self.tickets.page;
        self.queue(Work::FetchEmails { ticket, mailbox });
    }

    /// Queue an email open; only the newest one's answer is applied.
    fn open_email(&mut self, id: String) {
        self.tickets.open += 1;
        let ticket = self.tickets.open;
        self.queue(Work::OpenEmail { ticket, id });
    }

    /// Queue a contact search; only the newest one's answer is applied.
    fn fetch_contacts(&mut self, book: String, filter: String) {
        self.tickets.contacts += 1;
        let ticket = self.tickets.contacts;
        self.queue(Work::FetchContacts {
            ticket,
            book,
            filter,
        });
    }

    /// Select a calendar and fetch its events.
    pub fn select_calendar(&mut self, id: String) {
        self.calendars.selected = Some(id.clone());
        self.calendars.busy = true;
        self.queue(Work::FetchEvents { calendar: id });
    }

    /// Select an address book and fetch its contacts.
    pub fn select_book(&mut self, id: String) {
        self.contacts.selected = Some(id.clone());
        self.contacts.busy = true;
        let filter = self.contacts.filter.clone();
        self.fetch_contacts(id, filter);
    }

    /// Apply one frame's interactions.
    pub fn apply_ui(&mut self, event: crate::view::UiEvent) {
        use crate::view::UiEvent;
        match event {
            UiEvent::Command(id, origin) => {
                let _ = (id, origin);
                // Commands run through the registry (commands.rs), which
                // the shell drives; nothing reaches the state here.
            }
            UiEvent::Mode(mode) => {
                if self.mode != mode {
                    self.mode = mode;
                    self.config_dirty = true;
                }
                self.fetch_missing_for_mode();
            }
            UiEvent::SelectMailbox(id) => self.select_mailbox(id),
            UiEvent::OpenEmail(id) => {
                // Optimistically mark the row read; the server's word
                // replaces the counts when it answers.
                if let Some(email) = self.mail.emails.iter_mut().find(|email| email.id == id)
                    && !email.is_read()
                {
                    email
                        .keywords
                        .get_or_insert_with(|| serde_json::json!({}))
                        .as_object_mut()
                        .map(|k| k.insert("$seen".into(), serde_json::json!(true)));
                    self.queue(Work::MarkRead {
                        id: id.clone(),
                        read: true,
                    });
                }
                self.open_email(id);
            }
            UiEvent::SelectCalendar(id) => self.select_calendar(id),
            UiEvent::SelectEvent(index) => self.calendars.open = Some(index),
            UiEvent::SelectBook(id) => self.select_book(id),
            UiEvent::SelectContact(index) => self.contacts.open = Some(index),
            UiEvent::ContactFilter(filter) => {
                self.contacts.filter = filter;
                if let Some(book) = self.contacts.selected.clone() {
                    self.contacts.busy = true;
                    let filter = self.contacts.filter.clone();
                    self.fetch_contacts(book, filter);
                }
            }
            UiEvent::Compose => {
                self.compose = Some(ComposeState::new(Composed {
                    from: self
                        .account
                        .as_ref()
                        .map(|account| account.email.clone())
                        .unwrap_or_default(),
                    ..Composed::default()
                }));
            }
            UiEvent::Reply => {
                // A reply works from the open email.
                if let Some(open) = self.mail.open.clone() {
                    let to = open
                        .from
                        .as_ref()
                        .and_then(|from| from.first())
                        .map(|a| a.email.clone())
                        .unwrap_or_default();
                    let subject = match open.subject.as_deref() {
                        Some(subject) if subject.to_lowercase().starts_with("re:") => {
                            subject.to_owned()
                        }
                        Some(subject) => format!("Re: {subject}"),
                        None => crate::label("reply-subject"),
                    };
                    self.compose = Some(ComposeState::new(Composed {
                        from: self
                            .account
                            .as_ref()
                            .map(|account| account.email.clone())
                            .unwrap_or_default(),
                        to: vec![to],
                        subject,
                        in_reply_to: open
                            .message_id
                            .clone()
                            .and_then(|ids| ids.into_iter().next()),
                        ..Composed::default()
                    }));
                }
            }
            UiEvent::ComposeField(field, value) => {
                if let Some(compose) = &mut self.compose {
                    // Keep what was typed; the lists are parsed from it.
                    match field {
                        ComposeField::To => {
                            compose.draft.to = split_addresses(&value);
                            compose.to_text = value;
                        }
                        ComposeField::Cc => {
                            compose.draft.cc = split_addresses(&value);
                            compose.cc_text = value;
                        }
                        ComposeField::Bcc => {
                            compose.draft.bcc = split_addresses(&value);
                            compose.bcc_text = value;
                        }
                        ComposeField::Subject => compose.draft.subject = value,
                        ComposeField::Body => compose.draft.body = value,
                    }
                }
            }
            UiEvent::CloseCompose => self.compose = None,
            UiEvent::SendCompose => {
                if let Some(compose) = &self.compose
                    && !compose.busy
                {
                    let draft = compose.draft.clone();
                    self.tickets.send += 1;
                    let ticket = self.tickets.send;
                    self.queue(Work::Send { ticket, draft });
                    if let Some(compose) = &mut self.compose {
                        compose.busy = true;
                        compose.error = None;
                        compose.sending = Some(ticket);
                    }
                }
            }
            UiEvent::SetupField(field, value) => match field {
                SetupField::Url => self.setup.url = value,
                SetupField::Email => self.setup.email = value,
                SetupField::Password => self.setup.password = value,
                SetupField::CaPath => self.setup.ca_path = value,
            },
            UiEvent::SetupConnect => self.setup_connect(),
            UiEvent::Retry => {
                if let Some(account) = self.account.clone() {
                    self.phase = Phase::Connecting;
                    self.status = crate::label("connecting");
                    self.queue(Work::Connect(account));
                }
            }
            UiEvent::SetupEdit => {
                self.phase = Phase::Setup;
            }
            UiEvent::SplitPlaces(split) => {
                if (split - self.ui.places).abs() > 0.005 {
                    self.ui.places = split;
                    self.config_dirty = true;
                }
            }
            UiEvent::SplitReader(split) => {
                if (split - self.ui.reader).abs() > 0.005 {
                    self.ui.reader = split;
                    self.config_dirty = true;
                }
            }
            UiEvent::CloseDialog => self.ui.dialog = None,
            UiEvent::SetMode(mode) => {
                self.ui.theme.mode = Some(mode);
            }
        }
    }

    /// Fetch whatever the newly-current mode has nothing of yet.
    fn fetch_missing_for_mode(&mut self) {
        match self.mode {
            Mode::Mail => {
                if self.mail.mailboxes.is_empty() {
                    self.queue(Work::FetchMailboxes);
                }
            }
            Mode::Calendar => {
                if self.calendars.calendars.is_empty() {
                    self.queue(Work::FetchCalendars);
                } else if self.calendars.events.is_empty() && self.calendars.selected.is_some() {
                    let calendar = self.calendars.selected.clone().unwrap();
                    self.calendars.busy = true;
                    self.queue(Work::FetchEvents { calendar });
                }
            }
            Mode::Contacts => {
                if self.contacts.books.is_empty() {
                    self.queue(Work::FetchBooks);
                } else if self.contacts.contacts.is_empty() && self.contacts.selected.is_some() {
                    let book = self.contacts.selected.clone().unwrap();
                    let filter = self.contacts.filter.clone();
                    self.contacts.busy = true;
                    self.fetch_contacts(book, filter);
                }
            }
        }
    }

    /// Validate the setup form and connect with it.
    fn setup_connect(&mut self) {
        if self.setup.busy {
            return;
        }
        let url = self.setup.url.trim().trim_end_matches('/').to_owned();
        let email = self.setup.email.trim().to_owned();
        let ca_pem = if self.setup.ca_path.trim().is_empty() {
            None
        } else {
            match std::fs::read_to_string(self.setup.ca_path.trim()) {
                Ok(pem) => Some(pem),
                Err(error) => {
                    self.setup.error = Some(format!("{}: {error}", crate::label("setup-ca-error")));
                    return;
                }
            }
        };
        if url.is_empty() || email.is_empty() || self.setup.password.is_empty() {
            self.setup.error = Some(crate::label("setup-incomplete"));
            return;
        }
        if !url.starts_with("https://") {
            self.setup.error = Some(crate::label("setup-url-error"));
            return;
        }
        let account = Account {
            url,
            email,
            password: self.setup.password.clone(),
            ca_pem,
        };
        self.candidate = Some(account.clone());
        self.setup.busy = true;
        self.setup.error = None;
        self.phase = Phase::Connecting;
        self.status = crate::label("connecting");
        self.queue(Work::Connect(account));
    }

    /// Whether a compose is sendable right now (the dialog's Send button).
    pub fn compose_sendable(&self) -> bool {
        match &self.compose {
            Some(compose) => {
                !compose.busy
                    && !compose.draft.to.is_empty()
                    && !compose.draft.subject.trim().is_empty()
            }
            None => false,
        }
    }

    /// The Sent mailbox id, for the send recipe.
    pub fn sent_mailbox_id(&self) -> Option<String> {
        self.mail.by_role("sent").map(|m| m.id.clone())
    }

    // The command registry's entry points (commands.rs).

    /// Refetch the current mode's surfaces.
    pub fn refresh(&mut self) {
        self.queue(Work::FetchMailboxes);
        match self.mode {
            Mode::Mail => {
                if let Some(mailbox) = self.mail.selected.clone() {
                    self.mail.busy = true;
                    self.fetch_emails(mailbox);
                }
            }
            Mode::Calendar => {
                self.queue(Work::FetchCalendars);
                if let Some(calendar) = self.calendars.selected.clone() {
                    self.calendars.busy = true;
                    self.queue(Work::FetchEvents { calendar });
                }
            }
            Mode::Contacts => {
                self.queue(Work::FetchBooks);
                if let Some(book) = self.contacts.selected.clone() {
                    let filter = self.contacts.filter.clone();
                    self.contacts.busy = true;
                    self.fetch_contacts(book, filter);
                }
            }
        }
    }

    /// A command asked the window to close.
    pub fn quit(&mut self) {
        self.exiting = true;
    }

    /// Open a compose dialog.
    pub fn compose(&mut self) {
        self.apply_ui(crate::view::UiEvent::Compose);
    }

    /// Reply to the open email.
    pub fn reply(&mut self) {
        self.apply_ui(crate::view::UiEvent::Reply);
    }

    /// Switch surfaces.
    pub fn set_mode(&mut self, mode: Mode) {
        self.apply_ui(crate::view::UiEvent::Mode(mode));
    }

    /// Open a modal dialog.
    pub fn open_dialog(&mut self, dialog: Dialog) {
        self.ui.dialog = Some(dialog);
    }

    /// The shell installed a session theme; remember its mode so the
    /// title-bar toggle can invert what is actually drawn.
    pub fn set_session_mode(&mut self, mode: design::Mode) {
        self.session_mode = mode;
    }

    /// The mode this window is drawing: the choice's, else the session's.
    pub fn effective_mode(&self) -> design::Mode {
        self.ui.theme.mode.unwrap_or(self.session_mode)
    }

    /// The title bar's light/dark toggle: invert the drawn mode.
    pub fn toggle_mode(&mut self) {
        let target = match self.effective_mode() {
            design::Mode::Dark => design::Mode::Light,
            design::Mode::Light => design::Mode::Dark,
        };
        self.apply_ui(crate::view::UiEvent::SetMode(target));
    }
}

/// Split an address line on commas and semicolons, dropping empties.
fn split_addresses(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Event;
    use crate::view::UiEvent;
    use serde_json::json;

    fn account() -> Account {
        Account {
            url: "https://example.invalid".into(),
            email: "ann@example.invalid".into(),
            password: "secret".into(),
            ca_pem: None,
        }
    }

    fn connected() -> State {
        let mut state = State::new(Some(account()), UiState::default(), Mode::Mail);
        state.take_work();
        state.apply(Event::Connected(Ok(SessionInfo {
            account_name: "Account 7".into(),
            account_id: "7".into(),
        })));
        state.take_work();
        // The shell saves the config when the connect settles.
        state.config_saved();
        state
    }

    fn mailbox(id: &str, role: Option<&str>, unread: u64) -> Mailbox {
        Mailbox {
            id: id.into(),
            name: id.to_uppercase(),
            role: role.map(str::to_owned),
            unread_emails: unread,
            ..Mailbox::default()
        }
    }

    #[test]
    fn a_saved_account_connects_at_launch() {
        let mut state = State::new(Some(account()), UiState::default(), Mode::Mail);
        assert!(matches!(state.phase, Phase::Connecting));
        assert!(matches!(state.take_work().as_slice(), [Work::Connect(_)]));
    }

    #[test]
    fn no_account_opens_the_setup_form() {
        let mut state = State::new(None, UiState::default(), Mode::Mail);
        assert!(matches!(state.phase, Phase::Setup));
        assert!(state.take_work().is_empty());
    }

    #[test]
    fn a_first_connect_adopts_the_setup_form_account() {
        let mut state = State::new(None, UiState::default(), Mode::Mail);
        state.apply_ui(UiEvent::SetupField(
            SetupField::Url,
            "https://example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SetupField(
            SetupField::Email,
            "ann@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SetupField(SetupField::Password, "secret".into()));
        state.apply_ui(UiEvent::SetupConnect);
        assert!(matches!(state.phase, Phase::Connecting));
        assert!(state.account().is_none(), "adopted only once it connects");
        match state.take_work().as_slice() {
            [Work::Connect(account)] => assert_eq!(account.url, "https://example.invalid"),
            other => panic!("{other:?}"),
        }
        state.apply(Event::Connected(Ok(SessionInfo {
            account_name: "Account 7".into(),
            account_id: "7".into(),
        })));
        assert_eq!(state.account().unwrap().password, "secret");
    }

    #[test]
    fn a_failed_edit_never_replaces_the_saved_account() {
        let mut state = connected();
        let saved = state.account().unwrap().password.clone();
        state.apply_ui(UiEvent::SetupEdit);
        state.apply_ui(UiEvent::SetupField(SetupField::Password, "wrong".into()));
        state.apply_ui(UiEvent::SetupConnect);
        state.take_work();
        // A mode change while connecting marks the config dirty.
        state.set_mode(Mode::Calendar);
        assert_eq!(state.account().unwrap().password, saved);
        state.apply(Event::Connected(Err(jmap::Error::Unauthorized(
            "no".into(),
        ))));
        state.set_mode(Mode::Mail);
        assert_eq!(state.account().unwrap().password, saved);
    }

    #[test]
    fn an_incomplete_setup_form_is_refused() {
        let mut state = State::new(None, UiState::default(), Mode::Mail);
        state.apply_ui(UiEvent::SetupConnect);
        assert!(state.setup.error.is_some());
        assert!(state.take_work().is_empty());
    }

    #[test]
    fn a_failed_first_connect_returns_to_the_form_with_the_error() {
        let mut state = State::new(None, UiState::default(), Mode::Mail);
        state.apply_ui(UiEvent::SetupField(
            SetupField::Url,
            "https://example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SetupField(
            SetupField::Email,
            "ann@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SetupField(SetupField::Password, "wrong".into()));
        state.apply_ui(UiEvent::SetupConnect);
        state.take_work();
        state.apply(Event::Connected(Err(jmap::Error::Unauthorized(
            "no".into(),
        ))));
        assert!(matches!(state.phase, Phase::Setup));
        assert!(state.setup.error.is_some());
        assert!(!state.setup.busy);
    }

    #[test]
    fn a_failed_reconnect_shows_the_error_with_a_retry() {
        let mut state = connected();
        state.apply(Event::Connected(Err(jmap::Error::Unauthorized(
            "the server went away".into(),
        ))));
        assert!(matches!(state.phase, Phase::Failed(_)));
        state.apply_ui(UiEvent::Retry);
        assert!(matches!(state.phase, Phase::Connecting));
        assert!(matches!(state.take_work().as_slice(), [Work::Connect(_)]));
    }

    #[test]
    fn arriving_mailboxes_select_the_inbox_and_fetch_its_first_page() {
        let mut state = connected();
        state.apply(Event::Mailboxes(Ok(vec![
            mailbox("m-sent", Some("sent"), 0),
            mailbox("m-in", Some("inbox"), 3),
        ])));
        assert_eq!(state.mail.selected.as_deref(), Some("m-in"));
        match state.take_work().as_slice() {
            [Work::FetchEmails { mailbox, .. }] => assert_eq!(mailbox, "m-in"),
            other => panic!("{other:?}"),
        }
    }

    fn unread_email(id: &str) -> Email {
        serde_json::from_value(json!({
            "id": id,
            "subject": "Hello",
            "from": [{ "name": "Bob", "email": "bob@example.invalid" }],
            "messageId": ["<m1@example.invalid>"],
            "receivedAt": "2026-10-13T01:02:03Z",
            "keywords": {}
        }))
        .unwrap()
    }

    #[test]
    fn opening_an_unread_email_marks_it_read_locally_and_on_the_server() {
        let mut state = connected();
        state.mail.selected = Some("m-in".into());
        state.mail.emails = vec![unread_email("E1")];
        state.apply_ui(UiEvent::OpenEmail("E1".into()));
        let work = state.take_work();
        assert!(
            work.iter()
                .any(|w| matches!(w, Work::MarkRead { id, read: true } if id == "E1"))
        );
        assert!(
            work.iter()
                .any(|w| matches!(w, Work::OpenEmail { id, .. } if id == "E1"))
        );
        assert!(state.mail.emails[0].is_read());
    }

    #[test]
    fn reply_prefills_to_subject_and_in_reply_to() {
        let mut state = connected();
        state.mail.open = Some(
            serde_json::from_value(json!({
                "id": "E1",
                "subject": "Lunch",
                "from": [{ "name": "Bob", "email": "bob@example.invalid" }],
                "messageId": ["<m1@example.invalid>"]
            }))
            .unwrap(),
        );
        state.apply_ui(UiEvent::Reply);
        let compose = state.compose.expect("a compose dialog");
        assert_eq!(compose.draft.to, ["bob@example.invalid"]);
        assert_eq!(compose.draft.subject, "Re: Lunch");
        assert_eq!(
            compose.draft.in_reply_to.as_deref(),
            Some("<m1@example.invalid>")
        );
        assert_eq!(compose.draft.from, "ann@example.invalid");
    }

    #[test]
    fn compose_fields_split_address_lists() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid, carol@example.invalid; ".into(),
        ));
        state.apply_ui(UiEvent::ComposeField(ComposeField::Subject, "Hi".into()));
        let compose = state.compose.clone().unwrap();
        assert_eq!(
            compose.draft.to,
            ["bob@example.invalid", "carol@example.invalid"]
        );
        assert!(state.compose_sendable());
    }

    #[test]
    fn an_empty_compose_cannot_send() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        assert!(!state.compose_sendable());
    }

    #[test]
    fn sending_closes_the_dialog_and_refreshes_the_surfaces() {
        let mut state = connected();
        state.mail.mailboxes = vec![mailbox("m-sent", Some("sent"), 0)];
        state.mail.selected = Some("m-sent".into());
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SendCompose);
        assert!(state.compose.as_ref().is_some_and(|c| c.busy));
        let ticket = sent_ticket(&mut state);
        state.apply(Event::Sent(ticket, Ok(())));
        assert!(state.compose.is_none());
        let work = state.take_work();
        assert!(work.iter().any(|w| matches!(w, Work::FetchMailboxes)));
        assert!(matches!(
            work.as_slice(),
            [_, Work::FetchEmails { mailbox, .. }] if mailbox == "m-sent"
        ));
    }

    /// Take the queued work and return the send's ticket.
    fn sent_ticket(state: &mut State) -> u64 {
        state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::Send { ticket, .. } => Some(*ticket),
                _ => None,
            })
            .expect("a send was queued")
    }

    #[test]
    fn a_failed_send_keeps_the_dialog_with_the_error() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SendCompose);
        let ticket = sent_ticket(&mut state);
        state.apply(Event::Sent(
            ticket,
            Err(jmap::Error::method("serverFail", Some("no relay"))),
        ));
        let compose = state.compose.expect("the dialog stays");
        assert!(!compose.busy);
        assert!(compose.error.is_some());
    }

    #[test]
    fn a_late_send_success_leaves_a_newer_draft_alone() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(ComposeField::To, "bob@example.invalid".into()));
        state.apply_ui(UiEvent::SendCompose);
        let first = sent_ticket(&mut state);
        // Cancelled while sending, then a new draft.
        state.apply_ui(UiEvent::CloseCompose);
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(ComposeField::Subject, "next".into()));
        state.apply(Event::Sent(first, Ok(())));
        let compose = state.compose.as_ref().expect("the new draft stays");
        assert_eq!(compose.draft.subject, "next");
        assert!(!compose.busy);
    }

    #[test]
    fn a_typed_separator_stays_in_the_recipient_field() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(ComposeField::To, "bob@example.invalid,".into()));
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.to_text, "bob@example.invalid,");
        assert_eq!(compose.draft.to, ["bob@example.invalid"]);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid, cy@example.invalid".into(),
        ));
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.draft.to, ["bob@example.invalid", "cy@example.invalid"]);
    }

    #[test]
    fn a_superseded_answer_is_dropped() {
        let mut state = connected();
        state.mail.mailboxes = vec![mailbox("m-in", Some("inbox"), 0), mailbox("m-b", None, 0)];
        state.select_mailbox("m-in".into());
        state.select_mailbox("m-b".into());
        let tickets: Vec<u64> = state
            .take_work()
            .iter()
            .filter_map(|w| match w {
                Work::FetchEmails { ticket, .. } => Some(*ticket),
                _ => None,
            })
            .collect();
        let [old, new] = tickets[..] else {
            panic!("{tickets:?}")
        };
        let page = |id: &str| jmap::EmailPage {
            emails: vec![
                serde_json::from_value(serde_json::json!({ "id": id, "keywords": {} })).unwrap(),
            ],
            total: 1,
        };
        state.apply(Event::Page(new, Ok(page("B1"))));
        // The first mailbox's page arrives last: it must not replace m-b's.
        state.apply(Event::Page(old, Ok(page("A1"))));
        assert_eq!(state.mail.emails[0].id, "B1");
        // Likewise a contact search and an opened body.
        state.apply(Event::Contacts(state.tickets.contacts + 1, Ok(Vec::new())));
        state.apply(Event::Opened(
            state.tickets.open + 1,
            Box::new(Ok(page("X").emails.remove(0))),
        ));
        assert!(state.mail.open.is_none());
    }

    #[test]
    fn a_count_refresh_keeps_the_open_email() {
        let mut state = connected();
        state.apply(Event::Mailboxes(Ok(vec![mailbox("m-in", Some("inbox"), 1)])));
        state.take_work();
        state.apply_ui(UiEvent::OpenEmail("E1".into()));
        let open = state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::OpenEmail { ticket, .. } => Some(*ticket),
                _ => None,
            })
            .unwrap();
        let email: Email =
            serde_json::from_value(serde_json::json!({ "id": "E1", "keywords": {} })).unwrap();
        state.apply(Event::Opened(open, Box::new(Ok(email))));
        // The mark-read's count refresh answers after the body.
        state.apply(Event::Mailboxes(Ok(vec![mailbox("m-in", Some("inbox"), 0)])));
        assert_eq!(state.mail.open.as_ref().map(|e| e.id.as_str()), Some("E1"));
    }

    #[test]
    fn a_mode_change_fetches_what_the_mode_lacks() {
        let mut state = connected();
        state.apply(Event::Calendars(Ok(vec![Calendar {
            id: "C1".into(),
            name: "Main".into(),
            ..Calendar::default()
        }])));
        state.take_work();
        state.apply_ui(UiEvent::Mode(Mode::Calendar));
        match state.take_work().as_slice() {
            [Work::FetchEvents { calendar }] => assert_eq!(calendar, "C1"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_contact_filter_refetches_the_selected_book() {
        let mut state = connected();
        state.apply(Event::Books(Ok(vec![AddressBook {
            id: "B1".into(),
            name: "Main".into(),
            ..AddressBook::default()
        }])));
        state.take_work();
        state.apply_ui(UiEvent::ContactFilter("an".into()));
        match state.take_work().as_slice() {
            [Work::FetchContacts { book, filter, .. }] => {
                assert_eq!(book, "B1");
                assert_eq!(filter, "an");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_split_change_marks_the_config_dirty() {
        let mut state = connected();
        assert!(!state.config_dirty());
        state.apply_ui(UiEvent::SplitPlaces(0.25));
        assert!(state.config_dirty());
        state.config_saved();
        assert!(!state.config_dirty());
    }
}
