// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! What the state asks the shell to perform. The state queues [`Work`];
//! the shell runs each item on its effects runtime and hands the result
//! back as an [`crate::shell::Event`]. The state never touches the network
//! or the disk and the shell never decides policy.
//!
//! Everything an account's server does is addressed by the settings account
//! id, and the result comes back with it: a late answer from one account
//! can never land in another's view.

use jmap::Composed;
use serde_json::Value;

pub use crate::commands::{Query, Then, Write};

/// A filesystem inspection fenced to one version of one compose. The candidate
/// draft is applied atomically only after the worker has checked the batch.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposeInspection {
    pub ticket: u64,
    pub draft_id: String,
    pub revision: u64,
    pub reply: Option<u64>,
    pub opening: bool,
    pub paths: Vec<std::path::PathBuf>,
    pub draft: Composed,
    pub limit: Option<u64>,
}

pub type InspectedAttachments =
    Result<Vec<crate::state::ComposeAttachment>, (&'static str, String)>;

impl ComposeInspection {
    /// Runs on the effects runtime's blocking pool, never on the window thread.
    pub fn inspect(&self) -> InspectedAttachments {
        use crate::state::{ComposeAttachment, ComposeAttachmentState, code};

        if self.opening {
            jmap::validate_attachments(&self.draft, self.limit)
                .map_err(|error| crate::commands::server_refusal(&error))?;
        }
        let attachments = ComposeAttachment::inspect_all(&self.draft.attachments);
        if !self.opening {
            for path in &self.paths {
                let attachment = attachments
                    .iter()
                    .find(|a| a.path == *path)
                    .expect("candidate contains every requested path");
                let refusal = match attachment.state {
                    ComposeAttachmentState::Ready => continue,
                    ComposeAttachmentState::Relative => code::ATTACHMENT_RELATIVE,
                    ComposeAttachmentState::Missing | ComposeAttachmentState::NotFile => {
                        code::ATTACHMENT_NOT_FILE
                    }
                    ComposeAttachmentState::Unreadable => code::ATTACHMENT_UNREADABLE,
                };
                return Err((
                    refusal,
                    crate::label_with(
                        "compose-attachment-add-failed",
                        &[
                            ("path", &attachment.path.display().to_string()),
                            ("state", &crate::label(attachment.state.label())),
                        ],
                    ),
                ));
            }
        }
        Ok(attachments)
    }
}

/// How long a settings write may stay unsettled before its caller hears
/// it timed out.
pub const WRITE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// How to reach one account's server. The password is never printed.
#[derive(Clone, PartialEq)]
pub struct Login {
    pub url: String,
    pub email: String,
    pub password: String,
    /// A PEM bundle to trust as well as the system roots (the shell reads
    /// it when it connects).
    pub ca_path: Option<String>,
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("url", &self.url)
            .field("email", &self.email)
            .field("password", &"<withheld>")
            .field("ca_path", &self.ca_path)
            .finish()
    }
}

