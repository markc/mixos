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

pub use crate::verbs::{Query, Then, Write};

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
    /// A mailbox's first page. `ticket` comes back with the page, so an
    /// answer to an older request cannot replace a newer one (also for
    /// `OpenEmail`, `Send`, `FetchEvents` and `FetchContacts`); tickets are
    /// per account.
    FetchEmails {
        account: String,
        ticket: u64,
        mailbox: String,
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
    /// Set or clear `$seen`.
    MarkRead {
        account: String,
        id: String,
        read: bool,
    },
    /// The send recipe (build, upload, create in Sent, submit).
    Send {
        account: String,
        ticket: u64,
        draft: Composed,
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
    /// Compose `compose`'s edit number `edits` arms the draft timer:
    /// [`crate::state::State::draft_due`] is called with both once
    /// [`crate::state::drafts::DRAFT_DELAY`] has passed.
    DraftTimer {
        compose: u64,
        edits: u64,
    },
    /// Save `draft` in `mailbox` (Drafts), replacing draft `replaces`; the
    /// answer comes back under operation `op`.
    SaveDraft {
        account: String,
        op: u64,
        draft: Composed,
        mailbox: String,
        replaces: Option<String>,
    },
    /// Read draft `id` back for compose.
    ResumeDraft {
        account: String,
        ticket: u64,
        id: String,
    },
    /// An address field's edit `ticket` arms the suggestion timer:
    /// [`crate::state::State::suggest_due`] after
    /// [`crate::state::suggest::SUGGEST_DELAY`].
    SuggestTimer {
        compose: u64,
        ticket: u64,
    },
    /// Find the contacts matching `text` for suggestion request `ticket`.
    Suggest {
        account: String,
        ticket: u64,
        text: String,
        limit: u64,
    },
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
            | Work::MarkRead { account, .. }
            | Work::Send { account, .. }
            | Work::FetchCalendars { account }
            | Work::FetchEvents { account, .. }
            | Work::FetchBooks { account }
            | Work::FetchContacts { account, .. }
            | Work::SaveDraft { account, .. }
            | Work::ResumeDraft { account, .. }
            | Work::Suggest { account, .. } => Some(account),
            Work::Settings { .. }
            | Work::WriteDeadline { .. }
            | Work::ReadSettings
            | Work::SaveSecrets { .. }
            | Work::OpenPrefs
            | Work::DraftTimer { .. }
            | Work::SuggestTimer { .. } => None,
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
