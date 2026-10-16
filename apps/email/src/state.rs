// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The engine: everything the window shows, and every rule that changes
//! it. [`State::apply_ui`] takes a frame's [`view::UiEvent`]s, and
//! [`State::apply`] takes the shell's [`shell::Event`] completions; both
//! mutate the state and queue [`work::Work`] for the shell to perform
//! (`take_work`). Drawing never mutates; a snapshot needs no network.
//!
//! The state holds one [`AccountView`] per settings account, in tree order,
//! each with its own connection phase, mail, calendars and contacts, and its
//! own request tickets: an answer is addressed by account id and connection
//! generation, so a late answer from one account (or from an account's
//! previous connection) can never change another view. The accounts come
//! from settingsd ([`State::set_settings`]), or from the cache of the last
//! ones seen while settingsd is absent; their passwords come from the
//! app's secrets file and never leave the state except toward the account's
//! own server.
//!
//! The state stays UI-pure: the connected [`jmap::Client`]s live in the
//! shell, and the events carry only what the state needs to know.

use std::collections::BTreeMap;

use jmap::{AddressBook, Calendar, CalendarEvent, Composed, Contact, Email, EmailAddress, Mailbox};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use settings::email::{AccountEntry, Email as EmailSettings};

use crate::secrets::Secrets;
use crate::work::{Login, Work};
use crate::writer::{self, Change, Refusal};

/// Which surface the centre pane shows.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Mail,
    Calendar,
    Contacts,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Mail, Mode::Calendar, Mode::Contacts];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Mail => "mail",
            Mode::Calendar => "calendar",
            Mode::Contacts => "contacts",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// Where one account stands with its server.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// A connect is in flight.
    Connecting,
    /// Connected; the session summary is held.
    Ready,
    /// The last connect failed; the error shows in the centre pane.
    Failed(String),
    /// Enabled, but no password is held for it yet.
    NoPassword,
    /// Turned off in the settings: never connected.
    Disabled,
}

impl Phase {
    /// The phase's name on the Bus.
    pub fn name(&self) -> &'static str {
        match self {
            Phase::Connecting => "connecting",
            Phase::Ready => "ready",
            Phase::Failed(_) => "failed",
            Phase::NoPassword => "no_password",
            Phase::Disabled => "disabled",
        }
    }
}

/// What the shell knows about a connected account.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionInfo {
    pub account_name: String,
    pub account_id: String,
}

/// Which account's connection an answer came from: an answer whose
/// generation is not the account's current one is dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub account: String,
    pub generation: u64,
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

    /// The mailbox with a role.
    pub fn by_role(&self, role: &str) -> Option<&Mailbox> {
        self.mailboxes
            .iter()
            .find(|m| m.role.as_deref() == Some(role))
    }

    /// Every mailbox's unread count, summed.
    pub fn unread(&self) -> u64 {
        self.mailboxes.iter().map(|m| m.unread_emails).sum()
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
    /// The account it sends from.
    pub account: String,
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
    pub fn new(account: &str, draft: Composed) -> Self {
        let text = |list: &[EmailAddress]| {
            list.iter()
                .map(jmap::format_address)
                .collect::<Vec<_>>()
                .join(", ")
        };
        Self {
            account: account.to_owned(),
            to_text: text(&draft.to),
            cc_text: text(&draft.cc),
            bcc_text: text(&draft.bcc),
            draft,
            busy: false,
            error: None,
            sending: None,
        }
    }
}

/// The latest request of each kind whose answer can be superseded, per
/// account. A completion carries the ticket of its request; an older one
/// is dropped.
#[derive(Debug, Default)]
struct Tickets {
    page: u64,
    open: u64,
    /// The mailbox the latest open was issued under: its answer is dropped
    /// when the selection has changed since.
    open_mailbox: Option<String>,
    events: u64,
    contacts: u64,
    send: u64,
}

/// One settings account and everything the window holds for it.
#[derive(Debug)]
pub struct AccountView {
    /// The account as settingsd (or the cache) holds it.
    pub entry: AccountEntry,
    pub phase: Phase,
    pub session: Option<SessionInfo>,
    pub mail: MailState,
    pub calendars: CalendarState,
    pub contacts: ContactState,
    tickets: Tickets,
    /// Bumped on every connect: answers from an older connection are void.
    generation: u64,
}

impl AccountView {
    fn new(entry: AccountEntry) -> Self {
        Self {
            entry,
            phase: Phase::Disabled,
            session: None,
            mail: MailState::default(),
            calendars: CalendarState::default(),
            contacts: ContactState::default(),
            tickets: Tickets::default(),
            generation: 0,
        }
    }

    pub fn id(&self) -> &str {
        &self.entry.id
    }

    /// The connection generation answers must carry.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Drop everything fetched (a new connection starts empty). Tickets
    /// keep counting, so nothing in flight can match a new request.
    fn forget(&mut self) {
        self.session = None;
        self.mail = MailState::default();
        self.calendars = CalendarState::default();
        let filter = std::mem::take(&mut self.contacts.filter);
        self.contacts = ContactState {
            filter,
            ..ContactState::default()
        };
        self.tickets.open += 1;
        self.tickets.open_mailbox = None;
    }

    fn work(&self) -> String {
        self.entry.id.clone()
    }

    /// Select a mailbox and fetch its first page.
    fn select_mailbox(&mut self, id: String, limit: u32, work: &mut Vec<Work>) {
        self.mail.selected = Some(id.clone());
        self.mail.open = None;
        // The old mailbox's rows go at once, so none of them can be opened
        // under the new selection while its page loads.
        self.mail.emails.clear();
        self.mail.total = 0;
        // An open still in flight belongs to the old mailbox.
        self.tickets.open += 1;
        self.mail.busy = true;
        self.fetch_emails(id, limit, work);
    }

    /// Queue a page fetch; only the newest one's answer is applied.
    fn fetch_emails(&mut self, mailbox: String, limit: u32, work: &mut Vec<Work>) {
        self.tickets.page += 1;
        work.push(Work::FetchEmails {
            account: self.work(),
            ticket: self.tickets.page,
            mailbox,
            limit,
        });
    }

    /// Queue an email open; only the newest one's answer is applied.
    fn open_email(&mut self, id: String, work: &mut Vec<Work>) {
        self.tickets.open += 1;
        self.tickets.open_mailbox = self.mail.selected.clone();
        work.push(Work::OpenEmail {
            account: self.work(),
            ticket: self.tickets.open,
            id,
        });
    }

    /// Select a calendar and fetch its events.
    fn select_calendar(&mut self, id: String, work: &mut Vec<Work>) {
        self.calendars.selected = Some(id.clone());
        self.calendars.busy = true;
        self.fetch_events(id, work);
    }

    /// Queue a calendar's events; only the newest one's answer is applied.
    fn fetch_events(&mut self, calendar: String, work: &mut Vec<Work>) {
        self.tickets.events += 1;
        work.push(Work::FetchEvents {
            account: self.work(),
            ticket: self.tickets.events,
            calendar,
        });
    }

    /// Select an address book and fetch its contacts.
    fn select_book(&mut self, id: String, work: &mut Vec<Work>) {
        self.contacts.selected = Some(id.clone());
        self.contacts.busy = true;
        let filter = self.contacts.filter.clone();
        self.fetch_contacts(id, filter, work);
    }

    /// Queue a contact search; only the newest one's answer is applied.
    fn fetch_contacts(&mut self, book: String, filter: String, work: &mut Vec<Work>) {
        self.tickets.contacts += 1;
        work.push(Work::FetchContacts {
            account: self.work(),
            ticket: self.tickets.contacts,
            book,
            filter,
        });
    }

    /// Everything this account's surfaces show, fetched again.
    fn refresh(&mut self, limit: u32, work: &mut Vec<Work>) {
        if self.phase != Phase::Ready {
            return;
        }
        work.push(Work::FetchMailboxes {
            account: self.work(),
        });
        work.push(Work::FetchCalendars {
            account: self.work(),
        });
        work.push(Work::FetchBooks {
            account: self.work(),
        });
        if let Some(mailbox) = self.mail.selected.clone() {
            self.mail.busy = true;
            self.fetch_emails(mailbox, limit, work);
        }
    }

    /// Fetch whatever `mode` shows of this account and has nothing of yet.
    fn fetch_missing(&mut self, mode: Mode, work: &mut Vec<Work>) {
        if self.phase != Phase::Ready {
            return;
        }
        match mode {
            Mode::Mail => {
                if self.mail.mailboxes.is_empty() {
                    work.push(Work::FetchMailboxes {
                        account: self.work(),
                    });
                }
            }
            Mode::Calendar => {
                if self.calendars.calendars.is_empty() {
                    work.push(Work::FetchCalendars {
                        account: self.work(),
                    });
                } else if self.calendars.events.is_empty()
                    && let Some(calendar) = self.calendars.selected.clone()
                {
                    self.calendars.busy = true;
                    self.fetch_events(calendar, work);
                }
            }
            Mode::Contacts => {
                if self.contacts.books.is_empty() {
                    work.push(Work::FetchBooks {
                        account: self.work(),
                    });
                } else if self.contacts.contacts.is_empty()
                    && let Some(book) = self.contacts.selected.clone()
                {
                    let filter = self.contacts.filter.clone();
                    self.contacts.busy = true;
                    self.fetch_contacts(book, filter, work);
                }
            }
        }
    }
}

/// A node of the account tree. Its [`Node::key`] names it in the
/// remembered expand state.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Account(String),
    /// An account's mailbox.
    Mailbox(String, String),
    /// An account's Calendars group.
    Calendars(String),
    Calendar(String, String),
    /// An account's Address Books group.
    Books(String),
    Book(String, String),
}

impl Node {
    /// The account this node belongs to.
    pub fn account(&self) -> &str {
        match self {
            Node::Account(a)
            | Node::Mailbox(a, _)
            | Node::Calendars(a)
            | Node::Calendar(a, _)
            | Node::Books(a)
            | Node::Book(a, _) => a,
        }
    }

    /// The stable key the expand state is remembered under.
    pub fn key(&self) -> String {
        match self {
            Node::Account(a) => a.clone(),
            Node::Mailbox(a, m) => format!("{a}/mailbox/{m}"),
            Node::Calendars(a) => format!("{a}/calendars"),
            Node::Calendar(a, c) => format!("{a}/calendar/{c}"),
            Node::Books(a) => format!("{a}/books"),
            Node::Book(a, b) => format!("{a}/book/{b}"),
        }
    }
}

/// What a tree row stands for, for its icon.
#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    /// An account root, with its phase.
    Account(Phase),
    /// A mailbox, with its role.
    Mailbox(Option<String>),
    CalendarGroup,
    Calendar,
    BookGroup,
    Book,
}

/// One visible row of the account tree, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeRow {
    pub node: Node,
    pub depth: usize,
    pub label: String,
    pub kind: RowKind,
    /// Unread mail: a mailbox's own, an account's total.
    pub unread: u64,
    pub expandable: bool,
    pub expanded: bool,
    /// The row the centre pane is showing.
    pub selected: bool,
}

/// A tree key press (the tree has the keyboard focus).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKey {
    Up,
    Down,
    /// Collapse, or move to the parent.
    Left,
    /// Expand, or move to the first child.
    Right,
    /// Select the cursor's node.
    Enter,
}

/// The modal dialogs.
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

