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
use toolkit::datatable::{
    Align, Column, Order, SelectionMode, Sort, TableEvent, TableState, Total, Width,
};

use crate::drafts::{DraftOp, SavedDraft};
use crate::secrets::Secrets;
use crate::work::{Batch, BatchKind, Login, Work};
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
    pub max_size_upload: Option<u64>,
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
    /// The Reply-To header: the identity's, else none; editable per draft.
    ReplyTo,
    Subject,
    Body,
}

impl ComposeField {
    /// The rows a compose shows only when asked (or when they hold
    /// something), in the order they sit under To.
    pub const OPTIONAL: [ComposeField; 3] =
        [ComposeField::Cc, ComposeField::Bcc, ComposeField::ReplyTo];
}

/// The mail surface.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailState {
    pub mailboxes: Vec<Mailbox>,
    pub selected: Option<String>,
    pub emails: Vec<Email>,
    pub total: u64,
    pub open: Option<Email>,
    /// The request that installed `open`, independent of any pending open.
    pub open_ticket: u64,
    /// How the open email's text body is shown, and its cached parse.
    pub reader: crate::reader::Reader,
    pub busy: bool,
    /// The open email's attachment saves and opens, by part id.
    pub transfers: BTreeMap<String, Transfer>,
    /// Addresses this account has seen: Sent pages' recipients and other
    /// pages' senders, for completion. In memory only.
    pub recent: Vec<crate::suggest::Suggestion>,
    /// The message list's table: its sort, page, selection, keyboard row
    /// and dragged widths. The list is one page of the server's query, in
    /// the table's sort (newest first when it has none); the sort and the
    /// page live here only, never in settings.
    pub table: TableState,
    /// "Select all N" was taken: every email of the selected mailbox's
    /// query (`total` of them) is selected, not only the shown page.
    pub everything: bool,
    /// The keyboard row's email while a new sort or page loads: the row
    /// stays on it if the new page still holds it.
    pub refocus: Option<String>,
    /// A "Select all N" action waiting on the query's ids.
    pub fetching: Option<Fetching>,
    /// The ids the last batch could not change, each with the reason.
    pub failed: Vec<(String, String)>,
}

/// A message action on the list's target (`mail.flag`, `mail.mark_unread`,
/// `mail.delete`, `mail.move`, `mail.junk`, `mail.not_junk`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Flag every target, or unflag them when every shown one is flagged.
    Flag,
    /// Mark every target read, or unread when every shown one is read.
    Read,
    /// To Trash: asks first for two or more, else as the settings say.
    Delete,
    /// Choose a mailbox, then move.
    Move,
    /// To Junk (`true`) or out of it to the inbox.
    Junk(bool),
}

/// What a message action applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// "Select all N": every email of the mailbox's query, `N` of them;
    /// their ids are fetched before acting.
    Everything(u64),
    /// These shown rows, in list order.
    Rows(Vec<String>),
}

/// The most emails "Select all N" acts on: above it, nothing is done.
pub const SELECT_ALL_CAP: u64 = 5000;

/// A "Select all N" id fetch in flight, in pages of
/// [`jmap::MAX_IDS_PER_REQUEST`].
#[derive(Clone, Debug, PartialEq)]
pub struct Fetching {
    pub ticket: u64,
    pub action: Action,
    pub mailbox: String,
    pub sort: jmap::EmailSort,
    /// The query's count: the list's total, then the server's.
    pub expected: u64,
    pub ids: Vec<String>,
}

/// The message list's column ids.
pub const COLUMN_SUBJECT: &str = "subject";
pub const COLUMN_FROM: &str = "from";
pub const COLUMN_DATE: &str = "date";

/// `n` with its thousands grouped: 2866 is "2,866".
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The message list's columns, Subject first: Subject (a resizable 55%),
/// From (the rest) and Date (`date_width` points, right-aligned). Each
/// sorts from its header, ascending first; the server does the sorting.
///
/// Only Subject is resizable: egui_extras' resize handle of a column that
/// follows the remainder is pinned (the remainder absorbs the drag), so the
/// remainder sits between the one resizable column and the fixed one.
pub fn list_columns(date_width: f32) -> Vec<Column> {
    vec![
        Column::new(COLUMN_SUBJECT, "column-subject")
            .width(Width::Share {
                share: 0.55,
                min: 120.0,
            })
            .sortable(Order::Ascending),
        Column::new(COLUMN_FROM, "column-from")
            .width(Width::Remainder { min: 80.0 })
            .sortable(Order::Ascending),
        Column::new(COLUMN_DATE, "column-date")
            .width(Width::Fixed(date_width))
            .align(Align::End)
            .sortable(Order::Ascending),
    ]
}

/// The list's sort when the person has chosen none: the server's own,
/// newest first.
pub fn default_sort() -> Sort {
    Sort::new(COLUMN_DATE, Order::Descending)
}

/// The Email/query sort for the list's `sort`: Subject is `subject`, From
/// `from`, Date `receivedAt` (the date the list shows). No sort, or a
/// column the server cannot sort by, is the server's own order.
pub fn email_sort(sort: Option<&Sort>) -> jmap::EmailSort {
    let Some(sort) = sort else {
        return jmap::EmailSort::default();
    };
    let by = match sort.column.as_str() {
        COLUMN_SUBJECT => jmap::SortBy::Subject,
        COLUMN_FROM => jmap::SortBy::From,
        COLUMN_DATE => jmap::SortBy::ReceivedAt,
        _ => return jmap::EmailSort::default(),
    };
    jmap::EmailSort::new(by, sort.order == Order::Ascending)
}

/// The most recent addresses remembered per account.
pub const RECENT_MAX: usize = 500;

/// How long typing pauses before contacts are looked up.
pub const SUGGEST_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(200);

impl MailState {
    /// Remember the addresses on `emails`, a page of the selected mailbox:
    /// recipients in Sent, senders elsewhere.
    fn remember(&mut self, emails: &[Email]) {
        let sent = self
            .selected_mailbox()
            .is_some_and(|m| m.role.as_deref() == Some("sent"));
        for email in emails {
            let lists: Vec<&Option<Vec<EmailAddress>>> = if sent {
                vec![&email.to, &email.cc]
            } else {
                vec![&email.from]
            };
            for address in lists.into_iter().flatten().flatten() {
                let known = self
                    .recent
                    .iter()
                    .any(|s| s.email.eq_ignore_ascii_case(&address.email));
                if !known && !address.email.trim().is_empty() {
                    self.recent.push(crate::suggest::Suggestion {
                        name: address.name.clone().unwrap_or_default(),
                        email: address.email.clone(),
                        source: crate::suggest::Source::Recent,
                    });
                }
            }
        }
        if self.recent.len() > RECENT_MAX {
            let excess = self.recent.len() - RECENT_MAX;
            self.recent.drain(..excess);
        }
    }
}

/// Where one attachment's save or open has got to.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "lowercase")]
pub enum Transfer {
    Saving,
    Opening,
    /// Saved at this path.
    Saved(String),
    /// Saved at this path and handed to the desktop.
    Opened(String),
    Failed(String),
}

impl MailState {
    /// The shown rows' keys (email ids), in list order.
    pub fn keys(&self) -> Vec<String> {
        self.emails.iter().map(|e| e.id.clone()).collect()
    }

    /// How many emails are selected: the mailbox's total once "Select all
    /// N" was taken, else the shown page's selected rows.
    pub fn selection_count(&self) -> u64 {
        if self.everything {
            self.total.max(self.table.selection.len() as u64)
        } else {
            self.table.selection.len() as u64
        }
    }

    /// The list's sort in force: the person's, else newest first.
    pub fn sort(&self) -> Sort {
        self.table.sort.clone().unwrap_or_else(default_sort)
    }

    /// The list's table as it is drawn: its sort in force (the Date arrow
    /// down until another is chosen), and an email opened some other way
    /// (the Bus, a delete's advance) shown as selected.
    pub fn list_table(&self) -> TableState {
        let mut table = self.table.clone();
        table.sort = Some(self.sort());
        if let Some(open) = &self.open
            && table.selection.is_empty()
        {
            table.selection = vec![open.id.clone()];
            table.focus = self.emails.iter().position(|e| e.id == open.id);
        }
        table
    }

    /// What a message action applies to: the selection when there is one
    /// (two or more rows, or one picked by Ctrl+click), else the keyboard
    /// row. The open email is the target only when it is the keyboard
    /// row as the list draws it (an email opened from elsewhere with
    /// nothing selected).
    pub fn target(&self) -> Option<Target> {
        if self.everything {
            return Some(Target::Everything(self.selection_count()));
        }
        let table = self.list_table();
        let selected: Vec<String> = self
            .emails
            .iter()
            .filter(|e| table.selection.contains(&e.id))
            .map(|e| e.id.clone())
            .collect();
        if !selected.is_empty() {
            return Some(Target::Rows(selected));
        }
        table
            .focus
            .and_then(|at| self.emails.get(at))
            .map(|e| Target::Rows(vec![e.id.clone()]))
    }

    /// The shown rows of the target (the whole page for "Select all N").
    pub fn target_rows(&self) -> Vec<&Email> {
        match self.target() {
            None => Vec::new(),
            Some(Target::Everything(_)) => self.emails.iter().collect(),
            Some(Target::Rows(ids)) => self.emails.iter().filter(|e| ids.contains(&e.id)).collect(),
        }
    }

    /// The line beside the actions naming their target: "Applies to 6
    /// selected", "Applies to this email", or "Fetching N…" while
    /// "Select all N"'s ids load. None when there is no target.
    pub fn target_line(&self) -> Option<String> {
        if let Some(fetching) = &self.fetching {
            let count = grouped(fetching.expected);
            return Some(crate::label_with("fetching-ids", &[("count", &count)]));
        }
        let count = match self.target()? {
            Target::Everything(n) => n,
            Target::Rows(ids) => ids.len() as u64,
        };
        Some(if count == 1 {
            crate::label("applies-this")
        } else {
            crate::label_with("applies-selected", &[("count", &grouped(count))])
        })
    }

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

    /// Change the selected mailbox's unread count by `delta`, ahead of the
    /// server's word.
    fn shift_unread(&mut self, delta: i64) {
        let selected = self.selected.clone();
        if let Some(mailbox) = self
            .mailboxes
            .iter_mut()
            .find(|m| Some(&m.id) == selected.as_ref())
        {
            mailbox.unread_emails = mailbox.unread_emails.saturating_add_signed(delta);
        }
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

/// The composer's text and its revision.
pub struct Body {
    pub doc: editor::local::Document,
    pub revision: u64,
    #[cfg(test)]
    reads: std::cell::Cell<usize>,
}

impl Body {
    pub fn new(text: &str) -> Result<Self, editor::local::Error> {
        Ok(Self {
            doc: editor::local::Document::new(text, "markdown")?,
            revision: 0,
            #[cfg(test)]
            reads: std::cell::Cell::new(0),
        })
    }

    pub fn contents(&self) -> String {
        #[cfg(test)]
        self.reads.set(self.reads.get() + 1);
        self.doc.contents()
    }

    pub fn set_body(&mut self, text: &str) -> Result<(), editor::local::Error> {
        self.doc.set_contents(text)?;
        self.revision += 1;
        Ok(())
    }
}

impl Clone for Body {
    /// Rebuild from the text. The clone drops undo history.
    fn clone(&self) -> Self {
        Self {
            revision: self.revision,
            // This text has already passed the buffer's limits.
            ..Self::new(&self.contents()).expect("existing composer text is valid")
        }
    }
}

impl PartialEq for Body {
    fn eq(&self, other: &Self) -> bool {
        self.contents() == other.contents()
    }
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Body").field(&self.contents()).finish()
    }
}

/// A compose dialog in flight.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposeState {
    /// The account it sends from.
    pub account: String,
    /// The message headers. Its wire body stays empty while composing.
    pub draft: Composed,
    /// File information checked on compose events, never by drawing a frame.
    pub attachments: Vec<ComposeAttachment>,
    /// Pending inspections prevent Send from racing an attachment batch.
    pub attachment_inspections: std::collections::BTreeSet<u64>,
    pub attachment_error: Option<String>,
    pub attachments_over_limit: bool,
    pub body: Body,
    /// The reply-position check copies text only after a body edit.
    reply_movable: std::cell::Cell<Option<(u64, bool)>>,
    /// The recipient fields as typed. The draft's lists are parsed from
    /// them, so a separator the user just typed is not lost.
    pub to_text: String,
    pub cc_text: String,
    pub bcc_text: String,
    pub busy: bool,
    pub error: Option<String>,
    /// The send this compose is waiting on: only its completion closes it.
    pub sending: Option<u64>,
    /// Captured when this send starts; retained through deferred work.
    pub send_origin: Option<SendOrigin>,
    /// The account's new address, when its login (URL or address) changed
    /// while this draft was open: Send waits until the person takes it.
    pub stale: Option<StaleFrom>,
    /// The address completions showing under a recipient field.
    pub suggest: Option<Suggest>,
    /// The local draft this compose is kept as (its file name), until the
    /// message is sent or the person discards it.
    pub draft_id: String,
    /// Bumped on every edit; `saved` is the revision last handed to the
    /// draft store.
    pub revision: u64,
    pub saved: u64,
    /// Opened from a draft an earlier run kept.
    pub restored: bool,
    /// What earlier sends of this message did on the server: a retry
    /// submits the same message instead of creating a second copy.
    pub attempt: jmap::SendAttempt,
    /// The quote a reply opened with, so the person can move it to the other
    /// side of their text. None for a new message or a reply without a quote.
    pub reply: Option<ReplyQuote>,
    /// The sender identity (its id within `account`) the draft is from.
    pub identity: String,
    /// The optional rows (Cc, Bcc, Reply-To) the person opened from the To
    /// row's buttons. A row that holds something shows whatever this says.
    pub extra: Vec<ComposeField>,
}

/// Paths and metadata only. File contents belong to the send worker.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ComposeAttachment {
    pub path: std::path::PathBuf,
    /// Plain file base name; the display qualifier is separate.
    pub name: String,
    /// Text shown by the chip, qualified by its directory when needed.
    pub label: String,
    pub size: Option<u64>,
    pub state: ComposeAttachmentState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SendOrigin {
    pub caller_key: String,
    pub drive_request_id: Option<u64>,
    pub mixed_input: bool,
}

impl Default for SendOrigin {
    fn default() -> Self {
        Self {
            caller_key: "window".into(),
            drive_request_id: None,
            mixed_input: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposeAttachmentState {
    Ready,
    Missing,
    NotFile,
    Unreadable,
    Relative,
}

impl ComposeAttachmentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "compose-attachment-ready",
            Self::Missing => "compose-attachment-missing",
            Self::NotFile => "compose-attachment-not-file",
            Self::Unreadable => "compose-attachment-unreadable",
            Self::Relative => "compose-attachment-relative",
        }
    }
}

impl ComposeAttachment {
    pub(crate) fn inspect_all(paths: &[std::path::PathBuf]) -> Vec<Self> {
        let mut attachments: Vec<_> = paths.iter().map(|path| Self::inspect(path)).collect();
        let names: Vec<_> = attachments.iter().map(|a| a.name.clone()).collect();
        for (index, attachment) in attachments.iter_mut().enumerate() {
            if !paths
                .iter()
                .enumerate()
                .any(|(other, path)| *path != paths[index] && names[other] == names[index])
            {
                continue;
            }
            let parent = paths[index]
                .parent()
                .unwrap_or_else(|| std::path::Path::new(""));
            let directory = parent.file_name().unwrap_or(parent.as_os_str());
            // If even the immediate parents collide, include their full path.
            let same_parent_name = paths.iter().enumerate().any(|(other, path)| {
                *path != paths[index]
                    && names[other] == names[index]
                    && path.parent().and_then(std::path::Path::file_name) == parent.file_name()
            });
            let directory = if same_parent_name {
                parent.as_os_str()
            } else {
                directory
            };
            attachment.label = crate::label_with(
                "compose-attachment-name",
                &[
                    ("name", &names[index]),
                    ("directory", &directory.to_string_lossy()),
                ],
            );
        }
        attachments
    }

    fn inspect(path: &std::path::Path) -> Self {
        use std::os::unix::ffi::OsStrExt;

        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let mut attachment = Self {
            path: path.to_owned(),
            label: name.clone(),
            name,
            size: None,
            state: ComposeAttachmentState::Ready,
        };
        attachment.state = if !path.is_absolute() {
            ComposeAttachmentState::Relative
        } else {
            match std::fs::metadata(path) {
                Ok(metadata) if metadata.is_file() => {
                    attachment.size = Some(metadata.len());
                    // Check effective-user read permission without opening the
                    // file or reading any bytes. Send still opens and checks it.
                    let readable =
                        std::ffi::CString::new(path.as_os_str().as_bytes()).is_ok_and(|path| {
                            // SAFETY: path is a live NUL-terminated string.
                            unsafe {
                                libc::faccessat(
                                    libc::AT_FDCWD,
                                    path.as_ptr(),
                                    libc::R_OK,
                                    libc::AT_EACCESS,
                                ) == 0
                            }
                        });
                    if readable {
                        ComposeAttachmentState::Ready
                    } else {
                        ComposeAttachmentState::Unreadable
                    }
                }
                Ok(_) => ComposeAttachmentState::NotFile,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ComposeAttachmentState::Missing
                }
                Err(_) => ComposeAttachmentState::Unreadable,
            }
        };
        attachment
    }
}

/// The quote a reply carries: the attribution and quoted original, the
/// signature, and which side of the person's text they sit on.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplyQuote {
    pub quote: String,
    pub signature: String,
    pub position: crate::quote::Position,
}

/// The three panes of the window, in Tab order: the account tree, the message
/// list, the reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Tree,
    List,
    Reader,
}

/// The identities dialog: one account's identities, edited before they are
/// saved. `rows` starts as the account's senders, so an account with none of
/// its own shows its default identity to change.
#[derive(Clone, Debug, PartialEq)]
pub struct IdentityEditor {
    pub account: String,
    pub rows: Vec<settings::email::Identity>,
    pub selected: usize,
    /// Why the last save was refused, as the settings validation names it.
    pub error: Option<String>,
}

/// The text fields of an identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityField {
    Name,
    Address,
    Signature,
    ReplyTo,
}

/// A body's signature block: the signature after the standard `-- `
/// separator, or nothing when there is no signature.
pub fn signature_block(signature: &str) -> String {
    let signature = signature.trim_end();
    if signature.is_empty() {
        String::new()
    } else {
        format!("\n\n-- \n{signature}\n")
    }
}

/// The display name and Reply-To a sender identity gives a draft. An
/// account with no identities of its own keeps the bare address in From, as
/// it did before identities, so its name is left out.
fn from_fields(explicit: bool, sender: &settings::email::Identity) -> (String, Option<String>) {
    let name = if explicit {
        sender.name.clone()
    } else {
        String::new()
    };
    (name, sender.reply_to.clone())
}

/// A question that must be answered before a draft would close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// Cancel or Escape on a compose that holds something typed.
    CloseCompose,
    /// The window (or `file.quit`) closing over one.
    Quit,
}

/// The answer to a [`Confirm`], or to the offer of a kept draft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftChoice {
    /// The default: keep the draft (and close); for an offer, restore it.
    Keep,
    /// Delete the draft for good.
    Discard,
    /// Back to editing; for an offer, later (every draft stays kept).
    Back,
}

/// How long typing pauses before the draft is saved (the shell's timer).
/// Every close path saves at once, whatever the timer.
pub const DRAFT_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(1);

/// Completions for the entry being typed in a recipient field.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggest {
    pub field: ComposeField,
    /// The entry looked up.
    pub query: String,
    /// The newest lookup: only its answer applies.
    pub ticket: u64,
    pub items: Vec<crate::suggest::Suggestion>,
    /// The highlighted row.
    pub cursor: usize,
}

impl ComposeState {
    pub fn set_body(&mut self, text: &str) -> bool {
        if let Err(error) = self.body.set_body(text) {
            self.error = Some(error.to_string());
            return false;
        }
        self.error = None;
        true
    }

    pub fn reply_movable(&self) -> bool {
        let Some(reply) = &self.reply else {
            return false;
        };
        if let Some((revision, movable)) = self.reply_movable.get()
            && revision == self.body.revision
        {
            return movable;
        }
        let movable = crate::quote::moved(
            &self.body.contents(),
            &reply.quote,
            &reply.signature,
            reply.position,
        )
        .is_some();
        self.reply_movable.set(Some((self.body.revision, movable)));
        movable
    }

    /// The text of recipient field `field` (empty for the others).
    pub fn recipients(&self, field: ComposeField) -> &str {
        match field {
            ComposeField::To => &self.to_text,
            ComposeField::Cc => &self.cc_text,
            ComposeField::Bcc => &self.bcc_text,
            ComposeField::ReplyTo => self.draft.reply_to.as_deref().unwrap_or(""),
            ComposeField::Subject | ComposeField::Body => "",
        }
    }

    /// Whether row `field` shows: To, Subject and the body always; Cc, Bcc
    /// and Reply-To once opened or while they hold something.
    pub fn shows(&self, field: ComposeField) -> bool {
        !ComposeField::OPTIONAL.contains(&field)
            || self.extra.contains(&field)
            || !self.recipients(field).trim().is_empty()
    }

    /// Whether closing this compose could lose something: it was edited,
    /// restored from a kept draft, or has been sent before.
    pub fn keepable(&self) -> bool {
        self.revision > 0
            || self.restored
            || !self.draft.attachments.is_empty()
            || self.attempt != jmap::SendAttempt::default()
    }

    pub fn attachment_size(&self) -> u64 {
        self.attachments
            .iter()
            .filter_map(|a| a.size)
            .fold(0, u64::saturating_add)
    }

    /// The compose as the draft store keeps it.
    pub fn saved_draft(&self) -> SavedDraft {
        SavedDraft {
            schema_version: crate::drafts::CURRENT_SCHEMA,
            id: self.draft_id.clone(),
            account: self.account.clone(),
            from: self.draft.from.clone(),
            to: self.to_text.clone(),
            cc: self.cc_text.clone(),
            bcc: self.bcc_text.clone(),
            subject: self.draft.subject.clone(),
            body: self.body.contents(),
            attachments: self.draft.attachments.clone(),
            in_reply_to: self.draft.in_reply_to.clone(),
            references: self.draft.references.clone(),
            identity: self.identity.clone(),
            from_name: self.draft.from_name.clone(),
            reply_to: self.draft.reply_to.clone(),
            saved_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            attempt: self.attempt.clone(),
        }
    }
}

/// A replacement From, bound to the account and connection it came from:
/// it can be taken only for that account, on that connection.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StaleFrom {
    pub account: String,
    pub generation: u64,
    pub address: String,
}

impl ComposeState {
    pub fn new(account: &str, mut draft: Composed) -> Self {
        let text = |list: &[EmailAddress]| {
            list.iter()
                .map(jmap::format_address)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (body, error) = match Body::new(&std::mem::take(&mut draft.body)) {
            Ok(body) => (body, None),
            Err(error) => (
                Body::new("").expect("empty text is within the buffer's limits"),
                Some(error.to_string()),
            ),
        };
        let revision = u64::from(!draft.attachments.is_empty());
        let attachments = ComposeAttachment::inspect_all(&draft.attachments);
        Self {
            account: account.to_owned(),
            body,
            reply_movable: std::cell::Cell::new(None),
            to_text: text(&draft.to),
            cc_text: text(&draft.cc),
            bcc_text: text(&draft.bcc),
            draft,
            attachments,
            attachment_inspections: std::collections::BTreeSet::new(),
            attachment_error: None,
            attachments_over_limit: false,
            busy: false,
            error,
            sending: None,
            send_origin: None,
            stale: None,
            suggest: None,
            draft_id: String::new(),
            revision,
            saved: 0,
            restored: false,
            attempt: jmap::SendAttempt::default(),
            reply: None,
            identity: String::new(),
            extra: Vec::new(),
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
    /// The email the latest open asked for, until a different one is asked
    /// for. A double-click's second half must not ask for it again.
    open_requested: Option<String>,
    events: u64,
    contacts: u64,
    send: u64,
    /// The latest "Select all N" id fetch: a cancelled or replaced one's
    /// pages are dropped.
    ids: u64,
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

    /// Another email than the open one was asked for and has not loaded
    /// (or failed to): actions on "the open email" would hit the wrong one.
    pub fn opening_another(&self) -> bool {
        self.tickets
            .open_requested
            .as_deref()
            .is_some_and(|id| self.mail.open.as_ref().is_none_or(|e| e.id != id))
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
        self.tickets.open_requested = None;
        self.tickets.ids += 1;
    }

    fn work(&self) -> String {
        self.entry.id.clone()
    }

    /// Select a mailbox and fetch its first page.
    fn select_mailbox(&mut self, id: String, limit: u32, work: &mut Vec<Work>) {
        self.mail.selected = Some(id.clone());
        self.mail.open = None;
        // An open still loading was for the old mailbox: its answer is
        // dropped, so it is no longer asked for.
        self.tickets.open_requested = None;
        // The old mailbox's rows go at once, so none of them can be opened
        // under the new selection while its page loads.
        self.mail.emails.clear();
        self.mail.total = 0;
        // Nothing selected in the new list, and its first page; the sort
        // and the dragged widths stay.
        self.mail.table = TableState {
            widths: std::mem::take(&mut self.mail.table.widths),
            sort: self.mail.table.sort.take(),
            ..TableState::default()
        };
        self.mail.everything = false;
        self.mail.refocus = None;
        self.cancel_fetch();
        // An open still in flight belongs to the old mailbox.
        self.tickets.open += 1;
        self.mail.busy = true;
        self.fetch_emails(id, limit, work);
    }

    /// Queue a fetch of the list's page, in its sort, `limit` to a page;
    /// only the newest one's answer is applied.
    fn fetch_emails(&mut self, mailbox: String, limit: u32, work: &mut Vec<Work>) {
        self.tickets.page += 1;
        // The pager counts in the page size the server is asked for.
        self.mail.table.page_size = limit.max(1);
        work.push(Work::FetchEmails {
            account: self.work(),
            ticket: self.tickets.page,
            mailbox,
            sort: email_sort(self.mail.table.sort.as_ref()),
            position: self.mail.table.offset(),
            limit,
        });
    }

    /// A header click or the pager (`event`, a sort or a page change):
    /// the selection goes, the keyboard row is remembered by its email,
    /// and the new page is asked for.
    fn requery(&mut self, event: &TableEvent, limit: u32, work: &mut Vec<Work>) {
        let Some(mailbox) = self.mail.selected.clone() else {
            return;
        };
        self.mail.refocus = self
            .mail
            .list_table()
            .focus
            .and_then(|at| self.mail.emails.get(at))
            .map(|e| e.id.clone());
        self.mail.table.apply(event, SelectionMode::Multi);
        self.mail.table.selection.clear();
        self.mail.table.anchor = None;
        self.mail.everything = false;
        self.cancel_fetch();
        self.mail.busy = true;
        self.fetch_emails(mailbox, limit, work);
    }

    /// Drop a "Select all N" id fetch in flight; its pages are ignored.
    /// True when there was one.
    fn cancel_fetch(&mut self) -> bool {
        if self.mail.fetching.take().is_some() {
            self.tickets.ids += 1;
            true
        } else {
            false
        }
    }

    /// A page of the list arrived. The keyboard row stays on its email if
    /// the page holds it; a page past the end (the list shrank under a
    /// reload) asks for the last page instead.
    fn page_arrived(&mut self, page: jmap::EmailPage, limit: u32, work: &mut Vec<Work>) {
        let last = self
            .mail
            .table
            .last_page(Total::Known(page.total))
            .unwrap_or(0);
        if page.emails.is_empty()
            && self.mail.table.page > last
            && let Some(mailbox) = self.mail.selected.clone()
        {
            self.mail.table.page = last;
            self.mail.total = page.total;
            self.fetch_emails(mailbox, limit, work);
            return;
        }
        self.mail.remember(&page.emails);
        self.mail.busy = false;
        self.mail.emails = page.emails;
        self.mail.total = page.total;
        let keys = self.mail.keys();
        if let Some(key) = self.mail.refocus.take() {
            self.mail.table.focus = keys.iter().position(|k| *k == key);
        }
        self.mail.table.rows_changed(&keys);
        // Every email still selected: the new page shows so.
        if self.mail.everything {
            self.mail.table.selection = keys;
        }
    }

    /// Queue an email open; only the newest one's answer is applied (and
    /// `reply`, a Bus request, is answered whatever happens).
    fn open_email(&mut self, id: String, reply: Option<u64>, work: &mut Vec<Work>) {
        self.tickets.open += 1;
        self.tickets.open_mailbox = self.mail.selected.clone();
        self.tickets.open_requested = Some(id.clone());
        work.push(Work::OpenEmail {
            account: self.work(),
            ticket: self.tickets.open,
            id,
            reply,
        });
    }

    /// Set or clear a keyword on the shown copies of `ids` (the row and the
    /// open email), ahead of the server's word.
    fn set_keyword(&mut self, ids: &[String], keyword: &str, on: bool) {
        let open = self.mail.open.iter_mut();
        for email in self.mail.emails.iter_mut().chain(open) {
            if ids.contains(&email.id) {
                let keywords = email.keywords.get_or_insert_with(|| json!({}));
                if let Some(map) = keywords.as_object_mut() {
                    if on {
                        map.insert(keyword.into(), json!(true));
                    } else {
                        map.remove(keyword);
                    }
                }
            }
        }
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

/// A form over the window, for one account.
#[derive(Clone, Debug, PartialEq)]
pub enum Form {
    /// Delete these emails? (`email.mail.confirm_delete`)
    Delete {
        account: String,
        generation: u64,
        ids: Vec<String>,
    },
    /// Move these emails: the mailbox is chosen in the form.
    Move {
        account: String,
        generation: u64,
        ids: Vec<String>,
    },
    Event(EventForm),
    Contact(ContactForm),
}

/// The New Event form. `start` and `end` are RFC 3339 instants as typed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventForm {
    pub account: String,
    /// The connection it was opened on: it submits only through that one.
    pub generation: u64,
    pub calendar: String,
    pub title: String,
    pub start: String,
    pub end: String,
    pub location: String,
    pub busy: bool,
    /// The create in flight: only its completion settles the form.
    pub op: Option<u64>,
    pub error: Option<String>,
}

/// The New Contact form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContactForm {
    pub account: String,
    /// The connection it was opened on: it submits only through that one.
    pub generation: u64,
    pub book: String,
    pub name: String,
    pub email: String,
    pub phone: String,
    pub organization: String,
    pub busy: bool,
    /// The create in flight: only its completion settles the form.
    pub op: Option<u64>,
    pub error: Option<String>,
}

/// One field of the event or contact form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormField {
    Title,
    Start,
    End,
    Location,
    Name,
    Email,
    Phone,
    Organization,
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
    pub decorations: Option<design::RequestedDecorations>,
    pub captions: Option<design::CaptionSide>,
}

/// The remembered window shape (persisted) plus the open dialog and the
/// tree's keyboard cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct UiState {
    pub attachments_expanded: bool,
    pub attachments_inline: bool,
    pub macros_open: bool,
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
            macros_open: false,
            attachments_expanded: false,
            attachments_inline: true,
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
    pub const DOCUMENT_TOO_LARGE: &str = "DOCUMENT_TOO_LARGE";
    pub const DOCUMENT_CARRIAGE_RETURN: &str = "DOCUMENT_CARRIAGE_RETURN";
    pub const DOCUMENT_FRAMING: &str = "DOCUMENT_FRAMING";
    pub const DOCUMENT_HEADER_NOT_ALLOWED: &str = "DOCUMENT_HEADER_NOT_ALLOWED";
    pub const DOCUMENT_MALFORMED_HEADER: &str = "DOCUMENT_MALFORMED_HEADER";
    pub const DOCUMENT_DUPLICATE_HEADER: &str = "DOCUMENT_DUPLICATE_HEADER";
    pub const DOCUMENT_EMPTY_VALUE: &str = "DOCUMENT_EMPTY_VALUE";
    pub const DOCUMENT_INVALID_VALUE: &str = "DOCUMENT_INVALID_VALUE";
    pub const DOCUMENT_PATH: &str = "DOCUMENT_PATH";
    pub const DOCUMENT_IO: &str = "DOCUMENT_IO";
    pub const DOCUMENT_ENCODING: &str = "DOCUMENT_ENCODING";
    pub const DOCUMENT_INLINE: &str = "DOCUMENT_INLINE";
    pub const IDENTITY_MISMATCH: &str = "IDENTITY_MISMATCH";
    pub const ATTACHMENT_RELATIVE: &str = "ATTACHMENT_RELATIVE";
    pub const ATTACHMENT_NOT_FILE: &str = "ATTACHMENT_NOT_FILE";
    pub const ATTACHMENT_UNREADABLE: &str = "ATTACHMENT_UNREADABLE";
    pub const MESSAGE_TOO_LARGE: &str = "MESSAGE_TOO_LARGE";
    pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
    /// No account has the id given.
    pub const UNKNOWN_ACCOUNT: &str = "UNKNOWN_ACCOUNT";
    /// The account has no email, mailbox, event or contact with the id given.
    pub const NOT_FOUND: &str = "NOT_FOUND";
    pub const NOT_CONNECTED: &str = "NOT_CONNECTED";
    /// The account reconnected (a new URL or login) while the request was
    /// pending: its answer is not used under the new connection.
    pub const STALE: &str = "STALE";
    /// A newer open (or a mailbox change) replaced this one in the reader
    /// before it loaded.
    pub const SUPERSEDED: &str = "SUPERSEDED";
    /// settingsd is absent: the accounts are read-only.
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
    pub const CONFLICT: &str = "CONFLICT";
    /// A settings write went unanswered past its deadline.
    pub const TIMEOUT: &str = "TIMEOUT";
    /// The account is still migrating from the schema-1 config: its
    /// password cannot change until that finishes (retry later).
    pub const MIGRATION_PENDING: &str = "MIGRATION_PENDING";
    pub const REFUSED: &str = "REFUSED";
    pub const OUTCOME_UNKNOWN: &str = "OUTCOME_UNKNOWN";
    /// The window closed before the request was started or answered.
    pub const CANCELLED: &str = "CANCELLED";
    pub const SECRETS: &str = "SECRETS";
    pub const SERVER: &str = "SERVER";
    pub const BUSY: &str = "BUSY";
    pub const UNKNOWN_COMMAND: &str = "UNKNOWN_COMMAND";
    pub const UNKNOWN_ACTION: &str = "UNKNOWN_ACTION";
    pub const DENIED: &str = "DENIED";
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
    /// The settings read ([`State::epoch`]) it was computed against: its
    /// prediction is installed only if no newer read has arrived since.
    base: u64,
    /// The account's credential operation when the write was made: its
    /// password (or a removal's forget) lands only if still the latest.
    credential: u64,
    /// The write's own id, kept across its retries: its deadline is
    /// [`Work::WriteDeadline`] with it.
    op: u64,
    /// The account whose credential operation this write is: released
    /// (the next one runs) when the write settles.
    holds: Option<String>,
}

/// Where the schema-1 migration stands. Each step needs the one before it
/// to have landed durably; a failure stops it where it is, the schema-1
/// file untouched, and the next launch runs it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// The password is being written to the secrets file.
    Saving,
    /// The password is durable: the apply goes out once settingsd is read.
    Ready,
    /// The apply to settingsd is in flight.
    Applying,
    /// settingsd holds the account under another id: the password is being
    /// written under that id.
    Transferring,
    /// settingsd has the account and its password is durable where
    /// settingsd will look for it: the schema-2 config rewrite is asked
    /// of the shell, and the lock holds until it is confirmed.
    Rewriting,
    /// A step failed this run (a save refused, settingsd refused).
    Stopped,
}

/// The schema-1 account on its way into settingsd.
#[derive(Clone, Debug)]
struct Migration {
    entry: AccountEntry,
    stage: Stage,
    /// Every id settingsd held the account as during this migration: they
    /// stay locked until it ends, whatever the current read lists.
    adopted: std::collections::BTreeSet<String>,
    /// settingsd accepted the account: if it disappears from settingsd
    /// before the migration ends, the migration applies it again.
    applied: bool,
}

/// A credential-bearing operation on one account. At most one runs per
/// account; later ones wait in submission order and run when it settles
/// (its settings write and its secrets save both landed, or failed), so
/// none is ever superseded silently and each caller hears its own outcome.
#[derive(Clone, Debug)]
enum CredentialOp {
    /// An account write carrying a password, or a removal.
    Write {
        change: Change,
        reply: Option<u64>,
        password: Option<String>,
    },
    /// `email.account.password.set` with a password.
    Password {
        password: String,
        reply: Option<u64>,
    },
    /// `email.account.password.set` with an empty password.
    Forget { reply: u64 },
}

