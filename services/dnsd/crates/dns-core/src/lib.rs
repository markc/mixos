// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Private authoritative DNS core: zone model, strict-data loader, snapshots,
//! persistence, wire codec, resolver and UDP/TCP serve loops.

pub mod candidate;
pub mod canonical;
pub mod model;
pub mod persistence;
pub mod resolver;
pub mod rr;
pub mod serve;
pub mod snapshot;
pub mod store;
pub mod strict_data;
pub mod validate;
pub mod wire;

pub use candidate::{
    CandidateSnapshot, RejectReason, adopt_candidate, build_candidate, validate_candidate,
};
pub use canonical::Name;
pub use model::{
    Bundle, OwnerId, ParsedZones, RecordAssertion, RrsetKey, RrsetValue, Serial, SerialCmp,
    SoaConfig, ZoneAnswerState, ZoneName, ZoneSnapshot, ZoneSource,
};
pub use persistence::{
    FilePersistence, InMemoryPersistence, Persistence, PersistenceError, StateLoad,
};
pub use resolver::resolve;
pub use rr::{RData, RecordType};
pub use serve::{ResponseObserver, serve_tcp, serve_tcp_observed, serve_udp, serve_udp_observed};
pub use snapshot::SnapshotHash;
pub use store::{StaticZoneStore, StoreInitError, ZoneStore};
pub use strict_data::{ZonesParseError, parse_zones, parse_zones_file};
pub use wire::{decode, encode_tcp, encode_udp};