/// The remembered window shape (persisted) plus the open dialog and the
/// tree's keyboard cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct UiState {
    pub places: f32,
    pub reader: f32,
    pub dialog: Option<Dialog>,
    pub theme: ThemeChoice,
    /// Tree nodes by [`Node::key`]; absent is expanded.
    pub expanded: BTreeMap<String, bool>,
    /// The tree row the keyboard is on.
    pub cursor: Option<Node>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            places: 0.18,
            reader: 0.42,
            dialog: None,
            theme: ThemeChoice::default(),
            expanded: BTreeMap::new(),
            cursor: None,
        }
    }
}

/// Where the accounts in force came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The cache of the last settings seen: settingsd not read yet.
    Cache,
    /// settingsd's current settings.
    Settings,
    /// settingsd is not running: the cache stands, read-only.
    Missing,
}

/// What a settingsd apply answered, as the shell read it.
#[derive(Clone, Debug, PartialEq)]
pub enum Settled {
    Accepted,
    /// Another writer changed the profile since it was read.
    Conflict,
    /// settingsd refused the change.
    Refused(String),
    /// Not sent: settingsd is not running, or the Bus refused the call.
    Unavailable(String),
    /// The reply was lost after the call may have arrived.
    Unknown(String),
}

/// A Bus reply the state has decided; the shell sends it.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub id: u64,
    pub rc: u8,
    pub body: Value,
}

/// Refusal codes (`error_code`), rc 10.
pub mod code {
    pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
    /// No account has the id given.
    pub const UNKNOWN_ACCOUNT: &str = "UNKNOWN_ACCOUNT";
    /// The account has no email, mailbox, event or contact with the id given.
    pub const NOT_FOUND: &str = "NOT_FOUND";
    pub const NOT_CONNECTED: &str = "NOT_CONNECTED";
    /// settingsd is absent: the accounts are read-only.
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
    pub const CONFLICT: &str = "CONFLICT";
    pub const REFUSED: &str = "REFUSED";
    pub const OUTCOME_UNKNOWN: &str = "OUTCOME_UNKNOWN";
    pub const SECRETS: &str = "SECRETS";
    pub const SERVER: &str = "SERVER";
    pub const BUSY: &str = "BUSY";
    pub const UNKNOWN_VERB: &str = "UNKNOWN_VERB";
    pub const UNKNOWN_COMMAND: &str = "UNKNOWN_COMMAND";
    pub const DISABLED: &str = "DISABLED";
}

/// A refusal body.
pub fn refusal(code: &str, message: &str) -> Value {
    json!({"error_code": code, "message": message})
}

/// A settings write in flight.
#[derive(Clone, Debug)]
struct PendingWrite {
    change: Change,
    /// The Bus request waiting on it.
    reply: Option<u64>,
    /// The password to hold once settingsd has the account (never printed:
    /// `PendingWrite` is not shown anywhere).
    password: Option<String>,
    /// Sends so far; a conflict retries on the fresh read, a few times.
    attempts: u32,
    /// Waiting for a settings read before it is (re)computed and sent.
    awaiting_look: bool,
    /// The schema-1 migration.
    migration: bool,
    /// What the settings become, and the account the change is about, as
    /// computed when it was sent.
    sent: Option<(EmailSettings, Option<String>)>,
}

/// How many times a write is sent before a conflict is reported.
const WRITE_ATTEMPTS: u32 = 3;

/// Everything the window shows.
pub struct State {
    /// The accounts, in tree order.
    pub accounts: Vec<AccountView>,
    /// The account the centre pane shows.
    pub selected: Option<String>,
    pub mode: Mode,
    /// The `email.*` settings in force.
    pub settings: EmailSettings,
    pub source: Source,
    secrets: Secrets,
    /// The schema-1 account until settingsd holds it.
    migration: Option<AccountEntry>,
    /// The migration was refused this run: not retried until a restart.
    migration_refused: bool,
    /// The migration completed: the shell rewrites the config as schema 2.
    migrated: bool,
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
    replies: Vec<Reply>,
    writes: BTreeMap<u64, PendingWrite>,
    /// Secrets saves in flight, with the reply each one owes.
    saves: BTreeMap<u64, Option<(u64, Value)>>,
    next_ticket: u64,
    /// Whether the persisted config (shape, cache) has changed.
    config_dirty: bool,
}

impl State {
    /// The state a launch starts in: the cached accounts (the last settings
    /// seen) connecting with the passwords held, the persisted shape, mode
    /// and selected account. settingsd's answer reconciles them.
    pub fn new(
        ui: UiState,
        mode: Mode,
        selected: Option<String>,
        cache: EmailSettings,
        secrets: Secrets,
    ) -> Self {
        let mut state = Self {
            accounts: Vec::new(),
            selected,
            mode,
            settings: cache,
            source: Source::Cache,
            secrets,
            migration: None,
            migration_refused: false,
            migrated: false,
            compose: None,
            ui,
            status: String::new(),
            exiting: false,
            session_mode: design::Mode::Dark,
            work: Vec::new(),
            replies: Vec::new(),
            writes: BTreeMap::new(),
            saves: BTreeMap::new(),
            next_ticket: 0,
            config_dirty: false,
        };
        state.reconcile();
        state.config_dirty = false;
        state
    }

    /// The schema-1 account: shown and connected at once (from the cache
    /// side, so it works without settingsd), its password written to the
    /// secrets file, and applied to settingsd once settingsd is read.
    pub fn begin_migration(&mut self, entry: AccountEntry, password: &str) {
        if self.secrets.set(&entry.id, password) {
            self.save_secrets(None);
        }
        self.migration = Some(entry);
        self.reconcile();
        self.config_dirty = false;
    }

    /// Whether the schema-1 account is still to migrate.
    pub fn migrating(&self) -> bool {
        self.migration.is_some()
    }

    /// The migration completed since the last call: the shell rewrites
    /// the config as schema 2.
    pub fn take_migrated(&mut self) -> bool {
        std::mem::take(&mut self.migrated)
    }

    /// The passwords, for the shell to persist.
    pub fn secrets(&self) -> &Secrets {
        &self.secrets
    }

    /// Whether `account` has a password held.
    pub fn has_password(&self, account: &str) -> bool {
        self.secrets.has(account)
    }

