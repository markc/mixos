// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! The reserved `<svc>.log` singleton and its validation hooks.
//!
//! Requests use `key: ""`; responses name the canonical `current` row.
//! The schema carries `level`, `filter`, `format`, `applied_at`,
//! `applied_by` and `source`. All six remain mutable. The watcher applies
//! level and filter only: format does not rebuild sinks, and the last
//! three fields are stored metadata without watcher writeback.
//!
//! Hooks reject unknown enum variants, malformed EnvFilter directives,
//! and deletion of the singleton. Reset through a replacement set.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::LogError;
use props::bus::mutation::PropsRouter;
use props::capability::{Capability, CapabilitySet};
use props::hooks::{HookCtx, HookError, HookFuture, HookHandler, Hooks};
use props::namespace::{
    AuthPolicy, Cardinality, FieldSchema, FieldType, NamespaceLifecycle, NamespaceName,
    NamespaceSpec, PeerIdentity, PropertySchema, StorageBackendKind,
};
use props::runtime::Runtime;
use props::sqlite::{JsonValuesMapping, SqliteStore};
use props::value::PropValue;
use tokio::sync::Notify;
use tracing_subscriber::EnvFilter;

/// Unqualified namespace name (the substrate yields `<svc>.log` once
/// the router's service prefix is composed).
pub const NAMESPACE: &str = "log";

/// Singleton canonical key — wire requests carry `key: ""`, the
/// substrate fills in `"current"` on responses.
pub const CANONICAL_KEY: &str = "current";

/// `before_set` rejects malformed `EnvFilter` directives behind this
/// prefix so call-sites can match on it without scraping the full
/// message. "props.set with a malformed filter returns
/// `Err` from the command; the live filter is unchanged."
pub const INVALID_FILTER_PREFIX: &str = "log_filter_invalid:";

/// `before_set` rejects unknown enum variants for the three enum-typed
/// fields. `FieldType::Enum` is descriptive in v0.1, not enforced at
/// commit time — the hook is the only enforcer.
pub const UNKNOWN_ENUM_VARIANT_PREFIX: &str = "log_unknown_enum_variant:";

/// `before_delete` rejects every delete on this singleton row.
pub const UNDELETABLE_PREFIX: &str = "log_undeletable:";

/// What `register` hands back. The host service holds it
/// for the lifetime of the process and passes it to [`super::attach`].
///
/// It bundles the substrate `Runtime` with a **dedicated** wake `Notify`
/// owned by this namespace. The watcher MUST NOT park on
/// `Runtime::events_signal()` — that notify is single-consumer
/// (`notify_one`) and already drained by the props dispatcher, so a
/// shared wait races the dispatcher and the live filter swap silently
/// stalls. Instead, the namespace's `after_set` hook (which fires only
/// for `<svc>.log` writes) pulses this private `Notify`, guaranteeing the
/// watcher is the one woken on every accepted log-row change.
#[derive(Clone)]
pub struct LogNamespace {
    pub(crate) runtime: Arc<Runtime>,
    pub(crate) signal: Arc<Notify>,
}

impl LogNamespace {
    /// The underlying substrate `Runtime` for the `<svc>.log` namespace.
    /// Daemons normally just pass the whole `LogNamespace` to
    /// [`super::attach`]; this accessor is for callers that need the raw
    /// runtime (e.g. tests driving writes through the substrate path).
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }
}

pub fn namespace_name() -> NamespaceName {
    NamespaceName::new(NAMESPACE).expect("constant namespace name is valid")
}

/// Frozen `level` variants. Mirrors `LogLevel` in `opts.rs` but lives
/// here as plain strings because the substrate schema speaks in
/// `FieldType::Enum { variants: Vec<String> }`.
pub fn level_variants() -> Vec<String> {
    vec![
        "none".into(),
        "error".into(),
        "warn".into(),
        "info".into(),
        "debug".into(),
    ]
}

/// Frozen `format` variants. Mirrors `LogFormat`.
pub fn format_variants() -> Vec<String> {
    vec!["human".into(), "json".into()]
}

/// Frozen `applied_by` variants.
pub fn applied_by_variants() -> Vec<String> {
    vec!["cli".into(), "env".into(), "props".into(), "default".into()]
}