/// What a secrets save finishing moves on.
#[derive(Clone, Debug)]
enum AfterSave {
    Nothing,
    /// The credential operation on this account has settled.
    Credential(String),
    /// The migration's password is durable.
    Migration,
    /// The migrated password is durable under the adopted id.
    Transfer,
}

/// The print page's body size, in points: the reader's default until the
/// reader font setting lands.
pub const PRINT_FONT_PT: f32 = 15.0;

/// How many times a write is sent before a conflict is reported.
const WRITE_ATTEMPTS: u32 = 3;

/// Everything the window shows.
pub struct State {
    pub previews: crate::previews::Cache,
    /// Origin of the command/input being applied now. Each accepted send
    /// copies it onto the compose and work item before the scope ends.
    input_origin: Option<SendOrigin>,
    /// The accounts, in tree order.
    pub accounts: Vec<AccountView>,
    /// The account the centre pane shows.
    pub selected: Option<String>,
    pub mode: Mode,
    /// The `email.*` settings in force.
    pub settings: EmailSettings,
    pub source: Source,
    /// How many settingsd reads have been applied: a write's prediction is
    /// installed only over the read it was computed against.
    epoch: u64,
    /// The accounts kept while settingsd shows none: an empty list arriving
    /// while accounts are held may be a settingsd that omits the group (it
    /// omits it at its defaults, so absent and empty look the same), so the
    /// views and clients stay until a non-empty read or an explicit removal.
    held: Option<Vec<AccountEntry>>,
    secrets: Secrets,
    /// Accounts whose password changed and is not yet durably saved, with
    /// the secrets version that changed it: a save of an older version
    /// never clears a newer change.
    unsaved: BTreeMap<String, u64>,
    /// Bumped on every change to the passwords held.
    secrets_version: u64,
    /// Per account, the latest credential operation (a password set, a
    /// forget, a removal): a write's password effect lands only if no
    /// newer operation happened since the write was made.
    credentials: BTreeMap<String, u64>,
    /// The last credential operation number handed out.
    credential_ops: u64,
    /// Accounts with a credential operation in flight.
    credential_busy: std::collections::BTreeSet<String>,
    /// The credential operations waiting, per account, in order.
    credential_queue: BTreeMap<String, std::collections::VecDeque<CredentialOp>>,
    /// The last connection generation handed out: one counter for every
    /// account, so a removed and restored account never reuses one.
    generations: u64,
    /// The schema-1 account until settingsd holds it and its password is
    /// durable under the id settingsd holds it as.
    migration: Option<Migration>,
    /// The migration completed: the shell rewrites the config as schema 2.
    migrated: bool,
    pub compose: Option<ComposeState>,
    pub attachment_chooser: Option<toolkit::file_chooser::FileChooser>,
    pub documents: crate::documents::DocumentState,
    pub attachment_notice: Option<String>,
    last_attachment_directory: Option<std::path::PathBuf>,
    /// The identities dialog, while open.
    pub identity_editor: Option<IdentityEditor>,
    /// The pane that has the keyboard: Tab moves it, and the arrow keys act on it.
    pub pane: Pane,
    /// A question over the compose before a draft closes.
    pub confirm: Option<Confirm>,
    /// Drafts kept from earlier (an earlier run, or a compose set aside),
    /// offered one at a time while no compose is open.
    pub restore: Vec<SavedDraft>,
    /// Draft saves and removals for the shell's store, in order.
    draft_ops: Vec<DraftOp>,
    /// Draft ids handed out this run.
    drafts_made: u64,
    /// Drafts the person discarded this run: a send still in flight never
    /// brings one back.
    discarded: std::collections::BTreeSet<String>,
    /// Each send in flight, by ticket, with the draft as it was sent.
    sends: BTreeMap<u64, SavedDraft>,
    /// The person answered the quit question: closing goes ahead.
    quit_ok: bool,
    /// A form over the window: a delete to confirm, a move's target, a new
    /// event or contact.
    pub form: Option<Form>,
    /// Why the open form cannot be submitted (its account's login changed).
    pub form_note: Option<String>,
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
    /// Bus requests sent to an account's server and not yet answered:
    /// `true` for a write (its outcome is unknown if the window closes).
    outstanding: BTreeMap<u64, bool>,
    writes: BTreeMap<u64, PendingWrite>,
    /// Secrets saves in flight, with the reply each one owes and what its
    /// landing moves on.
    saves: BTreeMap<u64, (Option<(u64, Value)>, AfterSave)>,
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
        mut cache: EmailSettings,
        secrets: Secrets,
    ) -> Self {
        cache.reset_legacy_composer_family();
        let mut state = Self {
            previews: crate::previews::Cache::default(),
            input_origin: None,
            accounts: Vec::new(),
            selected,
            mode,
            settings: cache,
            source: Source::Cache,
            epoch: 0,
            held: None,
            secrets,
            unsaved: BTreeMap::new(),
            secrets_version: 0,
            credentials: BTreeMap::new(),
            credential_ops: 0,
            credential_busy: std::collections::BTreeSet::new(),
            credential_queue: BTreeMap::new(),
            generations: 0,
            migration: None,
            migrated: false,
            compose: None,
            attachment_chooser: None,
            documents: crate::documents::DocumentState::default(),
            attachment_notice: None,
            last_attachment_directory: None,
            identity_editor: None,
            pane: Pane::List,
            confirm: None,
            restore: Vec::new(),
            draft_ops: Vec::new(),
            drafts_made: 0,
            discarded: std::collections::BTreeSet::new(),
            sends: BTreeMap::new(),
            quit_ok: false,
            form: None,
            form_note: None,
            ui,
            status: String::new(),
            exiting: false,
            session_mode: design::Mode::Dark,
            work: Vec::new(),
            replies: Vec::new(),
            outstanding: BTreeMap::new(),
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
    /// side, so it works without settingsd). Its password is written to the
    /// secrets file first; only once that save has landed is the account
    /// applied to settingsd.
    pub fn begin_migration(&mut self, entry: AccountEntry, password: &str) {
        self.hold(&entry.id, password);
        let durable = !self.unsaved.contains_key(&entry.id) && self.secrets.has(&entry.id);
        self.migration = Some(Migration {
            adopted: std::collections::BTreeSet::new(),
            applied: false,
            entry,
            stage: if durable { Stage::Ready } else { Stage::Saving },
        });
        if !durable {
            self.save_secrets_then(None, AfterSave::Migration);
        }
        self.reconcile();
        self.config_dirty = false;
    }

    /// Hold `password` for `id` in memory; a change is unsaved until a
    /// save lands.
    fn hold(&mut self, id: &str, password: &str) {
        if self.secrets.set(id, password) {
            self.mark(id);
        }
    }

    /// `id`'s password is unsaved as of a new secrets version.
    fn mark(&mut self, id: &str) {
        self.secrets_version += 1;
        self.unsaved.insert(id.to_owned(), self.secrets_version);
    }

    /// A fresh connection generation.
    fn next_generation(&mut self) -> u64 {
        self.generations += 1;
        self.generations
    }

    /// `id`'s latest credential operation.
    fn credential(&self, id: &str) -> u64 {
        self.credentials.get(id).copied().unwrap_or(0)
    }

    /// A new credential operation on `id`: any write made before it no
    /// longer touches `id`'s password.
    fn credential_op(&mut self, id: &str) {
        self.credential_ops += 1;
        self.credentials.insert(id.to_owned(), self.credential_ops);
    }

    /// The passwords and the version they are at, for the shell to save.
    pub fn secrets_snapshot(&self) -> (u64, Secrets) {
        (self.secrets_version, self.secrets.clone())
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

    /// What the config's accounts cache should hold: the settings in force,
    /// with the accounts kept through an empty read (never the schema-1
    /// account, which the schema-1 file still holds).
    pub fn cache(&self) -> EmailSettings {
        let mut cache = self.settings.clone();
        if let Some(held) = &self.held {
            cache.accounts = held.clone();
        }
        cache
    }

    /// Whether settingsd showed no accounts while some were held, and they
    /// are kept for now.
    pub fn holding(&self) -> bool {
        self.held.is_some()
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

    /// Queue `work` from a Bus command (`commands.rs` builds it).
    pub fn queue_work(&mut self, work: Work) {
        self.queue(work);
    }

    pub(crate) fn ticket(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }

    /// Answer Bus request `id`.
    pub fn reply(&mut self, id: u64, rc: u8, body: Value) {
        self.outstanding.remove(&id);
        self.replies.push(Reply { id, rc, body });
    }

    /// The window is closing: every caller still waiting hears now. A
    /// settings write sent and unanswered, a server write in flight or a
    /// secrets save not confirmed may have happened (OUTCOME_UNKNOWN); a
    /// query, or anything not yet started, is CANCELLED. Nothing more is
    /// started after this.
    pub fn close_out(&mut self) {
        let unknown = crate::label("closing-unknown");
        let cancelled = crate::label("closing-cancelled");
        for (_, pending) in std::mem::take(&mut self.writes) {
            if let Some(reply) = pending.reply {
                if pending.sent.is_some() && !pending.awaiting_look {
                    self.refuse(reply, code::OUTCOME_UNKNOWN, &unknown);
                } else {
                    self.refuse(reply, code::CANCELLED, &cancelled);
                }
            }
        }
        for (_, queue) in std::mem::take(&mut self.credential_queue) {
            for op in queue {
                let reply = match op {
                    CredentialOp::Write { reply, .. } | CredentialOp::Password { reply, .. } => {
                        reply
                    }
                    CredentialOp::Forget { reply } => Some(reply),
                };
                if let Some(reply) = reply {
                    self.refuse(reply, code::CANCELLED, &cancelled);
                }
            }
        }
        self.credential_busy.clear();
        for (_, (owed, _)) in std::mem::take(&mut self.saves) {
            if let Some((reply, _)) = owed {
                self.refuse(reply, code::OUTCOME_UNKNOWN, &unknown);
            }
        }
        for (reply, write) in std::mem::take(&mut self.outstanding) {
            if write {
                self.refuse(reply, code::OUTCOME_UNKNOWN, &unknown);
            } else {
                self.refuse(reply, code::CANCELLED, &cancelled);
            }
        }
        self.documents = crate::documents::DocumentState::default();
        self.work.clear();
    }

    pub(crate) fn await_document(&mut self, id: u64) {
        self.outstanding.insert(id, false);
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
        let mut list = self
            .held
            .clone()
            .unwrap_or_else(|| self.settings.accounts.clone());
        if let Some(Migration { entry: legacy, .. }) = &self.migration
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
    ///
    /// It never forgets a password: only an explicit removal through Email
    /// (`email.account.remove`) or an empty `email.account.password.set`
    /// does. An account removed elsewhere (in Prefs, by a reset) leaves its
    /// entry in the 0600 secrets file, unused.
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
                    if relogin {
                        self.login_changed(&view);
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
        // What is left was removed: its client and view go. A draft open on
        // it is kept and offered again, never dropped.
        for mut gone in old {
            self.disconnect(&mut gone);
            if self
                .compose
                .as_ref()
                .is_some_and(|c| c.account == gone.entry.id)
            {
                self.set_aside_compose();
            }
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
        self.refresh_previews();
    }

    /// `view`'s login (URL, address or CA) changed: a draft open on it is
    /// stale (its new From bound to this connection), and a form open on it
    /// can no longer be submitted.
    fn login_changed(&mut self, view: &AccountView) {
        if let Some(compose) = self.compose.as_mut()
            && compose.account == view.entry.id
        {
            compose.stale = Some(StaleFrom {
                account: view.entry.id.clone(),
                generation: view.generation,
                address: view.entry.email.clone(),
            });
        }
        if self
            .form
            .as_ref()
            .is_some_and(|f| form_account(f) == view.entry.id)
        {
            self.form_note = Some(crate::label("form-account-changed"));
        }
    }

    /// Connect `view` afresh with the password held, or wait for one.
    fn connect(&mut self, view: &mut AccountView) {
        self.disconnect(view);
        let Some(password) = self.secrets.password(&view.entry.id).map(str::to_owned) else {
            view.phase = Phase::NoPassword;
            return;
        };
        view.generation = self.next_generation();
        view.phase = Phase::Connecting;
        self.work.push(Work::Connect {
            account: view.entry.id.clone(),
            generation: view.generation,
            login: Login {
                url: view.entry.url.clone(),
                email: view.entry.email.clone(),
                password,
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
        view.generation = self.next_generation();
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
        self.refresh_previews();
        true
    }

    /// settingsd's `email.*` settings, new to the follower: they are in
    /// force from now on. A migration still to do is sent; writes that
    /// waited for this read are computed against it and sent again.
    ///
    /// An empty account list while accounts are shown is not taken as a
    /// removal of them all (see [`State::holding`]): settingsd omits the
    /// group at its defaults, so an older settingsd, a reset or a fallback
    /// profile look the same. The views stay, the status line says so, and
    /// the next non-empty read or an explicit removal through Email ends
    /// it. Writes are still computed against what settingsd holds.
    pub fn set_settings(&mut self, mut email: EmailSettings) {
        email.reset_legacy_composer_family();
        let page_size = self.settings.mail.page_size;
        let shown = self
            .held
            .clone()
            .unwrap_or_else(|| self.settings.accounts.clone());
        if email.accounts.is_empty() && !shown.is_empty() {
            self.held = Some(shown);
            self.status = crate::label("settings-empty-held");
        } else {
            self.held = None;
        }
        // settingsd came back: the status bar stops saying it is missing,
        // also in the "Accounts not saved: …" form a refused write left.
        // (The look's own status, if any, was set above and stands.)
        if self.source == Source::Missing && self.status.contains(&crate::label("settings-missing"))
        {
            self.status = crate::label("settings-back");
        }
        self.settings = email;
        self.source = Source::Settings;
        self.epoch += 1;
        self.replan_if_gone();
        self.start_migration();
        self.reconcile();
        // A new page size applies to the lists shown.
        if self.settings.mail.page_size != page_size {
            let limit = self.settings.mail.page_size;
            for view in &mut self.accounts {
                if view.phase == Phase::Ready
                    && let Some(mailbox) = view.mail.selected.clone()
                {
                    // Pages of another size: back to the first.
                    view.mail.table.page = 0;
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

    /// Send the migration's apply, once its password is durable and
    /// settingsd has been read.
    fn start_migration(&mut self) {
        let ready = self
            .migration
            .as_ref()
            .filter(|m| m.stage == Stage::Ready)
            .map(|m| m.entry.clone());
        if let (Some(entry), Source::Settings) = (ready, self.source) {
            if let Some(m) = self.migration.as_mut() {
                m.stage = Stage::Applying;
            }
            self.write(Change::Migrate(entry), None, None, true, None);
        }
    }

    /// The migration is finished: settingsd holds the account and its
    /// password is durable under the id settingsd holds it as. The shell
    /// rewrites the schema-1 config now, and not before.
    fn finish_migration(&mut self) {
        if let Some(m) = self.migration.as_mut() {
            m.stage = Stage::Rewriting;
        }
        self.migrated = true;
    }

    /// The shell rewrote the config as schema 2 (`Ok(true)`), or could
    /// not. Only a confirmed rewrite ends the migration and its lock; a
    /// failed one keeps both, the schema-1 file stays, and the next launch
    /// runs the migration again.
    pub fn migration_rewritten(&mut self, result: Result<bool, String>) {
        if self.migration.as_ref().map(|m| m.stage) != Some(Stage::Rewriting) {
            return;
        }
        match result {
            Ok(true) => {
                self.migration = None;
                self.status = crate::label("migrated");
                self.reconcile();
            }
            Ok(false) => self.stop_migration(&crate::label("config-refused")),
            Err(error) => self.stop_migration(&error),
        }
    }

    /// A migration step failed: it stops here this run; the schema-1 file
    /// stays as it is and the next launch tries again.
    fn stop_migration(&mut self, message: &str) {
        if let Some(m) = self.migration.as_mut() {
            m.stage = Stage::Stopped;
        }
        self.status = crate::label_with("migration-stopped", &[("message", message)]);
    }

    /// A settings read answered with nothing new (the follower had seen
    /// it): writes waiting on a read are computed against what is in force
    /// and sent again, so none waits forever.
    pub fn settings_unchanged(&mut self) {
        if self.source != Source::Settings {
            return;
        }
        // A migration waiting to be (re)sent goes too.
        self.start_migration();
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

    /// Write `op`'s deadline passed: if it is still unsettled (sent but not
    /// answered, or waiting on a read that never came) its caller hears so.
    pub fn write_deadline(&mut self, op: u64) {
        let Some(ticket) = self
            .writes
            .iter()
            .find(|(_, w)| w.op == op)
            .map(|(ticket, _)| *ticket)
        else {
            return;
        };
        if let Some(pending) = self.writes.remove(&ticket) {
            self.write_failed(pending, code::TIMEOUT, &crate::label("settings-timeout"));
        }
    }

    /// Remove `id`, shown only because settingsd showed no accounts: the
    /// held entry goes (the others stay) and its secret is forgotten
    /// durably before `reply` hears rc 0.
    fn remove_held(&mut self, id: &str, reply: Option<u64>, holds: Option<String>) {
        if let Some(held) = self.held.as_mut() {
            held.retain(|a| a.id != id);
            if held.is_empty() {
                self.held = None;
            }
        }
        self.credential_op(id);
        let body = json!({ "id": id });
        let forgot = self.secrets.set(id, "");
        if forgot {
            self.mark(id);
        }
        self.reconcile();
        let then = holds
            .clone()
            .map_or(AfterSave::Nothing, AfterSave::Credential);
        if forgot || self.unsaved.contains_key(id) {
            self.save_secrets_then(reply.map(|reply| (reply, body)), then);
        } else {
            if let Some(reply) = reply {
                self.reply(reply, 0, body);
            }
            if let Some(id) = &holds {
                self.credential_done(id);
            }
        }
    }

    /// Whether `id` is the account still migrating from the schema-1
    /// config (or the one settingsd holds it as): its password may not be
    /// changed or forgotten until the migration finishes, or a restart
    /// would bring the old one back.
    pub fn migration_locks(&self, id: &str) -> bool {
        let Some(Migration { entry, adopted, .. }) = &self.migration else {
            return false;
        };
        entry.id == id
            || adopted.contains(id)
            || self.settings.accounts.iter().any(|a| {
                a.id == id && a.url == entry.url && a.email.eq_ignore_ascii_case(&entry.email)
            })
    }

    /// The account settingsd holds as the migrating one: the same URL and
    /// address (case aside).
    fn holds_legacy(&self, account: &AccountEntry) -> bool {
        self.migration.as_ref().is_some_and(|m| {
            account.url == m.entry.url && account.email.eq_ignore_ascii_case(&m.entry.email)
        })
    }

    /// settingsd no longer lists the account an applied migration put
    /// there (Prefs removed it directly): the migration starts over against
    /// the current settings, so the account comes back rather than its
    /// password being lost.
    fn replan_if_gone(&mut self) {
        let applied = self.migration.as_ref().is_some_and(|m| m.applied);
        if !applied || self.held.is_some() {
            return;
        }
        let present = self.settings.accounts.iter().any(|a| self.holds_legacy(a));
        if !present {
            self.replan();
        }
    }

    /// Plan the migration again: back to Ready, sent on this read.
    fn replan(&mut self) {
        if let Some(m) = self.migration.as_mut() {
            m.stage = Stage::Ready;
            m.applied = false;
        }
        self.status = crate::label("migration-still-running");
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
                self.write_failed(
                    pending,
                    code::UNAVAILABLE,
                    &crate::label("settings-missing"),
                );
            }
        }
    }

    /// Write `change` to settingsd; `reply` is answered once it is settled.
    /// `password` is held for the account once settingsd has it.
    pub fn change_accounts(
        &mut self,
        change: Change,
        reply: Option<u64>,
        password: Option<String>,
    ) {
        let credential = password.is_some() || matches!(change, Change::Remove(_));
        match change.account().map(str::to_owned) {
            Some(id) if credential => self.credential_start(
                &id,
                CredentialOp::Write {
                    change,
                    reply,
                    password,
                },
            ),
            _ => self.write(change, reply, password, false, None),
        }
    }

    /// Run credential operation `op` on `id` now, or after the one in
    /// flight there.
    fn credential_start(&mut self, id: &str, op: CredentialOp) {
        if self.credential_busy.contains(id) {
            self.credential_queue
                .entry(id.to_owned())
                .or_default()
                .push_back(op);
            return;
        }
        self.credential_busy.insert(id.to_owned());
        self.credential_run(id, op);
    }

    fn credential_run(&mut self, id: &str, op: CredentialOp) {
        match op {
            CredentialOp::Write {
                change,
                reply,
                password,
            } => self.write(change, reply, password, false, Some(id.to_owned())),
            CredentialOp::Password { password, reply } => self.password_now(id, &password, reply),
            CredentialOp::Forget { reply } => self.forget_now(id, reply),
        }
    }

    /// `id`'s credential operation settled: the next one runs.
    fn credential_done(&mut self, id: &str) {
        let next = self
            .credential_queue
            .get_mut(id)
            .and_then(std::collections::VecDeque::pop_front);
        match next {
            Some(op) => self.credential_run(id, op),
            None => {
                self.credential_busy.remove(id);
                self.credential_queue.remove(id);
            }
        }
    }

    fn write(
        &mut self,
        change: Change,
        reply: Option<u64>,
        password: Option<String>,
        migration: bool,
        holds: Option<String>,
    ) {
        // #17: removing an account shown only because settingsd showed none
        // (it is no longer in settingsd) removes it here, and its secret.
        if let Change::Remove(id) = &change
            && self.settings.account(id).is_none()
            && self
                .held
                .as_ref()
                .is_some_and(|h| h.iter().any(|a| a.id == *id))
        {
            let id = id.clone();
            return self.remove_held(&id, reply, holds);
        }
        let credential = change.account().map_or(0, |id| self.credential(id));
        let op = self.ticket();
        self.queue(Work::WriteDeadline { op });
        let pending = PendingWrite {
            change,
            reply,
            password,
            attempts: 0,
            awaiting_look: false,
            credential,
            op,
            holds,
            migration,
            sent: None,
            base: 0,
        };
        match self.source {
            Source::Missing => {
                self.write_failed(
                    pending,
                    code::UNAVAILABLE,
                    &crate::label("settings-missing"),
                );
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
        pending.base = self.epoch;
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
                    (
                        self.settings.clone(),
                        pending.change.account().map(str::to_owned),
                    )
                });
                // In force at once when nothing newer was read since it was
                // sent; otherwise the newer read stands and the read this
                // asks for brings the change (a prediction never goes over
                // a newer authoritative read).
                let current = pending.base == self.epoch;
                self.queue(Work::ReadSettings);
                self.write_done(pending, about, current.then_some(next));
            }
            // A newer read is already in force: compute against it now.
            Settled::Conflict if pending.attempts < WRITE_ATTEMPTS && self.epoch > pending.base => {
                self.send_write(pending);
            }
            // Otherwise the next read releases it, an unchanged one too.
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
                    self.stop_migration(&message);
                }
                self.write_failed(pending, code::REFUSED, &message);
            }
            Settled::Unavailable(message) => {
                self.write_failed(pending, code::UNAVAILABLE, &message)
            }
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
            self.migration_applied(about.as_deref());
            return;
        }
        // An explicit removal through Email: the views go even if it was the
        // last account, and its password is forgotten.
        let mut save = false;
        let current = |state: &State, id: &str| state.credential(id) == pending.credential;
        if let Change::Remove(id) = &pending.change {
            self.held = None;
            // Unless a newer password or forget came since (that one stands).
            if current(self, id) {
                self.credential_op(id);
                if self.secrets.set(id, "") {
                    self.mark(id);
                    save = true;
                } else if self.unsaved.contains_key(id) {
                    // Gone from memory, perhaps not from disk: save again.
                    save = true;
                }
            }
        }
        // A new password, or one held but not yet durably saved: an account
        // already shown reconnects with it; a new one connects with it in
        // the reconcile.
        let mut renewed = None;
        if let (Some(id), Some(password)) = (&about, &pending.password)
            && current(self, id)
            && (self.secrets.password(id) != Some(password.as_str())
                || self.unsaved.contains_key(id))
        {
            let changed = self.secrets.password(id) != Some(password.as_str());
            self.credential_op(id);
            self.hold(id, password);
            self.mark(id);
            save = true;
            renewed = changed.then(|| (id.clone(), self.account(id).is_some()));
        }
        self.reconcile();
        if let Some((id, true)) = &renewed {
            self.reconnect(id);
        }
        let then = pending
            .holds
            .clone()
            .map_or(AfterSave::Nothing, AfterSave::Credential);
        if save {
            self.save_secrets_then(pending.reply.map(|reply| (reply, body)), then);
        } else {
            if let Some(reply) = pending.reply {
                self.reply(reply, 0, body);
            }
            if let Some(id) = &pending.holds {
                self.credential_done(id);
            }
        }
    }

    /// settingsd took the migrated account (as `id`). When it holds it
    /// under another id, the password is written under that one first; the
    /// migration finishes once the password is durable where settingsd
    /// will look for it.
    fn migration_applied(&mut self, id: Option<&str>) {
        let Some(legacy) = self.migration.as_ref().map(|m| m.entry.id.clone()) else {
            return;
        };
        let adopted = id.unwrap_or(&legacy).to_owned();
        if let Some(m) = self.migration.as_mut() {
            m.applied = true;
        }
        if adopted != legacy {
            // Still the account the legacy one was matched with (same URL
            // and address)? Otherwise the password never moves to it.
            let same = self
                .settings
                .account(&adopted)
                .is_some_and(|a| self.holds_legacy(a));
            if !same {
                self.replan();
                return self.start_migration();
            }
            if let Some(m) = self.migration.as_mut() {
                m.adopted.insert(adopted.clone());
            }
        }
        if adopted == legacy || (self.secrets.has(&adopted) && !self.unsaved.contains_key(&adopted))
        {
            return self.finish_migration();
        }
        let Some(password) = self.secrets.password(&legacy).map(str::to_owned) else {
            return self.stop_migration(&crate::label("migration-no-password"));
        };
        if !self.secrets.has(&adopted) {
            self.hold(&adopted, &password);
        }
        if let Some(m) = self.migration.as_mut() {
            m.stage = Stage::Transferring;
        }
        self.save_secrets_then(None, AfterSave::Transfer);
        self.reconcile();
        self.reconnect(&adopted);
    }

    fn write_failed(&mut self, pending: PendingWrite, code: &str, message: &str) {
        // A migration that did not land is sent again on the next read.
        if pending.migration
            && let Some(m) = self.migration.as_mut()
            && m.stage == Stage::Applying
        {
            m.stage = Stage::Ready;
        }
        self.status = crate::label_with("settings-failed", &[("message", message)]);
        if let Some(reply) = pending.reply {
            self.refuse(reply, code, message);
        }
        if let Some(id) = &pending.holds {
            self.credential_done(id);
        }
    }

    /// Hold `password` for account `id` and reconnect it, answering `reply`
    /// once the secrets file has it.
    pub fn set_password(&mut self, id: &str, password: &str, reply: Option<u64>) -> bool {
        if self.account(id).is_none() {
            return false;
        }
        let password = password.to_owned();
        self.credential_start(id, CredentialOp::Password { password, reply });
        true
    }

    fn password_now(&mut self, id: &str, password: &str, reply: Option<u64>) {
        self.credential_op(id);
        self.hold(id, password);
        self.mark(id);
        let body = json!({ "id": id, "has_password": !password.is_empty() });
        self.save_secrets_then(
            reply.map(|reply| (reply, body)),
            AfterSave::Credential(id.to_owned()),
        );
        self.reconnect(id);
    }

    /// Forget account `id`'s password, whether or not an account has that
    /// id now, answering `reply` with `forgotten` (whether one was held)
    /// once the secrets file no longer has it.
    pub fn forget_password(&mut self, id: &str, reply: u64) {
        self.credential_start(id, CredentialOp::Forget { reply });
    }

    fn forget_now(&mut self, id: &str, reply: u64) {
        let body =
            |forgotten: bool| json!({ "id": id, "has_password": false, "forgotten": forgotten });
        self.credential_op(id);
        if !self.secrets.has(id) && !self.unsaved.contains_key(id) {
            self.reply(reply, 0, body(false));
            return self.credential_done(id);
        }
        let held = self.secrets.has(id);
        self.hold(id, "");
        self.mark(id);
        self.save_secrets_then(
            Some((reply, body(held))),
            AfterSave::Credential(id.to_owned()),
        );
        // A shown account has no password from now on.
        if self.account(id).is_some() {
            self.reconnect(id);
        }
    }

    /// Queue a secrets save that answers `reply` when it lands and then
    /// moves `then` on.
    fn save_secrets_then(&mut self, reply: Option<(u64, Value)>, then: AfterSave) {
        let ticket = self.ticket();
        self.saves.insert(ticket, (reply, then));
        self.queue(Work::SaveSecrets { ticket });
    }

    /// Secrets save `ticket` finished, of the passwords as they were at
    /// `version` (the shell saves a snapshot off the UI thread): what had
    /// changed by then is durable, a later change stays unsaved. A poison
    /// pill refusal (`Ok(false)`) is a failure like any other. The error
    /// never carries a password.
    pub fn secrets_saved(&mut self, ticket: u64, version: u64, result: Result<bool, String>) {
        let Some((owed, then)) = self.saves.remove(&ticket) else {
            return;
        };
        let refused = match result {
            Ok(true) => None,
            Ok(false) => Some(crate::label("secrets-refused")),
            Err(error) => Some(error),
        };
        match &refused {
            None => self.unsaved.retain(|_, changed| *changed > version),
            Some(message) => {
                self.status = crate::label_with("secrets-failed", &[("message", message)]);
            }
        }
        let release = match &then {
            AfterSave::Credential(id) => Some(id.clone()),
            _ => None,
        };
        match (then, &refused) {
            (AfterSave::Migration, None) => {
                if let Some(m) = self.migration.as_mut()
                    && m.stage == Stage::Saving
                {
                    m.stage = Stage::Ready;
                }
                self.start_migration();
            }
            // Only the transfer the migration is still waiting on.
            (AfterSave::Transfer, None)
                if self.migration.as_ref().map(|m| m.stage) == Some(Stage::Transferring) =>
            {
                self.finish_migration()
            }
            (AfterSave::Transfer, None) => {}
            (AfterSave::Migration | AfterSave::Transfer, Some(message)) => {
                let message = message.clone();
                self.stop_migration(&message);
            }
            (AfterSave::Nothing | AfterSave::Credential(_), _) => {}
        }
        if let Some((reply, body)) = owed {
            match refused {
                None => self.reply(reply, 0, body),
                Some(message) => self.refuse(reply, code::SECRETS, &message),
            }
        }
        // The account's next credential operation runs after this one's
        // caller has heard.
        if let Some(id) = release {
            self.credential_done(&id);
        }
    }

    /// Apply one shell completion.
    pub fn apply(&mut self, event: crate::shell::Event) {
        use crate::shell::{Done, Event};
        match event {
            Event::Preview(ticket, result) => {
                self.refresh_previews();
                self.previews.complete(ticket, result);
            }
            Event::MailDocument(ticket, answer) => self.document_done(ticket, answer),
            Event::ComposeAttachments(inspection, result) => {
                self.compose_attachments_inspected(inspection, result);
            }
            Event::Connected(tag, result) => {
                self.connected(tag, result);
                self.check_compose_attachment_size();
            }
            // A Bus request is answered whatever became of the account
            // since: only the view's update depends on it.
            Event::Account(tag, Done::Answered(reply, then, result)) => {
                self.answered(&tag, reply, then, result);
            }
            Event::Account(tag, Done::Wrote(reply, form, write, result)) => {
                self.wrote(&tag, reply, form, *write, result);
            }
            Event::Account(tag, Done::Batched(batch, outcome)) => {
                self.batched(&tag, *batch, outcome);
            }
            Event::Account(tag, Done::Ids(ticket, result)) => {
                self.ids_arrived(&tag, ticket, result);
            }
            // A send is settled by its own ticket even when the account has
            // reconnected since: the person must hear whether it went.
            Event::Account(tag, Done::Sent(ticket, attempt, result)) => {
                self.sent(&tag, ticket, attempt, result)
            }
            // A save is answered whatever became of the connection: the
            // file is on disk (or not) either way.
            Event::Account(tag, Done::Attachment(done)) => self.attachment_done(&tag, *done),
            Event::Account(tag, Done::Opened(ticket, Some(reply), result)) => {
                // The caller hears "loaded" only if the reader will show it:
                // the same connection, and still the latest open.
                let view = self.account(&tag.account);
                let current = view.is_some_and(|v| v.generation == tag.generation);
                let latest = view.is_some_and(|v| {
                    v.tickets.open == ticket && v.tickets.open_mailbox == v.mail.selected
                });
                match result.as_ref() {
                    _ if !current => {
                        self.refuse(reply, code::STALE, &crate::label("stale-connection"))
                    }
                    _ if !latest => {
                        self.refuse(reply, code::SUPERSEDED, &crate::label("open-superseded"))
                    }
                    Ok(email) => self.reply(
                        reply,
                        0,
                        json!({ "account": tag.account, "id": email.id, "subject": email.subject }),
                    ),
                    Err(error) => {
                        let (code, message) = crate::commands::server_refusal(error);
                        self.refuse(reply, code, &message);
                    }
                }
                self.apply(Event::Account(tag, Done::Opened(ticket, None, result)));
            }
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
                            self.refresh_previews();
                        }
                    }
                    Done::Page(ticket, _) if ticket != view.tickets.page => {}
                    Done::Page(_, Ok(page)) => view.page_arrived(page, limit, &mut self.work),
                    // A server that refuses the chosen sort: back to its
                    // own order (newest first), saying so.
                    Done::Page(_, Err(jmap::Error::Method { error_type, .. }))
                        if error_type == "unsupportedSort" && view.mail.table.sort.is_some() =>
                    {
                        view.mail.table.sort = None;
                        view.mail.table.page = 0;
                        self.status = crate::label("sort-unsupported");
                        if let Some(mailbox) = view.mail.selected.clone() {
                            view.fetch_emails(mailbox, limit, &mut self.work);
                        }
                    }
                    Done::Page(_, Err(error)) => {
                        view.mail.busy = false;
                        self.status = error.to_string();
                    }
                    Done::Opened(ticket, _, _)
                        if ticket != view.tickets.open
                            || view.tickets.open_mailbox != view.mail.selected => {}
                    Done::Opened(ticket, _, result) => {
                        match *result {
                            Ok(email) => {
                                view.mail.reader.open(&email);
                                // Another email: its own attachment transfers.
                                if view.mail.open.as_ref().is_none_or(|o| o.id != email.id) {
                                    view.mail.transfers.clear();
                                }
                                view.mail.open = Some(email);
                                view.mail.open_ticket = ticket;
                            }
                            Err(ref error) => {
                                // Nothing is loading now: the email shown stays
                                // the one its actions answer, and the failed row
                                // can be clicked again to retry.
                                view.tickets.open_requested =
                                    view.mail.open.as_ref().map(|e| e.id.clone());
                                self.status = error.to_string();
                            }
                        }
                        self.refresh_previews();
                    }
                    Done::Marked(Ok(())) => self.work.push(Work::FetchMailboxes {
                        account: tag.account.clone(),
                    }),
                    // Settled above, before the generation check.
                    Done::Sent(..) => {}
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
                    // Answered above, before the generation check.
                    Done::Answered(..)
                    | Done::Wrote(..)
                    | Done::Batched(..)
                    | Done::Ids(..)
                    | Done::Attachment(..) => {}
                    Done::Suggested(ticket, found) => {
                        let contacts = found.unwrap_or_default();
                        let recent = view.mail.recent.clone();
                        if let Some(compose) = self.compose.as_mut()
                            && compose.account == tag.account
                            && let Some(suggest) = compose.suggest.as_mut()
                            && suggest.ticket == ticket
                        {
                            let text = match suggest.field {
                                ComposeField::To => &compose.to_text,
                                ComposeField::Cc => &compose.cc_text,
                                ComposeField::Bcc => &compose.bcc_text,
                                _ => return,
                            };
                            let found: Vec<_> = contacts
                                .iter()
                                .flat_map(crate::suggest::from_contact)
                                .collect();
                            suggest.items = crate::suggest::rank(
                                found,
                                &recent,
                                &suggest.query,
                                &crate::suggest::present(text),
                                crate::suggest::LIMIT,
                            );
                            suggest.cursor =
                                suggest.cursor.min(suggest.items.len().saturating_sub(1));
                        }
                    }
                }
            }
            Event::Settled(ticket, answer) => self.settled(ticket, answer),
            Event::WriteDeadline(op) => self.write_deadline(op),
            Event::SecretsSaved(ticket, version, result) => {
                self.secrets_saved(ticket, version, result)
            }
            Event::Delivery(..)
            | Event::Look(..)
            | Event::LookRetry(_)
            | Event::Shown(..)
            | Event::PrefsOpened(_) => {}
            Event::Printed(reply, result) => self.printed(reply, result),
            Event::SuggestDue(ticket) => self.suggest_due(ticket),
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
                // The status bar reads each account's connection from its
                // own phase; the status line is for operations only.
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
            // The account's own pane and its tree row say so; the status
            // line stays about the account shown.
            Err(error) => view.phase = Phase::Failed(error.to_string()),
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

    /// Make `id` the current account, fetching nothing: its caller has.
    fn focus(&mut self, id: &str) {
        if self.selected.as_deref() != Some(id) {
            self.selected = Some(id.to_owned());
            self.config_dirty = true;
        }
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
        self.focus(account);
        self.set_mode_only(Mode::Mail);
        self.refresh_previews();
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
        self.focus(account);
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
        self.focus(account);
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
        // The list's keyboard row is the row it opened; a row outside the
        // selection becomes the selection, one inside it keeps it.
        let index = view.mail.emails.iter().position(|e| e.id == id);
        if !view.mail.table.is_selected(&id) {
            view.mail.table.selection = vec![id.clone()];
            view.mail.everything = false;
        }
        view.mail.table.focus = index;
        view.open_email(id, None, &mut self.work);
        true
    }

    /// The message list reported `event` on `account`'s view. Selecting
    /// a plain click picks one row and opens it; Ctrl+click toggles one,
    /// Shift+click and Shift with an arrow take a range, Ctrl+A the page,
    /// Escape collapses to the keyboard's row, and Up and Down move the
    /// keyboard's row and the selection. Only a plain click opens among the
    /// selection gestures; Enter and a double-click also open (activation). "Select all N"
    /// selects every email of the mailbox's query; a dragged width is
    /// remembered. A header click or a pager button asks the server for
    /// that sort or page and clears the selection.
    pub fn mail_table(&mut self, account: &str, event: TableEvent) {
        let limit = self.settings.mail.page_size;
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return;
        };
        match &event {
            TableEvent::Activated { key, .. } => {
                let opened = view.mail.open.as_ref().is_some_and(|e| e.id == *key)
                    || view.tickets.open_requested.as_deref() == Some(key.as_str());
                if !view.mail.table.is_selected(key) {
                    view.mail.everything = false;
                    view.cancel_fetch();
                }
                view.mail.table.apply(&event, SelectionMode::Multi);
                if !opened {
                    self.open_email(account, key.clone());
                }
            }
            // A plain click opens its row, as other mail programs do; Ctrl+click
            // and Shift+click only select. A new selection drops a "Select all N"
            // fetch still loading: it was for the old one.
            TableEvent::Selected { key, pick, .. } => {
                let key = key.clone();
                let plain = matches!(pick, toolkit::datatable::Pick::Only);
                let opened = view.mail.open.as_ref().is_some_and(|e| e.id == key)
                    || view.tickets.open_requested.as_deref() == Some(key.as_str());
                view.cancel_fetch();
                view.mail.table.apply(&event, SelectionMode::Multi);
                view.mail.everything = false;
                if plain && !opened {
                    self.open_email(account, key);
                }
            }
            TableEvent::SelectionChanged { .. } => {
                view.cancel_fetch();
                view.mail.table.apply(&event, SelectionMode::Multi);
                view.mail.everything = false;
            }
            // The right-clicked row's menu: its Reply, Forward and Send as
            // New are for that email, so it opens (a row outside the
            // selection was selected and opened just before). A selection
            // it is part of stays, for the row actions.
            TableEvent::ContextMenu { key, .. } => {
                let opened = view.mail.open.as_ref().is_some_and(|e| e.id == *key)
                    || view.tickets.open_requested.as_deref() == Some(key.as_str());
                if !opened {
                    self.open_email(account, key.clone());
                }
            }
            // Offered only once the whole page is selected: the count is
            // the mailbox's total, which the list already has (its ids are
            // fetched only when an action needs them).
            TableEvent::SelectEverything => {
                view.mail.table.selection = view.mail.keys();
                view.mail.everything = true;
            }
            TableEvent::ColumnResized { .. } => {
                view.mail.table.apply(&event, SelectionMode::Multi);
            }
            // A header click or the pager: the server sorts and pages.
            TableEvent::SortChanged(_) | TableEvent::PageChanged(_) => {
                view.requery(&event, limit, &mut self.work);
            }
            // No page size picker, menu or drag in this list yet.
            _ => {}
        }
    }

    /// `email.open`: show `account`'s email `id` in the reader, whether or
    /// not the shown page holds it, answering `reply` once it loads.
    pub fn open_by_bus(&mut self, account: &str, id: String, reply: u64) {
        self.focus(account);
        self.set_mode_only(Mode::Mail);
        let mark = self.settings.mail.mark_read_on_open;
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return;
        };
        if mark && view.mail.emails.iter().any(|e| e.id == id && !e.is_read()) {
            view.set_keyword(std::slice::from_ref(&id), "$seen", true);
            self.work.push(Work::MarkRead {
                account: account.to_owned(),
                id: id.clone(),
                read: true,
            });
        }
        view.open_email(id, Some(reply), &mut self.work);
        self.outstanding.insert(reply, false);
    }

    /// Ask `account`'s server `query` for Bus request `reply`.
    pub fn query(
        &mut self,
        account: String,
        reply: u64,
        query: crate::commands::Query,
        then: crate::commands::Then,
    ) {
        self.outstanding.insert(reply, false);
        self.queue(Work::Query {
            account,
            reply,
            query,
            then,
        });
    }

    /// Make a change on `account`'s server; `reply` (a Bus request) is
    /// answered with what it made.
    pub fn write_server(
        &mut self,
        account: String,
        reply: Option<u64>,
        write: crate::commands::Write,
    ) {
        if let Some(reply) = reply {
            self.outstanding.insert(reply, true);
        }
        self.queue(Work::Write {
            account,
            reply,
            form: None,
            expect: None,
            write,
        });
    }

    /// A form's change on `account`'s server, under operation `op`.
    fn write_form(&mut self, account: String, op: u64, write: crate::commands::Write) {
        let generation = self.generation_of(&account);
        self.write_bound(account, Some(op), generation, write);
    }

    /// A form's change, sent only through connection `generation`: the
    /// shell refuses it as stale on any other.
    fn write_bound(
        &mut self,
        account: String,
        form: Option<u64>,
        generation: u64,
        write: crate::commands::Write,
    ) {
        self.queue(Work::Write {
            account,
            reply: None,
            form,
            expect: Some(generation),
            write,
        });
    }

    /// A Bus query answered: the reply goes out, or the compose it was
    /// for opens.
    pub fn answered(
        &mut self,
        tag: &Tag,
        reply: u64,
        then: crate::commands::Then,
        result: Result<Value, jmap::Error>,
    ) {
        use crate::commands::Then;
        // An answer from a connection the account has left since (a new URL
        // or login) is not the account's answer any more.
        if self.account(&tag.account).map(AccountView::generation) != Some(tag.generation) {
            return self.refuse(reply, code::STALE, &crate::label("stale-connection"));
        }
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                let (code, message) = crate::commands::server_refusal(&error);
                return self.refuse(reply, code, &message);
            }
        };
        if let Then::Suggest { text, limit } = &then {
            let contacts: Vec<jmap::Contact> = value
                .get("contacts")
                .cloned()
                .and_then(|list| serde_json::from_value(list).ok())
                .unwrap_or_default();
            let recent = self
                .account(&tag.account)
                .map(|v| v.mail.recent.clone())
                .unwrap_or_default();
            let found: Vec<_> = contacts
                .iter()
                .flat_map(crate::suggest::from_contact)
                .collect();
            let ranked = crate::suggest::rank(found, &recent, text.trim(), &[], *limit);
            return self.reply(reply, 0, json!({ "suggestions": ranked }));
        }
        let email = match then {
            Then::Answer => return self.reply(reply, 0, value),
            _ => serde_json::from_value::<Email>(value),
        };
        let email = match email {
            Ok(email) => email,
            Err(error) => return self.refuse(reply, code::SERVER, &error.to_string()),
        };
        // The email came over a connection the account may have left since
        // (a new URL or login): it never opens under the new identity.
        let account = tag.account.as_str();
        if self.account(account).map(AccountView::generation) != Some(tag.generation) {
            return self.refuse(reply, code::STALE, &crate::label("stale-connection"));
        }
        // Attachment queries and saves need no compose either.
        match &then {
            Then::Attachments => {
                let list = attachments_view(&email, None);
                return self.reply(reply, 0, json!({ "id": email.id, "attachments": list }));
            }
            Then::SaveAttachment { part, dir } => {
                let (part, dir) = (part.clone(), dir.clone());
                return match attachment_work(account, &email, &part, false, dir, Some(reply)) {
                    Ok(work) => self.queue(work),
                    Err((code, message)) => self.refuse(reply, code, &message),
                };
            }
            _ => {}
        }
        // A print needs no compose: it goes ahead whatever is open.
        if then == Then::Print {
            let markdown = crate::reader::detect(&email) == crate::reader::Format::Markdown;
            return self.print_email(&email, markdown, Some(reply));
        }
        if self.compose.is_some() {
            return self.refuse(reply, code::BUSY, &crate::label("compose-open"));
        }
        let opened = match then {
            Then::Print
            | Then::Attachments
            | Then::SaveAttachment { .. }
            | Then::Suggest { .. } => false,
            Then::Reply(all, quote) => self.reply_to(account, &email, all, quote),
            Then::Forward(to) => self.forward(account, &email, &to),
            Then::Answer => false,
        };
        if opened {
            if let Some(error) = self.close_failed_compose() {
                return self.refuse(reply, code::INVALID_ARGUMENT, &error);
            }
            let subject = self.compose.as_ref().map(|c| c.draft.subject.clone());
            self.reply(
                reply,
                0,
                json!({ "composing": account, "subject": subject }),
            );
        } else {
            self.refuse(reply, code::UNKNOWN_ACCOUNT, &crate::label("no-account"));
        }
    }

    /// A server write finished: the view it changed is fetched again (the
    /// moved or deleted emails leave the list at once), a form waiting on
    /// it closes, and `reply` hears what the server made.
    pub fn wrote(
        &mut self,
        tag: &Tag,
        reply: Option<u64>,
        form: Option<u64>,
        write: crate::commands::Write,
        result: Result<Value, jmap::Error>,
    ) {
        // Only the form that sent this write (by its operation ticket)
        // hears of it.
        let mine = form.is_some()
            && match &self.form {
                Some(Form::Event(f)) => f.op == form,
                Some(Form::Contact(f)) => f.op == form,
                _ => false,
            };
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                let message = error.to_string();
                if mine {
                    match self.form.as_mut() {
                        Some(Form::Event(f)) => {
                            f.busy = false;
                            f.op = None;
                            f.error = Some(message.clone());
                        }
                        Some(Form::Contact(f)) => {
                            f.busy = false;
                            f.op = None;
                            f.error = Some(message.clone());
                        }
                        _ => {}
                    }
                }
                self.status = message;
                if let Some(reply) = reply {
                    let (code, message) = crate::commands::server_refusal(&error);
                    self.refuse(reply, code, &message);
                }
                return;
            }
        };
        let limit = self.settings.mail.page_size;
        // The row to open once the open email has gone (delete, move, junk).
        let mut advance = None;
        if let Some(view) = self
            .accounts
            .iter_mut()
            .find(|v| v.entry.id == tag.account && v.generation == tag.generation)
        {
            match write.touches() {
                Mode::Mail => {
                    // A batched write names the ids it changed: an id that
                    // failed stays in the list.
                    let done: Option<Vec<&str>> = value
                        .get("done")
                        .and_then(Value::as_array)
                        .map(|list| list.iter().filter_map(Value::as_str).collect());
                    let gone: Vec<String> = write
                        .removes()
                        .iter()
                        .filter(|id| done.as_ref().is_none_or(|d| d.contains(&id.as_str())))
                        .cloned()
                        .collect();
                    let gone = gone.as_slice();
                    let failed: Vec<(String, String)> = value
                        .get("failed")
                        .and_then(Value::as_array)
                        .map(|list| {
                            list.iter()
                                .map(|f| {
                                    let field =
                                        |k: &str| f[k].as_str().unwrap_or_default().to_owned();
                                    (field("id"), field("reason"))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if !failed.is_empty() {
                        let reasons = failed
                            .iter()
                            .map(|(id, why)| format!("{id} ({why})"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        self.status = crate::label_with(
                            "batch-failed",
                            &[
                                ("failed", &failed.len().to_string()),
                                ("count", &write.ids().len().to_string()),
                                ("reasons", &reasons),
                            ],
                        );
                    }
                    if let Some(open) = view.mail.open.as_ref().filter(|e| gone.contains(&e.id)) {
                        advance = neighbour(&view.mail.emails, &open.id, gone);
                    }
                    view.mail.emails.retain(|e| !gone.contains(&e.id));
                    view.mail.table.rows_changed(&view.mail.keys());
                    // The whole mailbox is no longer what is selected.
                    if !gone.is_empty() {
                        view.mail.everything = false;
                    }
                    if view
                        .mail
                        .open
                        .as_ref()
                        .is_some_and(|e| gone.contains(&e.id))
                    {
                        view.mail.open = None;
                        view.tickets.open_requested = None;
                    }
                    self.work.push(Work::FetchMailboxes {
                        account: tag.account.clone(),
                    });
                    if let Some(mailbox) = view.mail.selected.clone() {
                        view.fetch_emails(mailbox, limit, &mut self.work);
                    }
                }
                Mode::Calendar => {
                    if let Some(calendar) = view.calendars.selected.clone() {
                        view.fetch_events(calendar, &mut self.work);
                    }
                }
                Mode::Contacts => {
                    if let Some(book) = view.contacts.selected.clone() {
                        let filter = view.contacts.filter.clone();
                        view.fetch_contacts(book, filter, &mut self.work);
                    }
                }
            }
        }
        self.refresh_previews();
        // The open email left: the reader moves on to its neighbour, as a
        // row click would open it.
        if let Some(id) = advance {
            self.open_email(&tag.account, id);
        }
        if mine {
            self.form = None;
            self.status = crate::label("saved");
        }
        if let Some(reply) = reply {
            self.reply(reply, 0, value);
        }
    }

    /// A send finished. Its compose is settled by the send's own ticket
    /// whatever became of the account's connection since, so a send that
    /// went (or failed) is always reported; the views are refreshed only
    /// when the connection is still the one that sent.
    ///
    /// A message that went has its local draft removed, whether or not its
    /// compose is still open. One that did not keeps its draft, with what
    /// the try did on the server (`attempt`), so the next Send resumes it
    /// rather than sending a second copy.
    fn sent(
        &mut self,
        tag: &Tag,
        ticket: u64,
        attempt: jmap::SendAttempt,
        result: Result<jmap::SendReport, jmap::Error>,
    ) {
        let mine = self.compose.as_ref().is_some_and(|compose| {
            compose.account == tag.account && compose.sending == Some(ticket)
        });
        let snapshot = self.sends.remove(&ticket);
        match result {
            Ok(report) => {
                if let Some(draft) = &snapshot {
                    self.discard_draft(&draft.id);
                }
                if mine {
                    self.compose = None;
                    self.confirm = None;
                }
                self.status = match report.unfiled {
                    Some(error) => crate::label_with("sent-unfiled", &[("error", &error)]),
                    None if report.earlier => crate::label("sent-earlier"),
                    None => crate::label("sent"),
                };
                let limit = self.settings.mail.page_size;
                if let Some(view) = self
                    .accounts
                    .iter_mut()
                    .find(|v| v.entry.id == tag.account && v.generation == tag.generation)
                {
                    // The Sent mailbox and its counts just changed.
                    self.work.push(Work::FetchMailboxes {
                        account: tag.account.clone(),
                    });
                    if let Some(mailbox) = view.mail.selected_mailbox() {
                        let id = mailbox.id.clone();
                        view.fetch_emails(id, limit, &mut self.work);
                    }
                }
            }
            Err(error) => {
                let error = error.to_string();
                if mine && let Some(compose) = &mut self.compose {
                    compose.busy = false;
                    compose.sending = None;
                    compose.send_origin = None;
                    compose.error = Some(error.clone());
                    compose.attempt = attempt;
                    self.refresh_compose_attachments();
                    self.store_draft(true);
                } else if let Some(mut draft) = snapshot
                    && !self.discarded.contains(&draft.id)
                {
                    // Its compose was closed (and kept) while sending.
                    draft.attempt = attempt;
                    self.draft_ops.push(DraftOp::Save(Box::new(draft)));
                }
                self.status = error;
            }
        }
    }

    // Drafts: kept locally as the person types, and on every close path.

    /// A fresh draft id: the time and a counter, lowercase hex.
    fn new_draft_id(&mut self) -> String {
        self.drafts_made += 1;
        let nanos = chrono::Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_default()
            .unsigned_abs();
        format!(
            "{nanos:016x}{:08x}{:04x}",
            std::process::id(),
            self.drafts_made
        )
    }

    /// Hand the open draft to the store when it holds something worth
    /// keeping and changed since its last save (`force`: whatever changed).
    fn store_draft(&mut self, force: bool) {
        let Some(compose) = self.compose.as_mut() else {
            return;
        };
        if !compose.keepable() || compose.draft_id.is_empty() {
            return;
        }
        if !force && compose.saved == compose.revision {
            return;
        }
        compose.saved = compose.revision;
        let draft = compose.saved_draft();
        self.discarded.remove(&draft.id);
        self.draft_ops.push(DraftOp::Save(Box::new(draft)));
    }

    /// The shell's debounce fired: save the open draft if it changed.
    pub fn save_draft(&mut self) {
        self.store_draft(false);
    }

    /// The open draft's revision while it has changes not yet saved.
    pub fn draft_unsaved(&self) -> Option<u64> {
        self.compose
            .as_ref()
            .filter(|c| c.keepable() && !c.draft_id.is_empty() && c.saved != c.revision)
            .map(|c| c.revision)
    }

    /// The draft saves and removals decided since the last call, in order.
    pub fn take_draft_ops(&mut self) -> Vec<DraftOp> {
        std::mem::take(&mut self.draft_ops)
    }

    /// Remove draft `id` from the store for good.
    fn discard_draft(&mut self, id: &str) {
        if !id.is_empty() {
            self.discarded.insert(id.to_owned());
            self.draft_ops.push(DraftOp::Remove(id.to_owned()));
        }
    }

    /// Close the compose without asking, keeping its draft: saved, and
    /// offered again once nothing else is open.
    fn set_aside_compose(&mut self) {
        self.store_draft(false);
        if let Some(compose) = self.compose.take()
            && compose.keepable()
            && !compose.draft_id.is_empty()
            && !self.restore.iter().any(|d| d.id == compose.draft_id)
        {
            self.restore.insert(0, compose.saved_draft());
        }
        self.confirm = None;
    }

    /// Cancel or Escape on the compose: one with nothing typed closes; one
    /// with something typed is saved at once and the person is asked.
    fn close_compose(&mut self) {
        match &self.compose {
            Some(compose) if compose.keepable() => {
                self.store_draft(false);
                self.confirm = Some(Confirm::CloseCompose);
            }
            Some(_) => self.compose = None,
            None => {}
        }
    }

    /// The answer to the close or quit question.
    fn confirm_draft(&mut self, choice: DraftChoice) {
        let Some(asked) = self.confirm.take() else {
            return;
        };
        match choice {
            DraftChoice::Back => return,
            DraftChoice::Keep => {
                self.store_draft(false);
                if self.compose.take().is_some() {
                    self.status = crate::label("draft-kept");
                }
            }
            DraftChoice::Discard => {
                if let Some(compose) = self.compose.take() {
                    self.discard_draft(&compose.draft_id);
                    self.status = crate::label("draft-discarded");
                }
            }
        }
        if asked == Confirm::Quit {
            self.quit_ok = true;
            self.exiting = true;
        }
    }

    /// Drafts an earlier run kept: offered one at a time.
    pub fn offer_drafts(&mut self, drafts: Vec<SavedDraft>) {
        for draft in drafts {
            if !self.restore.iter().any(|d| d.id == draft.id) {
                self.restore.push(draft);
            }
        }
    }

    /// The answer to the offer of the first kept draft.
    fn restore_draft(&mut self, choice: DraftChoice) {
        if self.compose.is_some() || self.restore.is_empty() {
            return;
        }
        match choice {
            // Later: every draft stays kept, offered at the next start.
            DraftChoice::Back => self.restore.clear(),
            DraftChoice::Discard => {
                let draft = self.restore.remove(0);
                self.discard_draft(&draft.id);
            }
            DraftChoice::Keep => {
                if self.open_saved(self.restore[0].clone()) {
                    self.restore.remove(0);
                }
            }
        }
    }

    /// Open a kept draft in the compose dialog, as it was typed. Its
    /// account, when it is gone, becomes the default one (with its From).
    fn open_saved(&mut self, saved: SavedDraft) -> bool {
        // A draft whose account is gone is sent from that account's current
        // default instead, so it keeps no identity of its own.
        let (account, from, from_name, reply_to, identity) = match self.account(&saved.account) {
            Some(_) => (
                saved.account.clone(),
                saved.from.clone(),
                saved.from_name.clone(),
                saved.reply_to.clone(),
                saved.identity.clone(),
            ),
            None => match self.compose_account() {
                Some(id) => {
                    let from = self
                        .account(&id)
                        .map_or(saved.from.clone(), |v| v.entry.email.clone());
                    (id, from, String::new(), None, String::new())
                }
                None => (
                    saved.account.clone(),
                    saved.from.clone(),
                    saved.from_name.clone(),
                    saved.reply_to.clone(),
                    saved.identity.clone(),
                ),
            },
        };
        let draft = Composed {
            from,
            from_name,
            reply_to,
            subject: saved.subject,
            body: saved.body,
            attachments: saved.attachments,
            in_reply_to: saved.in_reply_to,
            references: saved.references,
            ..Composed::default()
        };
        let mut compose = ComposeState::new(&account, draft);
        if let Some(error) = compose.error {
            self.status = error;
            return false;
        }
        compose.identity = identity;
        compose.to_text = saved.to;
        compose.cc_text = saved.cc;
        compose.bcc_text = saved.bcc;
        compose.draft_id = saved.id;
        compose.restored = true;
        compose.attempt = saved.attempt;
        self.compose = Some(compose);
        self.check_compose_attachment_size();
        true
    }

    /// The window is asked to close: yes at once, unless a draft is open
    /// with something typed. Then it is saved and the person is asked
    /// (keeping the draft is the default).
    pub fn request_quit(&mut self) -> bool {
        if self.quit_ok {
            return true;
        }
        match &self.compose {
            Some(compose) if compose.keepable() => {
                self.store_draft(false);
                self.confirm = Some(Confirm::Quit);
                false
            }
            _ => true,
        }
    }

    /// The open email of the current account, with its account.
    fn reading(&self) -> Option<(String, Email)> {
        let view = self.current()?;
        Some((view.entry.id.clone(), view.mail.open.clone()?))
    }

    /// `account`'s connection generation, for a form to be bound to.
    fn generation_of(&self, account: &str) -> u64 {
        self.account(account).map_or(0, AccountView::generation)
    }

    /// Open `form`; a note left by an earlier one goes.
    fn open_form(&mut self, form: Form) {
        self.form_note = None;
        self.form = Some(form);
    }

    /// `mail.delete`: the target to Trash. Two or more always ask first,
    /// with the count; one asks when the settings say so.
    pub fn delete_target(&mut self) {
        self.act(Action::Delete);
    }

    /// Apply `action` to the current account's target. "Select all N"
    /// fetches the query's ids first (refused above [`SELECT_ALL_CAP`]);
    /// one fetch at a time.
    pub fn act(&mut self, action: Action) {
        if self.mode != Mode::Mail {
            return;
        }
        let Some(account) = self.selected.clone() else {
            return;
        };
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return;
        };
        if view.mail.fetching.is_some() {
            return;
        }
        match view.mail.target() {
            None => {}
            Some(Target::Everything(count)) => {
                if count > SELECT_ALL_CAP {
                    self.status = too_many(count);
                    return;
                }
                let Some(mailbox) = view.mail.selected.clone() else {
                    return;
                };
                view.tickets.ids += 1;
                let ticket = view.tickets.ids;
                let sort = email_sort(view.mail.table.sort.as_ref());
                view.mail.fetching = Some(Fetching {
                    ticket,
                    action,
                    mailbox: mailbox.clone(),
                    sort,
                    expected: count,
                    ids: Vec::new(),
                });
                self.work.push(Work::FetchIds {
                    account,
                    ticket,
                    mailbox,
                    sort,
                    position: 0,
                });
            }
            Some(Target::Rows(ids)) => self.act_on(&account, ids, action),
        }
    }

    /// Escape while "Fetching N…": the fetch stops and nothing changes.
    pub fn cancel_fetch(&mut self) {
        let Some(account) = self.selected.clone() else {
            return;
        };
        if let Some(view) = self.account_mut(&account)
            && view.cancel_fetch()
        {
            self.status = crate::label("fetch-cancelled");
        }
    }

    /// A page of "Select all N"'s ids arrived: ask for the next, refuse
    /// once the query is past [`SELECT_ALL_CAP`], or act once all are in.
    fn ids_arrived(&mut self, tag: &Tag, ticket: u64, result: Result<jmap::IdPage, jmap::Error>) {
        let Some(view) = self
            .accounts
            .iter_mut()
            .find(|v| v.entry.id == tag.account && v.generation == tag.generation)
        else {
            return;
        };
        if view.tickets.ids != ticket
            || view
                .mail
                .fetching
                .as_ref()
                .is_none_or(|f| f.ticket != ticket)
        {
            return;
        }
        let page = match result {
            Ok(page) => page,
            Err(error) => {
                view.cancel_fetch();
                self.status = crate::label_with("fetch-failed", &[("error", &error.to_string())]);
                return;
            }
        };
        let Some(fetching) = view.mail.fetching.as_mut() else {
            return;
        };
        let more = !page.ids.is_empty();
        fetching.expected = page.total;
        for id in page.ids {
            if !fetching.ids.contains(&id) {
                fetching.ids.push(id);
            }
        }
        let have = fetching.ids.len() as u64;
        if page.total > SELECT_ALL_CAP || have > SELECT_ALL_CAP {
            let count = page.total.max(have);
            view.cancel_fetch();
            self.status = too_many(count);
            return;
        }
        if more && have < page.total {
            let work = Work::FetchIds {
                account: tag.account.clone(),
                ticket,
                mailbox: fetching.mailbox.clone(),
                sort: fetching.sort,
                position: have,
            };
            self.work.push(work);
            return;
        }
        if let Some(done) = view.mail.fetching.take() {
            self.act_on(&tag.account, done.ids, done.action);
        }
    }

    /// Apply `action` to `account`'s emails `ids`.
    fn act_on(&mut self, account: &str, ids: Vec<String>, action: Action) {
        if ids.is_empty() {
            return;
        }
        let Some(view) = self.account(account) else {
            return;
        };
        let generation = view.generation;
        let shown: Vec<&Email> = view
            .mail
            .emails
            .iter()
            .filter(|e| ids.contains(&e.id))
            .collect();
        let all = |test: fn(&Email) -> bool| !shown.is_empty() && shown.iter().all(|e| test(e));
        match action {
            Action::Flag => {
                let flagged = !all(Email::is_flagged);
                let change = jmap::EmailChange::Keywords {
                    read: None,
                    flagged: Some(flagged),
                };
                self.batch(account, generation, BatchKind::Flag, change, ids);
            }
            Action::Read => {
                let read = !all(Email::is_read);
                let change = jmap::EmailChange::Keywords {
                    read: Some(read),
                    flagged: None,
                };
                self.batch(account, generation, BatchKind::Read, change, ids);
            }
            Action::Delete => {
                if ids.len() >= 2 || self.settings.mail.confirm_delete {
                    self.open_form(Form::Delete {
                        account: account.to_owned(),
                        generation,
                        ids,
                    });
                } else {
                    self.trash(account, generation, ids);
                }
            }
            Action::Move => self.open_form(Form::Move {
                account: account.to_owned(),
                generation,
                ids,
            }),
            Action::Junk(junk) => {
                let Some(mailbox) = junk_target(view, junk) else {
                    self.status = crate::label(if junk { "no-junk-mailbox" } else { "no-inbox" });
                    return;
                };
                let change = jmap::EmailChange::Move { mailbox };
                self.batch(account, generation, BatchKind::Junk, change, ids);
            }
        }
    }

    /// `ids` to the account's Trash mailbox, through `generation` only.
    fn trash(&mut self, account: &str, generation: u64, ids: Vec<String>) {
        let trash = self
            .account(account)
            .and_then(|v| v.mail.by_role("trash"))
            .map(|m| m.id.clone());
        let Some(mailbox) = trash else {
            self.status = crate::label("no-trash-mailbox");
            return;
        };
        let change = jmap::EmailChange::Move { mailbox };
        self.batch(account, generation, BatchKind::Delete, change, ids);
    }

    /// Queue one batched action. A keyword change shows at once on the
    /// rows (and the mailbox's unread count); what each shown row held
    /// before goes with it, so a failed id is put back.
    fn batch(
        &mut self,
        account: &str,
        generation: u64,
        kind: BatchKind,
        change: jmap::EmailChange,
        ids: Vec<String>,
    ) {
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return;
        };
        let mut before = Vec::new();
        if let Some((keyword, on)) = keyword_of(&change) {
            for email in view.mail.emails.iter().filter(|e| ids.contains(&e.id)) {
                before.push((email.id.clone(), has_keyword(email, keyword)));
            }
            if keyword == "$seen" {
                let flips = before.iter().filter(|(_, was)| *was != on).count() as u64;
                view.mail
                    .shift_unread(if on { -(flips as i64) } else { flips as i64 });
            }
            view.set_keyword(&ids, keyword, on);
        }
        view.mail.failed.clear();
        self.work.push(Work::Batch {
            account: account.to_owned(),
            generation,
            batch: Box::new(Batch {
                kind,
                change,
                ids,
                before,
            }),
        });
    }

    /// A batched action finished. The ids it changed stay changed (a
    /// delete, move or junk takes them out of the list at once); each
    /// failed id is put back as it was and named with its reason in the
    /// status line. A flag or read change keeps the selection, so the
    /// person can carry on; a delete, move or junk clears it. If the open
    /// email left, the reader opens the next row, as a single delete did.
    fn batched(&mut self, tag: &Tag, batch: Batch, outcome: jmap::SetOutcome) {
        let limit = self.settings.mail.page_size;
        let Some(view) = self
            .accounts
            .iter_mut()
            .find(|v| v.entry.id == tag.account && v.generation == tag.generation)
        else {
            return;
        };
        let failed: Vec<&String> = outcome.failed.iter().map(|(id, _)| id).collect();
        if let Some((keyword, on)) = keyword_of(&batch.change) {
            let mut put_back = 0i64;
            for (id, was) in batch.before.iter().filter(|(id, _)| failed.contains(&id)) {
                view.set_keyword(std::slice::from_ref(id), keyword, *was);
                if keyword == "$seen" && *was != on {
                    put_back += 1;
                }
            }
            if keyword == "$seen" {
                view.mail
                    .shift_unread(if on { put_back } else { -put_back });
            }
        }
        let mut advance = None;
        if batch.kind.removes() {
            let gone = &outcome.done;
            if let Some(open) = view.mail.open.as_ref().filter(|e| gone.contains(&e.id)) {
                advance = neighbour(&view.mail.emails, &open.id, gone);
                view.mail.open = None;
                view.tickets.open_requested = None;
            }
            view.mail.emails.retain(|e| !gone.contains(&e.id));
            view.mail.table.selection.clear();
            view.mail.table.anchor = None;
            view.mail.everything = false;
            view.mail.table.rows_changed(&view.mail.keys());
            if let Some(mailbox) = view.mail.selected.clone() {
                view.fetch_emails(mailbox, limit, &mut self.work);
            }
        }
        self.work.push(Work::FetchMailboxes {
            account: tag.account.clone(),
        });
        view.mail.failed = outcome.failed.clone();
        if !outcome.failed.is_empty() {
            let reasons = outcome
                .failed
                .iter()
                .map(|(id, why)| format!("{id} ({why})"))
                .collect::<Vec<_>>()
                .join(", ");
            self.status = crate::label_with(
                "batch-failed",
                &[
                    ("failed", &outcome.failed.len().to_string()),
                    ("count", &batch.ids.len().to_string()),
                    ("reasons", &reasons),
                ],
            );
        }
        self.refresh_previews();
        if let Some(id) = advance {
            self.open_email(&tag.account, id);
        }
    }

    /// The entry typed in recipient `field` changed: show the recent
    /// addresses that match at once, and look the contacts up once typing
    /// pauses ([`SUGGEST_DEBOUNCE`]). Fewer than two characters close the
    /// list.
    fn suggest_typed(&mut self, field: ComposeField) {
        let ticket = self.ticket();
        let Some(compose) = self.compose.as_ref() else {
            return;
        };
        let text = compose.recipients(field).to_owned();
        let Some(query) = crate::suggest::query(&text).map(str::to_owned) else {
            if let Some(compose) = self.compose.as_mut() {
                compose.suggest = None;
            }
            return;
        };
        let recent = self
            .account(&compose.account)
            .map(|v| v.mail.recent.clone())
            .unwrap_or_default();
        let items = crate::suggest::rank(
            Vec::new(),
            &recent,
            &query,
            &crate::suggest::present(&text),
            crate::suggest::LIMIT,
        );
        if let Some(compose) = self.compose.as_mut() {
            compose.suggest = Some(Suggest {
                field,
                query,
                ticket,
                items,
                cursor: 0,
            });
        }
        self.queue(Work::SuggestWait { ticket });
    }

    /// The debounce for suggestion `ticket` passed: look its contacts up,
    /// if it is still the newest.
    fn suggest_due(&mut self, ticket: u64) {
        let Some(compose) = self.compose.as_ref() else {
            return;
        };
        if let Some(suggest) = &compose.suggest
            && suggest.ticket == ticket
        {
            let work = Work::Suggest {
                account: compose.account.clone(),
                ticket,
                text: suggest.query.clone(),
            };
            self.queue(work);
        }
    }

    /// Take suggestion `index` (or the highlighted one): the typed entry
    /// becomes it, with a separator for the next.
    fn suggest_accept(&mut self, index: Option<usize>) {
        let Some(compose) = self.compose.as_mut() else {
            return;
        };
        let Some(suggest) = compose.suggest.take() else {
            return;
        };
        let Some(item) = suggest.items.get(index.unwrap_or(suggest.cursor)) else {
            return;
        };
        let text = match suggest.field {
            ComposeField::To => &mut compose.to_text,
            ComposeField::Cc => &mut compose.cc_text,
            ComposeField::Bcc => &mut compose.bcc_text,
            ComposeField::ReplyTo | ComposeField::Subject | ComposeField::Body => return,
        };
        *text = crate::suggest::accept(text, item);
        compose.revision += 1;
    }

    /// `mail.attachments`: toggle the remembered attachment bar expansion.
    pub fn toggle_attachments(&mut self) {
        self.ui.attachments_expanded = !self.ui.attachments_expanded;
        self.config_dirty = true;
    }

    pub fn toggle_attachments_inline(&mut self) {
        self.ui.attachments_inline = !self.ui.attachments_inline;
        self.config_dirty = true;
        self.refresh_previews();
    }

    pub(crate) fn preview_owner(&self) -> Option<crate::previews::Owner> {
        let view = self.current()?;
        let email = view.mail.open.as_ref()?;
        (self.ui.attachments_inline && self.mode == Mode::Mail).then(|| {
            (
                view.id().to_owned(),
                view.generation,
                email.id.clone(),
                view.mail.open_ticket,
            )
        })
    }

    fn refresh_previews(&mut self) {
        let owner = self.preview_owner();
        let parts = self
            .current()
            .and_then(|v| v.mail.open.as_ref())
            .and_then(|e| e.attachments.clone())
            .unwrap_or_default();
        self.previews.sync(owner, &parts);
        if !self.current().is_some_and(AccountView::opening_another) {
            self.work.extend(self.previews.schedule(&parts));
        }
    }

    /// Reconcile the current email and upload completed images on the UI thread.
    pub fn prepare_previews(&mut self, ctx: &egui::Context) {
        self.refresh_previews();
        self.previews.upload(ctx);
    }

    /// Save the open email's attachment `part` to the downloads directory,
    /// or (`open`) to a private directory and open it with the desktop's
    /// default application.
    pub fn save_attachment(&mut self, part: &str, open: bool) {
        let Some((account, email)) = self.reading() else {
            return;
        };
        match attachment_work(&account, &email, part, open, None, None) {
            Ok(work) => {
                let transfer = if open {
                    Transfer::Opening
                } else {
                    Transfer::Saving
                };
                if let Some(view) = self.account_mut(&account) {
                    view.mail.transfers.insert(part.to_owned(), transfer);
                }
                self.status = crate::label(if open {
                    "attachment-opening"
                } else {
                    "attachment-saving"
                });
                self.queue(work);
            }
            Err((_, message)) => self.status = message,
        }
    }

    /// `mail.save_attachments`: save every attachment of the open email
    /// that can be fetched.
    pub fn save_all_attachments(&mut self) {
        let Some((_, email)) = self.reading() else {
            return;
        };
        for attachment in email.attachments.iter().flatten() {
            if attachment.downloadable()
                && let Some(part) = &attachment.part_id
            {
                self.save_attachment(part, false);
            }
        }
    }

    /// An attachment save or open finished.
    pub fn attachment_done(&mut self, tag: &Tag, done: crate::shell::AttachmentDone) {
        let crate::shell::AttachmentDone {
            email,
            part,
            open,
            reply,
            result,
        } = done;
        let transfer = match &result {
            Ok(path) if open => Transfer::Opened(path.clone()),
            Ok(path) => Transfer::Saved(path.clone()),
            Err(error) => Transfer::Failed(error.clone()),
        };
        if let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == tag.account)
            && view.mail.open.as_ref().is_some_and(|o| o.id == email)
        {
            view.mail.transfers.insert(part, transfer);
        }
        match result {
            Ok(path) => {
                let key = if open {
                    "attachment-opened"
                } else {
                    "attachment-saved"
                };
                self.status = crate::label_with(key, &[("path", &path)]);
                if let Some(reply) = reply {
                    self.reply(reply, 0, json!({ "path": path }));
                }
            }
            Err(error) => {
                self.status = error.clone();
                if let Some(reply) = reply {
                    self.refuse(reply, code::UNAVAILABLE, &error);
                }
            }
        }
    }

    /// `mail.print` (Ctrl+P): the open email as a print page, rendered as
    /// the reader shows it, opened in the browser.
    pub fn print_open(&mut self) {
        if let Some((account, email)) = self.reading() {
            self.print_shown(&account, &email, None);
        }
    }

    /// Print `account`'s `email` as its reader shows it (`reply`: the Bus
    /// request to answer).
    pub fn print_shown(&mut self, account: &str, email: &Email, reply: Option<u64>) {
        let markdown = self.account(account).is_some_and(|v| {
            crate::reader::shown(&v.mail.reader, email) == crate::reader::Format::Markdown
        });
        self.print_email(email, markdown, reply);
    }

    fn print_email(&mut self, email: &Email, markdown: bool, reply: Option<u64>) {
        let html = crate::print::page(
            email,
            markdown,
            PRINT_FONT_PT,
            &crate::print::Palette::light(),
        );
        self.status = crate::label("printing");
        self.queue(Work::Print {
            reply,
            name: crate::print::file_name(email),
            html,
        });
    }

    /// A print page was written (`opened`: and handed to the browser).
    pub fn printed(&mut self, reply: Option<u64>, result: Result<(String, bool), String>) {
        match result {
            Ok((path, opened)) => {
                let key = if opened {
                    "printed"
                } else {
                    "print-no-browser"
                };
                self.status = crate::label_with(key, &[("path", &path)]);
                if let Some(reply) = reply {
                    self.reply(reply, 0, json!({ "path": path, "opened": opened }));
                }
            }
            Err(error) => {
                self.status = error.clone();
                if let Some(reply) = reply {
                    self.refuse(reply, code::UNAVAILABLE, &error);
                }
            }
        }
    }

    /// `mail.junk` (J): move the target to the account's Junk mailbox.
    /// maild trains its spam filter on the move; there is no `$junk`
    /// keyword.
    pub fn mark_junk(&mut self) {
        self.act(Action::Junk(true));
    }

    /// `mail.not_junk` (Shift+J): move the target out of Junk to the
    /// inbox; maild trains it as ham.
    pub fn mark_not_junk(&mut self) {
        self.act(Action::Junk(false));
    }

    /// `mail.move`: choose where the target moves to.
    pub fn move_target(&mut self) {
        self.act(Action::Move);
    }

    /// `mail.flag` (S): flag the target, or unflag it when every shown
    /// target row is flagged.
    pub fn flag_target(&mut self) {
        self.act(Action::Flag);
    }

    /// `mail.mark_unread` (M): mark the target read when any shown target
    /// row is unread, else unread. The open email is not marked read again:
    /// only a fresh open does that (`mark_read_on_open`). The rows and the
    /// mailbox's unread count change at once.
    pub fn toggle_target_read(&mut self) {
        self.act(Action::Read);
    }

    /// Reply to everyone the open email went to.
    pub fn reply_all_open(&mut self) {
        if let Some(account) = self.selected.clone() {
            self.open_reply(&account, true, None);
        }
    }

    /// Reply to the sender, quoting the original.
    pub fn reply_open_quoted(&mut self) {
        self.reply_open_with(false, Some(true));
    }

    /// Reply to the sender without the original.
    pub fn reply_open_unquoted(&mut self) {
        self.reply_open_with(false, Some(false));
    }

    /// Reply to everyone, quoting the original.
    pub fn reply_all_open_quoted(&mut self) {
        self.reply_open_with(true, Some(true));
    }

    /// Reply to everyone without the original.
    pub fn reply_all_open_unquoted(&mut self) {
        self.reply_open_with(true, Some(false));
    }

    fn reply_open_with(&mut self, all: bool, quote: Option<bool>) {
        if let Some(account) = self.selected.clone() {
            self.open_reply(&account, all, quote);
        }
    }

    /// Whether a plain Reply quotes the original (`email.mail.quote_on_reply`).
    pub fn quote_on_reply(&self) -> bool {
        self.settings.mail.quote_on_reply
    }

    /// Where the person's text goes in a quoted reply
    /// (`email.mail.reply_position`).
    pub fn reply_position(&self) -> crate::quote::Position {
        match self.settings.mail.reply_position {
            settings::email::ReplyPosition::Above => crate::quote::Position::Above,
            settings::email::ReplyPosition::Below => crate::quote::Position::Below,
        }
    }

    /// Forward the open email.
    pub fn forward_open(&mut self) {
        if let Some((account, email)) = self.reading() {
            self.forward(&account, &email, "");
        }
    }

    /// Send the open email again as a new one (Edit as New Message).
    pub fn send_as_new_open(&mut self) {
        if let Some((account, email)) = self.reading() {
            self.send_as_new(&account, &email);
        }
    }

    /// Open the New Event form on the current account's selected calendar,
    /// starting at the next whole hour, an hour long.
    pub fn new_event(&mut self) {
        let Some(view) = self.current() else {
            return;
        };
        let Some(calendar) = view
            .calendars
            .selected
            .clone()
            .or_else(|| view.calendars.calendars.first().map(|c| c.id.clone()))
        else {
            return;
        };
        use chrono::Timelike;
        let later = chrono::Local::now() + chrono::Duration::hours(1);
        let hour = later
            .with_minute(0)
            .and_then(|t| t.with_second(0))
            .and_then(|t| t.with_nanosecond(0))
            .unwrap_or(later);
        let format = |t: chrono::DateTime<chrono::Local>| {
            t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        };
        let form = Form::Event(EventForm {
            account: view.entry.id.clone(),
            generation: view.generation,
            calendar,
            start: format(hour),
            end: format(hour + chrono::Duration::hours(1)),
            ..EventForm::default()
        });
        self.open_form(form);
    }

    /// Open the New Contact form on the current account's selected book.
    pub fn new_contact(&mut self) {
        let Some(view) = self.current() else {
            return;
        };
        let Some(book) = view
            .contacts
            .selected
            .clone()
            .or_else(|| view.contacts.books.first().map(|b| b.id.clone()))
        else {
            return;
        };
        let form = Form::Contact(ContactForm {
            account: view.entry.id.clone(),
            generation: view.generation,
            book,
            ..ContactForm::default()
        });
        self.open_form(form);
    }

    /// The form's answer: Delete confirmed, a Move target chosen, or Save.
    fn submit_form(&mut self, choice: Option<String>) {
        let Some(form) = self.form.clone() else {
            return;
        };
        // Bound to the connection it was opened on: after a login change it
        // says so and must be reopened, never submitted to the new server.
        if self.generation_of(form_account(&form)) != form_generation(&form) {
            self.form_note = Some(crate::label("form-account-changed"));
            return;
        }
        let generation = form_generation(&form);
        match form {
            Form::Delete { account, ids, .. } => {
                self.form = None;
                self.trash(&account, generation, ids);
            }
            Form::Move { account, ids, .. } => {
                if let Some(mailbox) = choice {
                    self.form = None;
                    let change = jmap::EmailChange::Move { mailbox };
                    self.batch(&account, generation, BatchKind::Move, change, ids);
                }
            }
            Form::Event(f) if !f.busy => {
                let checked = (|| {
                    if f.title.trim().is_empty() {
                        return Err(crate::label("title-needed"));
                    }
                    jmap::duration_between(f.start.trim(), f.end.trim())?;
                    Ok(())
                })();
                if let Err(error) = checked {
                    if let Some(Form::Event(form)) = &mut self.form {
                        form.error = Some(error);
                    }
                    return;
                }
                let draft = jmap::EventDraft {
                    title: f.title.trim().to_owned(),
                    start: f.start.trim().to_owned(),
                    end: f.end.trim().to_owned(),
                    location: Some(f.location.trim().to_owned()).filter(|l| !l.is_empty()),
                    ..jmap::EventDraft::default()
                };
                let op = self.ticket();
                if let Some(Form::Event(form)) = &mut self.form {
                    form.busy = true;
                    form.op = Some(op);
                    form.error = None;
                }
                let write = crate::commands::Write::CreateEvent {
                    calendar: f.calendar,
                    draft,
                };
                self.write_form(f.account, op, write);
            }
            Form::Contact(f) if !f.busy => {
                if f.name.trim().is_empty() {
                    if let Some(Form::Contact(form)) = &mut self.form {
                        form.error = Some(crate::label("name-needed"));
                    }
                    return;
                }
                let some = |text: &str| Some(text.trim().to_owned()).filter(|t| !t.is_empty());
                let draft = jmap::ContactDraft {
                    name: f.name.trim().to_owned(),
                    emails: some(&f.email).into_iter().collect(),
                    phones: some(&f.phone).into_iter().collect(),
                    organization: some(&f.organization),
                    notes: None,
                };
                let op = self.ticket();
                if let Some(Form::Contact(form)) = &mut self.form {
                    form.busy = true;
                    form.op = Some(op);
                    form.error = None;
                }
                let write = crate::commands::Write::CreateContact {
                    book: f.book,
                    draft,
                };
                self.write_form(f.account, op, write);
            }
            Form::Event(_) | Form::Contact(_) => {}
        }
    }

    /// The accounts as `email.accounts` reports them: settings, whether a
    /// password is held, connection state; never a password.
    pub fn accounts_view(&self) -> Value {
        let default = self.settings.from_account().map(|a| a.id.clone());
        Value::Array(
            self.accounts
                .iter()
                .map(|view| {
                    let entry = &view.entry;
                    json!({
                        "id": entry.id,
                        "name": entry.name,
                        "url": entry.url,
                        "email": entry.email,
                        "ca_path": entry.ca_path,
                        "enabled": entry.enabled,
                        "signature": entry.signature,
                        "identities": entry.identities,
                        "default": default.as_deref() == Some(entry.id.as_str()),
                        "has_password": self.secrets.has(&entry.id),
                        "state": view.phase.name(),
                        "error": match &view.phase { Phase::Failed(error) => Some(error.as_str()), _ => None },
                        "session": view.session,
                        "unread": view.mail.unread(),
                        "migrating": self.migration.as_ref().is_some_and(|m| m.entry.id == entry.id),
                    })
                })
                .collect(),
        )
    }

    /// The whole window state as `email.info` reports it, minus secrets.
    pub fn info(&self) -> Value {
        let views: Vec<Value> = self
            .accounts
            .iter()
            .zip(self.accounts_view().as_array().cloned().unwrap_or_default())
            .map(|(view, mut out)| {
                let shown = |emails: &[Email]| -> Vec<Value> {
                    emails
                        .iter()
                        .map(|e| {
                            json!({
                                "id": e.id,
                                "subject": e.subject,
                                "from": e.from_display(),
                                "date": e.date.as_ref().or(e.received_at.as_ref()),
                                "read": e.is_read(),
                                "flagged": e.keywords.as_ref().and_then(|k| k.get("$flagged")).and_then(Value::as_bool).unwrap_or(false),
                            })
                        })
                        .collect()
                };
                out["mail"] = json!({
                    "mailboxes": view.mail.mailboxes,
                    "selected": view.mail.selected,
                    "emails": shown(&view.mail.emails),
                    "total": view.mail.total,
                    "open": view.mail.open.as_ref().map(|e| &e.id),
                    "reader": view.mail.open.as_ref().map(|e| {
                        let mut reader = view.mail.reader.info(e);
                        reader["attachments_expanded"] = json!(self.ui.attachments_expanded);
                        reader["attachments_inline"] = json!(self.ui.attachments_inline);
                        let current = self.preview_owner().is_some_and(|owner| {
                            owner.0 == view.id() && self.previews.is_for(&owner)
                        });
                        reader["attachments"] = json!(e.attachments.iter().flatten().map(|a| {
                            let preview = a.blob_id.as_deref().filter(|_| current)
                                .and_then(|id| self.previews.get(id))
                                .map(crate::previews::Preview::info)
                                .unwrap_or_else(|| json!({"state": "none"}));
                            json!({"part": a.part_id, "preview": preview})
                        }).collect::<Vec<_>>());
                        reader
                    }),
                    "attachments": view.mail.open.as_ref().map(|e| attachments_view(e, Some(&view.mail.transfers))),
                    "busy": view.mail.busy,
                    // The list's selection: the shown page's selected ids,
                    // how many in all, and whether that is every email.
                    "selection": {
                        "ids": view.mail.table.selection,
                        "count": view.mail.selection_count(),
                        "all": view.mail.everything,
                    },
                    // The list's sort and page (0-based), as asked of the server.
                    "sort": view.mail.sort().column,
                    "order": view.mail.sort().order.name(),
                    "page": view.mail.table.page,
                    "page_size": view.mail.table.page_size,
                });
                out["calendar"] = json!({
                    "calendars": view.calendars.calendars,
                    "selected": view.calendars.selected,
                    "events": view.calendars.events.iter().map(|e| json!({"id": e.id, "title": e.display_title(), "start": e.start, "end": e.end})).collect::<Vec<_>>(),
                    "open": view.calendars.open.and_then(|i| view.calendars.events.get(i)).map(|e| &e.id),
                    "busy": view.calendars.busy,
                });
                out["contacts"] = json!({
                    "books": view.contacts.books,
                    "selected": view.contacts.selected,
                    "filter": view.contacts.filter,
                    "contacts": view.contacts.contacts.iter().map(|c| json!({"id": c.id, "name": c.display_name(), "email": c.display_email()})).collect::<Vec<_>>(),
                    "open": view.contacts.open.and_then(|i| view.contacts.contacts.get(i)).map(|c| &c.id),
                    "busy": view.contacts.busy,
                });
                out
            })
            .collect();
        let compose = self.compose.as_ref().map(|c| {
            json!({
                "account": c.account,
                "from": c.draft.from,
                "to": c.to_text,
                "cc": c.cc_text,
                "bcc": c.bcc_text,
                "subject": c.draft.subject,
                "body": c.body.contents(),
                "attachments": c.attachments,
                "attachment_size": c.attachment_size(),
                "attachment_error": c.attachment_error,
                "attachments_over_limit": c.attachments_over_limit,
                "sendable": self.compose_sendable(),
                "send_origin": c.send_origin,
                "in_reply_to": c.draft.in_reply_to,
                "stale": c.stale,
                "busy": c.busy,
                "error": c.error,
            })
        });
        let form = self.form.as_ref().map(|form| match form {
            Form::Delete { account, ids, .. } => json!({"kind": "delete", "account": account, "ids": ids}),
            Form::Move { account, ids, .. } => json!({"kind": "move", "account": account, "ids": ids}),
            Form::Event(f) => json!({"kind": "event", "account": f.account, "calendar": f.calendar, "title": f.title, "start": f.start, "end": f.end, "location": f.location, "busy": f.busy, "error": f.error}),
            Form::Contact(f) => json!({"kind": "contact", "account": f.account, "book": f.book, "name": f.name, "email": f.email, "phone": f.phone, "organization": f.organization, "busy": f.busy, "error": f.error}),
        });
        json!({
            "schema": crate::commands::SCHEMA,
            "version": env!("CARGO_PKG_VERSION"),
            "selected": self.selected,
            "mode": self.mode,
            "source": match self.source { Source::Cache => "cache", Source::Settings => "settings", Source::Missing => "missing" },
            "status": self.status,
            "default_account": self.settings.default_account,
            "mail_options": {
                "page_size": self.settings.mail.page_size,
                "mark_read_on_open": self.settings.mail.mark_read_on_open,
                "confirm_delete": self.settings.mail.confirm_delete,
                "quote_on_reply": self.settings.mail.quote_on_reply,
                "reply_position": self.settings.mail.reply_position,
            },
            "accounts": views,
            "tree": self.tree().iter().map(|r| json!({"key": r.node.key(), "depth": r.depth, "label": r.label, "unread": r.unread, "expandable": r.expandable, "expanded": r.expanded, "selected": r.selected})).collect::<Vec<_>>(),
            "compose": compose,
            "confirm": self.confirm.map(|c| match c { Confirm::CloseCompose => "close_compose", Confirm::Quit => "quit" }),
            "drafts_offered": self.restore.len(),
            "form": form,
            "dialog": self.ui.dialog.map(|d| match d { Dialog::About => "about", Dialog::Shortcuts => "shortcuts" }),
            "ui": {
                "places": self.ui.places,
                "reader": self.ui.reader,
                "cursor": self.ui.cursor.as_ref().map(Node::key),
                "expanded": self.ui.expanded,
                "theme_mode": self.ui.theme.mode.map(|m| format!("{m:?}").to_lowercase()),
            },
            "migrating": self.migration.is_some(),
            "exiting": self.exiting,
        })
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
                        .map(|c| {
                            (
                                Node::Calendar(id.clone(), c.id.clone()),
                                c.name.clone(),
                                RowKind::Calendar,
                            )
                        })
                        .collect::<Vec<_>>(),
                ),
                (
                    Node::Books(id.clone()),
                    "address-books",
                    RowKind::BookGroup,
                    view.contacts
                        .books
                        .iter()
                        .map(|b| {
                            (
                                Node::Book(id.clone(), b.id.clone()),
                                b.name.clone(),
                                RowKind::Book,
                            )
                        })
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
                None => m
                    .parent_id
                    .as_deref()
                    .is_none_or(|p| !known(p) || p == m.id),
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
            let children =
                view.mail.mailboxes.iter().any(|m| {
                    m.parent_id.as_deref() == Some(mailbox.id.as_str()) && m.id != mailbox.id
                });
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
        let signature = self
            .senders(account)
            .first()
            .map(|sender| sender.signature.clone())
            .unwrap_or_default();
        signature_block(&signature)
    }

    /// The sender identities `account` sends as, default first.
    pub fn senders(&self, account: &str) -> Vec<settings::email::Identity> {
        self.account(account)
            .map(|v| v.entry.senders())
            .unwrap_or_default()
    }

    /// Open the identities dialog for `account`.
    pub fn open_identities(&mut self, account: &str) {
        let Some(view) = self.account(account) else {
            return;
        };
        self.identity_editor = Some(IdentityEditor {
            account: account.to_owned(),
            rows: view.entry.senders(),
            selected: 0,
            error: None,
        });
    }

    /// Set one text field of the selected identity.
    pub fn identity_edit(&mut self, field: IdentityField, value: String) {
        let Some(editor) = &mut self.identity_editor else {
            return;
        };
        let Some(row) = editor.rows.get_mut(editor.selected) else {
            return;
        };
        match field {
            IdentityField::Name => row.name = value,
            IdentityField::Address => row.address = value,
            IdentityField::Signature => row.signature = value,
            IdentityField::ReplyTo => row.reply_to = Some(value).filter(|v| !v.is_empty()),
        }
        editor.error = None;
    }

    /// Select identity `index` for editing.
    pub fn identity_select(&mut self, index: usize) {
        if let Some(editor) = &mut self.identity_editor
            && index < editor.rows.len()
        {
            editor.selected = index;
        }
    }

    /// Add an identity with a fresh id, and select it.
    pub fn identity_add(&mut self) {
        let Some(editor) = &mut self.identity_editor else {
            return;
        };
        let mut n = editor.rows.len() + 1;
        let id = loop {
            let candidate = format!("identity-{n}");
            if !editor.rows.iter().any(|r| r.id == candidate) {
                break candidate;
            }
            n += 1;
        };
        editor.rows.push(settings::email::Identity {
            id,
            name: String::new(),
            address: String::new(),
            signature: String::new(),
            reply_to: None,
        });
        editor.selected = editor.rows.len() - 1;
        editor.error = None;
    }

    /// Remove the selected identity.
    pub fn identity_remove(&mut self) {
        let Some(editor) = &mut self.identity_editor else {
            return;
        };
        if editor.rows.is_empty() {
            return;
        }
        editor.rows.remove(editor.selected);
        editor.selected = editor.selected.min(editor.rows.len().saturating_sub(1));
        editor.error = None;
    }

    /// Save the dialog's identities for its account. The settings rules
    /// decide: a refused list stays open with the reason. Returns whether it
    /// was saved.
    pub fn identity_save(&mut self) -> bool {
        let Some(editor) = self.identity_editor.clone() else {
            return false;
        };
        let Some(view) = self.account(&editor.account) else {
            return false;
        };
        let mut entry = view.entry.clone();
        entry.identities = editor.rows;
        let mut next = self.settings.clone();
        if let Some(slot) = next.accounts.iter_mut().find(|a| a.id == entry.id) {
            *slot = entry.clone();
        }
        if let Err(diagnostic) = settings::email::validate(&next) {
            if let Some(open) = &mut self.identity_editor {
                open.error = Some(format!("{}: {}", diagnostic.path, diagnostic.message));
            }
            return false;
        }
        self.change_accounts(Change::Upsert(entry), None, None);
        self.identity_editor = None;
        true
    }

    /// Close the identities dialog, discarding its unsaved rows.
    pub fn close_identities(&mut self) {
        self.identity_editor = None;
    }

    /// Send the open draft as `identity` of `account` (the account's default
    /// when `None`). A signature the person left untouched follows the new
    /// identity; an edited one stays as it is.
    fn choose_sender(&mut self, account: String, identity: Option<String>) {
        let Some(compose) = &self.compose else {
            return;
        };
        if compose.busy {
            return;
        }
        let senders = self.senders(&account);
        let explicit = self
            .account(&account)
            .is_some_and(|v| !v.entry.identities.is_empty());
        let Some(sender) = identity
            .as_deref()
            .map_or(senders.first(), |id| senders.iter().find(|s| s.id == id))
            .cloned()
        else {
            return;
        };
        let previous = self
            .senders(&compose.account)
            .into_iter()
            .find(|s| s.id == compose.identity)
            .or_else(|| self.senders(&compose.account).into_iter().next());
        let previous_explicit = self
            .account(&compose.account)
            .is_some_and(|v| !v.entry.identities.is_empty());
        let old_reply_to = previous
            .as_ref()
            .and_then(|s| from_fields(previous_explicit, s).1);
        let old = signature_block(&previous.map(|s| s.signature).unwrap_or_default());
        let new = signature_block(&sender.signature);
        let (from_name, reply_to) = from_fields(explicit, &sender);
        if let Some(compose) = &mut self.compose {
            compose.error = None;
            if compose.body.contents() == old {
                compose.set_body(&new);
            }
            compose.account = account;
            compose.identity = sender.id;
            compose.draft.from = sender.address;
            compose.draft.from_name = from_name;
            // Like the signature: the old identity's Reply-To (or none)
            // follows the new identity; one the person typed stays.
            if compose.draft.reply_to.is_none() || compose.draft.reply_to == old_reply_to {
                compose.draft.reply_to = reply_to;
            }
            // The new identity's From is its current address.
            compose.stale = None;
            compose.revision += 1;
        }
    }

    /// Open a compose dialog from `account` (the default From when `None`)
    /// with `draft`'s fields; the account's address and signature fill in.
    pub fn open_compose(&mut self, account: Option<&str>, mut draft: Composed) -> bool {
        let Some(account) = account
            .map(str::to_owned)
            .or_else(|| self.compose_account())
        else {
            return false;
        };
        let Some(view) = self.account(&account) else {
            return false;
        };
        // The draft sends as the account's first identity; the person picks
        // another in the From list.
        let explicit = !view.entry.identities.is_empty();
        let senders = self.senders(&account);
        let Some(sender) = senders.first() else {
            return false;
        };
        draft.from = sender.address.clone();
        (draft.from_name, draft.reply_to) = from_fields(explicit, sender);
        let identity = sender.id.clone();
        // A compose already open is never dropped: its draft is kept and
        // offered again.
        if self.compose.is_some() {
            self.set_aside_compose();
        }
        let mut compose = ComposeState::new(&account, draft);
        if compose.error.is_none() && compose.body.doc.text().is_empty() {
            compose.set_body(&self.signed_body(&account));
        }
        compose.draft_id = self.new_draft_id();
        compose.identity = identity;
        if let Some(path) = compose.draft.attachments.last() {
            self.last_attachment_directory = path.parent().map(std::path::Path::to_path_buf);
        }
        self.attachment_notice = None;
        self.attachment_chooser = None;
        self.compose = Some(compose);
        self.check_compose_attachment_size();
        true
    }

    /// Close a newly opened compose whose body could not be loaded. Bus
    /// callers must report the refusal instead of claiming it opened.
    pub fn close_failed_compose(&mut self) -> Option<String> {
        let error = self.compose.as_ref()?.error.clone()?;
        self.compose = None;
        Some(error)
    }

    /// A reply to `account`'s open email (`all`: to everyone it went to).
    /// `quote`: with the original quoted, without it, or (`None`) as the
    /// settings say.
    pub fn open_reply(&mut self, account: &str, all: bool, quote: Option<bool>) -> bool {
        match self.account(account).and_then(|v| v.mail.open.clone()) {
            Some(open) => self.reply_to(account, &open, all, quote),
            None => false,
        }
    }

    /// A compose forwarding `email` from `account` to `to` (as typed): the
    /// subject marked, the original quoted under its headers.
    pub fn forward(&mut self, account: &str, email: &Email, to: &str) -> bool {
        let subject = match email.subject.as_deref() {
            Some(subject) if subject.to_lowercase().starts_with("fwd:") => subject.to_owned(),
            Some(subject) => format!("Fwd: {subject}"),
            None => crate::label("forward-subject"),
        };
        let addresses = |list: &Option<Vec<EmailAddress>>| {
            list.iter()
                .flatten()
                .map(jmap::format_address)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let quoted = crate::label_with(
            "forward-header",
            &[
                ("from", &addresses(&email.from)),
                ("date", &email.date_short()),
                ("subject", email.subject.as_deref().unwrap_or("")),
                ("to", &addresses(&email.to)),
            ],
        );
        let original = email.text_body_value().unwrap_or_default();
        let body = format!(
            "{}\n\n{quoted}\n\n{original}",
            self.signed_body(account).trim_end()
        );
        let opened = self.open_compose(
            Some(account),
            Composed {
                subject,
                body,
                ..Composed::default()
            },
        );
        if opened && let Some(compose) = &mut self.compose {
            compose.to_text = to.to_owned();
        }
        opened
    }

    /// A compose that sends `email` again as a new message from `account`:
    /// its To, Cc, subject and text body as they were, unmarked and
    /// unquoted, threaded to nothing. It sends as the identity the original
    /// came from when that is one of the account's, else the default; the
    /// body is taken as it is (its signature included), never re-signed.
    /// Received attachments do not come along when sending as new.
    pub fn send_as_new(&mut self, account: &str, email: &Email) -> bool {
        let list = |list: &Option<Vec<EmailAddress>>| list.clone().unwrap_or_default();
        let body = email.text_body_value().unwrap_or_default().to_owned();
        let opened = self.open_compose(
            Some(account),
            Composed {
                to: list(&email.to),
                cc: list(&email.cc),
                subject: email.subject.clone().unwrap_or_default(),
                body,
                ..Composed::default()
            },
        );
        if !opened {
            return false;
        }
        let from = email
            .from
            .as_ref()
            .and_then(|f| f.first())
            .map(|a| a.email.to_ascii_lowercase());
        let identity = from.and_then(|from| {
            self.senders(account)
                .into_iter()
                .find(|s| s.address.eq_ignore_ascii_case(&from))
                .map(|s| s.id)
        });
        let current = self.compose.as_ref().map(|c| c.identity.clone());
        if let Some(identity) = identity
            && current.as_deref() != Some(identity.as_str())
        {
            self.choose_sender(account.to_owned(), Some(identity));
            // Part of opening it, not an edit: an untouched compose still
            // closes without offering to keep a draft.
            if let Some(compose) = &mut self.compose {
                compose.revision = 0;
                compose.saved = 0;
            }
        }
        true
    }

    /// A compose replying to `email` from `account` (`all`: to everyone it
    /// went to but this account).
    pub fn reply_to(
        &mut self,
        account: &str,
        open: &Email,
        all: bool,
        quote: Option<bool>,
    ) -> bool {
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
        let reply = quote
            .unwrap_or_else(|| self.quote_on_reply())
            .then(|| ReplyQuote {
                quote: crate::quote::quote_block(&quotable(open), &attribution(open)),
                signature: self.signed_body(account),
                position: self.reply_position(),
            });
        let body = match &reply {
            Some(r) => crate::quote::arrange("", &r.quote, &r.signature, r.position),
            None => String::new(),
        };
        let opened = self.open_compose(
            Some(account),
            Composed {
                to,
                cc,
                subject,
                body,
                in_reply_to: open
                    .message_id
                    .clone()
                    .and_then(|ids| ids.into_iter().next()),
                ..Composed::default()
            },
        );
        if opened && let Some(compose) = &mut self.compose {
            compose.reply = reply;
        }
        opened
    }

    /// Apply one frame's interactions.
    pub fn apply_ui(&mut self, event: crate::view::UiEvent) {
        use crate::view::UiEvent;
        let check_size = matches!(
            &event,
            UiEvent::ComposeField(..)
                | UiEvent::ComposeEdit(..)
                | UiEvent::ComposeIdentity { .. }
                | UiEvent::ComposeAccount(..)
                | UiEvent::ComposeUseNewFrom
                | UiEvent::ComposeReplyPosition(..)
                | UiEvent::SuggestAccept(..)
        );
        match event {
            UiEvent::ComposeAttachmentRemove(path) => {
                self.remove_compose_attachment(&path);
            }
            UiEvent::CloseMacros => self.ui.macros_open = false,
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
            UiEvent::MailTable(account, event) => self.mail_table(&account, event),
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
                    self.open_reply(&account, false, None);
                }
            }
            UiEvent::ComposeAccount(account) => {
                if self.compose.as_ref().is_none_or(|c| c.account == account) {
                    return;
                }
                self.choose_sender(account, None);
            }
            UiEvent::ComposeIdentity { account, identity } => {
                self.choose_sender(account, Some(identity));
            }
            UiEvent::PaneFocused(pane) => self.pane = pane,
            UiEvent::SetReaderLight(on) => {
                // Saved in the profile: the reader's own style, not a session flag.
                let content = settings::content::Content {
                    light: on,
                    ..self.settings.reader.clone()
                };
                self.write(
                    writer::Change::Surface(writer::Surface::Reader, content),
                    None,
                    None,
                    false,
                    None,
                );
            }
            UiEvent::OpenIdentities(account) => self.open_identities(&account),
            UiEvent::IdentityEdit(field, value) => self.identity_edit(field, value),
            UiEvent::IdentitySelect(index) => self.identity_select(index),
            UiEvent::IdentityAdd => self.identity_add(),
            UiEvent::IdentityRemove => self.identity_remove(),
            UiEvent::IdentitySave => {
                self.identity_save();
            }
            UiEvent::CloseIdentities => self.close_identities(),
            // A Cc, Bcc or Reply-To button: the row opens, or closes again
            // (a row holding something stays while it does).
            UiEvent::ComposeShow(field) => {
                if let Some(compose) = &mut self.compose
                    && ComposeField::OPTIONAL.contains(&field)
                {
                    if let Some(at) = compose.extra.iter().position(|f| *f == field) {
                        compose.extra.remove(at);
                    } else {
                        compose.extra.push(field);
                    }
                }
            }
            UiEvent::ComposeEdit(event) => {
                if let Some(compose) = &mut self.compose
                    && !compose.busy
                {
                    let before = compose.body.doc.doc().revision;
                    compose.body.doc.apply(&event, &editor::View::prose());
                    if compose.body.doc.doc().revision != before {
                        compose.body.revision += 1;
                        compose.revision += 1;
                        compose.error = None;
                    }
                    if let Some(error) = compose.body.doc.take_refusal() {
                        compose.error = Some(error.to_string());
                    }
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
                        ComposeField::ReplyTo => {
                            compose.draft.reply_to = Some(value).filter(|v| !v.trim().is_empty());
                        }
                        ComposeField::Subject => compose.draft.subject = value,
                        ComposeField::Body => {
                            if !compose.set_body(&value) {
                                return;
                            }
                        }
                    }
                    compose.revision += 1;
                    // A row being edited stays open, even cleared to retype.
                    if ComposeField::OPTIONAL.contains(&field) && !compose.extra.contains(&field) {
                        compose.extra.push(field);
                    }
                    if matches!(
                        field,
                        ComposeField::To | ComposeField::Cc | ComposeField::Bcc
                    ) {
                        self.suggest_typed(field);
                    }
                }
            }
            UiEvent::SuggestMove(step) => {
                if let Some(suggest) = self.compose.as_mut().and_then(|c| c.suggest.as_mut())
                    && !suggest.items.is_empty()
                {
                    let last = suggest.items.len() - 1;
                    suggest.cursor = if step < 0 {
                        suggest.cursor.saturating_sub(1)
                    } else {
                        (suggest.cursor + 1).min(last)
                    };
                }
            }
            UiEvent::SuggestAccept(index) => self.suggest_accept(index),
            UiEvent::SuggestClose => {
                if let Some(compose) = self.compose.as_mut() {
                    compose.suggest = None;
                }
            }
            UiEvent::DocumentReplace(replace) => self.confirm_document_replace(replace),
            UiEvent::CloseCompose => self.close_compose(),
            UiEvent::CancelFetch => self.cancel_fetch(),
            UiEvent::ConfirmDraft(choice) => self.confirm_draft(choice),
            UiEvent::RestoreDraft(choice) => self.restore_draft(choice),
            UiEvent::ComposeReplyPosition(position) => {
                let Some(compose) = &mut self.compose else {
                    return;
                };
                if compose.busy {
                    return;
                }
                let Some(reply) = compose.reply.as_ref() else {
                    return;
                };
                if reply.position == position {
                    return;
                }
                // The person's text stays; only the quote and signature move.
                // An edited quote is left alone, and so is the toggle.
                let Some(body) = crate::quote::moved(
                    &compose.body.contents(),
                    &reply.quote,
                    &reply.signature,
                    position,
                ) else {
                    return;
                };
                if compose.set_body(&body) {
                    compose
                        .reply
                        .as_mut()
                        .expect("reply is still present")
                        .position = position;
                    compose.revision += 1;
                }
            }
            UiEvent::ComposeUseNewFrom => {
                // Only for the account and connection the address came from.
                let current = self.compose.as_ref().and_then(|c| {
                    let view = self.account(&c.account)?;
                    Some((c.account.clone(), view.generation))
                });
                let address = self
                    .compose
                    .as_ref()
                    .and_then(|c| self.account(&c.account))
                    .map(|v| v.entry.email.clone());
                if let Some(compose) = &mut self.compose
                    && let Some(stale) = compose.stale.as_mut()
                {
                    if current == Some((stale.account.clone(), stale.generation)) {
                        compose.draft.from = stale.address.clone();
                        compose.stale = None;
                        compose.error = None;
                    } else if let (Some((account, generation)), Some(address)) = (current, address)
                    {
                        // The connection moved on since: offer its address
                        // instead, and Send stays blocked until taken.
                        *stale = StaleFrom {
                            account,
                            generation,
                            address,
                        };
                    }
                }
            }
            UiEvent::SendCompose => self.send_compose(),
            UiEvent::Retry(account) => {
                self.reconnect(&account);
            }
            UiEvent::OpenPrefs => self.open_prefs(),
            UiEvent::SaveAttachment(part) => self.save_attachment(&part, false),
            UiEvent::OpenAttachment(part) => self.save_attachment(&part, true),
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
            UiEvent::FormField(field, value) => match &mut self.form {
                Some(Form::Event(f)) if !f.busy => match field {
                    FormField::Title => f.title = value,
                    FormField::Start => f.start = value,
                    FormField::End => f.end = value,
                    FormField::Location => f.location = value,
                    _ => {}
                },
                Some(Form::Contact(f)) if !f.busy => match field {
                    FormField::Name => f.name = value,
                    FormField::Email => f.email = value,
                    FormField::Phone => f.phone = value,
                    FormField::Organization => f.organization = value,
                    _ => {}
                },
                _ => {}
            },
            UiEvent::FormSubmit(choice) => self.submit_form(choice),
            UiEvent::FormCancel => {
                self.form = None;
                self.form_note = None;
            }
            UiEvent::CloseDialog => self.ui.dialog = None,
            UiEvent::SetMode(mode) => {
                self.ui.theme.mode = Some(mode);
            }
        }
        if check_size {
            self.check_compose_attachment_size();
        }
    }

    /// Refresh file metadata and the whole encoded-message estimate on an
    /// explicit compose event. No file contents are opened or kept here.
    fn refresh_compose_attachments(&mut self) {
        if let Some(compose) = self.compose.as_mut()
            && !compose.busy
        {
            compose.attachments = ComposeAttachment::inspect_all(&compose.draft.attachments);
        }
        self.check_compose_attachment_size();
    }

    pub(crate) fn check_compose_attachment_size(&mut self) {
        let limit = self
            .compose
            .as_ref()
            .and_then(|c| self.account(&c.account))
            .and_then(|a| a.session.as_ref())
            .and_then(|s| s.max_size_upload);
        let default_limit = limit.is_none();
        let Some(compose) = self.compose.as_mut() else {
            return;
        };
        if compose.busy {
            return;
        }
        compose.attachments_over_limit = false;
        compose.attachment_error = None;
        if compose.attachments.is_empty() {
            return;
        }
        if compose
            .attachments
            .iter()
            .any(|a| a.state != ComposeAttachmentState::Ready)
        {
            compose.attachment_error = Some(crate::label("compose-attachments-fix"));
        }
        let draft = Composed {
            to: jmap::parse_addresses(&compose.to_text).unwrap_or_default(),
            cc: jmap::parse_addresses(&compose.cc_text).unwrap_or_default(),
            bcc: jmap::parse_addresses(&compose.bcc_text).unwrap_or_default(),
            body: compose.body.contents(),
            ..compose.draft.clone()
        };
        let size = jmap::compose::estimated_message_size(
            &draft,
            compose
                .attachments
                .iter()
                .map(|a| (a.path.as_path(), a.size.unwrap_or(0))),
        );
        if let Err(error) = jmap::check_message_size(size, limit) {
            let message = match error {
                jmap::Error::MessageTooLarge { size, limit } => {
                    compose.attachments_over_limit = true;
                    crate::label_with(
                        if default_limit {
                            "compose-attachments-too-large-default"
                        } else {
                            "compose-attachments-too-large"
                        },
                        &[
                            ("size", &crate::attachments::human_size(size)),
                            ("limit", &crate::attachments::human_size(limit)),
                        ],
                    )
                }
                _ => crate::label("compose-attachments-fix"),
            };
            compose.attachment_error = Some(match compose.attachment_error.take() {
                Some(files) => format!("{files}\n{message}"),
                None => message,
            });
        }
    }

    /// All attachment entry points share validation, order and draft revision.
    pub fn add_compose_attachments(
        &mut self,
        paths: &[std::path::PathBuf],
    ) -> Result<(), (&'static str, String)> {
        self.inspect_compose_attachments(paths, None, false)
    }

    pub(crate) fn inspect_compose_attachments(
        &mut self,
        paths: &[std::path::PathBuf],
        reply: Option<u64>,
        opening: bool,
    ) -> Result<(), (&'static str, String)> {
        let ticket = self.ticket();
        let compose = self
            .compose
            .as_mut()
            .ok_or_else(|| (code::NOT_FOUND, crate::label("compose-not-open")))?;
        if compose.busy {
            return Err((code::BUSY, crate::label("sending")));
        }
        let mut draft = compose.draft.clone();
        draft.body = compose.body.contents();
        for path in paths {
            if opening || !draft.attachments.contains(path) {
                draft.attachments.push(path.clone());
            }
        }
        compose.attachment_inspections.insert(ticket);
        let mut inspection = crate::work::ComposeInspection {
            ticket,
            draft_id: compose.draft_id.clone(),
            revision: compose.revision,
            reply,
            opening,
            paths: paths.to_vec(),
            draft,
            limit: None,
        };
        if let Some(compose) = &self.compose {
            inspection.limit = self
                .account(&compose.account)
                .and_then(|a| a.session.as_ref())
                .and_then(|s| s.max_size_upload);
        }
        self.work.push(Work::InspectComposeAttachments(inspection));
        Ok(())
    }

    fn compose_attachments_inspected(
        &mut self,
        inspection: crate::work::ComposeInspection,
        result: crate::work::InspectedAttachments,
    ) {
        // A restored draft may reuse its id and revision. Only the compose
        // instance that queued this ticket can accept its result.
        let pending = self
            .compose
            .as_mut()
            .filter(|c| c.draft_id == inspection.draft_id)
            .is_some_and(|c| c.attachment_inspections.remove(&inspection.ticket));
        let refusal = match &self.compose {
            None => Some((code::NOT_FOUND, crate::label("compose-not-open"))),
            Some(c) if !pending || c.revision != inspection.revision => {
                Some((code::STALE, crate::label("compose-attachments-stale")))
            }
            Some(c) if c.busy => Some((code::BUSY, crate::label("sending"))),
            _ => None,
        };
        let superseded = refusal.is_some();
        let result = match refusal {
            Some(error) => Err(error),
            None => result,
        };
        match result {
            Err((code, message)) => {
                // An invalid new compose closes only the version that asked.
                if inspection.opening && !superseded {
                    self.compose = None;
                }
                if let Some(reply) = inspection.reply {
                    self.refuse(reply, code, &message);
                } else if !superseded {
                    self.attachment_notice = Some(message);
                }
            }
            Ok(attachments) => {
                let compose = self.compose.as_mut().expect("checked compose version");
                let added = inspection.draft.attachments != compose.draft.attachments;
                if added {
                    self.last_attachment_directory = inspection
                        .draft
                        .attachments
                        .last()
                        .and_then(|p| p.parent())
                        .map(std::path::Path::to_path_buf);
                    compose.draft.attachments = inspection.draft.attachments;
                    compose.revision += 1;
                }
                compose.attachments = attachments;
                self.attachment_notice = None;
                self.check_compose_attachment_size();
                if let Some(reply) = inspection.reply {
                    let mut body = json!({ "compose": self.info()["compose"] });
                    if inspection.opening {
                        body["composing"] = json!(self.compose.as_ref().unwrap().account);
                    }
                    self.reply(reply, 0, body);
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn finish_compose_inspections(&mut self) {
        let work = std::mem::take(&mut self.work);
        for item in work {
            if let Work::InspectComposeAttachments(inspection) = item {
                let result = inspection.inspect();
                self.apply(crate::shell::Event::ComposeAttachments(inspection, result));
            } else {
                self.work.push(item);
            }
        }
    }

    pub fn attach_files(&mut self) {
        if self.compose.as_ref().is_some_and(|c| !c.busy) && self.attachment_chooser.is_none() {
            self.attachment_chooser = Some(toolkit::file_chooser::FileChooser::new(
                self.last_attachment_directory
                    .clone()
                    .unwrap_or_else(toolkit::file_chooser::home),
                crate::label("compose-attach-title"),
                crate::label("compose-attach-confirm"),
            ));
        }
    }

    pub fn clear_compose_attachments(&mut self) {
        let Some(compose) = self.compose.as_mut().filter(|c| !c.busy) else {
            return;
        };
        if !compose.draft.attachments.is_empty() {
            compose.draft.attachments.clear();
            compose.revision += 1;
        }
        self.attachment_notice = None;
        self.refresh_compose_attachments();
    }

    /// The chip and Bus intent share this mutation, including draft revision.
    pub fn remove_compose_attachment(&mut self, path: &std::path::Path) -> bool {
        let Some(compose) = self.compose.as_mut() else {
            return false;
        };
        if compose.busy {
            return false;
        }
        let before = compose.draft.attachments.len();
        compose.draft.attachments.retain(|p| p != path);
        if compose.draft.attachments.len() == before {
            return false;
        }
        compose.revision += 1;
        self.refresh_compose_attachments();
        true
    }

    /// Send the open compose, when its recipients and attachments are valid.
    pub fn send_compose(&mut self) {
        if self
            .compose
            .as_ref()
            .is_some_and(|c| !c.attachment_inspections.is_empty())
        {
            return;
        }
        self.refresh_compose_attachments();
        let Some(compose) = &self.compose else {
            return;
        };
        if compose.busy || compose.attachment_error.is_some() {
            return;
        }
        // The account's login changed under this draft: it is not sent
        // from the old address through the new login unasked.
        if compose.stale.is_some() {
            if let Some(compose) = &mut self.compose {
                compose.error = Some(crate::label("compose-from-changed"));
            }
            return;
        }
        if let Some(compose) = &self.compose
            && let Err((_, message)) = crate::documents::identity(
                self,
                &compose.account,
                Some(&compose.draft.from),
                compose.draft.reply_to.as_deref(),
            )
        {
            if let Some(compose) = &mut self.compose {
                compose.error = Some(message);
            }
            return;
        }
        let account = compose.account.clone();
        // State-wide, so a removed and restored account never reuses one.
        let ticket = self.ticket();
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return;
        };
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
                let origin = self.input_origin.clone().unwrap_or_default();
                compose.send_origin = Some(origin.clone());
                let mut draft = compose.draft.clone();
                draft.body = compose.body.contents();
                let attempt = compose.attempt.clone();
                let snapshot = compose.saved_draft();
                view.tickets.send = ticket;
                self.work.push(Work::Send {
                    account,
                    ticket,
                    draft,
                    attempt,
                    origin,
                });
                // Kept on disk until the server confirms it went, so a
                // crash or a lost reply mid-send loses nothing.
                self.store_draft(true);
                self.sends.insert(ticket, snapshot);
            }
        }
    }

    /// Launch Prefs on its Email section (where accounts are configured).
    pub fn open_prefs(&mut self) {
        self.queue(Work::OpenPrefs);
    }

    pub(crate) fn with_send_origin<R>(
        &mut self,
        origin: Option<SendOrigin>,
        apply: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous = std::mem::replace(&mut self.input_origin, origin);
        let result = apply(self);
        self.input_origin = previous;
        result
    }

    /// Whether a compose is sendable right now (the dialog's Send button).
    pub fn compose_sendable(&self) -> bool {
        match &self.compose {
            Some(compose) => {
                !compose.busy
                    && compose.attachment_inspections.is_empty()
                    && compose.attachment_error.is_none()
                    && self.account(&compose.account).is_some()
                    && !compose.to_text.trim().is_empty()
                    && !compose.draft.subject.trim().is_empty()
            }
            None => false,
        }
    }

    /// `account`'s Drafts and Sent mailboxes, for the send recipe, or why
    /// there are none yet (the catalogue key).
    pub fn send_boxes(&self, account: &str) -> Result<jmap::SendBoxes, &'static str> {
        let mail = &self.account(account).ok_or("not-connected")?.mail;
        let id = |role: &str| mail.by_role(role).map(|m| m.id.clone());
        Ok(jmap::SendBoxes {
            drafts: id("drafts").ok_or("no-drafts-mailbox")?,
            sent: id("sent").ok_or("no-sent-mailbox")?,
        })
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

    /// A command asked the window to close: it does, unless a draft open
    /// with something typed needs the person's answer first.
    pub fn quit(&mut self) {
        if self.request_quit() {
            self.exiting = true;
        }
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

    /// Show the open email's text body as Markdown or as its source, the
    /// other way from now (`view.reader.markdown`).
    pub fn toggle_reader(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if let Some(view) = self.account_mut(&id)
            && let Some(open) = &view.mail.open
        {
            view.mail.reader.toggle(open);
        }
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

/// The text a reply quotes: the text body (a Markdown message's source),
/// else the HTML body's text.
fn quotable(email: &Email) -> String {
    let html_part = email
        .text_body
        .as_ref()
        .and_then(|parts| parts.first())
        .and_then(|part| part.content_type.as_deref())
        .is_some_and(|t| t.trim().to_ascii_lowercase().starts_with("text/html"));
    match email.text_body_value() {
        Some(text) if html_part => crate::quote::html_text(text),
        Some(text) => text.to_owned(),
        None => email
            .html_body_value()
            .map(crate::quote::html_text)
            .unwrap_or_default(),
    }
}

/// "On Fri 17 Oct 2026, 09:14, Ann wrote:", in the local time zone.
fn attribution(email: &Email) -> String {
    let when = email
        .date
        .as_deref()
        .or(email.received_at.as_deref())
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%a %-d %b %Y, %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    let who = email
        .from
        .as_ref()
        .and_then(|from| from.first())
        .map(|a| match a.name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => a.email.clone(),
        })
        .unwrap_or_default();
    crate::label_with("reply-attribution", &[("date", &when), ("name", &who)])
}

/// `email`'s attachments as `email.info` and `email.attachments` list
/// them (no content), with each one's save or open state.
pub fn attachments_view(email: &Email, transfers: Option<&BTreeMap<String, Transfer>>) -> Value {
    email
        .attachments
        .iter()
        .flatten()
        .enumerate()
        .map(|(index, a)| {
            json!({
                "part": a.part_id,
                "name": crate::attachments::file_name(a.name.as_deref(), index + 1, a.content_type.as_deref()),
                "type": a.content_type,
                "size": a.size,
                "undecodable": !a.downloadable(),
                "save_only": crate::attachments::refuses_open(
                    &crate::attachments::file_name(a.name.as_deref(), index + 1, a.content_type.as_deref()),
                    a.content_type.as_deref(),
                ),
                "transfer": a.part_id.as_ref().and_then(|p| transfers.and_then(|t| t.get(p))),
            })
        })
        .collect()
}

/// The work that saves (or opens) `email`'s attachment `part` from
/// `account`, or why it cannot: no such part, nothing to fetch, or an
/// executable asked to open.
pub fn attachment_work(
    account: &str,
    email: &Email,
    part: &str,
    open: bool,
    dir: Option<String>,
    reply: Option<u64>,
) -> Result<Work, (&'static str, String)> {
    let (index, attachment) = email
        .attachments
        .iter()
        .flatten()
        .enumerate()
        .find(|(_, a)| a.part_id.as_deref() == Some(part))
        .ok_or_else(|| (code::NOT_FOUND, crate::label("no-such-attachment")))?;
    let blob = attachment
        .blob_id
        .clone()
        .filter(|_| attachment.downloadable())
        .ok_or_else(|| (code::REFUSED, crate::label("attachment-undecodable")))?;
    let name = crate::attachments::file_name(
        attachment.name.as_deref(),
        index + 1,
        attachment.content_type.as_deref(),
    );
    if open && crate::attachments::refuses_open(&name, attachment.content_type.as_deref()) {
        return Err((code::REFUSED, crate::label("attachment-save-only")));
    }
    if let Some(dir) = &dir
        && !std::path::Path::new(dir).is_absolute()
    {
        return Err((code::INVALID_ARGUMENT, crate::label("dir-not-absolute")));
    }
    Ok(Work::Attachment {
        account: account.to_owned(),
        email: email.id.clone(),
        part: part.to_owned(),
        blob,
        name,
        open,
        dir,
        reply,
    })
}

/// The keyword a keyword change sets or clears, and which.
fn keyword_of(change: &jmap::EmailChange) -> Option<(&'static str, bool)> {
    match change {
        jmap::EmailChange::Keywords {
            flagged: Some(on), ..
        } => Some(("$flagged", *on)),
        jmap::EmailChange::Keywords { read: Some(on), .. } => Some(("$seen", *on)),
        _ => None,
    }
}

/// Whether `email` holds `keyword`.
fn has_keyword(email: &Email, keyword: &str) -> bool {
    email
        .keywords
        .as_ref()
        .and_then(|k| k.get(keyword))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The refusal of a "Select all N" past [`SELECT_ALL_CAP`].
fn too_many(count: u64) -> String {
    crate::label_with(
        "too-many-selected",
        &[
            ("count", &grouped(count)),
            ("cap", &grouped(SELECT_ALL_CAP)),
        ],
    )
}

/// The row to open after `open` and the other `gone` rows leave `emails`
/// (in shown order): the next one that stays, else the previous one.
pub fn neighbour(emails: &[Email], open: &str, gone: &[String]) -> Option<String> {
    let at = emails.iter().position(|e| e.id == open)?;
    let stays = |e: &&Email| !gone.contains(&e.id);
    emails[at + 1..]
        .iter()
        .find(stays)
        .or_else(|| emails[..at].iter().rev().find(stays))
        .map(|e| e.id.clone())
}

/// Where a junk move of `view`'s emails goes: the role=junk mailbox, or
/// (`junk == false`) the inbox.
pub fn junk_target(view: &AccountView, junk: bool) -> Option<String> {
    view.mail
        .by_role(if junk { "junk" } else { "inbox" })
        .map(|m| m.id.clone())
}

/// Whether `view` is showing its Junk mailbox (the open email is in it).
pub fn in_junk(view: &AccountView) -> bool {
    view.mail
        .selected_mailbox()
        .is_some_and(|m| m.role.as_deref() == Some("junk"))
}

/// The account a form is for.
fn form_account(form: &Form) -> &str {
    match form {
        Form::Delete { account, .. } | Form::Move { account, .. } => account,
        Form::Event(f) => &f.account,
        Form::Contact(f) => &f.account,
    }
}

/// The connection a form was opened on.
fn form_generation(form: &Form) -> u64 {
    match form {
        Form::Delete { generation, .. } | Form::Move { generation, .. } => *generation,
        Form::Event(f) => f.generation,
        Form::Contact(f) => f.generation,
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
    // Reply-To is sent as typed, so it is checked like the others: one
    // address (a name with it is fine).
    if let Some(reply_to) = &compose.draft.reply_to
        && jmap::parse_addresses(reply_to)?.len() != 1
    {
        return Err(format!("Reply-To must be one address: {reply_to:?}"));
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
            identities: Vec::new(),
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
            max_size_upload: None,
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
        assert!(
            work.iter()
                .any(|w| matches!(w, Work::Disconnect { account } if account == "c"))
        );
        // Then c is removed elsewhere: its view goes, its password stays
        // (review finding 1: a settings read never deletes a secret).
        state.set_settings(settings(&["a", "b", "d"]));
        assert!(state.account("c").is_none());
        let work = state.take_work();
        assert!(!work.iter().any(|w| matches!(w, Work::SaveSecrets { .. })));
        assert!(state.has_password("c"));
        // The order of the settings is the tree's.
        state.set_settings(settings(&["d", "a", "b"]));
        let order: Vec<&str> = state.accounts.iter().map(|v| v.id()).collect();
        assert_eq!(order, ["d", "a", "b"]);
    }

    #[test]
    fn a_late_answer_from_one_account_never_touches_another() {
        let mut state = connected_many(&["a", "b"]);
        done(
            &mut state,
            "a",
            Done::Mailboxes(Ok(vec![mailbox("in", Some("inbox"), 2)])),
        );
        done(
            &mut state,
            "b",
            Done::Mailboxes(Ok(vec![mailbox("in", Some("inbox"), 5)])),
        );
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
        assert!(matches!(
            state.account("a").unwrap().phase,
            Phase::Failed(_)
        ));
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
        let version = state.secrets_snapshot().0;
        state.secrets_saved(ticket, version, Ok(true));
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
        assert_eq!(
            state.account("a").unwrap().mail.selected.as_deref(),
            Some("m-in")
        );
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
        done(
            &mut state,
            "a",
            Done::Mailboxes(Ok(vec![mailbox("in", Some("inbox"), 0)])),
        );
        state.take_work();
        let mut next = settings(&["a"]);
        next.mail.page_size = 25;
        state.set_settings(next);
        assert!(
            state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::FetchEmails { limit: 25, .. }))
        );
    }

    #[test]
    fn reply_follows_the_quote_and_position_settings() {
        let mut state = connected();
        // The defaults: a reply quotes, with the person's text above it.
        assert!(state.quote_on_reply());
        assert_eq!(state.reply_position(), crate::quote::Position::Above);
        let mut next = state.settings.clone();
        next.mail.quote_on_reply = false;
        next.mail.reply_position = settings::email::ReplyPosition::Below;
        state.set_settings(next);
        assert!(!state.quote_on_reply());
        assert_eq!(state.reply_position(), crate::quote::Position::Below);
    }

    #[test]
    fn a_reply_moves_its_quote_and_keeps_what_was_typed() {
        use crate::quote::{Position, arrange};
        let mut state = connected();
        state.account_mut("a").unwrap().mail.open = Some(unread_email("E1"));
        state.apply_ui(UiEvent::Reply);
        let compose = state.compose.clone().expect("a compose dialog");
        let reply = compose.reply.clone().expect("a quoted reply");
        assert_eq!(reply.position, Position::Above);
        let typed = format!("Thanks!{}", compose.body.contents());
        state.apply_ui(UiEvent::ComposeField(ComposeField::Body, typed));

        state.apply_ui(UiEvent::ComposeReplyPosition(Position::Below));
        let compose = state.compose.clone().unwrap();
        assert_eq!(compose.reply.as_ref().unwrap().position, Position::Below);
        assert_eq!(
            compose.body.contents(),
            arrange("Thanks!", &reply.quote, &reply.signature, Position::Below)
        );
        assert!(compose.body.contents().starts_with(&reply.quote));

        state.apply_ui(UiEvent::ComposeReplyPosition(Position::Above));
        let compose = state.compose.clone().unwrap();
        assert_eq!(
            compose.body.contents(),
            arrange("Thanks!", &reply.quote, &reply.signature, Position::Above)
        );

        // An edited quote is left as the person wrote it: the switch does nothing.
        let edited = compose.body.contents().replace("wrote:", "wrote this:");
        assert_ne!(
            edited,
            compose.body.contents(),
            "the attribution is in the quote"
        );
        state.apply_ui(UiEvent::ComposeField(ComposeField::Body, edited.clone()));
        state.apply_ui(UiEvent::ComposeReplyPosition(Position::Below));
        let compose = state.compose.clone().unwrap();
        assert_eq!(compose.body.contents(), edited);
        assert_eq!(compose.reply.unwrap().position, Position::Above);
    }

    #[test]
    fn the_identities_dialog_saves_a_valid_list_and_keeps_a_refused_one_open() {
        let mut state = connected();
        state.open_identities("a");
        let rows = state.identity_editor.as_ref().unwrap().rows.clone();
        assert_eq!(
            rows.len(),
            1,
            "an account with none shows its default sender"
        );
        state.identity_add();
        state.identity_edit(IdentityField::Name, "Ann Work".into());
        state.identity_edit(IdentityField::Address, "ann@work.example.invalid".into());
        state.identity_edit(IdentityField::ReplyTo, "help@work.example.invalid".into());
        assert!(state.identity_save(), "a valid list saves");
        assert!(state.identity_editor.is_none(), "and the dialog closes");

        state.open_identities("a");
        state.identity_add();
        assert!(!state.identity_save(), "an empty address is refused");
        let open = state
            .identity_editor
            .as_ref()
            .expect("the dialog stays open");
        let error = open.error.as_deref().unwrap();
        assert!(error.contains("identities["), "{error}");
        state.identity_remove();
        state.identity_remove();
        assert!(state.identity_editor.as_ref().unwrap().rows.is_empty());
        state.close_identities();
        assert!(state.identity_editor.is_none());
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
        assert!(state.open_reply("a", true, Some(false)));
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
        assert_eq!(compose.body.contents(), "\n\n-- \nAnn\nGold Coast\n");
        // Switching From swaps an untouched signature.
        state.apply_ui(UiEvent::ComposeAccount("a".into()));
        let compose = state.compose.clone().unwrap();
        assert_eq!(compose.draft.from, "ann@a.example.invalid");
        assert!(compose.body.contents().is_empty());
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
        assert!(compose.body.contents().is_empty());
    }

    #[test]
    fn an_empty_compose_cannot_send() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        assert!(!state.compose_sendable());
    }

    fn compose_files(state: &mut State, paths: Vec<std::path::PathBuf>) {
        assert!(state.open_compose(
            Some("a"),
            Composed {
                to: jmap::parse_addresses("bob@example.invalid").unwrap(),
                subject: "Files".into(),
                attachments: paths,
                ..Composed::default()
            }
        ));
        state.take_work();
    }

    #[test]
    fn inspection_cannot_land_in_a_reopened_copy_of_the_same_draft() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one.txt");
        std::fs::write(&path, b"one").unwrap();
        let mut state = connected();
        compose_files(&mut state, Vec::new());
        state.add_compose_attachments(&[path]).unwrap();
        let inspection = state
            .take_work()
            .into_iter()
            .find_map(|work| match work {
                Work::InspectComposeAttachments(inspection) => Some(inspection),
                _ => None,
            })
            .unwrap();
        let saved = state.compose.as_ref().unwrap().saved_draft();
        state.compose = None;
        assert!(state.open_saved(saved));
        assert_eq!(
            state.compose.as_ref().unwrap().draft_id,
            inspection.draft_id
        );
        assert_eq!(
            state.compose.as_ref().unwrap().revision,
            inspection.revision
        );
        let result = inspection.inspect();
        state.apply(Event::ComposeAttachments(inspection, result));
        assert!(state.compose.as_ref().unwrap().draft.attachments.is_empty());
        assert!(state.compose.as_ref().unwrap().attachments.is_empty());
    }

    #[test]
    fn attachment_add_deduplicates_atomically_and_clear_obeys_busy() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one");
        let two = dir.path().join("two");
        std::fs::write(&one, b"one").unwrap();
        std::fs::write(&two, b"two").unwrap();
        let mut state = connected();
        compose_files(&mut state, vec![one.clone()]);
        let revision = state.compose.as_ref().unwrap().revision;
        assert!(
            state
                .add_compose_attachments(&[one.clone(), two.clone(), one.clone()])
                .is_ok()
        );
        state.finish_compose_inspections();
        assert_eq!(
            state.compose.as_ref().unwrap().draft.attachments,
            [one.clone(), two.clone()]
        );
        assert_eq!(state.compose.as_ref().unwrap().revision, revision + 1);
        assert_eq!(state.last_attachment_directory.as_deref(), Some(dir.path()));
        state
            .add_compose_attachments(&[dir.path().join("missing")])
            .unwrap();
        state.finish_compose_inspections();
        assert!(state.attachment_notice.is_some());
        assert_eq!(
            state.compose.as_ref().unwrap().draft.attachments,
            [one.clone(), two.clone()]
        );
        state.compose.as_mut().unwrap().busy = true;
        assert_eq!(
            state.add_compose_attachments(&[]).unwrap_err().0,
            code::BUSY
        );
        state.clear_compose_attachments();
        assert_eq!(state.compose.as_ref().unwrap().draft.attachments.len(), 2);
        state.compose.as_mut().unwrap().busy = false;
        state.clear_compose_attachments();
        let compose = state.compose.as_ref().unwrap();
        assert!(compose.draft.attachments.is_empty() && compose.attachments.is_empty());
        assert!(compose.attachment_error.is_none());
        assert_eq!(compose.revision, revision + 2);
        assert!(state.add_compose_attachments(&[two]).is_ok());
        state.finish_compose_inspections();
        state
            .account_mut("a")
            .unwrap()
            .session
            .as_mut()
            .unwrap()
            .max_size_upload = Some(1);
        assert!(state.add_compose_attachments(&[one]).is_ok());
        state.finish_compose_inspections();
        assert!(state.compose.as_ref().unwrap().attachments_over_limit);
        assert!(!state.compose_sendable());
    }

    #[test]
    fn compose_lists_paths_sizes_and_removes_through_the_chip_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"hello").unwrap();
        let mut state = connected();
        compose_files(&mut state, vec![path.clone(), path.clone()]);
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.attachments.len(), 2);
        assert_eq!(compose.attachments[0].name, "note.txt");
        assert_eq!(compose.attachments[0].size, Some(5));
        assert_eq!(compose.attachments[0].state, ComposeAttachmentState::Ready);
        assert_eq!(compose.attachment_size(), 10);
        assert!(
            compose.keepable(),
            "paths alone are worth keeping as a draft"
        );
        assert!(state.compose_sendable());
        let revision = compose.revision;
        state.apply_ui(UiEvent::ComposeAttachmentRemove(path));
        let compose = state.compose.as_ref().unwrap();
        assert!(compose.draft.attachments.is_empty());
        assert!(compose.attachments.is_empty());
        assert_eq!(compose.revision, revision + 1);
        assert!(state.compose_sendable());
        assert!(compose.saved_draft().attachments.is_empty());
    }

    #[test]
    fn bad_compose_files_disable_send_and_remain_listed() {
        let dir = tempfile::tempdir().unwrap();
        let loop_path = dir.path().join("loop");
        std::os::unix::fs::symlink(&loop_path, &loop_path).unwrap();
        for (path, expected) in [
            (dir.path().join("missing"), ComposeAttachmentState::Missing),
            (dir.path().to_owned(), ComposeAttachmentState::NotFile),
            (loop_path, ComposeAttachmentState::Unreadable),
            ("relative.txt".into(), ComposeAttachmentState::Relative),
        ] {
            let mut state = connected();
            compose_files(&mut state, vec![path.clone()]);
            assert!(!state.compose_sendable());
            assert_eq!(
                state.info()["compose"]["attachments"][0]["state"],
                serde_json::to_value(expected).unwrap()
            );
            state.apply_ui(UiEvent::SendCompose);
            assert!(
                !state
                    .take_work()
                    .iter()
                    .any(|w| matches!(w, Work::Send { .. }))
            );
            let compose = state.compose.as_ref().unwrap();
            assert_eq!(compose.attachments[0].state, expected);
            assert_eq!(compose.draft.attachments, std::slice::from_ref(&path));
            assert!(compose.attachment_error.is_some());
            assert!(state.remove_compose_attachment(&path));
            assert!(state.compose_sendable());
        }
    }

    #[test]
    fn compose_send_rechecks_vanished_replaced_and_grown_files() {
        let dir = tempfile::tempdir().unwrap();
        for change in ["vanish", "directory", "grow"] {
            let path = dir.path().join(change);
            std::fs::write(&path, b"before").unwrap();
            let mut state = connected();
            compose_files(&mut state, vec![path.clone()]);
            assert!(state.compose_sendable());
            std::fs::remove_file(&path).unwrap();
            let expected = match change {
                "vanish" => ComposeAttachmentState::Missing,
                "directory" => {
                    std::fs::create_dir(&path).unwrap();
                    ComposeAttachmentState::NotFile
                }
                _ => {
                    std::fs::File::create(&path)
                        .unwrap()
                        .set_len(jmap::DEFAULT_UPLOAD_LIMIT)
                        .unwrap();
                    ComposeAttachmentState::Ready
                }
            };
            // Idle reads do not stat files. The explicit Send rechecks them.
            assert!(state.info()["compose"]["sendable"].as_bool().unwrap());
            state.apply_ui(UiEvent::SendCompose);
            assert!(!state.compose_sendable());
            assert!(
                !state
                    .take_work()
                    .iter()
                    .any(|w| matches!(w, Work::Send { .. }))
            );
            assert_eq!(
                state.compose.as_ref().unwrap().attachments[0].state,
                expected
            );
            assert_eq!(
                state.compose.as_ref().unwrap().attachments_over_limit,
                change == "grow"
            );
        }
    }

    #[test]
    fn compose_rechecks_read_permission_without_reading_the_file() {
        use std::os::unix::fs::PermissionsExt;

        // Root can still read mode 000 files. Exercise effective-user access
        // under the normal unprivileged test runner.
        // SAFETY: geteuid has no arguments or memory preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private.txt");
        std::fs::write(&path, b"hello").unwrap();
        let mut state = connected();
        compose_files(&mut state, vec![path.clone()]);
        assert!(state.compose_sendable());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        state.apply_ui(UiEvent::SendCompose);
        assert_eq!(
            state.compose.as_ref().unwrap().attachments[0].state,
            ComposeAttachmentState::Unreadable
        );
        assert!(!state.compose_sendable());
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Send { .. }))
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn compose_limits_include_encoding_body_edits_and_sender_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.bin");
        std::fs::File::create(&path).unwrap().set_len(2048).unwrap();
        let mut state = connected_many(&["a", "b"]);
        state.accounts[0].session.as_mut().unwrap().max_size_upload = Some(2048);
        state.accounts[1].session.as_mut().unwrap().max_size_upload = Some(16 * 1024);
        compose_files(&mut state, vec![path.clone()]);
        assert!(!state.compose_sendable(), "base64 and headers count too");
        assert!(state.compose.as_ref().unwrap().attachments_over_limit);
        state.apply_ui(UiEvent::ComposeAccount("b".into()));
        assert!(state.compose_sendable());
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::Body,
            "x".repeat(16 * 1024),
        ));
        assert!(
            !state.compose_sendable(),
            "the limit covers the whole message"
        );
        state.apply_ui(UiEvent::ComposeField(ComposeField::Body, "small".into()));
        assert!(state.compose_sendable());
        state.apply_ui(UiEvent::SendCompose);
        assert!(state.compose.as_ref().unwrap().busy);
        assert!(
            !state.remove_compose_attachment(&path),
            "in-flight message is immutable"
        );
        assert!(state.take_work().iter().any(
            |w| matches!(w, Work::Send { draft, .. } if draft.attachments == std::slice::from_ref(&path))
        ));
    }

    #[test]
    fn saved_paths_round_trip_and_a_vanished_file_restores_as_missing() {
        use crate::drafts::DirStore;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.txt");
        std::fs::write(&path, b"contents never saved in the draft").unwrap();
        let mut store = DirStore::new(root.path());
        let mut state = connected();
        compose_files(&mut state, vec![path.clone()]);
        assert!(state.draft_unsaved().is_some());
        state.save_draft();
        assert!(crate::drafts::apply(&mut store, state.take_draft_ops()).is_empty());
        let saved = store.list();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].schema_version, 1);
        assert_eq!(saved[0].attachments, std::slice::from_ref(&path));
        let encoded = serde_json::to_string(&saved[0]).unwrap();
        assert!(!encoded.contains("contents never saved"));
        let mut restored = connected();
        restored.offer_drafts(store.list());
        restored.apply_ui(UiEvent::RestoreDraft(DraftChoice::Keep));
        assert!(restored.compose_sendable());
        std::fs::remove_file(&path).unwrap();
        let mut vanished = connected();
        vanished.offer_drafts(store.list());
        vanished.apply_ui(UiEvent::RestoreDraft(DraftChoice::Keep));
        let compose = vanished.compose.as_ref().unwrap();
        assert_eq!(compose.draft.attachments, [path]);
        assert_eq!(
            compose.attachments[0].state,
            ComposeAttachmentState::Missing
        );
        assert_eq!(compose.attachments[0].size, None);
        assert!(!vanished.compose_sendable());
        vanished.apply_ui(UiEvent::SendCompose);
        assert!(
            !vanished
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Send { .. }))
        );
    }

    #[test]
    fn failed_worker_send_refreshes_changed_files_before_enabling_retry() {
        let dir = tempfile::tempdir().unwrap();
        for change in ["missing", "grown"] {
            let path = dir.path().join(change);
            std::fs::write(&path, b"hello").unwrap();
            let mut state = connected();
            state.accounts[0].session.as_mut().unwrap().max_size_upload = Some(4096);
            compose_files(&mut state, vec![path.clone()]);
            state.apply_ui(UiEvent::SendCompose);
            let (_, ticket) = sent_ticket(&mut state);
            assert!(state.compose.as_ref().unwrap().busy);
            if change == "missing" {
                std::fs::remove_file(&path).unwrap();
            } else {
                std::fs::File::create(&path).unwrap().set_len(8192).unwrap();
            }
            done(
                &mut state,
                "a",
                failed(
                    ticket,
                    jmap::Error::Protocol("refused after file changed".into()),
                ),
            );
            let compose = state.compose.as_ref().unwrap();
            assert!(!compose.busy);
            assert!(compose.send_origin.is_none());
            assert!(compose.error.is_some());
            assert!(!state.compose_sendable());
            assert_eq!(
                compose.attachments[0].state,
                if change == "missing" {
                    ComposeAttachmentState::Missing
                } else {
                    ComposeAttachmentState::Ready
                }
            );
            assert_eq!(compose.attachments_over_limit, change == "grown");
        }
    }

    #[test]
    fn fallback_limit_warning_is_distinct_from_an_advertised_account_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.bin");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(jmap::DEFAULT_UPLOAD_LIMIT)
            .unwrap();
        for advertised in [None, Some(jmap::DEFAULT_UPLOAD_LIMIT)] {
            let mut state = connected();
            state.accounts[0].session.as_mut().unwrap().max_size_upload = advertised;
            compose_files(&mut state, vec![path.clone()]);
            let message = state
                .compose
                .as_ref()
                .unwrap()
                .attachment_error
                .as_ref()
                .unwrap();
            assert!(message.contains(if advertised.is_none() {
                "default upload limit"
            } else {
                "account limit"
            }));
        }
    }

    #[test]
    fn identical_parent_names_use_full_directory_qualifiers() {
        let root = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["first/reports", "second/reports"]
            .into_iter()
            .map(|parent| {
                let dir = root.path().join(parent);
                std::fs::create_dir_all(&dir).unwrap();
                let path = dir.join("note.txt");
                std::fs::write(&path, b"hello").unwrap();
                path
            })
            .collect();
        let attachments = ComposeAttachment::inspect_all(&paths);
        assert_eq!(attachments[0].name, "note.txt");
        assert_eq!(attachments[1].name, "note.txt");
        assert_ne!(attachments[0].label, attachments[1].label);
        for (attachment, path) in attachments.iter().zip(&paths) {
            assert!(
                attachment
                    .label
                    .contains(&path.parent().unwrap().display().to_string())
            );
        }
    }

    // Drafts: nothing typed is lost to Cancel, Escape, a close or a crash.

    use crate::drafts::{DraftStore, MemoryStore};

    /// Carry the state's draft operations out on `store`.
    fn keep(state: &mut State, store: &mut MemoryStore) {
        let errors = crate::drafts::apply(store, state.take_draft_ops());
        assert!(errors.is_empty(), "{errors:?}");
    }

    /// A compose with a recipient, a subject and a body typed.
    fn typed(state: &mut State) {
        state.apply_ui(UiEvent::Compose);
        for (field, value) in [
            (ComposeField::To, "bob@example.invalid"),
            (ComposeField::Subject, "Lunch"),
            (ComposeField::Body, "Noon at the usual place?"),
        ] {
            state.apply_ui(UiEvent::ComposeField(field, value.into()));
        }
    }

    #[test]
    fn round2_successful_replacement_clears_the_refusal() {
        let mut state = connected();
        typed(&mut state);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::Body,
            "x".repeat(65 * 1024 * 1024),
        ));
        assert!(state.compose.as_ref().unwrap().error.is_some());
        state.apply_ui(UiEvent::ComposeField(ComposeField::Body, "Accepted".into()));
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.body.contents(), "Accepted");
        assert!(compose.error.is_none());
    }

    #[test]
    fn round2_successful_editor_edit_clears_the_refusal() {
        let mut state = connected();
        typed(&mut state);
        state.apply_ui(UiEvent::ComposeEdit(editor::Event::Command(
            editor::EditCommand::Insert("x".repeat(2 * 1024 * 1024)),
        )));
        assert!(state.compose.as_ref().unwrap().error.is_some());
        let revision = state.compose.as_ref().unwrap().body.revision;
        state.apply_ui(UiEvent::ComposeEdit(editor::Event::Command(
            editor::EditCommand::Insert("Accepted".into()),
        )));
        let compose = state.compose.as_ref().unwrap();
        assert!(compose.error.is_none());
        assert_eq!(compose.body.revision, revision + 1);
    }

    #[test]
    fn round2_a_legacy_settings_read_does_not_block_account_writes() {
        let mut state = connected();
        let mut legacy = state.settings.clone();
        legacy.composer.font.family = Some("Inter".into());
        state.set_settings(legacy);
        assert_eq!(state.settings.composer.font.family, None);
        let entry = state.settings.accounts[0].clone();
        assert!(crate::writer::apply(&state.settings, &Change::Upsert(entry)).is_ok());
    }

    #[test]
    fn round2_oversized_restore_cannot_overwrite_the_kept_body() {
        let mut state = connected();
        typed(&mut state);
        let mut saved = state.compose.take().unwrap().saved_draft();
        saved.body = "x".repeat(65 * 1024 * 1024);
        let mut store = MemoryStore::default();
        store.save(&saved).unwrap();
        state.offer_drafts(vec![saved.clone()]);
        state.apply_ui(UiEvent::RestoreDraft(DraftChoice::Keep));
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::Subject,
            "Edited".into(),
        ));
        state.save_draft();
        state.store_draft(true);
        state.request_quit();
        state.apply_ui(UiEvent::CloseCompose);
        keep(&mut state, &mut store);
        assert!(
            store.list()[0].body == saved.body,
            "stored body was overwritten"
        );
        assert!(state.compose.is_none());
        assert_eq!(state.restore, vec![saved]);
        assert!(!state.status.is_empty());
    }

    #[test]
    fn round2_oversized_compose_returns_invalid_and_closes() {
        let body = json!({"account":"a", "body":"x".repeat(65 * 1024 * 1024)}).to_string();
        // The editor's cap wins with the fallback, a tiny upload limit, and
        // a server that would otherwise allow this body to be uploaded.
        for limit in [None, Some(1), Some(u64::MAX)] {
            let mut state = connected();
            state.accounts[0].session.as_mut().unwrap().max_size_upload = limit;
            crate::commands::command(
                &mut state,
                &crate::actions::registry(),
                &crate::strings(),
                1,
                "email.compose",
                &body,
            );
            let replies = state.take_replies();
            assert_eq!(replies.len(), 1);
            assert_eq!(replies[0].body["error_code"], code::INVALID_ARGUMENT);
            assert!(state.compose.is_none());
            state.save_draft();
            assert!(state.take_draft_ops().is_empty());
        }
    }

    fn oversized_quote_is_invalid(then: Then) {
        let mut state = connected();
        let mut email = serde_json::to_value(unread_email("E1")).unwrap();
        email["bodyValues"] = json!({"p1":{"value":"x".repeat(65 * 1024 * 1024)}});
        email["textBody"] = json!([{"partId":"p1", "type":"text/plain"}]);
        let tag = tag(&state, "a");
        state.apply(Event::Account(tag, Done::Answered(1, then, Ok(email))));
        let replies = state.take_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].body["error_code"], code::INVALID_ARGUMENT);
        assert!(state.compose.is_none());
        state.save_draft();
        assert!(state.take_draft_ops().is_empty());
    }

    #[test]
    fn round2_oversized_reply_returns_invalid_and_closes() {
        oversized_quote_is_invalid(Then::Reply(false, Some(true)));
    }

    #[test]
    fn round2_oversized_forward_returns_invalid_and_closes() {
        oversized_quote_is_invalid(Then::Forward("bob@example.invalid".into()));
    }

    #[test]
    fn round2_refused_body_does_not_dirty_the_draft() {
        let mut state = connected();
        typed(&mut state);
        state.save_draft();
        state.take_draft_ops();
        let before = state.compose.as_ref().unwrap().revision;
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::Body,
            "x".repeat(65 * 1024 * 1024),
        ));
        assert_eq!(state.compose.as_ref().unwrap().revision, before);
        assert!(state.draft_unsaved().is_none());
        state.save_draft();
        assert!(state.take_draft_ops().is_empty());
    }

    #[test]
    fn round2_refused_quote_move_keeps_position_and_revisions() {
        use crate::quote::Position;
        let mut state = connected();
        state.open_compose(
            Some("a"),
            Composed {
                body: format!("q{}", "x".repeat(64 * 1024 * 1024 - 1)),
                ..Composed::default()
            },
        );
        let compose = state.compose.as_mut().unwrap();
        assert!(compose.error.is_none());
        compose.reply = Some(ReplyQuote {
            quote: "q".into(),
            signature: String::new(),
            position: Position::Above,
        });
        let before = (compose.revision, compose.body.revision);
        state.apply_ui(UiEvent::ComposeReplyPosition(Position::Below));
        let compose = state.compose.as_ref().unwrap();
        assert!(compose.error.is_some());
        assert_eq!(compose.reply.as_ref().unwrap().position, Position::Above);
        assert_eq!((compose.revision, compose.body.revision), before);
        assert!(compose.body.contents().starts_with('q'));
    }

    #[test]
    fn editor_body_is_the_only_text_saved_reported_and_sent() {
        let mut state = connected();
        typed(&mut state);
        state.apply_ui(UiEvent::ComposeField(ComposeField::Body, String::new()));
        let revision = state.compose.as_ref().unwrap().body.revision;
        state.apply_ui(UiEvent::ComposeEdit(editor::Event::Command(
            editor_model::model::EditCommand::Insert("Typed in the editor".into()),
        )));
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.body.contents(), "Typed in the editor");
        assert_eq!(compose.body.revision, revision + 1);
        assert!(
            compose.draft.body.is_empty(),
            "no stale wire body while open"
        );
        assert_eq!(compose.body, compose.body.clone());
        assert_eq!(compose.saved_draft().body, "Typed in the editor");
        assert_eq!(state.info()["compose"]["body"], "Typed in the editor");
        state.save_draft();
        let mut store = MemoryStore::default();
        keep(&mut state, &mut store);
        assert_eq!(store.list()[0].body, "Typed in the editor");
        state.apply_ui(UiEvent::SendCompose);
        let sent = state
            .take_work()
            .into_iter()
            .find_map(|w| match w {
                Work::Send { draft, .. } => Some(draft),
                _ => None,
            })
            .unwrap();
        assert_eq!(sent.body, "Typed in the editor");
        let compose = state.compose.as_ref().unwrap();
        assert!(compose.draft.body.is_empty());
        let revision = compose.body.revision;
        state.apply_ui(UiEvent::ComposeEdit(editor::Event::Command(
            editor::EditCommand::Insert("refused while busy".into()),
        )));
        assert_eq!(state.compose.as_ref().unwrap().body.revision, revision);
        assert_eq!(state.compose.as_ref().unwrap().body.contents(), sent.body);
    }

    #[test]
    fn programmatic_body_replaces_text_and_advances_revision() {
        let mut state = connected();
        crate::commands::command(
            &mut state,
            &crate::actions::registry(),
            &crate::strings(),
            1,
            "email.compose",
            r#"{"account":"a","body":"Initial"}"#,
        );
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.body.contents(), "Initial");
        assert!(compose.draft.body.is_empty());
        let revision = compose.body.revision;
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::Body,
            "Replacement".into(),
        ));
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.body.contents(), "Replacement");
        assert_eq!(compose.body.revision, revision + 1);
        assert!(compose.draft.body.is_empty());
    }

    #[test]
    fn oversized_bus_compose_and_reply_quote_report_errors_without_panicking() {
        let mut state = connected();
        let huge = "x".repeat(65 * 1024 * 1024);
        crate::commands::command(
            &mut state,
            &crate::actions::registry(),
            &crate::strings(),
            1,
            "email.compose",
            &json!({"account":"a", "body":huge}).to_string(),
        );
        assert!(state.compose.is_none());
        assert_eq!(
            state.take_replies()[0].body["error_code"],
            code::INVALID_ARGUMENT
        );

        let mut email = serde_json::to_value(unread_email("E1")).unwrap();
        email["bodyValues"] = json!({"p1":{"value":huge}});
        email["textBody"] = json!([{"partId":"p1", "type":"text/plain"}]);
        assert!(state.reply_to(
            "a",
            &serde_json::from_value(email).unwrap(),
            false,
            Some(true)
        ));
        let compose = state.compose.as_ref().unwrap();
        assert!(compose.error.is_some());
        assert!(compose.body.doc.text().is_empty());
    }

    #[test]
    fn refused_replacements_keep_the_body_and_revision() {
        let mut compose = ComposeState::new(
            "a",
            Composed {
                body: "Keep this".into(),
                ..Composed::default()
            },
        );
        let revision = compose.body.revision;
        for text in ["x".repeat(65 * 1024 * 1024), "\n".repeat(2_000_000)] {
            compose.error = None;
            compose.set_body(&text);
            assert!(compose.error.is_some());
            assert_eq!(compose.body.contents(), "Keep this");
            assert_eq!(compose.body.revision, revision);
        }
    }

    #[test]
    fn programmatic_body_edits_wait_while_sending() {
        let mut state = connected();
        typed(&mut state);
        state.apply_ui(UiEvent::SendCompose);
        let compose = state.compose.as_ref().unwrap();
        assert!(compose.busy);
        let text = compose.body.contents();
        let body_revision = compose.body.revision;
        let draft_revision = compose.revision;
        state.apply_ui(UiEvent::ComposeField(ComposeField::Body, "Ignored".into()));
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.body.contents(), text);
        assert_eq!(compose.body.revision, body_revision);
        assert_eq!(compose.revision, draft_revision);
    }

    #[test]
    fn undo_after_set_body_does_not_restore_older_text() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeEdit(editor::Event::Command(
            editor::EditCommand::Insert("Earlier typing".into()),
        )));
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::Body,
            "Replacement".into(),
        ));
        state.apply_ui(UiEvent::ComposeEdit(editor::Event::Undo));
        assert_eq!(
            state.compose.as_ref().unwrap().body.contents(),
            "Replacement"
        );
        // Undoing the earlier typing overlaps the reload, so the buffer
        // refuses it rather than bringing the old text back.
        assert!(state.compose.as_ref().unwrap().error.is_some());
    }

    #[test]
    fn undo_to_saved_text_only_saves_the_identical_draft_once() {
        let mut state = connected();
        typed(&mut state);
        state.save_draft();
        state.take_draft_ops();
        let saved = state.compose.as_ref().unwrap().body.contents();
        state.apply_ui(UiEvent::ComposeEdit(editor::Event::Command(
            editor::EditCommand::Insert("Extra".into()),
        )));
        state.apply_ui(UiEvent::ComposeEdit(editor::Event::Undo));
        assert_eq!(state.compose.as_ref().unwrap().body.contents(), saved);
        assert!(state.draft_unsaved().is_some());
        state.save_draft();
        assert!(
            matches!(state.take_draft_ops().as_slice(), [DraftOp::Save(draft)] if draft.body == saved)
        );
        assert!(state.draft_unsaved().is_none());
        state.save_draft();
        assert!(state.take_draft_ops().is_empty());
    }

    #[test]
    fn idle_reply_frames_do_not_copy_the_body() {
        let mut state = connected();
        assert!(state.reply_to("a", &unread_email("E1"), false, Some(true)));
        let ctx = egui::Context::default();
        let theme = toolkit::Theme::embedded();
        toolkit::install(&ctx, &theme);
        let palette = editor::Palette::from_theme(&theme);
        let registry = crate::actions::registry();
        let strings = crate::strings();
        let frame = |state: &State| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = crate::view::view(ui, state, &registry, &strings, &palette);
            });
            output.textures_delta.clear();
        };
        frame(&state);
        let compose = state.compose.as_ref().unwrap();
        let reads = compose.body.reads.get();
        for _ in 0..3 {
            frame(&state);
        }
        assert_eq!(compose.body.reads.get(), reads, "idle frames read no text");
        let edited = compose.body.contents().replace("wrote:", "wrote this:");
        state.apply_ui(UiEvent::ComposeField(ComposeField::Body, edited));
        let compose = state.compose.as_ref().unwrap();
        let reads = compose.body.reads.get();
        frame(&state);
        assert_eq!(
            compose.body.reads.get(),
            reads + 1,
            "an edit refreshes the check"
        );
        assert!(!compose.reply_movable(), "an edited quote cannot move");
        frame(&state);
        assert_eq!(compose.body.reads.get(), reads + 1);
    }

    #[test]
    fn cloning_a_body_keeps_text_and_revision_and_drops_history() {
        let mut body = Body::new("Initial").unwrap();
        body.doc.apply(
            &editor::Event::Command(editor::EditCommand::Insert("Typed ".into())),
            &editor::View::prose(),
        );
        body.revision += 1;
        let mut cloned = body.clone();
        assert_eq!(body, cloned);
        assert_eq!(body.revision, cloned.revision);
        cloned.doc.undo().unwrap();
        assert_eq!(body, cloned, "the clone has no history");
        body.doc.undo().unwrap();
        assert_eq!(body.contents(), "Initial");
        assert_ne!(body, cloned);
    }

    #[test]
    fn editor_refusal_is_shown_and_motion_does_not_dirty_the_draft() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        let revision = state.compose.as_ref().unwrap().revision;
        state.apply_ui(UiEvent::ComposeEdit(editor::Event::Command(
            editor::EditCommand::Move {
                to: editor::Motion::DocEnd,
                extend: false,
            },
        )));
        assert_eq!(state.compose.as_ref().unwrap().revision, revision);
        state.apply_ui(UiEvent::ComposeEdit(editor::Event::Command(
            editor::EditCommand::Insert("x".repeat(2 * 1024 * 1024)),
        )));
        let compose = state.compose.as_ref().unwrap();
        assert!(compose.error.is_some());
        assert!(compose.body.contents().is_empty());
        assert_eq!(compose.revision, revision);
    }

    #[test]
    fn cancel_keeps_the_draft() {
        let mut state = connected();
        let mut store = MemoryStore::default();
        typed(&mut state);
        // Cancel and Escape both report CloseCompose: the person is asked,
        // and the draft is already on disk.
        state.apply_ui(UiEvent::CloseCompose);
        assert_eq!(state.confirm, Some(Confirm::CloseCompose));
        assert!(state.compose.is_some(), "nothing closes unasked");
        keep(&mut state, &mut store);
        let saved = store.list();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].body, "Noon at the usual place?");
        assert_eq!(saved[0].to, "bob@example.invalid");
        // Keep Editing goes back to the compose.
        state.apply_ui(UiEvent::ConfirmDraft(DraftChoice::Back));
        assert!(state.confirm.is_none() && state.compose.is_some());
        // The default, Keep Draft, closes and keeps it.
        state.apply_ui(UiEvent::CloseCompose);
        state.apply_ui(UiEvent::ConfirmDraft(DraftChoice::Keep));
        keep(&mut state, &mut store);
        assert!(state.compose.is_none());
        assert_eq!(store.list().len(), 1, "the draft is kept");
    }

    #[test]
    fn a_draft_is_removed_only_when_discarded_or_sent() {
        let mut state = connected();
        let mut store = MemoryStore::default();
        typed(&mut state);
        state.save_draft();
        keep(&mut state, &mut store);
        assert_eq!(store.list().len(), 1);
        state.apply_ui(UiEvent::CloseCompose);
        state.apply_ui(UiEvent::ConfirmDraft(DraftChoice::Discard));
        keep(&mut state, &mut store);
        assert!(state.compose.is_none());
        assert!(store.list().is_empty(), "discarded on request");
        // A send that went removes its draft.
        state.account_mut("a").unwrap().mail.mailboxes = vec![
            mailbox("m-drafts", Some("drafts"), 0),
            mailbox("m-sent", Some("sent"), 0),
        ];
        typed(&mut state);
        state.apply_ui(UiEvent::SendCompose);
        let (_, ticket) = sent_ticket(&mut state);
        keep(&mut state, &mut store);
        assert_eq!(store.list().len(), 1, "kept while sending");
        done(&mut state, "a", went(ticket));
        keep(&mut state, &mut store);
        assert!(store.list().is_empty());
    }

    #[test]
    fn an_untouched_compose_closes_without_asking() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::CloseCompose);
        assert!(state.compose.is_none() && state.confirm.is_none());
        assert!(state.take_draft_ops().is_empty());
        state.apply_ui(UiEvent::Compose);
        assert!(state.request_quit(), "nothing typed: the window closes");
    }

    #[test]
    fn close_with_unsaved_changes_asks() {
        let mut state = connected();
        let mut store = MemoryStore::default();
        typed(&mut state);
        assert!(state.draft_unsaved().is_some());
        // The window's close button (and file.quit) ask; the draft is saved
        // before the question shows.
        assert!(!state.request_quit());
        state.quit();
        assert!(!state.exiting);
        assert_eq!(state.confirm, Some(Confirm::Quit));
        keep(&mut state, &mut store);
        assert_eq!(store.list().len(), 1);
        assert!(state.draft_unsaved().is_none());
        state.apply_ui(UiEvent::ConfirmDraft(DraftChoice::Back));
        assert!(!state.exiting && state.compose.is_some());
        // Keep Draft and Quit: the window closes, the draft stays.
        assert!(!state.request_quit());
        state.apply_ui(UiEvent::ConfirmDraft(DraftChoice::Keep));
        keep(&mut state, &mut store);
        assert!(state.exiting);
        assert!(state.request_quit(), "answered: closing goes ahead");
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn restore_after_restart() {
        let mut store = MemoryStore::default();
        let mut first = connected();
        typed(&mut first);
        first.apply_ui(UiEvent::ComposeField(
            ComposeField::Cc,
            "cy@example.invalid".into(),
        ));
        // An earlier send got as far as creating the server copy.
        let attachments = vec![std::path::PathBuf::from("/home/user/report.pdf")];
        first.compose.as_mut().unwrap().draft.attachments = attachments.clone();
        first.compose.as_mut().unwrap().attempt.email_id = Some("E1".into());
        first.save_draft();
        keep(&mut first, &mut store);
        let id = first.compose.as_ref().unwrap().draft_id.clone();
        drop(first);
        // A new run: the draft is offered.
        let mut state = connected();
        state.offer_drafts(store.list());
        assert_eq!(state.restore.len(), 1);
        assert!(state.compose.is_none());
        state.apply_ui(UiEvent::RestoreDraft(DraftChoice::Keep));
        let compose = state.compose.clone().expect("restored");
        assert!(state.restore.is_empty());
        assert_eq!(compose.draft_id, id);
        assert!(compose.restored);
        assert_eq!(compose.to_text, "bob@example.invalid");
        assert_eq!(compose.cc_text, "cy@example.invalid");
        assert_eq!(compose.draft.subject, "Lunch");
        assert_eq!(compose.body.contents(), "Noon at the usual place?");
        assert_eq!(compose.attempt.email_id.as_deref(), Some("E1"));
        assert_eq!(compose.draft.attachments, attachments);
        assert_eq!(compose.saved_draft().attachments, attachments);
        // Closing a restored draft asks too: it holds text.
        state.apply_ui(UiEvent::CloseCompose);
        assert_eq!(state.confirm, Some(Confirm::CloseCompose));
    }

    #[test]
    fn a_later_offer_keeps_every_draft() {
        let mut store = MemoryStore::default();
        let mut first = connected();
        typed(&mut first);
        first.save_draft();
        keep(&mut first, &mut store);
        let mut state = connected();
        state.offer_drafts(store.list());
        state.apply_ui(UiEvent::RestoreDraft(DraftChoice::Back));
        keep(&mut state, &mut store);
        assert!(state.restore.is_empty() && state.compose.is_none());
        assert_eq!(store.list().len(), 1, "Later deletes nothing");
        state.offer_drafts(store.list());
        state.apply_ui(UiEvent::RestoreDraft(DraftChoice::Discard));
        keep(&mut state, &mut store);
        assert!(store.list().is_empty());
    }

    #[test]
    fn a_compose_replaced_or_orphaned_is_kept_and_offered() {
        let mut state = connected_many(&["a", "b"]);
        let mut store = MemoryStore::default();
        typed(&mut state);
        // A second compose over the first never drops it.
        assert!(state.open_compose(Some("b"), Composed::default()));
        keep(&mut state, &mut store);
        assert_eq!(store.list().len(), 1);
        assert_eq!(state.restore.len(), 1);
        assert_eq!(state.restore[0].subject, "Lunch");
    }

    #[test]
    fn a_failed_send_keeps_the_draft_and_what_the_server_did() {
        let mut state = connected();
        let mut store = MemoryStore::default();
        typed(&mut state);
        state.apply_ui(UiEvent::SendCompose);
        let (_, ticket) = sent_ticket(&mut state);
        let attempt = jmap::SendAttempt {
            email_id: Some("E1".into()),
            ambiguous: true,
            ..jmap::SendAttempt::default()
        };
        done(
            &mut state,
            "a",
            Done::Sent(
                ticket,
                attempt.clone(),
                Err(jmap::Error::Protocol("the reply was lost".into())),
            ),
        );
        keep(&mut state, &mut store);
        assert_eq!(store.list()[0].attempt, attempt, "kept with the draft");
        // The retry resumes the same server copy instead of a new one.
        state.apply_ui(UiEvent::SendCompose);
        let resumed = state
            .take_work()
            .into_iter()
            .find_map(|w| match w {
                Work::Send { attempt, .. } => Some(attempt),
                _ => None,
            })
            .expect("a send");
        assert_eq!(resumed, attempt);
    }

    /// A send that went.
    fn went(ticket: u64) -> Done {
        Done::Sent(
            ticket,
            jmap::SendAttempt::default(),
            Ok(jmap::SendReport::default()),
        )
    }

    /// A send that failed, having done nothing on the server.
    fn failed(ticket: u64, error: jmap::Error) -> Done {
        Done::Sent(ticket, jmap::SendAttempt::default(), Err(error))
    }

    /// Take the queued work and return the send's account and ticket.
    fn sent_ticket(state: &mut State) -> (String, u64) {
        state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::Send {
                    account, ticket, ..
                } => Some((account.clone(), *ticket)),
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
        done(&mut state, "a", went(ticket));
        assert!(state.compose.is_none());
        let work = state.take_work();
        assert!(
            work.iter()
                .any(|w| matches!(w, Work::FetchMailboxes { .. }))
        );
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
            failed(ticket, jmap::Error::method("serverFail", Some("no relay"))),
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
        let first_draft = state.compose.as_ref().unwrap().draft_id.clone();
        // Cancelled while sending (the draft kept), then a new draft.
        state.apply_ui(UiEvent::CloseCompose);
        state.apply_ui(UiEvent::ConfirmDraft(DraftChoice::Keep));
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(ComposeField::Subject, "next".into()));
        state.take_draft_ops();
        done(&mut state, "a", went(first));
        let compose = state.compose.as_ref().expect("the new draft stays");
        assert_eq!(compose.draft.subject, "next");
        assert!(!compose.busy);
        // The first message went: its kept draft goes.
        assert_eq!(state.take_draft_ops(), [DraftOp::Remove(first_draft)]);
    }

    #[test]
    fn another_accounts_send_never_closes_this_compose() {
        let mut state = connected_many(&["a", "b"]);
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SendCompose);
        let (account, ticket) = sent_ticket(&mut state);
        assert_eq!(account, "a");
        done(&mut state, "b", went(ticket));
        assert!(
            state.compose.as_ref().is_some_and(|c| c.busy),
            "still sending"
        );
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
            Done::Opened(next, None, Box::new(Ok(page("X").emails.remove(0)))),
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
        assert!(
            state.account("a").unwrap().mail.emails.is_empty(),
            "A's rows are gone"
        );
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
        done(
            &mut state,
            "a",
            Done::Opened(ticket, None, Box::new(Ok(body))),
        );
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
        done(
            &mut state,
            "a",
            Done::Events(second, Ok(vec![event("two")])),
        );
        done(&mut state, "a", Done::Events(first, Ok(vec![event("one")])));
        let view = state.account("a").unwrap();
        assert_eq!(view.calendars.events.len(), 1);
        assert_eq!(view.calendars.events[0].id, "two");
    }

    #[test]
    fn a_count_refresh_keeps_the_open_email() {
        let mut state = connected();
        done(
            &mut state,
            "a",
            Done::Mailboxes(Ok(vec![mailbox("m-in", Some("inbox"), 1)])),
        );
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
        done(
            &mut state,
            "a",
            Done::Opened(open, None, Box::new(Ok(email))),
        );
        // The mark-read's count refresh answers after the body.
        done(
            &mut state,
            "a",
            Done::Mailboxes(Ok(vec![mailbox("m-in", Some("inbox"), 0)])),
        );
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
        assert!(
            state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::FetchContacts { book, .. } if book == "B1"))
        );
        state.apply_ui(UiEvent::Select(Node::Account("b".into())));
        assert_eq!(state.selected.as_deref(), Some("b"));
        state.apply_ui(UiEvent::Select(Node::Mailbox("a".into(), "m-kid".into())));
        assert_eq!(
            (state.selected.as_deref(), state.mode),
            (Some("a"), Mode::Mail)
        );
        assert_eq!(
            state.account("a").unwrap().mail.selected.as_deref(),
            Some("m-kid")
        );
    }

    #[test]
    fn tree_keys_move_collapse_expand_and_select() {
        let mut state = tree_fixture();
        // The cursor starts on the shown row (the inbox).
        state.tree_key(TreeKey::Down);
        assert_eq!(
            state.ui.cursor,
            Some(Node::Mailbox("a".into(), "m-folder".into()))
        );
        state.tree_key(TreeKey::Left);
        assert!(!state.expanded(&Node::Mailbox("a".into(), "m-folder".into())));
        assert!(state.config_dirty());
        assert!(!labels(&state).contains(&"  M-KID".to_owned()));
        state.tree_key(TreeKey::Right);
        state.tree_key(TreeKey::Right);
        assert_eq!(
            state.ui.cursor,
            Some(Node::Mailbox("a".into(), "m-kid".into()))
        );
        state.tree_key(TreeKey::Left);
        assert_eq!(
            state.ui.cursor,
            Some(Node::Mailbox("a".into(), "m-folder".into()))
        );
        state.tree_key(TreeKey::Enter);
        assert_eq!(
            state.account("a").unwrap().mail.selected.as_deref(),
            Some("m-folder")
        );
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
        assert!(
            !changes.to_string().contains("pw"),
            "no secret goes to settingsd"
        );
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
        let version = state.secrets_snapshot().0;
        state.secrets_saved(save, version, Ok(true));
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
        assert!(
            state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::ReadSettings))
        );
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
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Settings { .. }))
        );
        assert_eq!(
            state.take_replies()[0].body["error_code"],
            code::UNKNOWN_ACCOUNT
        );
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
        assert_eq!(
            state.take_replies()[0].body["error_code"],
            code::UNAVAILABLE
        );
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Settings { .. }))
        );
    }

    /// settingsd comes back after being missing: the status bar stops
    /// saying it is missing (it was sticky on VT4 on 2026-10-21).
    #[test]
    fn settingsd_back_clears_the_missing_status() {
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            settings(&["a"]),
            secrets(&["a"]),
        );
        state.take_work();
        state.settings_missing();
        assert_eq!(state.status, crate::label("settings-missing"));
        state.set_settings(settings(&["a"]));
        assert_eq!(state.source, Source::Settings);
        assert_eq!(state.status, crate::label("settings-back"));
        // Missing again, then a look that holds the accounts shown: the
        // held notice wins over the comeback notice.
        state.settings_missing();
        state.set_settings(settings(&[]));
        assert_eq!(state.status, crate::label("settings-empty-held"));
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
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Settings { .. }))
        );
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
            identities: Vec::new(),
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
        assert!(state.migrating());
        // settingsd is read before the password is durable: nothing is
        // sent yet.
        state.set_settings(EmailSettings::default());
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Settings { .. }))
        );
        land(&mut state, &work, Ok(true));
        // Once it is, settingsd (with no accounts) gets the migration, and
        // the account stays shown (and connected) while it is.
        let (ticket, changes) = settings_ticket(&mut state);
        assert_eq!(changes["email.accounts"][0]["id"], id.as_str());
        assert_eq!(changes["email.default_account"], id.as_str());
        assert!(!changes.to_string().contains("pw-1"));
        assert_eq!(state.account(&id).unwrap().phase, Phase::Connecting);
        state.settled(ticket, Settled::Accepted);
        assert!(state.take_migrated());
        state.migration_rewritten(Ok(true));
        assert!(!state.migrating());
        assert!(state.has_password(&id));
        // The read that follows changes nothing, and nothing is sent again.
        let mut after = EmailSettings::default();
        after.accounts.push(legacy);
        after.default_account = Some(id.clone());
        state.take_work();
        state.set_settings(after);
        let work = state.take_work();
        assert!(
            !work
                .iter()
                .any(|w| matches!(w, Work::Settings { .. } | Work::Connect { .. }))
        );
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
        let work = state.take_work();
        land(&mut state, &work, Ok(true));
        state.set_settings(EmailSettings {
            accounts: vec![held],
            default_account: Some("a".into()),
            ..EmailSettings::default()
        });
        // Nothing to write to settingsd: the password is written under the
        // adopted id, and only once that lands does the migration finish.
        let work = state.take_work();
        assert!(!work.iter().any(|w| matches!(w, Work::Settings { .. })));
        assert!(state.migrating() && !state.take_migrated());
        assert!(state.has_password("a"));
        land(&mut state, &work, Ok(true));
        assert!(state.take_migrated());
        state.migration_rewritten(Ok(true));
        assert!(!state.migrating());
        assert_eq!(state.accounts.len(), 1);
    }

    use crate::commands::{Then, Write};

    /// A connected account showing its inbox with E1 open.
    fn reading() -> State {
        let mut state = connected();
        done(
            &mut state,
            "a",
            Done::Mailboxes(Ok(vec![
                mailbox("m-in", Some("inbox"), 1),
                mailbox("m-trash", Some("trash"), 0),
            ])),
        );
        state.take_work();
        let view = state.account_mut("a").unwrap();
        view.mail.emails = vec![unread_email("E1"), unread_email("E2")];
        view.mail.open = Some(unread_email("E1"));
        state
    }

    /// The batched actions queued since the last take.
    fn batches(state: &mut State) -> Vec<Batch> {
        state
            .take_work()
            .into_iter()
            .filter_map(|w| match w {
                Work::Batch { batch, .. } => Some(*batch),
                _ => None,
            })
            .collect()
    }

    fn writes(state: &mut State) -> Vec<(Option<u64>, Write)> {
        state
            .take_work()
            .into_iter()
            .filter_map(|w| match w {
                Work::Write { reply, write, .. } => Some((reply, write)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_plain_click_opens_and_ctrl_or_range_selects_only() {
        use toolkit::datatable::Pick;
        let mut state = reading();
        state.account_mut("a").unwrap().mail.total = 2866;
        // A Ctrl+click and a range select, and open nothing.
        state.mail_table(
            "a",
            TableEvent::Selected {
                index: 1,
                key: "E2".into(),
                pick: Pick::Toggle,
            },
        );
        state.mail_table(
            "a",
            TableEvent::SelectionChanged {
                keys: vec!["E1".into(), "E2".into()],
                focus: Some(0),
            },
        );
        assert!(state.take_work().is_empty(), "no open, no mark-read");
        let mail = &state.account("a").unwrap().mail;
        assert_eq!(mail.selection_count(), 2);
        assert!(!mail.emails[1].is_read());
        // "Select all N": the mailbox's count, with no query.
        state.mail_table("a", TableEvent::SelectEverything);
        assert!(state.take_work().is_empty());
        let mail = &state.account("a").unwrap().mail;
        assert!(mail.everything);
        assert_eq!(mail.selection_count(), 2866);
        assert_eq!(
            state.info()["accounts"][0]["mail"]["selection"]["count"],
            2866
        );
        // Activating a selected row opens it and keeps the selection.
        state.mail_table(
            "a",
            TableEvent::Activated {
                index: 1,
                key: "E2".into(),
            },
        );
        let mail = &state.account("a").unwrap().mail;
        assert!(mail.everything && mail.table.selection.len() == 2);
        assert!(
            state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::OpenEmail { id, .. } if id == "E2"))
        );
        // A plain click is a selection of its own again. E1 is the open email
        // already, so the click does not ask for it again.
        state.mail_table(
            "a",
            TableEvent::Selected {
                index: 0,
                key: "E1".into(),
                pick: Pick::Only,
            },
        );
        let mail = &state.account("a").unwrap().mail;
        assert!(!mail.everything);
        assert_eq!(mail.selection_count(), 1);
        assert!(
            state
                .take_work()
                .iter()
                .all(|w| !matches!(w, Work::OpenEmail { .. }))
        );
        assert_eq!(grouped(2866), "2,866");
        assert_eq!(grouped(12), "12");
        assert_eq!(grouped(1_000_000), "1,000,000");
    }

    /// `ids` as emails, unread.
    fn emails(ids: &[&str]) -> Vec<Email> {
        ids.iter().map(|id| unread_email(id)).collect()
    }

    /// The one page fetch in `work`: its ticket, sort, position and limit.
    fn fetched(work: &[Work]) -> (u64, jmap::EmailSort, u64, u32) {
        match work
            .iter()
            .filter_map(|w| match w {
                Work::FetchEmails {
                    ticket,
                    sort,
                    position,
                    limit,
                    ..
                } => Some((*ticket, *sort, *position, *limit)),
                _ => None,
            })
            .collect::<Vec<_>>()
            .as_slice()
        {
            [one] => *one,
            other => panic!("one page fetch, not {other:?}"),
        }
    }

    /// The answer to page fetch `ticket`: `ids`, of `total` in all.
    fn page_of(state: &mut State, ticket: u64, ids: &[&str], total: u64) {
        done(
            state,
            "a",
            Done::Page(
                ticket,
                Ok(jmap::EmailPage {
                    emails: emails(ids),
                    total,
                }),
            ),
        );
    }

    /// `reading()` with its inbox 236 emails long (its folder counts 999:
    /// the query's total is the one that counts).
    fn paged() -> State {
        let mut state = reading();
        let view = state.account_mut("a").unwrap();
        view.mail.total = 236;
        view.mail.mailboxes[0].total_emails = 999;
        state
    }

    const NEWEST: jmap::EmailSort = jmap::EmailSort {
        by: jmap::SortBy::ReceivedAt,
        ascending: false,
    };

    #[test]
    fn the_pager_moves_forward_and_back_one_page_at_a_time() {
        let mut state = paged();
        assert_eq!(state.account("a").unwrap().mail.table.page_size, 100);
        state.mail_table("a", TableEvent::PageChanged(1));
        let (ticket, sort, position, limit) = fetched(&state.take_work());
        assert_eq!((sort, position, limit), (NEWEST, 100, 100));
        assert!(state.account("a").unwrap().mail.busy);
        page_of(&mut state, ticket, &["P101", "P102"], 236);
        let mail = &state.account("a").unwrap().mail;
        assert_eq!(mail.table.page, 1);
        assert_eq!(mail.keys(), ["P101", "P102"]);
        assert_eq!(mail.list_table().range(2), Some((101, 102)));
        assert!(!mail.busy);
        // On to the last page, then back one.
        state.mail_table("a", TableEvent::PageChanged(2));
        let (ticket, _, position, _) = fetched(&state.take_work());
        assert_eq!(position, 200);
        page_of(&mut state, ticket, &["P201"], 236);
        state.mail_table("a", TableEvent::PageChanged(1));
        let (_, sort, position, _) = fetched(&state.take_work());
        assert_eq!((sort, position), (NEWEST, 100));
        assert_eq!(state.info()["accounts"][0]["mail"]["page"], 1);
        // A reload keeps the page.
        state.refresh();
        let (_, _, position, _) = fetched(&state.take_work());
        assert_eq!(position, 100);
        assert_eq!(state.account("a").unwrap().mail.table.page, 1);
    }

    #[test]
    fn a_page_past_the_end_falls_back_to_the_last() {
        let mut state = paged();
        state.mail_table("a", TableEvent::PageChanged(2));
        let (ticket, ..) = fetched(&state.take_work());
        // The mailbox shrank to 150 meanwhile: page 3 of 2 is empty.
        page_of(&mut state, ticket, &[], 150);
        let (ticket, _, position, _) = fetched(&state.take_work());
        assert_eq!(position, 100);
        assert_eq!(state.account("a").unwrap().mail.table.page, 1);
        page_of(&mut state, ticket, &["P101"], 150);
        assert_eq!(state.account("a").unwrap().mail.keys(), ["P101"]);
    }

    #[test]
    fn a_new_mailbox_starts_at_its_first_page_in_the_same_sort() {
        let mut state = paged();
        let view = state.account_mut("a").unwrap();
        view.mail.mailboxes.push(mailbox("m-sent", Some("sent"), 0));
        state.mail_table(
            "a",
            TableEvent::SortChanged(Some(Sort::new(COLUMN_FROM, Order::Ascending))),
        );
        state.mail_table("a", TableEvent::PageChanged(2));
        state.take_work();
        assert!(state.select_mailbox("a", "m-sent".into()));
        let (_, sort, position, _) = fetched(&state.take_work());
        assert_eq!(position, 0);
        assert_eq!(sort, jmap::EmailSort::new(jmap::SortBy::From, true));
        let mail = &state.account("a").unwrap().mail;
        assert_eq!(mail.table.page, 0);
        assert!(mail.refocus.is_none());
    }

    #[test]
    fn each_header_sorts_its_column_ascending_first_then_flips() {
        use toolkit::datatable::Sort as S;
        let columns = list_columns(80.0);
        let mut state = paged();
        // Newest first until a header is clicked: the Date arrow points down.
        let shown = state.account("a").unwrap().mail.list_table();
        assert_eq!(shown.sort, Some(S::new(COLUMN_DATE, Order::Descending)));
        // Date flips to oldest first; Subject and From start ascending.
        assert_eq!(
            shown.header_clicked(&columns, COLUMN_DATE, false),
            Some(TableEvent::SortChanged(Some(S::new(
                COLUMN_DATE,
                Order::Ascending
            ))))
        );
        for (column, by) in [
            (COLUMN_SUBJECT, jmap::SortBy::Subject),
            (COLUMN_FROM, jmap::SortBy::From),
            (COLUMN_DATE, jmap::SortBy::ReceivedAt),
        ] {
            // From page 2, so each sort is seen to start at the top again.
            state.mail_table("a", TableEvent::PageChanged(1));
            state.take_work();
            for ascending in [true, false] {
                let shown = state.account("a").unwrap().mail.list_table();
                let click = shown.header_clicked(&columns, column, false).unwrap();
                let order = if ascending {
                    Order::Ascending
                } else {
                    Order::Descending
                };
                // Date starts out descending, so its first click ascends too.
                assert_eq!(
                    click,
                    TableEvent::SortChanged(Some(S::new(column, order))),
                    "{column} {ascending}"
                );
                state.mail_table("a", click);
                let (_, sort, position, _) = fetched(&state.take_work());
                assert_eq!(sort, jmap::EmailSort::new(by, ascending));
                assert_eq!(position, 0);
                let mail = &state.account("a").unwrap().mail;
                assert_eq!(mail.table.page, 0);
                assert_eq!(state.info()["accounts"][0]["mail"]["sort"], column);
            }
        }
        // Nothing of it reaches settings.
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Settings { .. }))
        );
        assert_eq!(email_sort(None), NEWEST);
        assert_eq!(
            email_sort(Some(&S::new("size", Order::Ascending))),
            NEWEST,
            "a column the server cannot sort by"
        );
    }

    #[test]
    fn a_sort_change_clears_the_selection_and_keeps_the_keyboard_row_if_shown() {
        let mut state = paged();
        // Nothing open: the keyboard row is the table's own.
        state.account_mut("a").unwrap().mail.open = None;
        state.mail_table(
            "a",
            TableEvent::SelectionChanged {
                keys: vec!["E1".into(), "E2".into()],
                focus: Some(1),
            },
        );
        state.mail_table("a", TableEvent::SelectEverything);
        let subject = || TableEvent::SortChanged(Some(Sort::new(COLUMN_SUBJECT, Order::Ascending)));
        state.mail_table("a", subject());
        let mail = &state.account("a").unwrap().mail;
        assert!(mail.table.selection.is_empty() && !mail.everything);
        assert_eq!(mail.selection_count(), 0);
        let (ticket, ..) = fetched(&state.take_work());
        // E2, the keyboard's row, is on the new page: the row follows it.
        page_of(&mut state, ticket, &["E9", "E2", "E1"], 236);
        let mail = &state.account("a").unwrap().mail;
        assert_eq!(mail.table.focus, Some(1));
        assert!(mail.table.selection.is_empty());
        // A page without it: no keyboard row.
        state.mail_table("a", TableEvent::PageChanged(1));
        let (ticket, ..) = fetched(&state.take_work());
        page_of(&mut state, ticket, &["E7", "E8"], 236);
        assert_eq!(state.account("a").unwrap().mail.table.focus, None);
    }

    #[test]
    fn select_all_on_a_later_page_takes_the_query_total() {
        let mut state = paged();
        state.mail_table("a", TableEvent::PageChanged(2));
        let (ticket, ..) = fetched(&state.take_work());
        page_of(&mut state, ticket, &["P201", "P202"], 236);
        state.mail_table(
            "a",
            TableEvent::SelectionChanged {
                keys: vec!["P201".into(), "P202".into()],
                focus: Some(0),
            },
        );
        state.mail_table("a", TableEvent::SelectEverything);
        let mail = &state.account("a").unwrap().mail;
        assert!(mail.everything);
        // The query's 236, not the folder's 999.
        assert_eq!(mail.selection_count(), 236);
        assert!(state.take_work().is_empty(), "no query for it");
        // A reload of the page keeps every email selected.
        state.refresh();
        let (ticket, ..) = fetched(&state.take_work());
        page_of(&mut state, ticket, &["P201", "P202"], 236);
        let mail = &state.account("a").unwrap().mail;
        assert!(mail.everything && mail.table.selection.len() == 2);
    }

    #[test]
    fn a_refused_sort_falls_back_to_newest_first() {
        let mut state = paged();
        state.mail_table(
            "a",
            TableEvent::SortChanged(Some(Sort::new(COLUMN_FROM, Order::Descending))),
        );
        let (ticket, ..) = fetched(&state.take_work());
        done(
            &mut state,
            "a",
            Done::Page(
                ticket,
                Err(jmap::Error::Method {
                    error_type: "unsupportedSort".into(),
                    description: "no".into(),
                }),
            ),
        );
        let (_, sort, position, _) = fetched(&state.take_work());
        assert_eq!((sort, position), (NEWEST, 0));
        assert_eq!(state.status, crate::label("sort-unsupported"));
        assert_eq!(
            state.account("a").unwrap().mail.list_table().sort,
            Some(default_sort())
        );
    }

    #[test]
    fn the_neighbour_is_the_next_row_else_the_previous() {
        let rows: Vec<Email> = ["A", "B", "C", "D"]
            .iter()
            .map(|id| serde_json::from_value(json!({ "id": id })).unwrap())
            .collect();
        let gone = |ids: &[&str]| ids.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(neighbour(&rows, "B", &gone(&["B"])).as_deref(), Some("C"));
        assert_eq!(
            neighbour(&rows, "B", &gone(&["B", "C"])).as_deref(),
            Some("D")
        );
        assert_eq!(
            neighbour(&rows, "D", &gone(&["D"])).as_deref(),
            Some("C"),
            "the last moves back"
        );
        assert_eq!(
            neighbour(&rows, "D", &gone(&["C", "D"])).as_deref(),
            Some("B")
        );
        assert_eq!(neighbour(&rows[..1], "A", &gone(&["A"])), None);
        assert_eq!(neighbour(&rows, "Z", &gone(&["Z"])), None);
    }

    #[test]
    fn a_write_refreshes_its_view_and_answers_with_what_the_server_made() {
        let mut state = reading();
        let delete = Write::Delete {
            ids: vec!["E1".into()],
            destroy: false,
        };
        state.write_server("a".into(), Some(5), delete.clone());
        assert_eq!(writes(&mut state), [(Some(5), delete.clone())]);
        let tag = tag(&state, "a");
        state.apply(Event::Account(
            tag,
            Done::Wrote(Some(5), None, Box::new(delete), Ok(json!({"ids": ["E1"]}))),
        ));
        let view = state.account("a").unwrap();
        assert_eq!(view.mail.emails.len(), 1, "E1 left the list at once");
        assert!(view.mail.open.is_none());
        let work = state.take_work();
        assert!(
            work.iter()
                .any(|w| matches!(w, Work::OpenEmail { id, .. } if id == "E2")),
            "the reader moves on to the next row"
        );
        assert!(
            work.iter()
                .any(|w| matches!(w, Work::FetchMailboxes { .. }))
        );
        assert!(
            work.iter()
                .any(|w| matches!(w, Work::FetchEmails { mailbox, .. } if mailbox == "m-in"))
        );
        let reply = &state.take_replies()[0];
        assert_eq!((reply.id, reply.rc), (5, 0));
        assert_eq!(reply.body["ids"][0], "E1");
    }

    #[test]
    fn a_bus_write_with_one_bad_id_answers_and_keeps_that_row() {
        let mut state = reading();
        let shown: Vec<String> = state
            .account("a")
            .unwrap()
            .mail
            .emails
            .iter()
            .map(|e| e.id.clone())
            .collect();
        assert!(shown.len() >= 2, "{shown:?}");
        let delete = Write::Delete {
            ids: shown.clone(),
            destroy: false,
        };
        state.write_server("a".into(), Some(5), delete.clone());
        let _ = writes(&mut state);
        let tag = tag(&state, "a");
        let answer = json!({
            "ids": shown,
            "destroyed": false,
            "done": [shown[0]],
            "failed": [{"id": shown[1], "reason": "notFound"}],
        });
        state.apply(Event::Account(
            tag,
            Done::Wrote(Some(5), None, Box::new(delete), Ok(answer)),
        ));
        let view = state.account("a").unwrap();
        let left: Vec<&str> = view.mail.emails.iter().map(|e| e.id.as_str()).collect();
        assert!(!left.contains(&shown[0].as_str()), "the done id left");
        assert!(left.contains(&shown[1].as_str()), "the failed id stays");
        assert!(state.status.contains(&shown[1]), "{}", state.status);
        let reply = &state.take_replies()[0];
        assert_eq!((reply.id, reply.rc), (5, 0), "the call does not fail");
        assert_eq!(reply.body["failed"][0]["id"], shown[1].as_str());
    }

    #[test]
    fn a_bus_answer_goes_out_even_after_the_account_reconnected() {
        let mut state = connected();
        let old = tag(&state, "a");
        state.reconnect("a");
        state.apply(Event::Account(
            old.clone(),
            Done::Answered(8, Then::Answer, Ok(json!({"mailboxes": []}))),
        ));
        state.apply(Event::Account(
            old,
            Done::Wrote(
                Some(9),
                None,
                Box::new(Write::DeleteMailbox { id: "m".into() }),
                Err(jmap::Error::method("notFound", None)),
            ),
        ));
        // Both callers hear: the query refused as STALE (its data came from
        // the old connection), the write with its real outcome.
        let replies = state.take_replies();
        assert_eq!((replies[0].id, replies[0].rc), (8, 10));
        assert_eq!(replies[0].body["error_code"], code::STALE);
        assert_eq!((replies[1].id, replies[1].rc), (9, 10));
        assert_eq!(replies[1].body["error_code"], code::NOT_FOUND);
    }

    #[test]
    fn delete_confirms_when_the_settings_ask_and_not_otherwise() {
        let mut state = reading();
        state.delete_target();
        assert!(matches!(&state.form, Some(Form::Delete { ids, .. }) if ids == &["E1"]));
        assert!(batches(&mut state).is_empty(), "nothing until confirmed");
        state.apply_ui(UiEvent::FormSubmit(None));
        assert!(state.form.is_none());
        assert!(matches!(
            batches(&mut state).as_slice(),
            [Batch { kind: BatchKind::Delete, change: jmap::EmailChange::Move { mailbox }, .. }]
                if mailbox == "m-trash"
        ));
        let mut next = settings(&["a"]);
        next.mail.confirm_delete = false;
        state.set_settings(next);
        state.take_work();
        state.account_mut("a").unwrap().mail.open = Some(unread_email("E2"));
        state.delete_target();
        assert!(state.form.is_none());
        assert_eq!(batches(&mut state).len(), 1);
    }

    /// The open email is the target here: nothing is selected, so the list
    /// draws it as the keyboard row (phase 3b; these went as single
    /// writes before, now as batches of one).
    #[test]
    fn move_flag_and_mark_unread_act_on_the_target() {
        let mut state = reading();
        state.move_target();
        state.apply_ui(UiEvent::FormSubmit(Some("m-archive".into())));
        assert!(matches!(
            batches(&mut state).as_slice(),
            [Batch { kind: BatchKind::Move, change: jmap::EmailChange::Move { mailbox }, ids, .. }]
                if mailbox == "m-archive" && ids == &["E1"]
        ));
        state.flag_target();
        let view = state.account("a").unwrap();
        let flagged =
            view.mail.open.as_ref().unwrap().keywords.as_ref().unwrap()["$flagged"].clone();
        assert_eq!(flagged, json!(true), "shown at once");
        assert!(matches!(
            batches(&mut state).as_slice(),
            [Batch {
                change: jmap::EmailChange::Keywords {
                    flagged: Some(true),
                    read: None,
                },
                ..
            }]
        ));
        state.account_mut("a").unwrap().mail.emails[0].keywords = Some(json!({"$seen": true}));
        state.toggle_target_read();
        assert!(!state.account("a").unwrap().mail.emails[0].is_read());
        assert!(matches!(
            batches(&mut state).as_slice(),
            [Batch {
                change: jmap::EmailChange::Keywords {
                    read: Some(false),
                    ..
                },
                ..
            }]
        ));
    }

    #[test]
    fn the_event_form_checks_then_creates_and_closes_once_saved() {
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
        state.new_event();
        assert!(
            matches!(&state.form, Some(Form::Event(f)) if f.calendar == "C1" && !f.start.is_empty())
        );
        state.apply_ui(UiEvent::FormSubmit(None));
        assert!(
            matches!(&state.form, Some(Form::Event(f)) if f.error.is_some()),
            "a title is needed"
        );
        state.apply_ui(UiEvent::FormField(FormField::Title, "Standup".into()));
        state.apply_ui(UiEvent::FormField(FormField::End, "not a time".into()));
        state.apply_ui(UiEvent::FormSubmit(None));
        assert!(writes(&mut state).is_empty(), "a bad end is refused here");
        state.apply_ui(UiEvent::FormField(
            FormField::Start,
            "2026-10-17T09:00:00+10:00".into(),
        ));
        state.apply_ui(UiEvent::FormField(
            FormField::End,
            "2026-10-17T09:15:00+10:00".into(),
        ));
        state.apply_ui(UiEvent::FormSubmit(None));
        let write = match writes(&mut state).as_slice() {
            [(None, write @ Write::CreateEvent { calendar, draft })] => {
                assert_eq!((calendar.as_str(), draft.title.as_str()), ("C1", "Standup"));
                write.clone()
            }
            other => panic!("{other:?}"),
        };
        assert!(matches!(&state.form, Some(Form::Event(f)) if f.busy));
        let op = match &state.form {
            Some(Form::Event(f)) => f.op,
            _ => None,
        };
        let tag = tag(&state, "a");
        state.apply(Event::Account(
            tag,
            Done::Wrote(None, op, Box::new(write), Ok(json!({"id": "ev-1"}))),
        ));
        assert!(state.form.is_none());
        assert!(
            state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::FetchEvents { calendar, .. } if calendar == "C1"))
        );
    }

    #[test]
    fn send_as_new_copies_the_open_email_as_a_fresh_compose() {
        let mut state = reading();
        let mut email: serde_json::Value = serde_json::to_value(unread_email("E1")).unwrap();
        email["bodyValues"] =
            json!({ "p1": { "value": "Lunch at one?\n-- \nBob", "partId": "p1" } });
        email["textBody"] = json!([{ "partId": "p1", "type": "text/plain" }]);
        state.account_mut("a").unwrap().mail.open = Some(serde_json::from_value(email).unwrap());
        registry_run(&mut state, "mail.send_as_new");
        let compose = state.compose.clone().expect("a compose");
        assert_eq!(compose.draft.subject, "Hello", "no Re: or Fwd:");
        assert_eq!(compose.to_text, "ann@a.example.invalid, cy@example.invalid");
        assert_eq!(compose.cc_text, "dee@example.invalid");
        assert_eq!(
            compose.body.contents(),
            "Lunch at one?\n-- \nBob",
            "as it was, not re-signed"
        );
        assert_eq!(compose.draft.in_reply_to, None, "a new thread");
        assert!(compose.reply.is_none());
    }

    #[test]
    fn send_as_new_sends_as_the_identity_the_original_came_from() {
        let mut state = reading();
        let identity = |id: &str, address: &str| settings::email::Identity {
            id: id.into(),
            name: id.into(),
            address: address.into(),
            signature: String::new(),
            reply_to: None,
        };
        state.account_mut("a").unwrap().entry.identities = vec![
            identity("default", "ann@a.example.invalid"),
            identity("sales", "sales@a.example.invalid"),
        ];
        let mut email = unread_email("E1");
        email.from = Some(vec![EmailAddress {
            name: None,
            email: "Sales@a.example.invalid".into(),
        }]);
        state.account_mut("a").unwrap().mail.open = Some(email);
        state.send_as_new_open();
        let compose = state.compose.clone().expect("a compose");
        assert_eq!(compose.identity, "sales");
        assert_eq!(compose.draft.from, "sales@a.example.invalid");
        assert!(!compose.keepable(), "the switch is not an edit");
        // From someone else: the account's default.
        state.compose = None;
        state.account_mut("a").unwrap().mail.open = Some(unread_email("E1"));
        state.send_as_new_open();
        assert_eq!(state.compose.as_ref().unwrap().identity, "default");
    }

    #[test]
    fn open_email_actions_wait_while_another_email_loads() {
        let mut state = reading();
        let registry = crate::actions::registry();
        let enabled = |state: &State, id: &str| (registry.get(id).unwrap().enabled)(state);
        for id in ["mail.reply", "mail.forward", "mail.send_as_new"] {
            assert!(enabled(&state, id), "{id} with E1 open");
        }
        // A click (or a right-click) on E2: E1 is still shown until E2 loads.
        assert!(state.open_email("a", "E2".into()));
        for id in ["mail.reply", "mail.forward", "mail.send_as_new"] {
            assert!(!enabled(&state, id), "{id} while E2 loads");
        }
        // Row actions act on the selection, which is E2 already.
        assert!(enabled(&state, "mail.delete"));
        state.account_mut("a").unwrap().mail.open = Some(unread_email("E2"));
        assert!(enabled(&state, "mail.reply"), "E2 loaded");
    }

    #[test]
    fn a_typed_reply_to_survives_an_identity_change_and_the_identitys_follows() {
        let mut state = reading();
        let identity = |id: &str, reply_to: Option<&str>| settings::email::Identity {
            id: id.into(),
            name: id.into(),
            address: format!("{id}@a.example.invalid"),
            signature: String::new(),
            reply_to: reply_to.map(Into::into),
        };
        state.account_mut("a").unwrap().entry.identities = vec![
            identity("ann", Some("help@a.example.invalid")),
            identity("sales", Some("orders@a.example.invalid")),
        ];
        assert!(state.open_compose(Some("a"), Composed::default()));
        let reply_to = |state: &State| state.compose.as_ref().unwrap().draft.reply_to.clone();
        assert_eq!(reply_to(&state).as_deref(), Some("help@a.example.invalid"));
        // Untouched: the identity's Reply-To follows the identity.
        state.choose_sender("a".into(), Some("sales".into()));
        assert_eq!(
            reply_to(&state).as_deref(),
            Some("orders@a.example.invalid")
        );
        // Typed: it stays.
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::ReplyTo,
            "me@x.invalid".into(),
        ));
        state.choose_sender("a".into(), Some("ann".into()));
        assert_eq!(reply_to(&state).as_deref(), Some("me@x.invalid"));
    }

    #[test]
    fn an_optional_row_stays_open_while_cleared_and_reply_to_is_checked_at_send() {
        let mut state = reading();
        assert!(state.open_compose(Some("a"), Composed::default()));
        let shows = |state: &State, f| state.compose.as_ref().unwrap().shows(f);
        assert!(!shows(&state, ComposeField::Cc));
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::Cc,
            "x@y.invalid".into(),
        ));
        state.apply_ui(UiEvent::ComposeField(ComposeField::Cc, String::new()));
        assert!(
            shows(&state, ComposeField::Cc),
            "cleared to retype: still open"
        );
        // The button closes it again.
        state.apply_ui(UiEvent::ComposeShow(ComposeField::Cc));
        assert!(!shows(&state, ComposeField::Cc));
        // Reply-To must be one address.
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::ReplyTo,
            "a@x.invalid, b@x.invalid".into(),
        ));
        assert!(parse_recipients(state.compose.as_ref().unwrap()).is_err());
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::ReplyTo,
            "Help <help@x.invalid>".into(),
        ));
        assert!(parse_recipients(state.compose.as_ref().unwrap()).is_ok());
        // Cleared: no Reply-To at all.
        state.apply_ui(UiEvent::ComposeField(ComposeField::ReplyTo, "  ".into()));
        assert_eq!(state.compose.as_ref().unwrap().draft.reply_to, None);
    }

    #[test]
    fn a_failed_open_gives_the_actions_back_to_the_email_shown() {
        let mut state = reading();
        let registry = crate::actions::registry();
        assert!(state.open_email("a", "E2".into()));
        let ticket = state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::OpenEmail { ticket, .. } => Some(*ticket),
                _ => None,
            })
            .unwrap();
        assert!(!(registry.get("mail.reply").unwrap().enabled)(&state));
        let tag = tag(&state, "a");
        state.apply(Event::Account(
            tag,
            Done::Opened(
                ticket,
                None,
                Box::new(Err(jmap::Error::Protocol("gone".into()))),
            ),
        ));
        assert!(
            (registry.get("mail.reply").unwrap().enabled)(&state),
            "E1 is still shown and answers Reply"
        );
        // And a click on E2 tries it again.
        state.mail_table(
            "a",
            TableEvent::Selected {
                index: 1,
                key: "E2".into(),
                pick: toolkit::datatable::Pick::Only,
            },
        );
        assert!(
            state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::OpenEmail { id, .. } if id == "E2"))
        );
    }

    #[test]
    fn a_right_click_inside_a_selection_opens_that_row_and_keeps_the_selection() {
        let mut state = reading();
        state.account_mut("a").unwrap().mail.table.selection = vec!["E1".into(), "E2".into()];
        state.take_work();
        state.mail_table(
            "a",
            TableEvent::ContextMenu {
                index: 1,
                key: "E2".into(),
            },
        );
        assert!(
            state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::OpenEmail { id, .. } if id == "E2")),
            "the menu's Reply is for E2"
        );
        assert_eq!(
            state.account("a").unwrap().mail.table.selection,
            ["E1", "E2"],
            "Delete and Move still act on both"
        );
        // On the open row it opens nothing again.
        state.account_mut("a").unwrap().mail.open = Some(unread_email("E2"));
        state.mail_table(
            "a",
            TableEvent::ContextMenu {
                index: 1,
                key: "E2".into(),
            },
        );
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::OpenEmail { .. }))
        );
    }

    fn registry_run(state: &mut State, id: &str) {
        crate::actions::registry()
            .execute(id, state, toolkit::action::Origin::Bus)
            .unwrap();
    }

    #[test]
    fn a_bus_reply_or_forward_opens_compose_from_the_fetched_email() {
        let mut state = connected();
        let email = serde_json::to_value(unread_email("E7")).unwrap();
        let tag = tag(&state, "a");
        state.apply(Event::Account(
            tag.clone(),
            Done::Answered(3, Then::Reply(false, Some(false)), Ok(email.clone())),
        ));
        let compose = state.compose.clone().expect("a reply");
        assert_eq!(compose.draft.subject, "Re: Hello");
        assert_eq!(state.take_replies()[0].body["composing"], "a");
        // A second one while that compose is open is refused, not lost.
        state.apply(Event::Account(
            tag.clone(),
            Done::Answered(
                4,
                Then::Forward("x@example.invalid".into()),
                Ok(email.clone()),
            ),
        ));
        assert_eq!(state.take_replies()[0].body["error_code"], code::BUSY);
        state.compose = None;
        state.apply(Event::Account(
            tag,
            Done::Answered(5, Then::Forward("x@example.invalid".into()), Ok(email)),
        ));
        let compose = state.compose.clone().expect("a forward");
        assert_eq!(compose.draft.subject, "Fwd: Hello");
        assert_eq!(compose.to_text, "x@example.invalid");
        assert!(
            compose
                .body
                .contents()
                .contains("Bob <bob@example.invalid>"),
            "{}",
            compose.body.contents()
        );
    }

    #[test]
    fn a_bus_open_answers_once_the_email_loads() {
        let mut state = reading();
        state.open_by_bus("a", "E2".into(), 6);
        let ticket = state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::OpenEmail {
                    ticket,
                    reply: Some(6),
                    ..
                } => Some(*ticket),
                _ => None,
            })
            .expect("an open for the Bus");
        assert!(
            state.account("a").unwrap().mail.emails[1].is_read(),
            "marked read like a click"
        );
        done(
            &mut state,
            "a",
            Done::Opened(ticket, Some(6), Box::new(Ok(unread_email("E2")))),
        );
        assert_eq!(state.take_replies()[0].body["id"], "E2");
        assert_eq!(
            state.account("a").unwrap().mail.open.as_ref().unwrap().id,
            "E2"
        );
        // A real open (the latest) of an id the server does not hold.
        state.open_by_bus("a", "E9".into(), 7);
        let missing = state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::OpenEmail {
                    ticket,
                    reply: Some(7),
                    ..
                } => Some(*ticket),
                _ => None,
            })
            .unwrap();
        done(
            &mut state,
            "a",
            Done::Opened(
                missing,
                Some(7),
                Box::new(Err(jmap::Error::Protocol("email \"E9\" is gone".into()))),
            ),
        );
        assert_eq!(state.take_replies()[0].body["error_code"], code::NOT_FOUND);
    }

    /// Answer every secrets save in `work` with `result`, as the shell does.
    fn land(state: &mut State, work: &[Work], result: Result<bool, String>) {
        let version = state.secrets_snapshot().0;
        for w in work {
            if let Work::SaveSecrets { ticket } = w {
                state.secrets_saved(*ticket, version, result.clone());
            }
        }
    }

    fn saves(work: &[Work]) -> usize {
        work.iter()
            .filter(|w| matches!(w, Work::SaveSecrets { .. }))
            .count()
    }

    /// Review finding 1: a read with no accounts (an older settingsd that
    /// omits the group looks the same) keeps the shown accounts, their
    /// clients and their passwords; a later non-empty read ends it, and a
    /// removal seen there leaves the password in the file.
    #[test]
    fn review_1_an_empty_read_keeps_accounts_clients_and_passwords() {
        let mut state = connected_many(&["a", "b"]);
        state.set_settings(EmailSettings::default());
        let work = state.take_work();
        assert!(state.holding());
        assert_eq!(state.accounts.len(), 2);
        assert!(state.accounts.iter().all(|v| v.phase == Phase::Ready));
        assert!(state.has_password("a") && state.has_password("b"));
        assert_eq!(saves(&work), 0);
        assert!(!work.iter().any(|w| matches!(w, Work::Disconnect { .. })));
        assert_eq!(state.status, crate::label("settings-empty-held"));
        assert_eq!(state.cache().accounts.len(), 2, "the cache keeps them too");
        state.set_settings(settings(&["a"]));
        assert!(!state.holding());
        assert!(state.account("b").is_none());
        assert!(
            state.has_password("b"),
            "an orphan entry, never deleted by a read"
        );
        assert_eq!(saves(&state.take_work()), 0);
    }

    /// Review finding 1: only an explicit removal through Email forgets a
    /// password, and it ends the holding even for the last account.
    #[test]
    fn review_1_an_explicit_remove_forgets_the_password() {
        let mut state = connected_many(&["a"]);
        state.change_accounts(Change::Remove("a".into()), Some(1), None);
        let (ticket, _) = settings_ticket(&mut state);
        state.settled(ticket, Settled::Accepted);
        let work = state.take_work();
        assert!(state.accounts.is_empty());
        assert!(!state.has_password("a"));
        assert_eq!(saves(&work), 1);
        assert!(
            state.take_replies().is_empty(),
            "answered once the file has it"
        );
        land(&mut state, &work, Ok(true));
        assert_eq!(state.take_replies()[0].rc, 0);
        state.set_settings(EmailSettings::default());
        assert!(!state.holding());
    }

    fn legacy_matching(held: &AccountEntry) -> AccountEntry {
        AccountEntry {
            id: "acct-legacy".into(),
            ..held.clone()
        }
    }

    /// Review finding 2: settingsd holds the old account under another id
    /// and has no default; the default is applied, the password is written
    /// under the adopted id, and only then does the migration finish. The
    /// password is never dropped on the way.
    #[test]
    fn review_2_adopting_without_a_default_transfers_the_password_first() {
        let held = entry("a");
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            EmailSettings::default(),
            Secrets::default(),
        );
        state.begin_migration(legacy_matching(&held), "pw-1");
        let work = state.take_work();
        land(&mut state, &work, Ok(true));
        state.set_settings(EmailSettings {
            accounts: vec![held],
            ..EmailSettings::default()
        });
        let (ticket, changes) = settings_ticket(&mut state);
        assert_eq!(changes, json!({"email.default_account": "a"}));
        assert!(
            state.has_password("acct-legacy"),
            "kept while the apply is out"
        );
        state.settled(ticket, Settled::Accepted);
        let work = state.take_work();
        assert!(state.migrating() && !state.take_migrated());
        assert!(state.has_password("a") && state.has_password("acct-legacy"));
        assert_eq!(saves(&work), 1);
        land(&mut state, &work, Ok(true));
        assert!(state.take_migrated());
        state.migration_rewritten(Ok(true));
        assert!(!state.migrating());
    }

    /// Review finding 3: a secrets save refused by the poison pill
    /// (`Ok(false)`) stops the migration before settingsd, and the schema-1
    /// config is never rewritten; a failed transfer stops it too.
    #[test]
    fn review_3_a_refused_save_stops_the_migration() {
        let held = entry("a");
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            EmailSettings::default(),
            Secrets::default(),
        );
        state.begin_migration(legacy_matching(&held), "pw-1");
        let work = state.take_work();
        land(&mut state, &work, Ok(false));
        state.set_settings(EmailSettings::default());
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Settings { .. }))
        );
        assert!(state.migrating() && !state.take_migrated());
        // The same with the transfer failing.
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            EmailSettings::default(),
            Secrets::default(),
        );
        state.begin_migration(legacy_matching(&held), "pw-1");
        let work = state.take_work();
        land(&mut state, &work, Ok(true));
        state.set_settings(EmailSettings {
            accounts: vec![held.clone()],
            default_account: Some("a".into()),
            ..EmailSettings::default()
        });
        let work = state.take_work();
        land(&mut state, &work, Err("disk full".into()));
        assert!(state.migrating() && !state.take_migrated());
    }

    /// Review finding 4: an acknowledgement arriving after a newer read
    /// never installs its older prediction over it.
    #[test]
    fn review_4_a_late_ack_never_overwrites_a_newer_read() {
        let mut state = connected_many(&["a"]);
        let rename = writer::AccountPatch {
            name: Some("Renamed".into()),
            ..writer::AccountPatch::default()
        };
        state.change_accounts(Change::Update("a".into(), rename), Some(2), None);
        let (ticket, _) = settings_ticket(&mut state);
        let mut newer = settings(&["a", "c"]);
        newer.accounts[0].name = "Renamed".into();
        state.set_settings(newer.clone());
        state.settled(ticket, Settled::Accepted);
        assert_eq!(state.settings, newer);
        assert!(state.account("c").is_some());
        assert_eq!(state.take_replies()[0].body["id"], "a");
    }

    /// Review finding 8: a reply's email fetched over a connection the
    /// account has left is refused, never opened under the new identity.
    #[test]
    fn review_8_a_continuation_from_an_old_connection_is_stale() {
        let mut state = connected();
        let old = tag(&state, "a");
        state.reconnect("a");
        let email = serde_json::to_value(unread_email("E7")).unwrap();
        state.apply(Event::Account(
            old,
            Done::Answered(3, Then::Forward(String::new()), Ok(email)),
        ));
        assert!(state.compose.is_none());
        assert_eq!(state.take_replies()[0].body["error_code"], code::STALE);
    }

    /// Review finding 9: a send settles its compose by its own ticket even
    /// after the account reconnected; the outcome is always shown.
    #[test]
    fn review_9_a_send_settles_its_compose_after_a_reconnect() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid".into(),
        ));
        state.apply_ui(UiEvent::SendCompose);
        let (_, ticket) = sent_ticket(&mut state);
        let old = tag(&state, "a");
        state.reconnect("a");
        state.apply(Event::Account(
            old.clone(),
            failed(ticket, jmap::Error::Protocol("relay down".into())),
        ));
        let compose = state.compose.clone().expect("the dialog stays");
        assert!(!compose.busy && compose.error.is_some());
        // Sent again, and this time it went: the dialog closes.
        state.apply_ui(UiEvent::SendCompose);
        let (_, ticket) = sent_ticket(&mut state);
        state.reconnect("a");
        state.apply(Event::Account(old, went(ticket)));
        assert!(state.compose.is_none());
        assert_eq!(state.status, crate::label("sent"));
    }

    /// An open, submitted New Event form, with its operation ticket.
    fn submitted_event(state: &mut State) -> Option<u64> {
        state.new_event();
        for (field, value) in [
            (FormField::Title, "Standup"),
            (FormField::Start, "2026-10-17T09:00:00+10:00"),
            (FormField::End, "2026-10-17T09:15:00+10:00"),
        ] {
            state.apply_ui(UiEvent::FormField(field, value.into()));
        }
        state.apply_ui(UiEvent::FormSubmit(None));
        state.take_work();
        match &state.form {
            Some(Form::Event(f)) => f.op,
            _ => None,
        }
    }

    /// Review finding 10: a write's completion settles only the form that
    /// sent it, matched by its operation ticket.
    #[test]
    fn review_10_a_write_settles_only_the_form_that_sent_it() {
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
        let first = submitted_event(&mut state);
        let tag = tag(&state, "a");
        let other = Write::DeleteMailbox { id: "m".into() };
        state.apply(Event::Account(
            tag.clone(),
            Done::Wrote(
                Some(4),
                None,
                Box::new(other),
                Err(jmap::Error::Protocol("x".into())),
            ),
        ));
        assert!(
            matches!(&state.form, Some(Form::Event(f)) if f.busy),
            "an unrelated failure leaves it busy"
        );
        state.apply_ui(UiEvent::FormCancel);
        let second = submitted_event(&mut state);
        assert_ne!(first, second);
        let create = Write::CreateEvent {
            calendar: "C1".into(),
            draft: jmap::EventDraft::default(),
        };
        state.apply(Event::Account(
            tag.clone(),
            Done::Wrote(
                None,
                first,
                Box::new(create.clone()),
                Ok(json!({"id": "ev-1"})),
            ),
        ));
        assert!(
            matches!(&state.form, Some(Form::Event(f)) if f.busy),
            "the first create never closes the second form"
        );
        state.apply(Event::Account(
            tag,
            Done::Wrote(None, second, Box::new(create), Ok(json!({"id": "ev-2"}))),
        ));
        assert!(state.form.is_none());
    }

    /// Review finding 12: a password whose save failed is saved again when
    /// the same password is sent again, not reported saved.
    #[test]
    fn review_12_a_failed_password_save_is_retried() {
        let mut state = connected();
        let change = || Change::Update("a".into(), writer::AccountPatch::default());
        state.change_accounts(change(), Some(1), Some("pw-2".into()));
        let work = state.take_work();
        assert_eq!(saves(&work), 1);
        land(&mut state, &work, Err("disk full".into()));
        assert_eq!(state.take_replies()[0].body["error_code"], code::SECRETS);
        state.change_accounts(change(), Some(2), Some("pw-2".into()));
        assert!(state.take_replies().is_empty(), "not reported saved");
        let work = state.take_work();
        assert_eq!(saves(&work), 1, "saved again");
        land(&mut state, &work, Ok(true));
        let reply = &state.take_replies()[0];
        assert_eq!((reply.id, reply.rc), (2, 0));
        // Now durable: the same password again needs no save.
        state.change_accounts(change(), Some(3), Some("pw-2".into()));
        assert_eq!(saves(&state.take_work()), 0);
        assert_eq!(state.take_replies()[0].id, 3);
    }

    fn rename() -> Change {
        Change::Update(
            "a".into(),
            writer::AccountPatch {
                name: Some("Renamed".into()),
                ..writer::AccountPatch::default()
            },
        )
    }

    /// Review #4 remainder: a write's password (or a removal's forget)
    /// never lands over a newer password set or forget.
    #[test]
    fn review_4_a_late_write_never_overwrites_a_newer_password() {
        // A newer password set wins.
        let mut state = connected();
        state.change_accounts(rename(), Some(1), Some("pw-1".into()));
        let (ticket, _) = settings_ticket(&mut state);
        state.set_password("a", "pw-2", Some(2));
        state.settled(ticket, Settled::Accepted);
        drive(&mut state);
        assert_eq!(state.secrets.password("a"), Some("pw-2"));
        // A newer forget wins.
        let mut state = connected();
        state.change_accounts(rename(), Some(1), Some("pw-1".into()));
        let (ticket, _) = settings_ticket(&mut state);
        state.forget_password("a", 2);
        state.settled(ticket, Settled::Accepted);
        drive(&mut state);
        assert!(!state.has_password("a"));
        // A removal's forget yields to a newer password set.
        let mut state = connected();
        state.change_accounts(Change::Remove("a".into()), Some(1), None);
        let (ticket, _) = settings_ticket(&mut state);
        state.set_password("a", "pw-3", Some(2));
        state.settled(ticket, Settled::Accepted);
        drive(&mut state);
        assert_eq!(state.secrets.password("a"), Some("pw-3"));
    }

    /// Review #14: generations come from one counter, so an account
    /// removed and restored never reuses one, and an old answer stays void.
    #[test]
    fn review_14_a_restored_account_never_reuses_a_generation() {
        let mut state = connected_many(&["a", "b"]);
        let old = tag(&state, "a");
        state.set_settings(settings(&["b"]));
        assert!(state.account("a").is_none());
        state.set_settings(settings(&["a", "b"]));
        let restored = tag(&state, "a");
        assert!(restored.generation > old.generation);
        state.apply(Event::Connected(old, Ok(info())));
        assert_eq!(state.account("a").unwrap().phase, Phase::Connecting);
        let generations: Vec<u64> = state.accounts.iter().map(|v| v.generation).collect();
        assert_ne!(generations[0], generations[1], "unique across accounts too");
    }

    fn deadline_op(work: &[Work]) -> u64 {
        work.iter()
            .find_map(|w| match w {
                Work::WriteDeadline { op } => Some(*op),
                _ => None,
            })
            .expect("a deadline")
    }

    /// Review #16: a conflict retries at once over a newer read already in
    /// force, an unchanged read releases a waiting write, and a deadline
    /// answers one that never settles.
    #[test]
    fn review_16_no_write_waits_forever() {
        let mut state = connected_many(&["a"]);
        state.change_accounts(rename(), Some(1), None);
        let (ticket, _) = settings_ticket(&mut state);
        state.set_settings(settings(&["a", "b"]));
        state.take_work();
        state.settled(ticket, Settled::Conflict);
        let (ticket, changes) = settings_ticket(&mut state);
        assert_eq!(
            changes["email.accounts"][0]["name"], "Renamed",
            "sent again at once"
        );
        // No newer read: it waits, and an unchanged read releases it.
        state.settled(ticket, Settled::Conflict);
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Settings { .. }))
        );
        state.settings_unchanged();
        assert!(
            state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Settings { .. }))
        );
        // A write that never settles is answered at its deadline.
        let mut state = connected_many(&["a"]);
        state.change_accounts(rename(), Some(7), None);
        let op = deadline_op(&state.take_work());
        state.write_deadline(op);
        let reply = &state.take_replies()[0];
        assert_eq!(
            (reply.id, reply.body["error_code"].clone()),
            (7, json!(code::TIMEOUT))
        );
    }

    /// Review #17: removing an account shown only because settingsd showed
    /// none removes it and its secret, keeps the others, and answers rc 0.
    #[test]
    fn review_17_removing_a_held_account_removes_it_and_its_secret() {
        let mut state = connected_many(&["a", "b"]);
        state.set_settings(EmailSettings::default());
        assert!(state.holding());
        state.change_accounts(Change::Remove("a".into()), Some(1), None);
        let work = state.take_work();
        assert!(!work.iter().any(|w| matches!(w, Work::Settings { .. })));
        assert!(state.account("a").is_none() && state.account("b").is_some());
        assert!(state.holding() && !state.has_password("a") && state.has_password("b"));
        assert!(state.take_replies().is_empty(), "answered once durable");
        land(&mut state, &work, Ok(true));
        let reply = &state.take_replies()[0];
        assert_eq!((reply.id, reply.rc), (1, 0));
        assert_eq!(state.cache().accounts.len(), 1);
    }

    /// Review #18: a login change under an open draft makes it stale: Send
    /// is refused with an error until the new address is taken.
    #[test]
    fn review_18_a_compose_goes_stale_when_its_login_changes() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        state.apply_ui(UiEvent::ComposeField(
            ComposeField::To,
            "bob@example.invalid".into(),
        ));
        let mut next = settings(&["a"]);
        next.accounts[0].email = "new@a.example.invalid".into();
        state.set_settings(next);
        state.take_work();
        let compose = state.compose.clone().unwrap();
        assert_eq!(
            compose.stale.as_ref().map(|s| s.address.as_str()),
            Some("new@a.example.invalid")
        );
        assert_eq!(
            compose.draft.from, "ann@a.example.invalid",
            "unchanged until taken"
        );
        state.apply_ui(UiEvent::SendCompose);
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Send { .. }))
        );
        assert_eq!(
            state.compose.as_ref().unwrap().error.as_deref(),
            Some(crate::label("compose-from-changed").as_str())
        );
        state.apply_ui(UiEvent::ComposeUseNewFrom);
        let compose = state.compose.clone().unwrap();
        assert_eq!(compose.draft.from, "new@a.example.invalid");
        assert!(compose.stale.is_none());
        state.apply_ui(UiEvent::SendCompose);
        assert!(
            state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Send { .. }))
        );
    }

    /// Review #19: a save's ack clears only what had changed by the
    /// version it saved; a later change stays unsaved.
    #[test]
    fn review_19_a_late_save_ack_never_clears_a_newer_change() {
        // Two accounts, so the two saves are in flight together (one
        // account's credential operations run one at a time).
        let mut state = connected_many(&["a", "b"]);
        state.set_password("a", "pw-1", None);
        let first = state.secrets_snapshot().0;
        let ticket = state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::SaveSecrets { ticket } => Some(*ticket),
                _ => None,
            })
            .unwrap();
        state.set_password("b", "pw-2", None);
        let second = state.secrets_snapshot().0;
        assert!(second > first);
        state.secrets_saved(ticket, first, Ok(true));
        assert!(
            !state.unsaved.contains_key("a"),
            "pw-1 was in the saved snapshot"
        );
        assert!(state.unsaved.contains_key("b"), "pw-2 is not durable yet");
        let work = state.take_work();
        land(&mut state, &work, Ok(true));
        assert!(!state.unsaved.contains_key("b"));
    }

    /// Settle everything the state asks of settingsd and the secrets file
    /// (every apply accepted, every save landed) until nothing is left.
    fn drive(state: &mut State) {
        for _ in 0..32 {
            let work = state.take_work();
            if !work
                .iter()
                .any(|w| matches!(w, Work::Settings { .. } | Work::SaveSecrets { .. }))
            {
                return;
            }
            for w in &work {
                if let Work::Settings { ticket, .. } = w {
                    state.settled(*ticket, Settled::Accepted);
                }
            }
            land(state, &work, Ok(true));
        }
    }

    /// Round 4, #4: two account writes carrying passwords for one account,
    /// sent before either settled, end with the later password stored, and
    /// both callers hear rc 0.
    #[test]
    fn round4_4_concurrent_password_writes_keep_the_later_one() {
        let mut state = connected();
        state.change_accounts(rename(), Some(1), Some("pw-1".into()));
        state.change_accounts(rename(), Some(2), Some("pw-2".into()));
        drive(&mut state);
        assert_eq!(state.secrets.password("a"), Some("pw-2"));
        let replies = state.take_replies();
        let ids: Vec<(u64, u8)> = replies.iter().map(|r| (r.id, r.rc)).collect();
        assert_eq!(ids, [(1, 0), (2, 0)]);
    }

    /// Round 4, #18: switching the draft's From to another account never
    /// lets the first account's new address be taken for it.
    #[test]
    fn round4_18_switching_from_drops_the_other_accounts_new_address() {
        let mut state = connected_many(&["a", "b"]);
        state.apply_ui(UiEvent::Compose);
        let mut next = settings(&["a", "b"]);
        next.accounts[0].email = "new@a.example.invalid".into();
        state.set_settings(next);
        state.take_work();
        assert!(state.compose.as_ref().unwrap().stale.is_some());
        state.apply_ui(UiEvent::ComposeAccount("b".into()));
        state.apply_ui(UiEvent::ComposeUseNewFrom);
        let compose = state.compose.clone().unwrap();
        assert_eq!(compose.account, "b");
        assert_eq!(compose.draft.from, "ann@b.example.invalid");
        assert!(compose.stale.is_none(), "b's login did not change");
    }

    /// Round 4, #20: send ids are state-wide: an account removed and
    /// restored never reuses one, so an old completion leaves a new
    /// send alone.
    #[test]
    fn round4_20_a_restored_account_never_reuses_a_send_id() {
        let mut state = connected_many(&["a", "b"]);
        let send = |state: &mut State| {
            state.apply_ui(UiEvent::Compose);
            state.apply_ui(UiEvent::ComposeField(
                ComposeField::To,
                "bob@example.invalid".into(),
            ));
            state.apply_ui(UiEvent::SendCompose);
            sent_ticket(state).1
        };
        let first = send(&mut state);
        let old = tag(&state, "a");
        state.set_settings(settings(&["b"]));
        state.set_settings(settings(&["a", "b"]));
        let fresh = tag(&state, "a");
        state.apply(Event::Connected(fresh, Ok(info())));
        state.take_work();
        let second = send(&mut state);
        assert_ne!(first, second);
        state.apply(Event::Account(old, went(first)));
        assert!(
            state.compose.as_ref().is_some_and(|c| c.busy),
            "the old completion leaves the new send pending"
        );
    }

    /// Round 4, #22: a form opened before the account's login changed
    /// does not submit through the new connection.
    #[test]
    fn round4_22_a_form_from_an_old_connection_does_not_submit() {
        let mut state = reading();
        state.delete_target();
        assert!(matches!(state.form, Some(Form::Delete { .. })));
        let mut next = settings(&["a"]);
        next.accounts[0].url = "https://moved.example.invalid".into();
        state.set_settings(next);
        state.take_work();
        state.apply_ui(UiEvent::FormSubmit(None));
        assert!(
            batches(&mut state).is_empty(),
            "no delete through the new server"
        );
    }

    /// Round 4, #21: the migration (and its password lock) is not finished
    /// when settingsd takes the account: the schema-2 rewrite must land
    /// first.
    #[test]
    fn round4_21_the_migration_holds_until_the_config_is_rewritten() {
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            EmailSettings::default(),
            Secrets::default(),
        );
        state.begin_migration(entry("legacy"), "pw-1");
        let work = state.take_work();
        land(&mut state, &work, Ok(true));
        state.set_settings(EmailSettings::default());
        let (ticket, _) = settings_ticket(&mut state);
        state.settled(ticket, Settled::Accepted);
        assert!(state.take_migrated(), "the shell is asked to rewrite");
        assert!(state.migrating(), "still locked until the rewrite lands");
        assert!(state.migration_locks("legacy"));
    }

    /// Round 4, #24: an unchanged read resumes a migration whose apply
    /// came back with an unknown outcome.
    #[test]
    fn round4_24_an_unchanged_read_resumes_a_ready_migration() {
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            EmailSettings::default(),
            Secrets::default(),
        );
        state.begin_migration(entry("legacy"), "pw-1");
        let work = state.take_work();
        land(&mut state, &work, Ok(true));
        state.set_settings(EmailSettings::default());
        let (ticket, _) = settings_ticket(&mut state);
        state.settled(ticket, Settled::Unknown("timed out".into()));
        state.take_work();
        state.settings_unchanged();
        assert!(
            state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Settings { .. })),
            "the migration is sent again"
        );
    }

    /// Round 4, #21 (new API): a failed schema-2 rewrite keeps the
    /// migration and its lock, and says why; a confirmed one ends it.
    #[test]
    fn round4_21_a_failed_rewrite_keeps_the_migration() {
        let mut state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            EmailSettings::default(),
            Secrets::default(),
        );
        state.begin_migration(entry("legacy"), "pw-1");
        let work = state.take_work();
        land(&mut state, &work, Ok(true));
        state.set_settings(EmailSettings::default());
        let (ticket, _) = settings_ticket(&mut state);
        state.settled(ticket, Settled::Accepted);
        assert!(state.take_migrated());
        state.migration_rewritten(Err("read-only file system".into()));
        assert!(state.migrating() && state.migration_locks("legacy"));
        assert!(
            state.status.contains("read-only file system"),
            "{}",
            state.status
        );
        // A later confirmation for a stopped migration changes nothing.
        state.migration_rewritten(Ok(true));
        assert!(state.migrating());
    }

    /// Round 4, #22 (new API): the note shows at once on a login change,
    /// and a form write is bound to the connection it was opened on.
    #[test]
    fn round4_22_a_login_change_marks_the_open_form() {
        let mut state = reading();
        state.move_target();
        let mut next = settings(&["a"]);
        next.accounts[0].email = "new@a.example.invalid".into();
        state.set_settings(next);
        assert_eq!(
            state.form_note.as_deref(),
            Some(crate::label("form-account-changed").as_str())
        );
        state.apply_ui(UiEvent::FormSubmit(Some("m-archive".into())));
        assert!(batches(&mut state).is_empty());
        // A form opened now is bound to the new connection.
        state.apply_ui(UiEvent::FormCancel);
        let view = state.account_mut("a").unwrap();
        view.mail.emails = vec![unread_email("E1")];
        view.mail.open = Some(unread_email("E1"));
        state.move_target();
        assert!(state.form_note.is_none());
        state.apply_ui(UiEvent::FormSubmit(Some("m-archive".into())));
        let expect = state.take_work().into_iter().find_map(|w| match w {
            Work::Batch { generation, .. } => Some(generation),
            _ => None,
        });
        assert_eq!(expect, Some(tag(&state, "a").generation));
    }

    /// Round 4, #23: closing answers every caller still waiting: a sent
    /// settings write and a server write may have happened, a query and
    /// a queued credential operation are cancelled.
    #[test]
    fn round4_23_closing_answers_every_outstanding_caller() {
        let mut state = connected();
        state.change_accounts(rename(), Some(1), Some("pw-1".into()));
        settings_ticket(&mut state);
        state.set_password("a", "pw-2", Some(2));
        state.query(
            "a".into(),
            3,
            crate::commands::Query::Mailboxes,
            Then::Answer,
        );
        state.write_server("a".into(), Some(4), Write::DeleteMailbox { id: "m".into() });
        state.take_work();
        state.close_out();
        let codes: Vec<(u64, String)> = state
            .take_replies()
            .into_iter()
            .map(|r| (r.id, r.body["error_code"].as_str().unwrap_or("").to_owned()))
            .collect();
        for (id, expected) in [
            (1, code::OUTCOME_UNKNOWN),
            (2, code::CANCELLED),
            (3, code::CANCELLED),
            (4, code::OUTCOME_UNKNOWN),
        ] {
            assert!(
                codes.iter().any(|(i, c)| *i == id && c == expected),
                "{id}: {codes:?}"
            );
        }
        assert!(state.take_work().is_empty(), "nothing more is started");
    }

    /// A migration whose legacy account settingsd already holds as "a"
    /// with no default: begun, its password durable, settingsd read, the
    /// default-setting apply sent. Returns the apply's ticket.
    fn adopting(state: &mut State) -> u64 {
        let held = entry("a");
        state.begin_migration(legacy_matching(&held), "pw-legacy");
        let work = state.take_work();
        land(state, &work, Ok(true));
        state.set_settings(EmailSettings {
            accounts: vec![held],
            ..EmailSettings::default()
        });
        settings_ticket(state).0
    }

    fn fresh() -> State {
        State::new(
            UiState::default(),
            Mode::Mail,
            None,
            EmailSettings::default(),
            Secrets::default(),
        )
    }

    /// Round 5, #15: the adopted id stays locked even once settingsd no
    /// longer lists it, and the migration replans: the legacy account is
    /// applied again, so a removal elsewhere never loses it.
    #[test]
    fn round5_15_an_adopted_account_removed_elsewhere_stays_locked_and_comes_back() {
        let mut state = fresh();
        let ticket = adopting(&mut state);
        state.settled(ticket, Settled::Accepted);
        let work = state.take_work();
        land(&mut state, &work, Ok(true));
        assert!(state.take_migrated());
        state.migration_rewritten(Err("read-only file system".into()));
        assert!(state.migration_locks("a"));
        // Prefs removes "a" straight through settingsd.
        state.set_settings(settings(&["b"]));
        assert!(
            state.migration_locks("a"),
            "still locked without settings listing it"
        );
        let (_, changes) = settings_ticket(&mut state);
        let ids: Vec<&str> = changes["email.accounts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a["id"].as_str())
            .collect();
        assert!(
            ids.contains(&"acct-legacy"),
            "the legacy account is applied again: {ids:?}"
        );
        assert!(state.has_password("acct-legacy"));
    }

    /// Round 5, #18: a stale From from an older connection is recomputed
    /// from the current one, and Send stays blocked until it is taken.
    #[test]
    fn round5_18_a_mismatched_new_from_is_recomputed_not_dropped() {
        let mut state = connected();
        state.apply_ui(UiEvent::Compose);
        let mut next = settings(&["a"]);
        next.accounts[0].email = "new@a.example.invalid".into();
        state.set_settings(next);
        state.reconnect("a");
        state.take_work();
        state.apply_ui(UiEvent::ComposeUseNewFrom);
        let compose = state.compose.clone().unwrap();
        assert_eq!(compose.draft.from, "ann@a.example.invalid", "not taken yet");
        let stale = compose.stale.expect("still blocked");
        assert_eq!(stale.generation, tag(&state, "a").generation);
        assert_eq!(stale.address, "new@a.example.invalid");
        state.apply_ui(UiEvent::ComposeUseNewFrom);
        assert_eq!(
            state.compose.as_ref().unwrap().draft.from,
            "new@a.example.invalid"
        );
    }

    /// Round 5, #25: a late migration ack never moves the password to an
    /// adopted account whose server changed meanwhile; the migration
    /// replans against the current settings.
    #[test]
    fn round5_25_adoption_is_revalidated_before_the_transfer() {
        let mut state = fresh();
        let ticket = adopting(&mut state);
        let mut moved = entry("a");
        moved.url = "https://elsewhere.example.invalid".into();
        state.set_settings(EmailSettings {
            accounts: vec![moved],
            ..EmailSettings::default()
        });
        state.take_work();
        state.settled(ticket, Settled::Accepted);
        assert!(
            !state.has_password("a"),
            "never sent to the replacement server"
        );
        let (_, changes) = settings_ticket(&mut state);
        let ids: Vec<&str> = changes["email.accounts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a["id"].as_str())
            .collect();
        assert!(ids.contains(&"acct-legacy"), "replanned: {ids:?}");
    }

    /// Final review: an email.open answers "loaded" only when the reader
    /// shows it: an answer over an old connection is STALE, one a newer
    /// open replaced is SUPERSEDED, and neither changes the reader.
    #[test]
    fn final_an_open_reply_matches_what_the_reader_shows() {
        let mut state = reading();
        state.open_by_bus("a", "E2".into(), 6);
        let first = state
            .take_work()
            .iter()
            .find_map(|w| match w {
                Work::OpenEmail {
                    ticket,
                    reply: Some(6),
                    ..
                } => Some(*ticket),
                _ => None,
            })
            .unwrap();
        // A newer open replaces it.
        state.open_by_bus("a", "E1".into(), 7);
        state.take_work();
        state.account_mut("a").unwrap().mail.open = None;
        done(
            &mut state,
            "a",
            Done::Opened(first, Some(6), Box::new(Ok(unread_email("E2")))),
        );
        let reply = &state.take_replies()[0];
        assert_eq!(
            (reply.id, reply.body["error_code"].clone()),
            (6, json!("SUPERSEDED"))
        );
        assert!(state.account("a").unwrap().mail.open.is_none());
        // The account reconnects before the newer one loads.
        let old = tag(&state, "a");
        let latest = state.account("a").unwrap().tickets.open;
        state.reconnect("a");
        state.apply(Event::Account(
            old,
            Done::Opened(latest, Some(7), Box::new(Ok(unread_email("E1")))),
        ));
        let reply = &state.take_replies()[0];
        assert_eq!(
            (reply.id, reply.body["error_code"].clone()),
            (7, json!(code::STALE))
        );
    }

    /// Final review: a query answered over a connection the account has
    /// since left is STALE, not data from the old server.
    #[test]
    fn final_a_query_answer_from_an_old_connection_is_stale() {
        let mut state = connected();
        let old = tag(&state, "a");
        let mut next = settings(&["a"]);
        next.accounts[0].url = "https://moved.example.invalid".into();
        state.set_settings(next);
        state.apply(Event::Account(
            old,
            Done::Answered(5, Then::Answer, Ok(json!({"mailboxes": [{"id": "old"}]}))),
        ));
        let reply = &state.take_replies()[0];
        assert_eq!(
            (reply.id, reply.body["error_code"].clone()),
            (5, json!(code::STALE))
        );
    }

    /// Round 5, #26: removing an account whose earlier forget never
    /// reached the disk saves again, and answers only once it lands.
    #[test]
    fn round5_26_a_removal_saves_an_unsaved_forget() {
        let mut state = connected();
        state.change_accounts(Change::Remove("a".into()), Some(1), None);
        let (ticket, _) = settings_ticket(&mut state);
        state.settled(ticket, Settled::Accepted);
        let work = state.take_work();
        land(&mut state, &work, Err("disk full".into()));
        assert_eq!(state.take_replies()[0].body["error_code"], code::SECRETS);
        state.set_settings(settings(&["a"]));
        state.take_work();
        state.change_accounts(Change::Remove("a".into()), Some(2), None);
        let (ticket, _) = settings_ticket(&mut state);
        state.settled(ticket, Settled::Accepted);
        let work = state.take_work();
        assert_eq!(saves(&work), 1, "the forget is saved again");
        assert!(state.take_replies().is_empty());
        land(&mut state, &work, Ok(true));
        assert_eq!(state.take_replies()[0].rc, 0);
    }

    // Phase 3b: message actions act on the list's target, in batches.

    /// The inbox (with a Trash) showing `n` emails E1…En of `total`, E1
    /// open, nothing selected.
    fn listing(n: usize, total: u64) -> State {
        let mut state = reading();
        let view = state.account_mut("a").unwrap();
        view.mail.emails = (1..=n).map(|i| unread_email(&format!("E{i}"))).collect();
        view.mail.total = total;
        state.take_work();
        state
    }

    fn select(state: &mut State, ids: &[&str]) {
        state.mail_table(
            "a",
            TableEvent::SelectionChanged {
                keys: ids.iter().map(|s| (*s).to_owned()).collect(),
                focus: Some(0),
            },
        );
    }

    fn target(state: &State) -> Option<Target> {
        state.account("a").unwrap().mail.target()
    }

    fn rows(ids: &[&str]) -> Option<Target> {
        Some(Target::Rows(ids.iter().map(|s| (*s).to_owned()).collect()))
    }

    #[test]
    fn target_is_the_selection_else_the_keyboard_row_never_the_open_email() {
        let mut state = listing(4, 4);
        // Nothing selected: the open email is drawn as the keyboard row,
        // so it is the target.
        assert_eq!(target(&state), rows(&["E1"]));
        // Two selected: those two, not the open E1.
        select(&mut state, &["E2", "E3"]);
        assert_eq!(target(&state), rows(&["E2", "E3"]));
        // One picked (Ctrl+click) while E1 stays open: that one.
        select(&mut state, &["E3"]);
        assert_eq!(target(&state), rows(&["E3"]));
        // None selected and no email open: the keyboard row.
        let view = state.account_mut("a").unwrap();
        view.mail.table.selection.clear();
        view.mail.open = None;
        view.mail.table.focus = Some(3);
        assert_eq!(target(&state), rows(&["E4"]));
        // Nothing at all: no target, and the actions do nothing.
        state.account_mut("a").unwrap().mail.table.focus = None;
        assert_eq!(target(&state), None);
        state.flag_target();
        state.delete_target();
        assert!(batches(&mut state).is_empty() && state.form.is_none());
    }

    #[test]
    fn target_line_says_this_email_or_how_many_are_selected() {
        let mut state = listing(4, 236);
        let line = |state: &State| state.account("a").unwrap().mail.target_line();
        assert_eq!(line(&state).as_deref(), Some("Applies to this email"));
        select(&mut state, &["E1", "E2", "E3"]);
        assert_eq!(line(&state).as_deref(), Some("Applies to 3 selected"));
        select(&mut state, &["E1", "E2", "E3", "E4"]);
        state.mail_table("a", TableEvent::SelectEverything);
        assert_eq!(line(&state).as_deref(), Some("Applies to 236 selected"));
    }

    #[test]
    fn batch_of_n_selected_is_one_work_item_with_every_id() {
        let mut state = listing(4, 4);
        select(&mut state, &["E1", "E2", "E3", "E4"]);
        state.flag_target();
        let work = state.take_work();
        assert!(!work.iter().any(|w| matches!(w, Work::Write { .. })));
        let batches: Vec<&Batch> = work
            .iter()
            .filter_map(|w| match w {
                Work::Batch { batch, .. } => Some(&**batch),
                _ => None,
            })
            .collect();
        assert_eq!(batches.len(), 1, "one batch, not one write per email");
        assert_eq!(batches[0].ids, ["E1", "E2", "E3", "E4"]);
        assert_eq!(
            batches[0].change,
            jmap::EmailChange::Keywords {
                read: None,
                flagged: Some(true)
            }
        );
        // Every row shows it at once.
        let mail = &state.account("a").unwrap().mail;
        assert!(mail.emails.iter().all(Email::is_flagged));
    }

    #[test]
    fn batch_partial_failure_keeps_the_failed_ids_with_their_reason() {
        let mut state = listing(4, 4);
        select(&mut state, &["E1", "E2", "E3"]);
        state.flag_target();
        let batch = batches(&mut state).remove(0);
        let failed = vec![("E2".to_owned(), "notFound: no such email".to_owned())];
        done(
            &mut state,
            "a",
            Done::Batched(
                Box::new(batch),
                jmap::SetOutcome {
                    done: vec!["E1".into(), "E3".into()],
                    failed: failed.clone(),
                },
            ),
        );
        let mail = &state.account("a").unwrap().mail;
        let flagged: Vec<bool> = mail.emails.iter().map(Email::is_flagged).collect();
        assert_eq!(flagged, [true, false, true, false], "E2 put back");
        assert_eq!(mail.failed, failed);
        assert!(
            state.status.contains("E2 (notFound: no such email)"),
            "{}",
            state.status
        );
        assert_eq!(
            mail.table.selection,
            ["E1", "E2", "E3"],
            "a flag keeps the selection"
        );
        // A delete: the done ids leave at once, the failed one stays, and
        // the selection clears.
        select(&mut state, &["E3", "E4"]);
        state.delete_target();
        state.apply_ui(UiEvent::FormSubmit(None));
        let batch = batches(&mut state).remove(0);
        done(
            &mut state,
            "a",
            Done::Batched(
                Box::new(batch),
                jmap::SetOutcome {
                    done: vec!["E3".into()],
                    failed: vec![("E4".into(), "forbidden".into())],
                },
            ),
        );
        let mail = &state.account("a").unwrap().mail;
        assert_eq!(mail.keys(), ["E1", "E2", "E4"]);
        assert!(mail.table.selection.is_empty(), "a delete clears it");
        assert!(state.status.contains("E4 (forbidden)"));
    }

    /// The id fetches queued: (ticket, position).
    fn id_fetches(work: &[Work]) -> Vec<(u64, u64)> {
        work.iter()
            .filter_map(|w| match w {
                Work::FetchIds {
                    ticket, position, ..
                } => Some((*ticket, *position)),
                _ => None,
            })
            .collect()
    }

    fn id_page(from: usize, count: usize, total: u64) -> Result<jmap::IdPage, jmap::Error> {
        Ok(jmap::IdPage {
            ids: (from..from + count).map(|i| format!("X{i}")).collect(),
            total,
        })
    }

    /// The whole page selected, then "Select all `total`".
    fn everything(total: u64) -> State {
        let mut state = listing(4, total);
        select(&mut state, &["E1", "E2", "E3", "E4"]);
        state.mail_table("a", TableEvent::SelectEverything);
        state
    }

    #[test]
    fn select_all_fetches_ids_across_pages_then_acts() {
        let mut state = everything(1200);
        state.flag_target();
        let work = state.take_work();
        let fetches = id_fetches(&work);
        assert_eq!(fetches.len(), 1);
        let (ticket, position) = fetches[0];
        assert_eq!(position, 0);
        assert!(!work.iter().any(|w| matches!(w, Work::Batch { .. })));
        assert_eq!(
            state.account("a").unwrap().mail.target_line().as_deref(),
            Some("Fetching 1,200…")
        );
        done(&mut state, "a", Done::Ids(ticket, id_page(0, 500, 1200)));
        assert_eq!(id_fetches(&state.take_work()), [(ticket, 500)]);
        done(&mut state, "a", Done::Ids(ticket, id_page(500, 500, 1200)));
        assert_eq!(id_fetches(&state.take_work()), [(ticket, 1000)]);
        done(&mut state, "a", Done::Ids(ticket, id_page(1000, 200, 1200)));
        let batch = batches(&mut state).remove(0);
        assert_eq!(batch.ids.len(), 1200, "every id of every page");
        assert_eq!(batch.ids[0], "X0");
        assert_eq!(batch.ids[1199], "X1199");
        let mail = &state.account("a").unwrap().mail;
        assert!(
            mail.fetching.is_none() && mail.everything,
            "still all selected"
        );
    }

    #[test]
    fn select_all_above_the_cap_refuses_and_changes_nothing() {
        // Known to be over the cap: refused at once.
        let mut state = everything(SELECT_ALL_CAP + 1);
        state.flag_target();
        assert!(state.take_work().is_empty(), "no fetch, no write");
        assert!(
            state.status.contains("nothing was changed"),
            "{}",
            state.status
        );
        let mail = &state.account("a").unwrap().mail;
        assert!(!mail.emails.iter().any(Email::is_flagged));
        assert!(mail.everything && mail.fetching.is_none());
        // Grown past the cap on the server while fetching: refused too.
        let mut state = everything(4000);
        state.delete_target();
        let (ticket, _) = id_fetches(&state.take_work())[0];
        done(&mut state, "a", Done::Ids(ticket, id_page(0, 500, 5200)));
        assert!(state.take_work().is_empty());
        assert!(state.form.is_none(), "no delete asked");
        assert!(state.status.contains("5,200"), "{}", state.status);
        assert!(state.account("a").unwrap().mail.fetching.is_none());
    }

    #[test]
    fn escape_cancels_a_pending_fetch_and_changes_nothing() {
        let mut state = everything(900);
        state.toggle_target_read();
        let (ticket, _) = id_fetches(&state.take_work())[0];
        state.apply_ui(UiEvent::CancelFetch);
        let mail = &state.account("a").unwrap().mail;
        assert!(mail.fetching.is_none());
        assert!(mail.everything, "the selection is as it was");
        assert_eq!(mail.table.selection.len(), 4);
        // The cancelled fetch's answer is dropped: nothing more is asked
        // for and nothing is written.
        done(&mut state, "a", Done::Ids(ticket, id_page(0, 500, 900)));
        assert!(state.take_work().is_empty());
        let mail = &state.account("a").unwrap().mail;
        assert!(!mail.emails.iter().any(Email::is_read), "nothing marked");
        assert!(state.form.is_none());
    }

    #[test]
    fn delete_of_two_or_more_asks_first_with_the_count() {
        let mut state = listing(4, 4);
        let mut next = settings(&["a"]);
        next.mail.confirm_delete = false;
        state.set_settings(next);
        state.take_work();
        // The setting governs a single delete only.
        select(&mut state, &["E2"]);
        state.delete_target();
        assert!(state.form.is_none());
        assert_eq!(batches(&mut state)[0].ids, ["E2"]);
        select(&mut state, &["E3", "E4"]);
        state.delete_target();
        assert!(
            matches!(&state.form, Some(Form::Delete { ids, .. }) if ids == &["E3", "E4"]),
            "two always ask"
        );
        assert!(batches(&mut state).is_empty(), "nothing until confirmed");
        assert_eq!(
            crate::label_with("delete-body", &[("count", "2")]),
            "Move to the Trash? Emails: 2"
        );
        state.apply_ui(UiEvent::FormSubmit(None));
        let batch = batches(&mut state).remove(0);
        assert_eq!(batch.kind, BatchKind::Delete);
        assert_eq!(batch.ids, ["E3", "E4"]);
        assert_eq!(
            batch.change,
            jmap::EmailChange::Move {
                mailbox: "m-trash".into()
            }
        );
    }

    #[test]
    fn deleting_the_open_email_closes_it_and_selects_the_next_row() {
        let mut state = listing(4, 4);
        state.account_mut("a").unwrap().mail.open = Some(unread_email("E2"));
        select(&mut state, &["E2", "E3"]);
        state.delete_target();
        state.apply_ui(UiEvent::FormSubmit(None));
        let batch = batches(&mut state).remove(0);
        done(
            &mut state,
            "a",
            Done::Batched(
                Box::new(batch),
                jmap::SetOutcome {
                    done: vec!["E2".into(), "E3".into()],
                    failed: Vec::new(),
                },
            ),
        );
        let work = state.take_work();
        assert!(
            work.iter()
                .any(|w| matches!(w, Work::OpenEmail { id, .. } if id == "E4")),
            "the reader moves on to the next row: {work:?}"
        );
        let mail = &state.account("a").unwrap().mail;
        assert!(mail.open.is_none(), "E2 is closed");
        assert_eq!(mail.keys(), ["E1", "E4"]);
        assert_eq!(mail.table.selection, ["E4"]);
        assert_eq!(mail.table.focus, Some(1));
    }
}
