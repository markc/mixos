// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! What the state asks the shell to perform. The state queues [`Work`];
//! the shell runs each item on its effects runtime and hands the result
//! back as an [`crate::shell::Event`]. The state never touches the network
//! and the shell never decides policy.

use jmap::{Account, Composed};

/// One unit of background work.
#[derive(Clone, Debug, PartialEq)]
pub enum Work {
    /// Connect: session discovery and the token exchange.
    Connect(Account),
    FetchMailboxes,
    /// A mailbox's first page. `ticket` comes back with the page, so an
    /// answer to an older request cannot replace a newer one (also for
    /// `OpenEmail`, `Send` and `FetchContacts`).
    FetchEmails {
        ticket: u64,
        mailbox: String,
    },
    /// Fetch one email with its bodies.
    OpenEmail {
        ticket: u64,
        id: String,
    },
    /// Set or clear `$seen`.
    MarkRead {
        id: String,
        read: bool,
    },
    /// The send recipe (build, upload, create in Sent, submit).
    Send {
        ticket: u64,
        draft: Composed,
    },
    FetchCalendars,
    FetchEvents {
        calendar: String,
    },
    FetchBooks,
    FetchContacts {
        ticket: u64,
        book: String,
        filter: String,
    },
}