/// Namespace schema. Defaults are conservative: `level=info`,
/// `filter=""`, `format=human`, audit-trail trio = `"default"` /
/// empty strings. The defaults exist so a Patch over a bootstrap row
/// that omits a field has a deterministic effective value rather
/// than failing on a half-shaped record.
pub fn schema() -> PropertySchema {
    PropertySchema::new(vec![
        FieldSchema {
            name: "level".into(),
            ty: FieldType::Enum {
                variants: level_variants(),
            },
            default: Some(PropValue::String("info".into())),
            secret: false,
            help: "Effective global default level — one of none|error|warn|info|debug".into(),
            since: None,
            until: None,
            validators: Vec::new(),
        },
        FieldSchema {
            name: "filter".into(),
            ty: FieldType::String,
            default: Some(PropValue::String(String::new())),
            secret: false,
            help: "Per-target overrides in tracing-subscriber EnvFilter syntax (empty = none)"
                .into(),
            since: None,
            until: None,
            validators: Vec::new(),
        },
        FieldSchema {
            name: "format".into(),
            ty: FieldType::Enum {
                variants: format_variants(),
            },
            default: Some(PropValue::String("human".into())),
            secret: false,
            help: "Output format — one of human|json. Changing mid-run does not re-init sinks."
                .into(),
            since: None,
            until: None,
            validators: Vec::new(),
        },
        FieldSchema {
            name: "applied_at".into(),
            ty: FieldType::String,
            default: Some(PropValue::String(String::new())),
            secret: false,
            help: "RFC 3339 UTC timestamp of the last filter swap (empty before first swap)".into(),
            since: None,
            until: None,
            validators: Vec::new(),
        },
        FieldSchema {
            name: "applied_by".into(),
            ty: FieldType::Enum {
                variants: applied_by_variants(),
            },
            default: Some(PropValue::String("default".into())),
            secret: false,
            help: "Origin of the currently applied filter — one of cli|env|props|default".into(),
            since: None,
            until: None,
            validators: Vec::new(),
        },
        FieldSchema {
            name: "source".into(),
            ty: FieldType::String,
            default: Some(PropValue::String(String::new())),
            secret: false,
            help: "Literal directive tracing-subscriber accepted (reflects what is running)".into(),
            since: None,
            until: None,
            validators: Vec::new(),
        },
    ])
}

/// Capabilities scoped to `<svc>.log`. The current policy grants every
/// reachable peer read, describe, write and audit access. A write can
/// silence logging through `level=none`; services that require narrower
/// access must supply a namespace authorisation policy before exposing it.
pub fn auth_policy(service: &str) -> AuthPolicy {
    let read = Capability::new(format!("props.read:{service}.log")).expect("non-empty capability");
    let describe_public = Capability::new(format!("props.describe:{service}.log:public"))
        .expect("non-empty capability");
    let describe_full = Capability::new(format!("props.describe:{service}.log:full"))
        .expect("non-empty capability");
    let write =
        Capability::new(format!("props.write:{service}.log")).expect("non-empty capability");
    let audit =
        Capability::new(format!("props.audit:{service}.log")).expect("non-empty capability");
    let caps: CapabilitySet = [read, describe_public, describe_full, write, audit]
        .into_iter()
        .collect();
    AuthPolicy::new(move |_peer: &PeerIdentity| caps.clone())
}

pub fn spec(service: &str, hooks: Hooks) -> NamespaceSpec {
    let mut s = NamespaceSpec::new(
        namespace_name(),
        schema(),
        Cardinality::Singleton {
            canonical_key: CANONICAL_KEY.into(),
        },
        StorageBackendKind::SqliteTable {
            table: "__props_values".into(),
        },
    );
    s.lifecycle = NamespaceLifecycle::Simple;
    s.auth = auth_policy(service);
    s.hooks = hooks;
    s
}

/// Hook handler — validates enum variants and EnvFilter syntax on
/// every set, rejects every delete, and (on a committed set) pulses the
/// namespace-private wake `Notify` so the live-filter watcher re-reads
/// and swaps deterministically.
pub struct LogNamespaceHooks {
    signal: Arc<Notify>,
}

impl LogNamespaceHooks {
    pub fn new(signal: Arc<Notify>) -> Self {
        Self { signal }
    }
}

impl HookHandler for LogNamespaceHooks {
    fn before_set<'a>(&'a self, ctx: &'a HookCtx) -> HookFuture<'a, ()> {
        Box::pin(async move {
            let Some(new_value) = ctx.new.as_ref() else {
                return Ok(());
            };
            let obj = match new_value.as_object() {
                Some(o) => o,
                None => {
                    return Err(HookError::validation(format!(
                        "log namespace body must be an object (got {})",
                        new_value.type_name()
                    )));
                }
            };
            validate_enum(obj, "level", &level_variants())?;
            validate_enum(obj, "format", &format_variants())?;
            validate_enum(obj, "applied_by", &applied_by_variants())?;
            validate_filter(obj)?;
            Ok(())
        })
    }

    fn after_set<'a>(&'a self, _ctx: &'a HookCtx) -> HookFuture<'a, ()> {
        // The row is now durably committed. Pulse the namespace-private
        // wake so the watcher (parked on this same `Notify`) re-reads the
        // committed row and swaps the live filter. `notify_one` coalesces
        // — if the watcher is mid-read, the stored permit makes it loop
        // once more and converge on the latest row. Unlike the shared
        // `Runtime::events_signal()`, nothing else drains this notify, so
        // the watcher is always the woken party.
        self.signal.notify_one();
        Box::pin(async { Ok(()) })
    }