    /// What the config's accounts cache should hold: the settings in force
    /// (never the schema-1 account, which the schema-1 file still holds).
    pub fn cache(&self) -> EmailSettings {
        self.settings.clone()
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

    /// The Bus replies decided since the last call, in order.
    pub fn take_replies(&mut self) -> Vec<Reply> {
        std::mem::take(&mut self.replies)
    }

    fn queue(&mut self, work: Work) {
        self.work.push(work);
    }

    fn ticket(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }

    /// Answer Bus request `id`.
    pub fn reply(&mut self, id: u64, rc: u8, body: Value) {
        self.replies.push(Reply { id, rc, body });
    }

    /// Answer Bus request `id` with a refusal.
    pub fn refuse(&mut self, id: u64, code: &str, message: &str) {
        self.reply(id, 10, refusal(code, message));
    }

    /// The account with `id`.
    pub fn account(&self, id: &str) -> Option<&AccountView> {
        self.accounts.iter().find(|v| v.entry.id == id)
    }

    fn account_mut(&mut self, id: &str) -> Option<&mut AccountView> {
        self.accounts.iter_mut().find(|v| v.entry.id == id)
    }

    /// The account the centre pane shows.
    pub fn current(&self) -> Option<&AccountView> {
        self.selected.as_deref().and_then(|id| self.account(id))
    }

    /// The accounts in force: the settings', plus the schema-1 account
    /// while it migrates (unless settingsd already has it).
    fn effective(&self) -> Vec<AccountEntry> {
        let mut list = self.settings.accounts.clone();
        if let Some(legacy) = &self.migration
            && !list.iter().any(|a| {
                a.id == legacy.id
                    || (a.url == legacy.url && a.email.eq_ignore_ascii_case(&legacy.email))
            })
        {
            list.push(legacy.clone());
        }
        list
    }

    /// Make the views match the accounts in force: a new enabled account
    /// with a password connects, a removed one drops its client and view, a
    /// changed URL, address or CA path reconnects, a new name only
    /// relabels, and a disabled one disconnects.
    fn reconcile(&mut self) {
        let wanted = self.effective();
        let mut old = std::mem::take(&mut self.accounts);
        let mut next = Vec::with_capacity(wanted.len());
        for entry in wanted {
            let view = match old.iter().position(|v| v.entry.id == entry.id) {
                Some(index) => {
                    let mut view = old.remove(index);
                    let relogin = view.entry.url != entry.url
                        || view.entry.email != entry.email
                        || view.entry.ca_path != entry.ca_path;
                    let was_enabled = view.entry.enabled;
                    view.entry = entry;
                    if !view.entry.enabled {
                        if view.phase != Phase::Disabled {
                            self.disconnect(&mut view);
                            view.phase = Phase::Disabled;
                        }
                    } else if relogin || !was_enabled {
                        self.connect(&mut view);
                    }
                    view
                }
                None => {
                    let mut view = AccountView::new(entry);
                    if view.entry.enabled {
                        self.connect(&mut view);
                    }
                    view
                }
            };
            next.push(view);
        }
        // What is left was removed: its client goes, and (seen live from
        // settingsd) its password with it.
        let mut forgot = false;
        for mut gone in old {
            self.disconnect(&mut gone);
            if self.compose.as_ref().is_some_and(|c| c.account == gone.entry.id) {
                self.compose = None;
            }
            if self.source == Source::Settings {
                forgot |= self.secrets.set(&gone.entry.id, "");
            }
        }
        if forgot {
            self.save_secrets(None);
        }
        self.accounts = next;
        if self.current().is_none() {
            self.selected = self.accounts.first().map(|v| v.entry.id.clone());
        }
        if let Some(cursor) = &self.ui.cursor
            && self.account(cursor.account()).is_none()
        {
            self.ui.cursor = None;
        }
        self.config_dirty = true;
    }

    /// Connect `view` afresh with the password held, or wait for one.
    fn connect(&mut self, view: &mut AccountView) {
        self.disconnect(view);
        let Some(password) = self.secrets.password(&view.entry.id) else {
            view.phase = Phase::NoPassword;
            return;
        };
        view.generation += 1;
        view.phase = Phase::Connecting;
        self.work.push(Work::Connect {
            account: view.entry.id.clone(),
            generation: view.generation,
            login: Login {
                url: view.entry.url.clone(),
                email: view.entry.email.clone(),
                password: password.to_owned(),
                ca_path: view.entry.ca_path.clone(),
            },
        });
    }

    /// Drop `view`'s connection and everything it fetched.
    fn disconnect(&mut self, view: &mut AccountView) {
        if view.phase != Phase::Disabled && view.phase != Phase::NoPassword {
            self.work.push(Work::Disconnect {
                account: view.entry.id.clone(),
            });
        }
        // Answers of the old connection are void from here.
        view.generation += 1;
        view.forget();
    }

    /// Connect account `id` again (Retry, `email.account.connect`).
    pub fn reconnect(&mut self, id: &str) -> bool {
        let Some(index) = self.accounts.iter().position(|v| v.entry.id == id) else {
            return false;
        };
        let mut view = self.accounts.remove(index);
        if view.entry.enabled {
            self.connect(&mut view);
            self.status = crate::label("connecting");
        }
        self.accounts.insert(index, view);
        true
    }

    /// settingsd's `email.*` settings, new to the follower: they are in
    /// force from now on. A migration still to do is sent; writes that
    /// waited for this read are computed against it and sent again.
    pub fn set_settings(&mut self, email: EmailSettings) {
        let page_size = self.settings.mail.page_size;
        self.settings = email;
        self.source = Source::Settings;
        if self.migration.is_some()
            && !self.migration_refused
            && !self.writes.values().any(|w| w.migration)
            && let Some(entry) = self.migration.clone()
        {
            self.write(Change::Migrate(entry), None, None, true);
        }
        self.reconcile();
        // A new page size applies to the lists shown.
        if self.settings.mail.page_size != page_size {
            let limit = self.settings.mail.page_size;
            for view in &mut self.accounts {
                if view.phase == Phase::Ready
                    && let Some(mailbox) = view.mail.selected.clone()
                {
                    view.fetch_emails(mailbox, limit, &mut self.work);
                }
            }
        }
        let waiting: Vec<u64> = self
            .writes
            .iter()
            .filter(|(_, w)| w.awaiting_look)
            .map(|(ticket, _)| *ticket)
            .collect();
        for ticket in waiting {
            if let Some(pending) = self.writes.remove(&ticket) {
                self.send_write(pending);
            }
        }
    }

    /// settingsd is not running: the cached accounts stand, read-only.
    pub fn settings_missing(&mut self) {
        self.source = Source::Missing;
        self.status = crate::label("settings-missing");
        // Writes that waited for a read cannot be sent.
        let waiting: Vec<u64> = self
            .writes
            .iter()
            .filter(|(_, w)| w.awaiting_look)
            .map(|(ticket, _)| *ticket)
            .collect();
        for ticket in waiting {
            if let Some(pending) = self.writes.remove(&ticket) {
                self.write_failed(pending, code::UNAVAILABLE, &crate::label("settings-missing"));
            }
        }
    }

    /// Write `change` to settingsd; `reply` is answered once it is settled.
    /// `password` is held for the account once settingsd has it.
    pub fn change_accounts(&mut self, change: Change, reply: Option<u64>, password: Option<String>) {
        self.write(change, reply, password, false);
    }

    fn write(&mut self, change: Change, reply: Option<u64>, password: Option<String>, migration: bool) {
        let pending = PendingWrite {
            change,
            reply,
            password,
            attempts: 0,
            awaiting_look: false,
            migration,
            sent: None,
        };
        match self.source {
            Source::Missing => {
                self.write_failed(pending, code::UNAVAILABLE, &crate::label("settings-missing"));
            }
            // Not read yet: computed and sent once the first read answers.
            Source::Cache => {
                let ticket = self.ticket();
                self.writes.insert(
                    ticket,
                    PendingWrite {
                        awaiting_look: true,
                        ..pending
                    },
                );
            }
            Source::Settings => self.send_write(pending),
        }
    }

    /// Compute `pending` against the settings in force and send it, or
    /// settle it at once when it changes nothing or cannot be written.
    fn send_write(&mut self, mut pending: PendingWrite) {
        let (next, about) = match writer::apply(&self.settings, &pending.change) {
            Ok(done) => done,
            Err(Refusal::UnknownAccount(id)) => {
                let message = crate::label_with("unknown-account", &[("id", &id)]);
                return self.write_failed(pending, code::UNKNOWN_ACCOUNT, &message);
            }
            Err(refusal @ Refusal::Invalid { .. }) => {
                return self.write_failed(pending, code::INVALID_ARGUMENT, &refusal.to_string());
            }
        };
        let changes = writer::changes(&self.settings, &next);
        if changes.as_object().is_some_and(|c| c.is_empty()) {
            return self.write_done(pending, about, None);
        }
        pending.attempts += 1;
        pending.awaiting_look = false;
        pending.sent = Some((next, about));
        let ticket = self.ticket();
        self.writes.insert(ticket, pending);
        self.queue(Work::Settings { ticket, changes });
    }

    /// settingsd answered write `ticket`.
    pub fn settled(&mut self, ticket: u64, answer: Settled) {
        let Some(pending) = self.writes.remove(&ticket) else {
            return;
        };
        match answer {
            Settled::Accepted => {
                let (next, about) = pending.sent.clone().unwrap_or_else(|| {
                    (self.settings.clone(), pending.change.account().map(str::to_owned))
                });
                // In force at once; the read this asks for confirms it (or
                // brings what another writer did since).
                self.queue(Work::ReadSettings);
                self.write_done(pending, about, Some(next));
            }
            Settled::Conflict if pending.attempts < WRITE_ATTEMPTS => {
                self.writes.insert(
                    ticket,
                    PendingWrite {
                        awaiting_look: true,
                        ..pending
                    },
                );
                self.queue(Work::ReadSettings);
            }
            Settled::Conflict => {
                self.write_failed(pending, code::CONFLICT, &crate::label("settings-conflict"));
            }
            Settled::Refused(message) => {
                if pending.migration {
                    self.migration_refused = true;
                }
                self.write_failed(pending, code::REFUSED, &message);
            }
            Settled::Unavailable(message) => self.write_failed(pending, code::UNAVAILABLE, &message),
            Settled::Unknown(message) => {
                self.queue(Work::ReadSettings);
                self.write_failed(pending, code::OUTCOME_UNKNOWN, &message);
            }
        }
    }

    /// A write settingsd took (or that changed nothing): its settings are in
    /// force, the password is held, a migration completes, and the caller
    /// hears the account id.
    fn write_done(
        &mut self,
        pending: PendingWrite,
        about: Option<String>,
        next: Option<EmailSettings>,
    ) {
        let body = json!({ "id": about });
        if let Some(next) = next {
            self.settings = next;
        }
        if pending.migration {
            // The account settingsd had already may carry another id: the
            // password moves to it unless it has one of its own.
            if let (Some(legacy), Some(id)) = (self.migration.take(), about.as_deref()) {
                if legacy.id != id
                    && !self.secrets.has(id)
                    && let Some(password) = self.secrets.password(&legacy.id).map(str::to_owned)
                {
                    self.secrets.set(id, &password);
                    self.secrets.set(&legacy.id, "");
                    self.save_secrets(None);
                }
            }
            self.migrated = true;
            self.status = crate::label("migrated");
        }
        // A new password: an account already shown reconnects with it; a
        // new one connects with it in the reconcile.
        let mut renewed = None;
        if let (Some(id), Some(password)) = (&about, &pending.password)
            && self.secrets.password(id) != Some(password.as_str())
        {
            self.secrets.set(id, password);
            renewed = Some((id.clone(), self.account(id).is_some()));
        }
        self.reconcile();
        match renewed {
            Some((id, shown)) => {
                if shown {
                    self.reconnect(&id);
                }
                self.save_secrets(pending.reply.map(|reply| (reply, body)));
            }
            None => {
                if let Some(reply) = pending.reply {
                    self.reply(reply, 0, body);
                }
            }
        }
    }

    fn write_failed(&mut self, pending: PendingWrite, code: &str, message: &str) {
        self.status = crate::label_with("settings-failed", &[("message", message)]);
        if let Some(reply) = pending.reply {
            self.refuse(reply, code, message);
        }
    }

    /// Hold `password` for account `id` and reconnect it, answering `reply`
    /// once the secrets file has it.
    pub fn set_password(&mut self, id: &str, password: &str, reply: Option<u64>) -> bool {
        if self.account(id).is_none() {
            return false;
        }
        self.secrets.set(id, password);
        self.save_secrets(reply.map(|reply| (reply, json!({ "id": id, "has_password": !password.is_empty() }))));
        self.reconnect(id);
        true
    }

    /// Queue a secrets save that answers `reply` when it lands.
    fn save_secrets(&mut self, reply: Option<(u64, Value)>) {
        let ticket = self.ticket();
        self.saves.insert(ticket, reply);
        self.queue(Work::SaveSecrets { ticket });
    }

    /// Secrets save `ticket` finished. The error never carries a password.
    pub fn secrets_saved(&mut self, ticket: u64, result: Result<bool, String>) {
        let Some(owed) = self.saves.remove(&ticket) else {
            return;
        };
        let refused = match result {
            Ok(true) => None,
            Ok(false) => Some(crate::label("secrets-refused")),
            Err(error) => Some(error),
        };
        if let Some(message) = &refused {
            self.status = crate::label_with("secrets-failed", &[("message", message)]);
        }
        if let Some((reply, body)) = owed {
            match refused {
                None => self.reply(reply, 0, body),
                Some(message) => self.refuse(reply, code::SECRETS, &message),
            }
        }
    }

    /// Apply one shell completion.
    pub fn apply(&mut self, event: crate::shell::Event) {
        use crate::shell::{Done, Event};
        match event {
            Event::Connected(tag, result) => self.connected(tag, result),
            Event::Account(tag, done) => {
                let limit = self.settings.mail.page_size;
                let Some(view) = self
                    .accounts
                    .iter_mut()
                    .find(|v| v.entry.id == tag.account && v.generation == tag.generation)
                else {
                    return;
                };
                match done {
                    Done::Mailboxes(Ok(mailboxes)) => {
                        view.mail.mailboxes = mailboxes;
                        let selected = view
                            .mail
                            .selected
                            .clone()
                            .filter(|id| view.mail.mailboxes.iter().any(|m| m.id == *id))
                            .or_else(|| {
                                view.mail
                                    .by_role("inbox")
                                    .map(|m| m.id.clone())
                                    .or_else(|| view.mail.mailboxes.first().map(|m| m.id.clone()))
                            });
                        // A refresh of the same mailbox (new counts after a
                        // mark-read) keeps the page and the open email; only
                        // a change reselects.
                        if let Some(id) = selected
                            && view.mail.selected.as_deref() != Some(id.as_str())
                        {
                            view.select_mailbox(id, limit, &mut self.work);
                        }
                    }
                    Done::Page(ticket, _) if ticket != view.tickets.page => {}
                    Done::Page(_, Ok(page)) => {
                        view.mail.busy = false;
                        view.mail.emails = page.emails;
                        view.mail.total = page.total;
                    }
                    Done::Page(_, Err(error)) => {
                        view.mail.busy = false;
                        self.status = error.to_string();
                    }
                    Done::Opened(ticket, _)
                        if ticket != view.tickets.open
                            || view.tickets.open_mailbox != view.mail.selected => {}
                    Done::Opened(_, result) => match *result {
                        Ok(email) => view.mail.open = Some(email),
                        Err(ref error) => self.status = error.to_string(),
                    },
                    Done::Marked(Ok(())) => self.work.push(Work::FetchMailboxes {
                        account: tag.account.clone(),
                    }),
                    Done::Sent(ticket, result) => {
                        // Only the compose that sent closes or shows the
                        // error; a send whose compose was cancelled leaves a
                        // newer one alone.
                        let mine = self.compose.as_ref().is_some_and(|compose| {
                            compose.account == tag.account && compose.sending == Some(ticket)
                        });
                        match result {
                            Ok(()) => {
                                if mine {
                                    self.compose = None;
                                }
                                self.status = crate::label("sent");
                                // The Sent mailbox and its counts just changed.
                                self.work.push(Work::FetchMailboxes {
                                    account: tag.account.clone(),
                                });
                                if let Some(mailbox) = view.mail.selected_mailbox() {
                                    let id = mailbox.id.clone();
                                    view.fetch_emails(id, limit, &mut self.work);
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
                    Done::Calendars(Ok(calendars)) => {
                        view.calendars.calendars = calendars;
                        let selected = view
                            .calendars
                            .selected
                            .clone()
                            .filter(|id| view.calendars.calendars.iter().any(|c| c.id == *id))
                            .or_else(|| view.calendars.calendars.first().map(|c| c.id.clone()));
                        if let Some(id) = selected {
                            view.select_calendar(id, &mut self.work);
                        }
                    }
                    Done::Events(ticket, _) if ticket != view.tickets.events => {}
                    Done::Events(_, Ok(events)) => {
                        view.calendars.busy = false;
                        view.calendars.events = events;
                        view.calendars.open = None;
                    }
                    Done::Events(_, Err(error)) => {
                        view.calendars.busy = false;
                        self.status = error.to_string();
                    }
                    Done::Books(Ok(books)) => {
                        view.contacts.books = books;
                        let selected = view
                            .contacts
                            .selected
                            .clone()
                            .filter(|id| view.contacts.books.iter().any(|b| b.id == *id))
                            .or_else(|| view.contacts.books.first().map(|b| b.id.clone()));
                        if let Some(id) = selected {
                            view.select_book(id, &mut self.work);
                        }
                    }
                    Done::Contacts(ticket, _) if ticket != view.tickets.contacts => {}
                    Done::Contacts(_, Ok(contacts)) => {
                        view.contacts.busy = false;
                        view.contacts.contacts = contacts;
                        view.contacts.open = None;
                    }
                    Done::Contacts(_, Err(error)) => {
                        view.contacts.busy = false;
                        self.status = error.to_string();
                    }
                    Done::Mailboxes(Err(error))
                    | Done::Marked(Err(error))
                    | Done::Calendars(Err(error))
                    | Done::Books(Err(error)) => self.status = error.to_string(),
                }
            }
            Event::Settled(ticket, answer) => self.settled(ticket, answer),
            Event::SecretsSaved(ticket, result) => self.secrets_saved(ticket, result),
            Event::Delivery(..) | Event::Look(..) | Event::LookRetry(_) => {}
        }
    }

    /// A connect finished. One for an older connection is void.
    fn connected(&mut self, tag: Tag, result: Result<SessionInfo, jmap::Error>) {
        let Some(view) = self
            .accounts
            .iter_mut()
            .find(|v| v.entry.id == tag.account && v.generation == tag.generation)
        else {
            return;
        };
        match result {
            Ok(info) => {
                view.phase = Phase::Ready;
                view.session = Some(info);
                self.status = crate::label("connected");
                for work in [
                    Work::FetchMailboxes {
                        account: tag.account.clone(),
                    },
                    Work::FetchCalendars {
                        account: tag.account.clone(),
                    },
                    Work::FetchBooks {
                        account: tag.account.clone(),
                    },
                ] {
                    self.work.push(work);
                }
            }
            Err(error) => {
                let error = error.to_string();
                view.phase = Phase::Failed(error.clone());
                self.status = error;
            }
        }
    }

    /// Select an account (and keep the mode).
    pub fn select_account(&mut self, id: &str) -> bool {
        if self.account(id).is_none() {
            return false;
        }
        if self.selected.as_deref() != Some(id) {
            self.selected = Some(id.to_owned());
            self.config_dirty = true;
        }
        let mode = self.mode;
        if let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == id) {
            view.fetch_missing(mode, &mut self.work);
        }
        true
    }

    /// Select the next (or previous) account in tree order.
    pub fn step_account(&mut self, forward: bool) {
        let count = self.accounts.len();
        if count == 0 {
            return;
        }
        let at = self
            .selected
            .as_deref()
            .and_then(|id| self.accounts.iter().position(|v| v.entry.id == id))
            .unwrap_or(0);
        let next = if forward {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        };
        let id = self.accounts[next].entry.id.clone();
        self.select_account(&id);
    }

    /// Select a mailbox of `account` and fetch its first page.
    pub fn select_mailbox(&mut self, account: &str, id: String) -> bool {
        let limit = self.settings.mail.page_size;
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return false;
        };
        if !view.mail.mailboxes.iter().any(|m| m.id == id) {
            return false;
        }
        view.select_mailbox(id, limit, &mut self.work);
        self.select_account(account);
        self.set_mode_only(Mode::Mail);
        true
    }

    /// Select a calendar of `account` and fetch its events.
    pub fn select_calendar(&mut self, account: &str, id: String) -> bool {
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return false;
        };
        if !view.calendars.calendars.iter().any(|c| c.id == id) {
            return false;
        }
        view.select_calendar(id, &mut self.work);
        self.select_account(account);
        self.set_mode_only(Mode::Calendar);
        true
    }

    /// Select an address book of `account` and fetch its contacts.
    pub fn select_book(&mut self, account: &str, id: String) -> bool {
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return false;
        };
        if !view.contacts.books.iter().any(|b| b.id == id) {
            return false;
        }
        view.select_book(id, &mut self.work);
        self.select_account(account);
        self.set_mode_only(Mode::Contacts);
        true
    }

    fn set_mode_only(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.config_dirty = true;
        }
    }

