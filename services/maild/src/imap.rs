// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! IMAP4rev2 and IMAP4rev1 over implicit TLS. Session negotiation controls
//! UTF-8 mailbox encoding and response forms. Command framing supports bounded
//! literals and literal8; message operations use native MailStore transactions.

pub mod capabilities;
pub mod codec;
pub mod config;
pub mod mailbox_name;
pub mod mailbox_encoding;
pub mod op;
pub mod response;
pub mod sasl;
pub mod seq;
pub mod server;
pub mod session;
pub mod state;
pub mod text;

pub use config::ImapConfig;
pub use server::{ImapHandle, start};
