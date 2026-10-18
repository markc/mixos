// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The JMAP wire protocol, headless. `Client::connect` discovers the
//! server's Session, exchanges the password for a Bearer token once, and
//! every method call after that is one HTTPS POST with as many methodCalls
//! as the task needs. [`mail`], [`compose`], [`calendar`] and [`contact`]
//! put the typed surface on top of the same envelope.
//!
//! The protocol core is ported from the retired Dioxus mail client
//! (`markc/cmctl` a9778d3) with its faults fixed: session discovery and
//! URL templates instead of hardcoded paths, token auth instead of Basic on
//! every call, real pagination, Bcc carried through, single-wrapped
//! `In-Reply-To`, CRLF stripped from header inputs, attachments downloadable,
//! and method-level error triples surfaced as [`Error::Method`] instead of a
//! missing-field serde failure.
//!
//! Server quirks a strict RFC server will not have are kept in one place —
//! [`client`] notes each one where it matters — so a later profile can swap
//! them without touching the typed surface.

pub mod auth;
pub mod calendar;
pub mod client;
pub mod compose;
pub mod contact;
pub mod error;
pub mod mail;
pub mod session;

pub use auth::{Account, Token};
pub use calendar::{Calendar, CalendarEvent, EventDraft, EventPatch, duration_between};
pub use client::{Call, Client, backref};
pub use compose::{
    Composed, SendAttempt, SendBoxes, SendReport, build_mime, format_address, parse_addresses,
};
pub use contact::{AddressBook, Contact, ContactDraft, ContactPatch};
pub use error::{Error, Result};
pub use mail::{
    Attachment, BodyPart, Email, EmailAddress, EmailChange, EmailPage, EmailSort, IdPage,
    MAX_IDS_PER_REQUEST, Mailbox, SetOutcome, SortBy, email_ids_call, email_set_calls, set_outcome,
};
pub use session::Session;