    /// Open an email of the current account's shown page.
    pub fn open_email(&mut self, account: &str, id: String) -> bool {
        let mark = self.settings.mail.mark_read_on_open;
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return false;
        };
        // Only a row of the shown page opens: a click that raced a mailbox
        // change names a row that is gone.
        let Some(email) = view.mail.emails.iter_mut().find(|email| email.id == id) else {
            return false;
        };
        // Optimistically mark the row read; the server's word replaces the
        // counts when it answers.
        if mark && !email.is_read() {
            email
                .keywords
                .get_or_insert_with(|| json!({}))
                .as_object_mut()
                .map(|k| k.insert("$seen".into(), json!(true)));
            self.work.push(Work::MarkRead {
                account: account.to_owned(),
                id: id.clone(),
                read: true,
            });
        }
        view.open_email(id, &mut self.work);
        true
    }

    /// The node the centre pane shows, for the tree's highlight.
    pub fn selected_node(&self) -> Option<Node> {
        let view = self.current()?;
        let account = view.entry.id.clone();
        Some(match self.mode {
            Mode::Mail => match &view.mail.selected {
                Some(id) => Node::Mailbox(account, id.clone()),
                None => Node::Account(account),
            },
            Mode::Calendar => match &view.calendars.selected {
                Some(id) => Node::Calendar(account, id.clone()),
                None => Node::Calendars(account),
            },
            Mode::Contacts => match &view.contacts.selected {
                Some(id) => Node::Book(account, id.clone()),
                None => Node::Books(account),
            },
        })
    }

    /// Whether `node` is expanded (absent is).
    pub fn expanded(&self, node: &Node) -> bool {
        self.ui.expanded.get(&node.key()).copied().unwrap_or(true)
    }

    /// Expand or collapse `node`; only a collapse is remembered as such.
    pub fn set_expanded(&mut self, node: &Node, open: bool) {
        if self.expanded(node) == open {
            return;
        }
        if open {
            self.ui.expanded.remove(&node.key());
        } else {
            self.ui.expanded.insert(node.key(), false);
        }
        self.config_dirty = true;
    }

    /// The tree's visible rows, top to bottom: each account, then under it
    /// its mailboxes nested by parent, a Calendars group and an Address
    /// Books group (once connected).
    pub fn tree(&self) -> Vec<TreeRow> {
        let selected = self.selected_node();
        let mut rows = Vec::new();
        for view in &self.accounts {
            let id = view.entry.id.clone();
            let root = Node::Account(id.clone());
            let open = self.expanded(&root);
            rows.push(TreeRow {
                selected: selected.as_ref() == Some(&root),
                expanded: open,
                node: root,
                depth: 0,
                label: view.entry.name.clone(),
                kind: RowKind::Account(view.phase.clone()),
                unread: view.mail.unread(),
                expandable: true,
            });
            if !open || view.phase != Phase::Ready {
                continue;
            }
            self.mailbox_rows(view, None, 1, selected.as_ref(), &mut rows);
            let groups = [
                (
                    Node::Calendars(id.clone()),
                    "calendars",
                    RowKind::CalendarGroup,
                    view.calendars
                        .calendars
                        .iter()
                        .map(|c| (Node::Calendar(id.clone(), c.id.clone()), c.name.clone(), RowKind::Calendar))
                        .collect::<Vec<_>>(),
                ),
                (
                    Node::Books(id.clone()),
                    "address-books",
                    RowKind::BookGroup,
                    view.contacts
                        .books
                        .iter()
                        .map(|b| (Node::Book(id.clone(), b.id.clone()), b.name.clone(), RowKind::Book))
                        .collect(),
                ),
            ];
            for (group, label, kind, children) in groups {
                if children.is_empty() {
                    continue;
                }
                let open = self.expanded(&group);
                rows.push(TreeRow {
                    selected: selected.as_ref() == Some(&group),
                    expanded: open,
                    expandable: true,
                    node: group,
                    depth: 1,
                    label: crate::label(label),
                    kind,
                    unread: 0,
                });
                if open {
                    for (node, label, kind) in children {
                        rows.push(TreeRow {
                            selected: selected.as_ref() == Some(&node),
                            node,
                            depth: 2,
                            label,
                            kind,
                            unread: 0,
                            expandable: false,
                            expanded: false,
                        });
                    }
                }
            }
        }
        rows
    }

    /// The mailboxes under `parent` (top level: no parent, or a parent the
    /// listing does not hold), in role then sort order, each followed by
    /// its own children when expanded.
    fn mailbox_rows(
        &self,
        view: &AccountView,
        parent: Option<&str>,
        depth: usize,
        selected: Option<&Node>,
        rows: &mut Vec<TreeRow>,
    ) {
        let known = |id: &str| view.mail.mailboxes.iter().any(|m| m.id == id);
        let mut level: Vec<&Mailbox> = view
            .mail
            .mailboxes
            .iter()
            .filter(|m| match parent {
                None => m.parent_id.as_deref().is_none_or(|p| !known(p) || p == m.id),
                Some(parent) => m.parent_id.as_deref() == Some(parent) && m.id != parent,
            })
            .collect();
        level.sort_by(|a, b| {
            (role_rank(a.role.as_deref()), a.sort_order, &a.name).cmp(&(
                role_rank(b.role.as_deref()),
                b.sort_order,
                &b.name,
            ))
        });
        // A cycle in a hostile listing cannot recurse forever.
        if depth > 16 {
            return;
        }
        for mailbox in level {
            let node = Node::Mailbox(view.entry.id.clone(), mailbox.id.clone());
            let children = view
                .mail
                .mailboxes
                .iter()
                .any(|m| m.parent_id.as_deref() == Some(mailbox.id.as_str()) && m.id != mailbox.id);
            let open = children && self.expanded(&node);
            rows.push(TreeRow {
                selected: selected == Some(&node),
                node,
                depth,
                label: mailbox.name.clone(),
                kind: RowKind::Mailbox(mailbox.role.clone()),
                unread: mailbox.unread_emails,
                expandable: children,
                expanded: open,
            });
            if open {
                self.mailbox_rows(view, Some(&mailbox.id), depth + 1, selected, rows);
            }
        }
    }

    /// Select what a tree node stands for: its account, and the mode and
    /// folder, calendar or book it names.
    pub fn select_node(&mut self, node: &Node) -> bool {
        self.ui.cursor = Some(node.clone());
        match node {
            Node::Account(a) => self.select_account(a),
            Node::Mailbox(a, m) => self.select_mailbox(a, m.clone()),
            Node::Calendars(a) => {
                let ok = self.select_account(a);
                if ok {
                    self.set_mode(Mode::Calendar);
                }
                ok
            }
            Node::Calendar(a, c) => self.select_calendar(a, c.clone()),
            Node::Books(a) => {
                let ok = self.select_account(a);
                if ok {
                    self.set_mode(Mode::Contacts);
                }
                ok
            }
            Node::Book(a, b) => self.select_book(a, b.clone()),
        }
    }

    /// A key press with the tree focused.
    pub fn tree_key(&mut self, key: TreeKey) {
        let rows = self.tree();
        if rows.is_empty() {
            return;
        }
        let at = self
            .ui
            .cursor
            .as_ref()
            .and_then(|c| rows.iter().position(|r| &r.node == c))
            .or_else(|| rows.iter().position(|r| r.selected))
            .unwrap_or(0);
        let row = &rows[at];
        match key {
            TreeKey::Up => self.ui.cursor = Some(rows[at.saturating_sub(1)].node.clone()),
            TreeKey::Down => self.ui.cursor = Some(rows[(at + 1).min(rows.len() - 1)].node.clone()),
            TreeKey::Left if row.expandable && row.expanded => {
                let node = row.node.clone();
                self.set_expanded(&node, false);
                self.ui.cursor = Some(node);
            }
            TreeKey::Left => {
                let parent = rows[..at].iter().rev().find(|r| r.depth < row.depth);
                self.ui.cursor = Some(parent.unwrap_or(row).node.clone());
            }
            TreeKey::Right if row.expandable && !row.expanded => {
                let node = row.node.clone();
                self.set_expanded(&node, true);
                self.ui.cursor = Some(node);
            }
            TreeKey::Right => {
                let child = rows.get(at + 1).filter(|r| r.depth > row.depth);
                self.ui.cursor = Some(child.unwrap_or(row).node.clone());
            }
            TreeKey::Enter => {
                let node = row.node.clone();
                self.select_node(&node);
            }
        }
    }

    /// The account a new mail is from: the settings' default when it is
    /// held and enabled, else the first enabled one, else the current one.
    pub fn compose_account(&self) -> Option<String> {
        self.settings
            .from_account()
            .map(|a| a.id.clone())
            .filter(|id| self.account(id).is_some())
            .or_else(|| {
                self.accounts
                    .iter()
                    .find(|v| v.entry.enabled)
                    .map(|v| v.entry.id.clone())
            })
            .or_else(|| self.selected.clone())
    }

    /// A new draft's body: empty, with the account's signature after the
    /// standard `-- ` separator when it has one.
    fn signed_body(&self, account: &str) -> String {
        match self.account(account).map(|v| v.entry.signature.trim_end()) {
            Some(signature) if !signature.is_empty() => format!("\n\n-- \n{signature}\n"),
            _ => String::new(),
        }
    }

    /// Open a compose dialog from `account` (the default From when `None`)
    /// with `draft`'s fields; the account's address and signature fill in.
    pub fn open_compose(&mut self, account: Option<&str>, mut draft: Composed) -> bool {
        let Some(account) = account.map(str::to_owned).or_else(|| self.compose_account()) else {
            return false;
        };
        let Some(view) = self.account(&account) else {
            return false;
        };
        draft.from = view.entry.email.clone();
        if draft.body.is_empty() {
            draft.body = self.signed_body(&account);
        }
        self.compose = Some(ComposeState::new(&account, draft));
        true
    }

    /// A reply to `account`'s open email (`all`: to everyone it went to).
    pub fn open_reply(&mut self, account: &str, all: bool) -> bool {
        let Some(open) = self.account(account).and_then(|v| v.mail.open.clone()) else {
            return false;
        };
        let me = self
            .account(account)
            .map(|v| v.entry.email.to_ascii_lowercase())
            .unwrap_or_default();
        // The sender's name travels with the address.
        let mut to: Vec<EmailAddress> = open
            .from
            .as_ref()
            .and_then(|from| from.first())
            .cloned()
            .into_iter()
            .collect();
        let mut cc = Vec::new();
        if all {
            let others = |list: &Option<Vec<EmailAddress>>| {
                list.iter()
                    .flatten()
                    .filter(|a| a.email.to_ascii_lowercase() != me)
                    .cloned()
                    .collect::<Vec<_>>()
            };
            to.extend(others(&open.to));
            to.dedup_by(|a, b| a.email.eq_ignore_ascii_case(&b.email));
            cc = others(&open.cc);
            cc.retain(|c| !to.iter().any(|t| t.email.eq_ignore_ascii_case(&c.email)));
        }
        let subject = match open.subject.as_deref() {
            Some(subject) if subject.to_lowercase().starts_with("re:") => subject.to_owned(),
            Some(subject) => format!("Re: {subject}"),
            None => crate::label("reply-subject"),
        };
        self.open_compose(
            Some(account),
            Composed {
                to,
                cc,
                subject,
                in_reply_to: open.message_id.clone().and_then(|ids| ids.into_iter().next()),
                ..Composed::default()
            },
        )
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
            UiEvent::Mode(mode) => self.set_mode(mode),
            UiEvent::Select(node) => {
                self.select_node(&node);
            }
            UiEvent::Expand(node, open) => {
                self.ui.cursor = Some(node.clone());
                self.set_expanded(&node, open);
            }
            UiEvent::TreeKey(key) => self.tree_key(key),
            UiEvent::OpenEmail(account, id) => {
                self.open_email(&account, id);
            }
            UiEvent::SelectEvent(account, index) => {
                if let Some(view) = self.account_mut(&account) {
                    view.calendars.open = Some(index);
                }
            }
            UiEvent::SelectContact(account, index) => {
                if let Some(view) = self.account_mut(&account) {
                    view.contacts.open = Some(index);
                }
            }
            UiEvent::ContactFilter(account, filter) => {
                let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
                    return;
                };
                view.contacts.filter = filter;
                if let Some(book) = view.contacts.selected.clone() {
                    view.contacts.busy = true;
                    let filter = view.contacts.filter.clone();
                    view.fetch_contacts(book, filter, &mut self.work);
                }
            }
            UiEvent::Compose => {
                self.open_compose(None, Composed::default());
            }
            UiEvent::Reply => {
                if let Some(account) = self.selected.clone() {
                    self.open_reply(&account, false);
                }
            }
            UiEvent::ComposeAccount(account) => {
                let signed = |state: &State, id: &str| state.signed_body(id);
                let Some(compose) = &self.compose else {
                    return;
                };
                if compose.busy || compose.account == account {
                    return;
                }
                let Some(email) = self.account(&account).map(|v| v.entry.email.clone()) else {
                    return;
                };
                // An untouched signature follows the account.
                let old = signed(self, &compose.account);
                let new = signed(self, &account);
                if let Some(compose) = &mut self.compose {
                    if compose.draft.body == old {
                        compose.draft.body = new;
                    }
                    compose.account = account;
                    compose.draft.from = email;
                }
            }
            UiEvent::ComposeField(field, value) => {
                // While a send is in flight the dialog shows what was sent:
                // edits wait for its answer (the view disables the fields).
                if let Some(compose) = &mut self.compose
                    && !compose.busy
                {
                    // Recipients stay as typed; they are parsed at send.
                    match field {
                        ComposeField::To => compose.to_text = value,
                        ComposeField::Cc => compose.cc_text = value,
                        ComposeField::Bcc => compose.bcc_text = value,
                        ComposeField::Subject => compose.draft.subject = value,
                        ComposeField::Body => compose.draft.body = value,
                    }
                }
            }
            UiEvent::CloseCompose => self.compose = None,
            UiEvent::SendCompose => self.send_compose(),
            UiEvent::Retry(account) => {
                self.reconnect(&account);
            }
            UiEvent::OpenPrefs => self.open_prefs(),
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

    /// Send the open compose, when its recipients parse.
    fn send_compose(&mut self) {
        let Some(compose) = &self.compose else {
            return;
        };
        if compose.busy {
            return;
        }
        let account = compose.account.clone();
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return;
        };
        let ticket = view.tickets.send + 1;
        let Some(compose) = &mut self.compose else {
            return;
        };
        match parse_recipients(compose) {
            Err(error) => {
                compose.error = Some(crate::label_with("bad-recipient", &[("error", &error)]));
            }
            Ok((to, cc, bcc)) => {
                compose.draft.to = to;
                compose.draft.cc = cc;
                compose.draft.bcc = bcc;
                compose.busy = true;
                compose.error = None;
                compose.sending = Some(ticket);
                let draft = compose.draft.clone();
                view.tickets.send = ticket;
                self.work.push(Work::Send {
                    account,
                    ticket,
                    draft,
                });
            }
        }
    }

    /// Launch Prefs on its Email section (where accounts are configured).
    pub fn open_prefs(&mut self) {
        self.queue(Work::OpenPrefs);
    }

    /// Whether a compose is sendable right now (the dialog's Send button).
    pub fn compose_sendable(&self) -> bool {
        match &self.compose {
            Some(compose) => {
                !compose.busy
                    && self.account(&compose.account).is_some()
                    && !compose.to_text.trim().is_empty()
                    && !compose.draft.subject.trim().is_empty()
            }
            None => false,
        }
    }

    /// `account`'s Sent mailbox id, for the send recipe.
    pub fn sent_mailbox_id(&self, account: &str) -> Option<String> {
        self.account(account)?.mail.by_role("sent").map(|m| m.id.clone())
    }

    // The command registry's entry points (commands.rs).

    /// Refetch the current account's surfaces.
    pub fn refresh(&mut self) {
        if let Some(id) = self.selected.clone() {
            self.refresh_account(&id);
        }
    }

    /// Refetch `id`'s surfaces, or connect it again when it is not
    /// connected.
    pub fn refresh_account(&mut self, id: &str) -> bool {
        let limit = self.settings.mail.page_size;
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == id) else {
            return false;
        };
        match view.phase {
            Phase::Ready => view.refresh(limit, &mut self.work),
            Phase::Failed(_) => {
                self.reconnect(id);
            }
            _ => {}
        }
        true
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
    pub fn reply_open(&mut self) {
        self.apply_ui(crate::view::UiEvent::Reply);
    }

    /// Switch surfaces, fetching what the current account lacks for it.
    pub fn set_mode(&mut self, mode: Mode) {
        self.set_mode_only(mode);
        if let Some(id) = self.selected.clone()
            && let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == id)
        {
            view.fetch_missing(mode, &mut self.work);
        }
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

