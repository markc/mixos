// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Property read surface, revisions and optional namespace storage.
//!
//! This crate carries `PropPath`, `PropValue`, `PropTree`,
//! `PropDescribe`, `redact`, `diff` — the read-side types every
//! `<svc>.props.*` consumer needs. The `store` feature supports namespace
//! schemas, records, lifecycle, capabilities, hooks, audit HMAC and the
//! in-memory runtime. `sqlite` enables persistent storage; `router` adds
//! Bus mutation routing and event dispatch.
//!
//! The default feature set ships only the pure-type surface. Enable
//! the opt-in `bus` feature to pull `bus` + `chrono` and
//! gain the `bus::dispatch_props` read wire and `publish::*` wire builders.
//!
//! `revwrite` adds an **opt-in** lightweight in-memory revisioned write
//! store (Q8) — a global monotonic revision, per-path canonical
//! `{value,revision,source_id,op_id}`, `if_revision` optimistic
//! concurrency, per-path coalescing, and a guaranteed terminal own-op
//! echo — for daemons that accept control writes and publish
//! `props.changed`. It is generic (no domain knowledge) and independent
//! of the `bus` feature; read-only `PropTree` consumers are unaffected.

pub mod describe;
pub mod diff;
pub mod path;
pub mod redact;
pub mod revwrite;
pub mod tree;
pub mod value;

#[cfg(feature = "store")]
pub mod audit;
#[cfg(feature = "store")]
pub mod capability;
#[cfg(feature = "store")]
pub mod hooks;
#[cfg(feature = "store")]
pub mod lifecycle;
#[cfg(feature = "store")]
pub mod memory;
#[cfg(feature = "store")]
pub mod namespace;
#[cfg(feature = "store")]
pub mod record;
#[cfg(feature = "store")]
pub mod runtime;
#[cfg(feature = "store")]
pub mod store;
#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "bus")]
pub mod bus;
#[cfg(feature = "bus")]
pub mod publish;

pub use describe::{PropDescribe, PropType};
pub use diff::diff;
pub use path::{PropPath, PropPathError};
pub use redact::redact;
pub use revwrite::{
    ChangedProp, RevWriteAck, RevWriteReject, RevWriteRequest, RevWriteResponse, RevWriteStore,
    accept_if_newer,
};
pub use tree::PropTree;
pub use value::PropValue;

#[cfg(feature = "store")]
pub use audit::{AuditKey, canonical_serialise, compute_digest, tombstone_value};
#[cfg(feature = "store")]
pub use capability::{AuthDecision, Capability, CapabilityError, CapabilitySet};
#[cfg(feature = "store")]
pub use hooks::{
    HookCtx, HookError, HookFuture, HookHandler, HookResult, Hooks, NoopHooks, WriteOrigin,
};
#[cfg(feature = "store")]
pub use lifecycle::{LIFECYCLE_FIELD, Lifecycle};
#[cfg(feature = "store")]
pub use memory::MemoryStore;
#[cfg(feature = "store")]
pub use namespace::{
    AuthPolicy, Cardinality, ConformanceLevel, DeleteMode, ExternalEditPolicy, FieldSchema,
    FieldType, NamespaceLifecycle, NamespaceName, NamespaceNameError, NamespaceSpec, PeerIdentity,
    PropertySchema, ReplayWindow, SchemaPublic, SchemaVersion, StorageBackendKind,
    SubscribePayload, Validator, ValidatorFn,
};
#[cfg(feature = "store")]
pub use record::{
    Actor, ActorError, AuditEpoch, AuditRow, EventKind, FieldPathSegment, Nseq, Record,
    RecordEvent, RecordKey, Version,
};
#[cfg(feature = "store")]
pub use runtime::{DeleteOpts, Runtime, RuntimeError, SetOpts, SetOutcome};
#[cfg(feature = "store")]
pub use store::{
    CompleteCommit, DeleteCommit, MergeMode, PropertyStore, ReconcileCommit, SetCommit, Snapshot,
    StoreError, StoreFuture, StoreResult,
};

#[cfg(feature = "sqlite")]
pub use sqlite::{JsonValuesMapping, SqliteStore, SqliteTableMapping};

#[cfg(feature = "router")]
pub mod subscribe_granter;
#[cfg(feature = "router")]
pub mod dispatcher;
#[cfg(feature = "router")]
pub use bus::mutation::PropsRouter;
#[cfg(feature = "router")]
pub use subscribe_granter::SubscribeGranter;

#[cfg(feature = "router")]
pub use dispatcher::{DispatchPublisher, DispatcherHandle, audit_topic, records_changed_topic, spawn_dispatcher};