    fn before_delete<'a>(&'a self, _ctx: &'a HookCtx) -> HookFuture<'a, ()> {
        Box::pin(async move {
            Err(HookError::validation(format!(
                "{UNDELETABLE_PREFIX} log namespace is a singleton; reset via props.set"
            )))
        })
    }
}

fn validate_enum(
    obj: &BTreeMap<String, PropValue>,
    field: &str,
    variants: &[String],
) -> Result<(), HookError> {
    match obj.get(field) {
        None => Ok(()),
        Some(PropValue::String(s)) => {
            if variants.iter().any(|v| v == s) {
                Ok(())
            } else {
                Err(HookError::validation(format!(
                    "{UNKNOWN_ENUM_VARIANT_PREFIX} {field}={s:?} not in {variants:?}"
                )))
            }
        }
        Some(other) => Err(HookError::validation(format!(
            "{field} must be a string enum variant (got {})",
            other.type_name()
        ))),
    }
}

fn validate_filter(obj: &BTreeMap<String, PropValue>) -> Result<(), HookError> {
    match obj.get("filter") {
        None => Ok(()),
        Some(PropValue::String(s)) if s.is_empty() => Ok(()),
        Some(PropValue::String(s)) => EnvFilter::try_new(s)
            .map(|_| ())
            .map_err(|e| HookError::validation(format!("{INVALID_FILTER_PREFIX} {s:?}: {e}"))),
        Some(other) => Err(HookError::validation(format!(
            "filter must be a string (got {})",
            other.type_name()
        ))),
    }
}

/// Wire the reserved `log` namespace into the host service's
/// substrate. Property-backed.
///
/// The router's service name (set at `PropsRouter::new("<svc>")`)
/// determines the fully-qualified namespace `<svc>.log` and the five
/// capability names. The returned `LogNamespace` (= `Arc<Runtime>`)
/// is what `logging::props::attach` watches — the host
/// service must hold it for the lifetime of the process.
///
/// Store registration is idempotent for the same delete mode, but
/// registering a second runtime named `log` on the router is a conflict.
pub fn register(
    router: &mut PropsRouter,
    store: &Arc<SqliteStore>,
) -> Result<LogNamespace, LogError> {
    let service = router.service().to_string();
    // Namespace-private wake shared between the `after_set` hook (pulses)
    // and the watcher (parks). See `LogNamespace`'s doc for why this
    // must NOT be `Runtime::events_signal()`.
    let signal = Arc::new(Notify::new());
    let hooks = Hooks::new(LogNamespaceHooks::new(signal.clone()));
    let spec = spec(&service, hooks);
    store
        .register_namespace(&spec, Arc::new(JsonValuesMapping::new(namespace_name())))
        .map_err(|e| LogError::Props(format!("register log namespace in store: {e}")))?;
    let runtime = Arc::new(Runtime::new(router.service(), spec, store.clone()));
    router
        .register(runtime.clone())
        .map_err(|e| LogError::Props(format!("register log runtime on router: {e}")))?;
    Ok(LogNamespace { runtime, signal })
}

#[cfg(test)]
mod tests {
    use super::*;
    use props::record::{Actor, RecordKey, Version};
    use props::runtime::SetOpts;
    use props::store::MergeMode;
    use rusqlite::Connection;
    use std::time::Duration;

    /// Regression guard for the live-swap BLOCKER: a committed
    /// `<svc>.log` write must pulse the namespace-PRIVATE `Notify` that
    /// the watcher parks on — NOT `Runtime::events_signal()`, which the
    /// props dispatcher already drains single-consumer. If `after_set`
    /// ever stops pulsing `LogNamespace::signal`, the agent-operable
    /// live level swap silently stalls and this test fails on the
    /// timeout.
    #[tokio::test(flavor = "multi_thread")]
    async fn after_set_pulses_the_private_watcher_signal() {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        let store = Arc::new(SqliteStore::new("demod", conn).expect("init store schema"));
        let mut router = PropsRouter::new("demod");
        let lp = register(&mut router, &store).expect("register log namespace");

        let value = {
            let mut m = BTreeMap::new();
            m.insert("level".to_string(), PropValue::String("debug".to_string()));
            PropValue::Object(m)
        };
        lp.runtime()
            .set(
                RecordKey::singleton(namespace_name()),
                value,
                SetOpts {
                    expected_version: Some(Version::zero()),
                    merge: MergeMode::Replace,
                    actor: Actor::operator("test").expect("valid actor"),
                    cause: None,
                    ts_ms: 0,
                },
            )
            .await
            .expect("valid log row commits");

        // `notify_one` stored a permit during `after_set`, so this is
        // immediately ready. The timeout converts "watcher would hang"
        // into a hard test failure.
        tokio::time::timeout(Duration::from_secs(2), lp.signal.notified())
            .await
            .expect("after_set must have pulsed the private watcher signal");
    }
}