/// A mailbox role's place among its siblings: the inbox first, the bins
/// last, folders without a role between.
fn role_rank(role: Option<&str>) -> u8 {
    match role {
        Some("inbox") => 0,
        Some("drafts") => 1,
        Some("sent") => 2,
        Some("archive") | Some("all") => 3,
        None => 4,
        Some("junk") | Some("spam") => 6,
        Some("trash") => 7,
        Some(_) => 5,
    }
}

/// The three recipient lists parsed from what was typed, or the reason
/// one is refused. To must name someone.
type Recipients = (Vec<EmailAddress>, Vec<EmailAddress>, Vec<EmailAddress>);

fn parse_recipients(compose: &ComposeState) -> Result<Recipients, String> {
    let to = jmap::parse_addresses(&compose.to_text)?;
    let cc = jmap::parse_addresses(&compose.cc_text)?;
    let bcc = jmap::parse_addresses(&compose.bcc_text)?;
    if to.is_empty() {
        return Err("no recipient in To".to_owned());
    }
    Ok((to, cc, bcc))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::shell::{Done, Event};
    use crate::view::UiEvent;
    use serde_json::json;

    pub(crate) fn entry(id: &str) -> AccountEntry {
        AccountEntry {
            id: id.into(),
            name: id.to_uppercase(),
            url: format!("https://{id}.example.invalid"),
            email: format!("ann@{id}.example.invalid"),
            ca_path: None,
            enabled: true,
            signature: String::new(),
        }
    }

    pub(crate) fn settings(ids: &[&str]) -> EmailSettings {
        EmailSettings {
            accounts: ids.iter().map(|id| entry(id)).collect(),
            ..EmailSettings::default()
        }
    }

    pub(crate) fn secrets(ids: &[&str]) -> Secrets {
        let mut secrets = Secrets::default();
        for id in ids {
            secrets.set(id, "secret");
        }
        secrets
    }

    pub(crate) fn tag(state: &State, account: &str) -> Tag {
        Tag {
            account: account.into(),
            generation: state.account(account).unwrap().generation,
        }
    }

    fn info() -> SessionInfo {
        SessionInfo {
            account_name: "Account 7".into(),
            account_id: "7".into(),
        }
    }

    /// `ids` from the cache, every one connected, settingsd read.
    pub(crate) fn connected_many(ids: &[&str]) -> State {
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            settings(ids),
            secrets(ids),
        );
        state.set_settings(settings(ids));
        state.take_work();
        for id in ids {
            let tag = tag(&state, id);
            state.apply(Event::Connected(tag, Ok(info())));
        }
        state.take_work();
        state.config_saved();
        state
    }

    fn connected() -> State {
        connected_many(&["a"])
    }

    fn done(state: &mut State, account: &str, done: Done) {
        let tag = tag(state, account);
        state.apply(Event::Account(tag, done));
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

    fn connects(work: &[Work]) -> Vec<&str> {
        work.iter()
            .filter_map(|w| match w {
                Work::Connect { account, .. } => Some(account.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_cached_account_with_a_password_connects_at_launch() {
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            settings(&["a", "b"]),
            secrets(&["a"]),
        );
        assert_eq!(state.account("a").unwrap().phase, Phase::Connecting);
        assert_eq!(state.account("b").unwrap().phase, Phase::NoPassword);
        assert_eq!(connects(&state.take_work()), ["a"]);
        assert_eq!(state.selected.as_deref(), Some("a"), "the first account");
    }

    #[test]
    fn no_accounts_is_an_empty_window() {
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            EmailSettings::default(),
            Secrets::default(),
        );
        assert!(state.accounts.is_empty() && state.selected.is_none());
        assert!(state.take_work().is_empty());
        assert!(state.tree().is_empty());
    }

    #[test]
    fn settings_reconcile_add_remove_reconnect_relabel_and_disable() {
        let mut state = connected_many(&["a", "b", "c"]);
        let mut next = settings(&["a", "b", "c", "d"]);
        next.accounts[0].name = "Renamed".into();
        next.accounts[1].url = "https://moved.example.invalid".into();
        next.accounts[2].enabled = false;
        let mut secrets = state.secrets.clone();
        secrets.set("d", "pw");
        state.secrets = secrets;
        state.set_settings(next);
        let work = state.take_work();
        // a relabels only, b reconnects, c disconnects, d connects.
        assert_eq!(state.account("a").unwrap().entry.name, "Renamed");
        assert_eq!(state.account("a").unwrap().phase, Phase::Ready);
        assert_eq!(state.account("b").unwrap().phase, Phase::Connecting);
        assert_eq!(state.account("c").unwrap().phase, Phase::Disabled);
        assert_eq!(state.account("d").unwrap().phase, Phase::Connecting);
        assert_eq!(connects(&work), ["b", "d"]);
        assert!(work.iter().any(|w| matches!(w, Work::Disconnect { account } if account == "c")));
        // Then c is removed: its view and its password go.
        state.set_settings(settings(&["a", "b", "d"]));
        assert!(state.account("c").is_none());
        let work = state.take_work();
        assert!(work.iter().any(|w| matches!(w, Work::SaveSecrets { .. })));
        assert!(!state.has_password("c"));
        // The order of the settings is the tree's.
        state.set_settings(settings(&["d", "a", "b"]));
        let order: Vec<&str> = state.accounts.iter().map(|v| v.id()).collect();
        assert_eq!(order, ["d", "a", "b"]);
    }

    #[test]
    fn a_late_answer_from_one_account_never_touches_another() {
        let mut state = connected_many(&["a", "b"]);
        done(&mut state, "a", Done::Mailboxes(Ok(vec![mailbox("in", Some("inbox"), 2)])));
        done(&mut state, "b", Done::Mailboxes(Ok(vec![mailbox("in", Some("inbox"), 5)])));
        state.take_work();
        // Both accounts' first page is ticket 1: the account id tells them apart.
        let page = |id: &str| jmap::EmailPage {
            emails: vec![serde_json::from_value(json!({ "id": id, "keywords": {} })).unwrap()],
            total: 1,
        };
        done(&mut state, "b", Done::Page(1, Ok(page("B1"))));
        assert!(state.account("a").unwrap().mail.emails.is_empty());
        assert_eq!(state.account("b").unwrap().mail.emails[0].id, "B1");
        done(&mut state, "a", Done::Page(1, Ok(page("A1"))));
        assert_eq!(state.account("a").unwrap().mail.emails[0].id, "A1");
        assert_eq!(state.account("b").unwrap().mail.emails[0].id, "B1");
        // An answer for an account that is gone is dropped.
        state.apply(Event::Account(
            Tag {
                account: "zz".into(),
                generation: 1,
            },
            Done::Page(1, Ok(page("Z"))),
        ));
        assert_eq!(state.tree().iter().filter(|r| r.depth == 0).count(), 2);
    }

    #[test]
    fn an_answer_from_an_older_connection_is_void() {
        let mut state = connected_many(&["a"]);
        let old = tag(&state, "a");
        // The URL changes: a new connection.
        let mut next = settings(&["a"]);
        next.accounts[0].url = "https://moved.example.invalid".into();
        state.set_settings(next);
        state.apply(Event::Account(
            old.clone(),
            Done::Mailboxes(Ok(vec![mailbox("in", Some("inbox"), 1)])),
        ));
        assert!(state.account("a").unwrap().mail.mailboxes.is_empty());
        state.apply(Event::Connected(old, Ok(info())));
        assert_eq!(state.account("a").unwrap().phase, Phase::Connecting);
    }

    #[test]
    fn a_failed_connect_shows_the_error_with_a_retry() {
        let mut state = connected();
        let tag = tag(&state, "a");
        state.apply(Event::Connected(
            tag,
            Err(jmap::Error::Unauthorized("the server went away".into())),
        ));
        assert!(matches!(state.account("a").unwrap().phase, Phase::Failed(_)));
        state.apply_ui(UiEvent::Retry("a".into()));
        assert_eq!(state.account("a").unwrap().phase, Phase::Connecting);
        assert_eq!(connects(&state.take_work()), ["a"]);
    }

    #[test]
    fn a_password_set_connects_an_account_that_had_none() {
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            settings(&["a"]),
            Secrets::default(),
        );
        assert_eq!(state.account("a").unwrap().phase, Phase::NoPassword);
        assert!(state.set_password("a", "pw", Some(7)));
        let work = state.take_work();
        assert_eq!(connects(&work), ["a"]);
        let ticket = work
            .iter()
            .find_map(|w| match w {
                Work::SaveSecrets { ticket } => Some(*ticket),
                _ => None,
            })
            .unwrap();
        assert!(state.take_replies().is_empty(), "answered once saved");
        state.secrets_saved(ticket, Ok(true));
        let replies = state.take_replies();
        assert_eq!(replies[0].id, 7);
        assert_eq!(replies[0].body["has_password"], true);
        assert!(!replies[0].body.to_string().contains("pw"));
        assert!(!state.set_password("nope", "pw", None));
    }

    #[test]
    fn arriving_mailboxes_select_the_inbox_and_fetch_its_first_page() {
        let mut state = connected();
        done(
            &mut state,
            "a",
            Done::Mailboxes(Ok(vec![
                mailbox("m-sent", Some("sent"), 0),
                mailbox("m-in", Some("inbox"), 3),
            ])),
        );
        assert_eq!(state.account("a").unwrap().mail.selected.as_deref(), Some("m-in"));
        match state.take_work().as_slice() {
            [Work::FetchEmails { mailbox, limit, .. }] => {
                assert_eq!(mailbox, "m-in");
                assert_eq!(*limit, 100);
            }
            other => panic!("{other:?}"),
        }
    }

    fn unread_email(id: &str) -> Email {
        serde_json::from_value(json!({
            "id": id,
            "subject": "Hello",
            "from": [{ "name": "Bob", "email": "bob@example.invalid" }],
            "to": [{ "email": "ann@a.example.invalid" }, { "email": "cy@example.invalid" }],
            "cc": [{ "email": "dee@example.invalid" }],
            "messageId": ["<m1@example.invalid>"],
            "receivedAt": "2026-10-13T01:02:03Z",
            "keywords": {}
        }))
        .unwrap()
    }

    #[test]
    fn opening_an_unread_email_marks_it_read_locally_and_on_the_server() {
        let mut state = connected();
        let view = state.account_mut("a").unwrap();
        view.mail.selected = Some("m-in".into());
        view.mail.emails = vec![unread_email("E1")];
        state.apply_ui(UiEvent::OpenEmail("a".into(), "E1".into()));
        let work = state.take_work();
        assert!(
            work.iter()
                .any(|w| matches!(w, Work::MarkRead { id, read: true, .. } if id == "E1"))
        );
        assert!(
            work.iter()
                .any(|w| matches!(w, Work::OpenEmail { id, .. } if id == "E1"))
        );
        assert!(state.account("a").unwrap().mail.emails[0].is_read());
    }

    #[test]
    fn mark_read_on_open_off_leaves_the_email_unread() {
        let mut state = connected();
        let mut next = settings(&["a"]);
        next.mail.mark_read_on_open = false;
        state.set_settings(next);
        state.take_work();
        let view = state.account_mut("a").unwrap();
        view.mail.emails = vec![unread_email("E1")];
        state.apply_ui(UiEvent::OpenEmail("a".into(), "E1".into()));
        let work = state.take_work();
        assert!(!work.iter().any(|w| matches!(w, Work::MarkRead { .. })));
        assert!(work.iter().any(|w| matches!(w, Work::OpenEmail { .. })));
        assert!(!state.account("a").unwrap().mail.emails[0].is_read());
    }

    #[test]
    fn a_new_page_size_refetches_with_it() {
        let mut state = connected();
        done(&mut state, "a", Done::Mailboxes(Ok(vec![mailbox("in", Some("inbox"), 0)])));
        state.take_work();
        let mut next = settings(&["a"]);
        next.mail.page_size = 25;
        state.set_settings(next);
        assert!(state.take_work().iter().any(|w| matches!(w, Work::FetchEmails { limit: 25, .. })));
    }

    #[test]
    fn reply_prefills_to_subject_and_in_reply_to() {
        let mut state = connected();
        state.account_mut("a").unwrap().mail.open = Some(unread_email("E1"));
        state.apply_ui(UiEvent::Reply);
        let compose = state.compose.clone().expect("a compose dialog");
        assert_eq!(
            compose.draft.to,
            [EmailAddress {
                name: Some("Bob".into()),
                email: "bob@example.invalid".into()
            }]
        );
        assert_eq!(compose.to_text, "Bob <bob@example.invalid>");
        assert_eq!(compose.draft.subject, "Re: Hello");
        assert_eq!(
            compose.draft.in_reply_to.as_deref(),
            Some("<m1@example.invalid>")
        );
        assert_eq!(compose.draft.from, "ann@a.example.invalid");
        assert!(compose.draft.cc.is_empty());
        // Reply all: everyone but me, Cc kept.
        assert!(state.open_reply("a", true));
        let compose = state.compose.clone().unwrap();
        let to: Vec<&str> = compose.draft.to.iter().map(|a| a.email.as_str()).collect();
        assert_eq!(to, ["bob@example.invalid", "cy@example.invalid"]);
        assert_eq!(compose.draft.cc[0].email, "dee@example.invalid");
    }

    #[test]
    fn compose_is_from_the_default_account_with_its_signature() {
        let mut state = connected_many(&["a", "b"]);
        let mut next = settings(&["a", "b"]);
        next.default_account = Some("b".into());
        next.accounts[1].signature = "Ann\nGold Coast".into();
        state.set_settings(next);
        state.apply_ui(UiEvent::Compose);
        let compose = state.compose.clone().unwrap();
        assert_eq!(compose.account, "b");
        assert_eq!(compose.draft.from, "ann@b.example.invalid");
        assert_eq!(compose.draft.body, "\n\n-- \nAnn\nGold Coast\n");
        // Switching From swaps an untouched signature.
        state.apply_ui(UiEvent::ComposeAccount("a".into()));
        let compose = state.compose.clone().unwrap();
        assert_eq!(compose.draft.from, "ann@a.example.invalid");
        assert!(compose.draft.body.is_empty());
    }

    fn bare(email: &str) -> EmailAddress {
        EmailAddress {
            name: None,
            email: email.into(),
        }
    }

    #[test]
    fn sending_parses_the_address_lists() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid, \"Lee, Cy\" <cy@example.invalid>; ".into(),
        ));
        state.apply_ui(UiEvent::ComposeField(ComposeField::Subject, "Hi".into()));
        assert!(state.compose_sendable());
        state.apply_ui(UiEvent::SendCompose);
        let compose = state.compose.clone().unwrap();
        assert_eq!(
            compose.draft.to,
            [
                bare("bob@example.invalid"),
                EmailAddress {
                    name: Some("Lee, Cy".into()),
                    email: "cy@example.invalid".into()
                }
            ]
        );
        assert!(compose.busy);
    }

    #[test]
    fn a_malformed_recipient_is_refused_with_a_compose_error() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(ComposeField::To, "Ann <ann@".into()));
        state.apply_ui(UiEvent::ComposeField(ComposeField::Subject, "Hi".into()));
        state.apply_ui(UiEvent::SendCompose);
        let compose = state.compose.clone().unwrap();
        assert!(!compose.busy, "nothing was sent");
        assert!(compose.error.is_some());
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Send { .. }))
        );
    }

    #[test]
    fn edits_while_sending_are_refused() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::ComposeField(ComposeField::Subject, "Hi".into()));
        state.apply_ui(UiEvent::SendCompose);
        for (field, value) in [
            (ComposeField::To, "eve@example.invalid"),
            (ComposeField::Cc, "eve@example.invalid"),
            (ComposeField::Bcc, "eve@example.invalid"),
            (ComposeField::Subject, "changed"),
            (ComposeField::Body, "changed"),
        ] {
            state.apply_ui(UiEvent::ComposeField(field, value.into()));
        }
        let compose = state.compose.clone().unwrap();
        assert_eq!(compose.to_text, "bob@example.invalid");
        assert!(compose.cc_text.is_empty() && compose.bcc_text.is_empty());
        assert_eq!(compose.draft.subject, "Hi");
        assert!(compose.draft.body.is_empty());
    }

    #[test]
    fn an_empty_compose_cannot_send() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        assert!(!state.compose_sendable());
    }

    /// Take the queued work and return the send's account and ticket.
    fn sent_ticket(state: &mut State) -> (String, u64) {
        state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::Send { account, ticket, .. } => Some((account.clone(), *ticket)),
                _ => None,
            })
            .expect("a send was queued")
    }

    #[test]
    fn sending_closes_the_dialog_and_refreshes_the_surfaces() {
        let mut state = connected();
        let view = state.account_mut("a").unwrap();
        view.mail.mailboxes = vec![mailbox("m-sent", Some("sent"), 0)];
        view.mail.selected = Some("m-sent".into());
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SendCompose);
        assert!(state.compose.as_ref().is_some_and(|c| c.busy));
        let (account, ticket) = sent_ticket(&mut state);
        assert_eq!(account, "a");
        done(&mut state, "a", Done::Sent(ticket, Ok(())));
        assert!(state.compose.is_none());
        let work = state.take_work();
        assert!(work.iter().any(|w| matches!(w, Work::FetchMailboxes { .. })));
        assert!(matches!(
            work.as_slice(),
            [_, Work::FetchEmails { mailbox, .. }] if mailbox == "m-sent"
        ));
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
        let (_, ticket) = sent_ticket(&mut state);
        done(
            &mut state,
            "a",
            Done::Sent(ticket, Err(jmap::Error::method("serverFail", Some("no relay")))),
        );
        let compose = state.compose.expect("the dialog stays");
        assert!(!compose.busy);
        assert!(compose.error.is_some());
    }

    #[test]
    fn a_late_send_success_leaves_a_newer_draft_alone() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SendCompose);
        let (_, first) = sent_ticket(&mut state);
        // Cancelled while sending, then a new draft.
        state.apply_ui(UiEvent::CloseCompose);
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(ComposeField::Subject, "next".into()));
        done(&mut state, "a", Done::Sent(first, Ok(())));
        let compose = state.compose.as_ref().expect("the new draft stays");
        assert_eq!(compose.draft.subject, "next");
        assert!(!compose.busy);
    }

    #[test]
    fn another_accounts_send_never_closes_this_compose() {
        let mut state = connected_many(&["a", "b"]);
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(ComposeField::To, "bob@example.invalid".into()));
        state.apply_ui(UiEvent::SendCompose);
        let (account, ticket) = sent_ticket(&mut state);
        assert_eq!(account, "a");
        done(&mut state, "b", Done::Sent(ticket, Ok(())));
        assert!(state.compose.as_ref().is_some_and(|c| c.busy), "still sending");
    }

    #[test]
    fn a_typed_separator_stays_in_the_recipient_field() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid,".into(),
        ));
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.to_text, "bob@example.invalid,");
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid, cy@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SendCompose);
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(
            compose.draft.to,
            [bare("bob@example.invalid"), bare("cy@example.invalid")]
        );
    }

    #[test]
    fn a_superseded_answer_is_dropped() {
        let mut state = connected();
        state.account_mut("a").unwrap().mail.mailboxes =
            vec![mailbox("m-in", Some("inbox"), 0), mailbox("m-b", None, 0)];
        state.select_mailbox("a", "m-in".into());
        state.select_mailbox("a", "m-b".into());
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
            emails: vec![serde_json::from_value(json!({ "id": id, "keywords": {} })).unwrap()],
            total: 1,
        };
        done(&mut state, "a", Done::Page(new, Ok(page("B1"))));
        // The first mailbox's page arrives last: it must not replace m-b's.
        done(&mut state, "a", Done::Page(old, Ok(page("A1"))));
        assert_eq!(state.account("a").unwrap().mail.emails[0].id, "B1");
        // Likewise a contact search and an opened body.
        let next = state.account("a").unwrap().tickets.contacts + 1;
        done(&mut state, "a", Done::Contacts(next, Ok(Vec::new())));
        let next = state.account("a").unwrap().tickets.open + 1;
        done(
            &mut state,
            "a",
            Done::Opened(next, Box::new(Ok(page("X").emails.remove(0)))),
        );
        assert!(state.account("a").unwrap().mail.open.is_none());
    }

    #[test]
    fn an_old_mailbox_row_cannot_open_under_the_new_selection() {
        let mut state = connected();
        state.account_mut("a").unwrap().mail.mailboxes =
            vec![mailbox("m-a", Some("inbox"), 0), mailbox("m-b", None, 0)];
        state.select_mailbox("a", "m-a".into());
        state.account_mut("a").unwrap().mail.emails =
            vec![serde_json::from_value(json!({ "id": "A1", "keywords": {} })).unwrap()];
        // An open issued under A, then the switch to B.
        state.apply_ui(UiEvent::OpenEmail("a".into(), "A1".into()));
        let ticket = state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::OpenEmail { ticket, .. } => Some(*ticket),
                _ => None,
            })
            .unwrap();
        state.select_mailbox("a", "m-b".into());
        assert!(state.account("a").unwrap().mail.emails.is_empty(), "A's rows are gone");
        // A click on A's row while B loads does nothing.
        state.take_work();
        state.apply_ui(UiEvent::OpenEmail("a".into(), "A1".into()));
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::OpenEmail { .. }))
        );
        // And A's body, arriving late, is dropped.
        let body: Email = serde_json::from_value(json!({ "id": "A1", "keywords": {} })).unwrap();
        done(&mut state, "a", Done::Opened(ticket, Box::new(Ok(body))));
        assert!(state.account("a").unwrap().mail.open.is_none());
    }

    #[test]
    fn a_late_calendar_answer_does_not_replace_the_selected_one() {
        let mut state = connected();
        state.account_mut("a").unwrap().calendars.calendars = vec![
            Calendar {
                id: "C1".into(),
                ..Calendar::default()
            },
            Calendar {
                id: "C2".into(),
                ..Calendar::default()
            },
        ];
        state.select_calendar("a", "C1".into());
        state.select_calendar("a", "C2".into());
        let tickets: Vec<u64> = state
            .take_work()
            .iter()
            .filter_map(|w| match w {
                Work::FetchEvents { ticket, .. } => Some(*ticket),
                _ => None,
            })
            .collect();
        let [first, second] = tickets[..] else {
            panic!("{tickets:?}")
        };
        let event = |title: &str| -> CalendarEvent {
            serde_json::from_value(json!({ "id": title, "calendarId": "C", "title": title }))
                .unwrap()
        };
        done(&mut state, "a", Done::Events(second, Ok(vec![event("two")])));
        done(&mut state, "a", Done::Events(first, Ok(vec![event("one")])));
        let view = state.account("a").unwrap();
        assert_eq!(view.calendars.events.len(), 1);
        assert_eq!(view.calendars.events[0].id, "two");
    }

    #[test]
    fn a_count_refresh_keeps_the_open_email() {
        let mut state = connected();
        done(&mut state, "a", Done::Mailboxes(Ok(vec![mailbox("m-in", Some("inbox"), 1)])));
        state.take_work();
        state.account_mut("a").unwrap().mail.emails =
            vec![serde_json::from_value(json!({ "id": "E1", "keywords": {} })).unwrap()];
        state.apply_ui(UiEvent::OpenEmail("a".into(), "E1".into()));
        let open = state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::OpenEmail { ticket, .. } => Some(*ticket),
                _ => None,
            })
            .unwrap();
        let email: Email = serde_json::from_value(json!({ "id": "E1", "keywords": {} })).unwrap();
        done(&mut state, "a", Done::Opened(open, Box::new(Ok(email))));
        // The mark-read's count refresh answers after the body.
        done(&mut state, "a", Done::Mailboxes(Ok(vec![mailbox("m-in", Some("inbox"), 0)])));
        let view = state.account("a").unwrap();
        assert_eq!(view.mail.open.as_ref().map(|e| e.id.as_str()), Some("E1"));
    }

    #[test]
    fn a_mode_change_fetches_what_the_mode_lacks() {
        let mut state = connected();
        done(
            &mut state,
            "a",
            Done::Calendars(Ok(vec![Calendar {
                id: "C1".into(),
                name: "Main".into(),
                ..Calendar::default()
            }])),
        );
        state.take_work();
        // The calendar's events answered empty: the mode change asks again.
        let ticket = state.account("a").unwrap().tickets.events;
        done(&mut state, "a", Done::Events(ticket, Ok(Vec::new())));
        state.apply_ui(UiEvent::Mode(Mode::Calendar));
        match state.take_work().as_slice() {
            [Work::FetchEvents { calendar, .. }] => assert_eq!(calendar, "C1"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_contact_filter_refetches_the_selected_book() {
        let mut state = connected();
        done(
            &mut state,
            "a",
            Done::Books(Ok(vec![AddressBook {
                id: "B1".into(),
                name: "Main".into(),
                ..AddressBook::default()
            }])),
        );
        state.take_work();
        state.apply_ui(UiEvent::ContactFilter("a".into(), "an".into()));
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

    fn tree_fixture() -> State {
        let mut state = connected_many(&["a", "b"]);
        let mut child = mailbox("m-kid", None, 4);
        child.parent_id = Some("m-folder".into());
        done(
            &mut state,
            "a",
            Done::Mailboxes(Ok(vec![
                mailbox("m-trash", Some("trash"), 0),
                mailbox("m-folder", None, 0),
                child,
                mailbox("m-in", Some("inbox"), 2),
            ])),
        );
        done(
            &mut state,
            "a",
            Done::Calendars(Ok(vec![Calendar {
                id: "C1".into(),
                name: "Main".into(),
                ..Calendar::default()
            }])),
        );
        done(
            &mut state,
            "a",
            Done::Books(Ok(vec![AddressBook {
                id: "B1".into(),
                name: "Friends".into(),
                ..AddressBook::default()
            }])),
        );
        state.take_work();
        state
    }

    fn labels(state: &State) -> Vec<String> {
        state
            .tree()
            .iter()
            .map(|r| format!("{}{}", " ".repeat(r.depth), r.label))
            .collect()
    }

    #[test]
    fn the_tree_nests_mailboxes_then_calendars_then_books_per_account() {
        let state = tree_fixture();
        assert_eq!(
            labels(&state),
            [
                "A",
                " M-IN",
                " M-FOLDER",
                "  M-KID",
                " M-TRASH",
                " Calendars",
                "  Main",
                " Address Books",
                "  Friends",
                "B",
            ]
        );
        let rows = state.tree();
        assert_eq!(rows[0].unread, 6, "the account's total");
        assert!(rows[1].selected, "the inbox is shown");
        assert!(rows[2].expandable && !rows[1].expandable);
    }

    #[test]
    fn a_tree_click_selects_the_account_the_mode_and_the_node() {
        let mut state = tree_fixture();
        state.apply_ui(UiEvent::Select(Node::Book("a".into(), "B1".into())));
        assert_eq!(state.mode, Mode::Contacts);
        assert!(state.take_work().iter().any(|w| matches!(w, Work::FetchContacts { book, .. } if book == "B1")));
        state.apply_ui(UiEvent::Select(Node::Account("b".into())));
        assert_eq!(state.selected.as_deref(), Some("b"));
        state.apply_ui(UiEvent::Select(Node::Mailbox("a".into(), "m-kid".into())));
        assert_eq!((state.selected.as_deref(), state.mode), (Some("a"), Mode::Mail));
        assert_eq!(state.account("a").unwrap().mail.selected.as_deref(), Some("m-kid"));
    }

    #[test]
    fn tree_keys_move_collapse_expand_and_select() {
        let mut state = tree_fixture();
        // The cursor starts on the shown row (the inbox).
        state.tree_key(TreeKey::Down);
        assert_eq!(state.ui.cursor, Some(Node::Mailbox("a".into(), "m-folder".into())));
        state.tree_key(TreeKey::Left);
        assert!(!state.expanded(&Node::Mailbox("a".into(), "m-folder".into())));
        assert!(state.config_dirty());
        assert!(!labels(&state).contains(&"  M-KID".to_owned()));
        state.tree_key(TreeKey::Right);
        state.tree_key(TreeKey::Right);
        assert_eq!(state.ui.cursor, Some(Node::Mailbox("a".into(), "m-kid".into())));
        state.tree_key(TreeKey::Left);
        assert_eq!(state.ui.cursor, Some(Node::Mailbox("a".into(), "m-folder".into())));
        state.tree_key(TreeKey::Enter);
        assert_eq!(state.account("a").unwrap().mail.selected.as_deref(), Some("m-folder"));
        // Collapsing an account hides everything under it.
        state.apply_ui(UiEvent::Expand(Node::Account("a".into()), false));
        assert_eq!(labels(&state), ["A", "B"]);
    }

    #[test]
    fn next_account_wraps_in_tree_order() {
        let mut state = connected_many(&["a", "b"]);
        state.step_account(true);
        assert_eq!(state.selected.as_deref(), Some("b"));
        state.step_account(true);
        assert_eq!(state.selected.as_deref(), Some("a"));
        state.step_account(false);
        assert_eq!(state.selected.as_deref(), Some("b"));
    }

    fn settings_ticket(state: &mut State) -> (u64, Value) {
        state
            .take_work()
            .into_iter()
            .find_map(|w| match w {
                Work::Settings { ticket, changes } => Some((ticket, changes)),
                _ => None,
            })
            .expect("a settings write")
    }

    #[test]
    fn an_account_write_answers_after_settingsd_accepts_then_holds_the_password() {
        let mut state = connected_many(&["a"]);
        state.change_accounts(Change::Upsert(entry("n")), Some(3), Some("pw".into()));
        let (ticket, changes) = settings_ticket(&mut state);
        assert_eq!(changes["email.accounts"].as_array().unwrap().len(), 2);
        assert!(!changes.to_string().contains("pw"), "no secret goes to settingsd");
        assert!(state.take_replies().is_empty());
        state.settled(ticket, Settled::Accepted);
        let work = state.take_work();
        assert!(work.iter().any(|w| matches!(w, Work::ReadSettings)));
        let save = work
            .iter()
            .find_map(|w| match w {
                Work::SaveSecrets { ticket } => Some(*ticket),
                _ => None,
            })
            .unwrap();
        // In force at once: the new account connects with its password.
        assert_eq!(connects(&work), ["n"]);
        state.secrets_saved(save, Ok(true));
        assert_eq!(state.take_replies()[0].body, json!({"id": "n"}));
        assert!(state.has_password("n"));
        // The follower's read then confirms it: nothing more to do.
        state.set_settings(settings(&["a", "n"]));
        assert!(connects(&state.take_work()).is_empty());
    }

    #[test]
    fn a_conflict_retries_on_the_fresh_read_and_refusals_answer_errors() {
        let mut state = connected_many(&["a"]);
        state.change_accounts(Change::Remove("a".into()), Some(4), None);
        let (ticket, _) = settings_ticket(&mut state);
        state.settled(ticket, Settled::Conflict);
        assert!(state.take_work().iter().any(|w| matches!(w, Work::ReadSettings)));
        // Someone added b meanwhile: the retry keeps it.
        state.set_settings(settings(&["a", "b"]));
        let (ticket, changes) = settings_ticket(&mut state);
        assert_eq!(changes["email.accounts"][0]["id"], "b");
        state.settled(ticket, Settled::Refused("nope".into()));
        let reply = &state.take_replies()[0];
        assert_eq!((reply.id, reply.rc), (4, 10));
        assert_eq!(reply.body["error_code"], code::REFUSED);
        // An unknown account never reaches settingsd.
        state.change_accounts(Change::Remove("zz".into()), Some(5), None);
        assert!(!state.take_work().iter().any(|w| matches!(w, Work::Settings { .. })));
        assert_eq!(state.take_replies()[0].body["error_code"], code::UNKNOWN_ACCOUNT);
    }

    #[test]
    fn without_settingsd_the_cache_stands_read_only() {
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            settings(&["a"]),
            secrets(&["a"]),
        );
        state.take_work();
        state.settings_missing();
        assert_eq!(state.source, Source::Missing);
        assert_eq!(state.accounts.len(), 1, "the cache stands");
        state.change_accounts(Change::Remove("a".into()), Some(9), None);
        assert_eq!(state.take_replies()[0].body["error_code"], code::UNAVAILABLE);
        assert!(!state.take_work().iter().any(|w| matches!(w, Work::Settings { .. })));
    }

    #[test]
    fn a_write_before_the_first_read_waits_for_it() {
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            settings(&["a"]),
            secrets(&["a"]),
        );
        state.take_work();
        state.change_accounts(Change::Default(Some("a".into())), Some(2), None);
        assert!(!state.take_work().iter().any(|w| matches!(w, Work::Settings { .. })));
        state.set_settings(settings(&["a"]));
        let (_, changes) = settings_ticket(&mut state);
        assert_eq!(changes, json!({"email.default_account": "a"}));
    }

    #[test]
    fn the_schema_one_account_migrates_once_and_loses_nothing() {
        let legacy = AccountEntry {
            id: writer::legacy_id("https://mail.example.invalid", "admin@example.invalid"),
            name: "admin@example.invalid".into(),
            url: "https://mail.example.invalid".into(),
            email: "admin@example.invalid".into(),
            ca_path: None,
            enabled: true,
            signature: String::new(),
        };
        let id = legacy.id.clone();
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            EmailSettings::default(),
            Secrets::default(),
        );
        state.begin_migration(legacy.clone(), "pw-1");
        // It connects at once and its password is written out first.
        let work = state.take_work();
        assert_eq!(connects(&work), [id.as_str()]);
        assert!(work.iter().any(|w| matches!(w, Work::SaveSecrets { .. })));
        assert!(state.migrating());
        // settingsd answers with no accounts: the migration is sent, and
        // the account stays shown (and connected) while it is.
        state.set_settings(EmailSettings::default());
        let (ticket, changes) = settings_ticket(&mut state);
        assert_eq!(changes["email.accounts"][0]["id"], id.as_str());
        assert_eq!(changes["email.default_account"], id.as_str());
        assert!(!changes.to_string().contains("pw-1"));
        assert_eq!(state.account(&id).unwrap().phase, Phase::Connecting);
        state.settled(ticket, Settled::Accepted);
        assert!(!state.migrating() && state.take_migrated());
        assert!(state.has_password(&id));
        // The read that follows changes nothing, and nothing is sent again.
        let mut after = EmailSettings::default();
        after.accounts.push(legacy);
        after.default_account = Some(id.clone());
        state.take_work();
        state.set_settings(after);
        let work = state.take_work();
        assert!(!work.iter().any(|w| matches!(w, Work::Settings { .. } | Work::Connect { .. })));
        assert_eq!(state.account(&id).unwrap().phase, Phase::Connecting);
    }

    #[test]
    fn a_migration_into_settings_that_already_hold_the_account_adopts_it() {
        let mut held = entry("a");
        held.url = "https://mail.example.invalid".into();
        held.email = "admin@example.invalid".into();
        let mut legacy = held.clone();
        legacy.id = "acct-legacy".into();
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            EmailSettings::default(),
            Secrets::default(),
        );
        state.begin_migration(legacy, "pw-1");
        state.set_settings(EmailSettings {
            accounts: vec![held],
            default_account: Some("a".into()),
            ..EmailSettings::default()
        });
        // Nothing to write: the migration completes and the password moves.
        assert!(!state.take_work().iter().any(|w| matches!(w, Work::Settings { .. })));
        assert!(!state.migrating());
        assert!(state.has_password("a") && !state.has_password("acct-legacy"));
        assert_eq!(state.accounts.len(), 1);
    }
}
