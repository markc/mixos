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
    FetchEmails { mailbox: String },
    /// Fetch one email with its bodies.
    OpenEmail { id: String },
    /// Set or clear `$seen`.
    MarkRead { id: String, read: bool },
    /// The send recipe (build, upload, create in Sent, submit).
    Send(Composed),
    FetchCalendars,
    FetchEvents { calendar: String },
    FetchBooks,
    FetchContacts { book: String, filter: String },
}