/// One unit of background work.
#[derive(Clone, Debug, PartialEq)]
pub enum Work {
    MailDocument(crate::documents::DocumentWork),
    /// One slot covering both the capped blob download and blocking decode.
    Preview {
        account: String,
        ticket: u64,
        blob: String,
        kind: image::ImageFormat,
    },
    InspectComposeAttachments(ComposeInspection),
    /// Connect `account`: session discovery and the token exchange. Only the
    /// answer of its latest `generation` is applied.
    Connect {
        account: String,
        generation: u64,
        login: Login,
    },
    /// Drop `account`'s client (removed, disabled, or about to reconnect).
    Disconnect {
        account: String,
    },
    FetchMailboxes {
        account: String,
    },
    /// A page of a mailbox: `limit` emails in `sort` order, starting
    /// `position` emails in. `ticket` comes back with the page, so an
    /// answer to an older request cannot replace a newer one (also for
    /// `OpenEmail`, `Send`, `FetchEvents` and `FetchContacts`); tickets are
    /// per account.
    FetchEmails {
        account: String,
        ticket: u64,
        mailbox: String,
        sort: jmap::EmailSort,
        position: u64,
        limit: u32,
    },
    /// Fetch one email with its bodies; `reply` is the Bus request that
    /// asked for it (`email.open`), answered when it loads.
    OpenEmail {
        account: String,
        ticket: u64,
        id: String,
        reply: Option<u64>,
    },
    /// A Bus query on the account's server, answered under `reply`.
    Query {
        account: String,
        reply: u64,
        query: Query,
        then: Then,
    },
    /// A change on the account's server, from the window (`reply` none) or
    /// the Bus.
    Write {
        account: String,
        reply: Option<u64>,
        /// The form operation it settles, if a form sent it.
        form: Option<u64>,
        /// The connection generation it must go through (a form's); on any
        /// other it is refused as stale.
        expect: Option<u64>,
        write: Write,
    },
    /// A message action on the list's target: one change to every id,
    /// sent as batched Email/set requests (at most
    /// [`jmap::MAX_IDS_PER_REQUEST`] ids each) through connection
    /// `generation` only. The answer reports each id done or failed.
    Batch {
        account: String,
        generation: u64,
        batch: Box<Batch>,
    },
    /// One page (at most 500) of the ids of `mailbox`'s query in `sort`
    /// order, from `position`, for "Select all N" fetch `ticket`.
    FetchIds {
        account: String,
        ticket: u64,
        mailbox: String,
        sort: jmap::EmailSort,
        position: u64,
    },
    /// Set or clear `$seen`.
    MarkRead {
        account: String,
        id: String,
        read: bool,
    },
    /// The send recipe (build, upload, create in Drafts, submit, file in
    /// Sent), resumed from what earlier tries of this message did.
    Send {
        account: String,
        ticket: u64,
        draft: Composed,
        attempt: jmap::SendAttempt,
        origin: crate::state::SendOrigin,
    },
    FetchCalendars {
        account: String,
    },
    FetchEvents {
        account: String,
        ticket: u64,
        calendar: String,
    },
    FetchBooks {
        account: String,
    },
    FetchContacts {
        account: String,
        ticket: u64,
        book: String,
        filter: String,
    },
    /// A fenced `settings.apply` of `changes` (`{path: value}`) on the
    /// session profile; its answer comes back under `ticket`.
    Settings {
        ticket: u64,
        changes: Value,
    },
    /// Read the session profile again (after an apply, or a conflict).
    ReadSettings,
    /// Settings write `op`'s deadline: [`crate::state::State::write_deadline`]
    /// is called with it once [`WRITE_DEADLINE`] has passed.
    WriteDeadline {
        op: u64,
    },
    /// Write the secrets file; its outcome comes back under `ticket`.
    SaveSecrets {
        ticket: u64,
    },
    /// Launch Prefs on its Email section.
    OpenPrefs,
    /// Download attachment `part` (blob `blob`) of `account`'s `email` and
    /// save it as `name`: in `dir`, else the downloads directory, or
    /// (`open`) in a private directory, then open it with the desktop's
    /// default application. `reply`: the Bus request to answer.
    /// Wait out the completion debounce for compose suggestion `ticket`;
    /// the shell answers with [`crate::shell::Event::SuggestDue`].
    SuggestWait {
        ticket: u64,
    },
    /// Fetch `account`'s contacts matching `text` for suggestion `ticket`.
    Suggest {
        account: String,
        ticket: u64,
        text: String,
    },
    Attachment {
        account: String,
        email: String,
        part: String,
        blob: String,
        name: String,
        open: bool,
        dir: Option<String>,
        reply: Option<u64>,
    },
    /// Write print page `html` as `name` in the private print directory and
    /// open it in the browser (`reply`: the Bus request to answer).
    Print {
        reply: Option<u64>,
        name: String,
        html: String,
    },
}

/// Which message action a batch is: it decides what the list does after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchKind {
    Flag,
    Read,
    Delete,
    Move,
    Junk,
}

impl BatchKind {
    /// The emails leave the shown mailbox.
    pub fn removes(self) -> bool {
        matches!(self, BatchKind::Delete | BatchKind::Move | BatchKind::Junk)
    }
}

/// One batched message action: what, to which ids, and (for a keyword
/// change) each id's keyword before it, so a failed id is put back.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub kind: BatchKind,
    pub change: jmap::EmailChange,
    pub ids: Vec<String>,
    /// For a keyword change: the shown ids' keyword value before it.
    pub before: Vec<(String, bool)>,
}

impl Work {
    /// The account this work is for, when it is an account's.
    pub fn account(&self) -> Option<&str> {
        match self {
            Work::Connect { account, .. }
            | Work::Disconnect { account }
            | Work::FetchMailboxes { account }
            | Work::FetchEmails { account, .. }
            | Work::OpenEmail { account, .. }
            | Work::Query { account, .. }
            | Work::Write { account, .. }
            | Work::Batch { account, .. }
            | Work::FetchIds { account, .. }
            | Work::MarkRead { account, .. }
            | Work::Send { account, .. }
            | Work::FetchCalendars { account }
            | Work::FetchEvents { account, .. }
            | Work::FetchBooks { account }
            | Work::FetchContacts { account, .. }
            | Work::Attachment { account, .. }
            | Work::Preview { account, .. }
            | Work::Suggest { account, .. } => Some(account),
            Work::Settings { .. }
            | Work::MailDocument(_)
            | Work::InspectComposeAttachments(_)
            | Work::WriteDeadline { .. }
            | Work::ReadSettings
            | Work::SaveSecrets { .. }
            | Work::Print { .. }
            | Work::SuggestWait { .. }
            | Work::OpenPrefs => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_never_prints_its_password() {
        let login = Login {
            url: "https://example.invalid".into(),
            email: "ann@example.invalid".into(),
            password: "hunter2".into(),
            ca_path: None,
        };
        let shown = format!(
            "{:?}",
            Work::Connect {
                account: "a".into(),
                generation: 1,
                login,
            }
        );
        assert!(
            !shown.contains("hunter2") && shown.contains("withheld"),
            "{shown}"
        );
    }
}
