// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Per-command handlers.
//!
//! Phase 1 shipped CAPABILITY, NOOP, LOGOUT, ID, LOGIN, AUTHENTICATE.
//! Phase 2 added LIST, LSUB, NAMESPACE. Phase 2 added SELECT,
//! EXAMINE, STATUS, CLOSE, UNSELECT. Phase 2 adds CREATE, DELETE,
//! RENAME, SUBSCRIBE, UNSUBSCRIBE. Commands outside this module are
//! rejected by `session::dispatch` (`BAD command not valid` or
//! `NO command not allowed in current state`).

pub mod append;
pub mod authenticate;
pub mod capability;
pub mod check;
pub mod close;
pub mod copy;
pub mod create;
pub mod delete;
pub mod expunge;
pub mod fetch;
pub mod flags;
pub mod id;
pub mod idle;
pub mod list;
pub mod login;
pub mod logout;
pub mod lsub;
pub mod mv;
pub mod namespace;
pub mod noop;
pub mod rename;
pub mod search;
pub mod select;
pub mod status;
pub mod store;
pub mod subscribe;
