// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! comp-model: the `comp.*` Bus wire as data, with no transport.
//!
//! The `comp.*` wire is frozen, including `{id, generation}` numeric window
//! addressing. This crate is that wire:
//!
//! - [`catalogue`]: the 34 commands and 11 topics;
//! - [`request`]: command routing ([`request::classify`]) and the argument
//!   parsers, which refuse unknown fields by name (`invalid_args`) and parse
//!   `{id, generation}`;
//! - [`reply`]: the `(rc, body)` reply shape and every refusal body;
//! - [`snapshot`]: the `CompSnapshot` props tree, its describe schema and the
//!   read commands (`comp.info`, `comp.windows.list`, `comp.props.*`);
//! - [`observation`]: topic payloads (every one carries `event_seq`), the
//!   gap message, the props value type and the `comp.props.set` gate;
//! - [`prop_path`]: the dotted property path;
//! - [`diff`]: the `props.changed` reducers between two snapshots.
//!
//! The ABP/Bus transport, admission, ordering fences and the snapshot
//! projection from the engine's state live in comp-service and policy-host;
//! they consume this model. Dependencies: serde, serde_json and the two
//! sibling cores (surfaces for ids, seats and target refusals,
//! ledger for the presentation leaves).

pub mod capture;
pub mod catalogue;
pub mod diff;
pub mod motion_run;
pub mod observation;
pub mod prop_path;
pub mod reply;
pub mod request;
pub mod snapshot;
pub mod tiling;

pub use prop_path::PropPath;
pub use reply::ControlReply;
pub use surfaces::{SeatKind, WindowTargetError};
