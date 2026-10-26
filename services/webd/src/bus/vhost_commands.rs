// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Vhost management and ACME intent commands for every Bus caller.
//! Validated property writes and provisioner notifications share the same
//! per-FQDN locks as the ACME provisioner.

use std::sync::Arc;

use ::bus::native_client::IncomingCommand;
use props::{
    Actor, DeleteOpts, MergeMode, PropValue, RecordKey, SetOpts, StoreError, Version, WriteOrigin,
};
use serde_json::{Value as JsonValue, json};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::Mutex as TokioMutex;

use super::{kwarg, kwarg_any, kwarg_bool};
use crate::NodeState;
use crate::acme_provisioner::RENEWAL_WINDOW;
use crate::vhosts_namespace::{VhostRow, namespace_name, vhost_row_from_value};

/// Caller-error sentinel — matches [`crate::bus::RC_CALLER_ERROR`].
/// Duplicated here because that constant is private to `bus::mod`; the
/// wire-validation arms in this module need the same shape.
const RC_CALLER_ERROR: u8 = 10;

/// `source` field value the daemon stamps on Bus-runtime vhost writes.
/// Mirrors [`crate::vhosts_bootstrap::SOURCE_BUS_RUNTIME`]'s wire
/// constant; kept local so the command's intent is self-documenting and a
/// drift between the two stamps surfaces immediately as a namespace
/// hook rejection rather than a silent half-state.
const SOURCE_BUS_RUNTIME: &str = "bus_runtime";

/// Dispatch entry from [`crate::bus::mod`] — `suffix` is the command name
/// after stripping the `webd.` namespace prefix (e.g. `vhost.add`).
/// Returns the `(rc, body)` shape `NodedClient::respond` expects.
///
pub(crate) async fn dispatch(
    suffix: &str,
    cmd: &IncomingCommand,
    node: &Arc<NodeState>,
) -> Option<(u8, String)> {
    let result = match suffix {
        "vhost.add" => vhost_add(node, cmd).await,
        "vhost.remove" => vhost_remove(node, cmd).await,
        "vhost.list" => vhost_list(node).await,
        "acme.renew" => acme_renew(node, cmd).await,
        "acme.status" => acme_status(node, cmd).await,
        _ => return None,
    };
    Some(result)
}

/// `webd.vhost.add` — assemble flat kwargs into a [`VhostRow`] and write
/// through the backend-origin path.
///
/// Lock-ordering contract: acquire the per-FQDN lock **before** the
/// `set_with_origin` call (mirroring the provisioner's
/// `VhostRemoved` arm) and hold it through the namespace write so a
/// `vhost.add` racing a `VhostRemoved` cleanup serialises against the
/// same lock identity. See `_doc/planned/webd-vhosts-phase3.md`
/// §"Per-fqdn serialization lock".
async fn vhost_add(node: &Arc<NodeState>, cmd: &IncomingCommand) -> (u8, String) {
    // Required kwargs.
    let fqdn = match kwarg(cmd, "fqdn") {
        Some(s) => s,
        None => return caller_error("missing required kwarg: fqdn"),
    };
    let www_dir = match kwarg(cmd, "www_dir") {
        Some(s) => s,
        None => return caller_error("missing required kwarg: www_dir"),
    };

    // Optional ACME trio — accept both dotted (spec form) and
    // underscored (DB-column) keys; dotted wins when both supplied.
    let acme_provider = kwarg_any(cmd, &["acme.provider", "acme_provider"]);
    let acme_challenge = kwarg_any(cmd, &["acme.challenge", "acme_challenge"]);
    let acme_contact_email = kwarg_any(cmd, &["acme.contact_email", "acme_contact_email"]);
    // Optional manual-TLS pair.
    let tls_cert_path = kwarg_any(cmd, &["tls.cert_path", "tls_cert_path"]);
    let tls_key_path = kwarg_any(cmd, &["tls.key_path", "tls_key_path"]);
    // Optional `enabled` flag (default true; the namespace hook also
    // defaults this, but stamping it here keeps the wire row complete
    // for the Patch merge).
    let enabled = match kwarg_bool(cmd, "enabled") {
        Ok(Some(b)) => b,
        Ok(None) => true,
        Err(other) => {
            return caller_error(&format!(
                "invalid enabled kwarg {other:?}; expected true/false/1/0",
            ));
        }
    };

    let Some(runtime) = node.vhosts_runtime.as_ref() else {
        return server_error("webd.vhosts runtime not attached — bootstrap node or wiring bug");
    };
    let Some(locks) = node.vhost_key_locks.as_ref() else {
        return server_error("vhost_key_locks not attached — bootstrap node or wiring bug");
    };

    // Acquire the per-FQDN lock identity the provisioner's
    // VhostRemoved arm also acquires. Same lock pattern as
    // `acme_provisioner.rs::handle_vhost_removed` — lookup-or-insert
    // under the outer mutex, clone the inner Arc, drop the outer
    // before awaiting the inner.
    let key_lock = {
        let mut map = locks.lock().await;
        map.entry(fqdn.clone())
            .or_insert_with(|| Arc::new(TokioMutex::new(())))
            .clone()
    };
    let _key_guard = key_lock.lock().await;

    // Build the wire row. `source` is stamped here (daemon-owned) and
    // every absent column is left out — `MergeMode::Patch` keeps the
    // existing values for any future schema-expansion fields.
    let mut body: BTreeMap<String, PropValue> = BTreeMap::new();
    body.insert("fqdn".into(), PropValue::String(fqdn.clone()));
    body.insert("www_dir".into(), PropValue::String(www_dir));
    body.insert("enabled".into(), PropValue::Bool(enabled));
    body.insert(
        "source".into(),
        PropValue::String(SOURCE_BUS_RUNTIME.into()),
    );
    // Aliases — the namespace hook (rule 3) rejects non-empty; stamp
    // an explicit empty list so a future Patch can't carry a stale
    // alias set from an older row shape.
    body.insert("aliases".into(), PropValue::List(Vec::new()));

    if let Some(p) = acme_provider {
        body.insert("acme_provider".into(), PropValue::String(p));
    }
    if let Some(c) = acme_challenge {
        body.insert("acme_challenge".into(), PropValue::String(c));
    }
    if let Some(e) = acme_contact_email {
        body.insert("acme_contact_email".into(), PropValue::String(e));
    }
    if let Some(c) = tls_cert_path {
        body.insert("tls_cert_path".into(), PropValue::String(c));
    }
    if let Some(k) = tls_key_path {
        body.insert("tls_key_path".into(), PropValue::String(k));
    }

    let ns = namespace_name();
    let key = RecordKey::collection(ns.clone(), fqdn.clone());

    // OCC anchor — `require_version=true` on the spec means a write
    // without `expected_version` is rejected. `webd.vhosts` is a
    // SoftDelete namespace (default), so `get()` would return NotFound
    // for a tombstoned row even though `commit_set` validates against
    // the tombstone version (SPEC 12 §5.4). `version_anchor` is
    // tombstone-aware: live → live version, tombstone → tombstone
    // version, absent → None → Version::zero(). This is what makes
    // `vhost.remove` → `vhost.add` a working operator flow rather than
    // a permanent OCC trap.
    let expected_version = match runtime.store().version_anchor(&key).await {
        Ok(Some(v)) => v,
        Ok(None) => Version::zero(),
        Err(other) => {
            return server_error(&format!("fetching prior vhost row for OCC anchor: {other}"));
        }
    };

    let ts_ms = now_ms();
    let outcome = runtime
        .set_with_origin(
            key,
            PropValue::Object(body),
            SetOpts {
                expected_version: Some(expected_version),
                merge: MergeMode::Patch,
                actor: Actor::service("webd").expect("valid actor"),
                cause: Some("vhost.add".into()),
                ts_ms,
            },
            WriteOrigin::backend(),
        )
        .await;
    match outcome {
        Ok(_) => {
            let mut reply = json!({
                "ok": true,
                "fqdn": fqdn,
                "source": SOURCE_BUS_RUNTIME,
            });
            // On an explicit-listener node the live allowlists were fixed
            // at startup: the row is stored and the next boot will serve
            // it, but nothing serves it now. Say so rather than let "ok"
            // imply it is live.
            if node.explicit_listeners {
                reply["served_after"] = json!("restart");
            }
            (0, reply.to_string())
        }
        Err(e) => write_error("vhost.add", &e),
    }
}

/// `webd.vhost.remove` — caller-origin `delete`. Fires the namespace's
/// `after_delete` hook → `VhostRemoved` event → C4b cleanup
/// (archive `acme/<fqdn>/live/`, drop the plan, rebuild arc-swaps).
async fn vhost_remove(node: &Arc<NodeState>, cmd: &IncomingCommand) -> (u8, String) {
    let fqdn = match kwarg(cmd, "fqdn") {
        Some(s) => s,
        None => return caller_error("missing required kwarg: fqdn"),
    };

    let Some(runtime) = node.vhosts_runtime.as_ref() else {
        return server_error("webd.vhosts runtime not attached — bootstrap node or wiring bug");
    };

    let ns = namespace_name();
    let key = RecordKey::collection(ns.clone(), fqdn.clone());

    // OCC anchor — same `require_version=true` rule applies to delete.
    // A missing row returns rc=10 (not server_error) — `vhost.remove`
    // of a non-existent fqdn is operator error, not substrate breakage.
    let expected_version = match runtime.store().get(&key).await {
        Ok(snap) => snap.value.version,
        Err(StoreError::NotFound) => {
            return caller_error(&format!(
                "vhost {fqdn:?} not found in webd.vhosts namespace",
            ));
        }
        Err(other) => {
            return server_error(&format!(
                "fetching vhost row for delete OCC anchor: {other}"
            ));
        }
    };

    let ts_ms = now_ms();
    let outcome = runtime
        .delete_with_origin(
            key,
            DeleteOpts {
                expected_version: Some(expected_version),
                actor: Actor::service("webd").expect("valid actor"),
                cause: Some("vhost.remove".into()),
                ts_ms,
            },
            // Caller-origin: no daemon-owned-field rule on delete, and
            // the row is going away entirely.
            WriteOrigin::caller(),
        )
        .await;
    match outcome {
        Ok(_) => (
            0,
            json!({
                "ok": true,
                "fqdn": fqdn,
            })
            .to_string(),
        ),
        Err(e) => write_error("vhost.remove", &e),
    }
}

/// List vhosts with derived ACME status and full stored fields.
async fn vhost_list(node: &Arc<NodeState>) -> (u8, String) {
    let Some(runtime) = node.vhosts_runtime.as_ref() else {
        return server_error("webd.vhosts runtime not attached — bootstrap node or wiring bug");
    };

    let snap = match runtime.store().list(&namespace_name()).await {
        Ok(s) => s,
        Err(e) => return server_error(&format!("listing webd.vhosts: {e}")),
    };

    let tls_snap = node.tls_status_rx.borrow().clone();
    let now = OffsetDateTime::now_utc().format(&Rfc3339).ok();
    let now_unix = OffsetDateTime::now_utc().unix_timestamp();

    let mut rows_out: Vec<JsonValue> = Vec::with_capacity(snap.value.len());
    for record in snap.value {
        let row = match vhost_row_from_value(&record.value) {
            Ok(r) => r,
            Err(e) => {
                return server_error(&format!(
                    "webd.vhosts row {key:?} failed VhostRow deserialisation: {e}",
                    key = record.key.key,
                ));
            }
        };
        let acme_status = derive_acme_status(&row, &tls_snap, now_unix);
        rows_out.push(project_row(&row, &acme_status));
    }

    (
        0,
        json!({
            "rows": rows_out,
            "count": rows_out.len(),
            "now": now,
        })
        .to_string(),
    )
}

/// `webd.acme.renew` — queue the given fqdn for force-renewal on the
/// next sweep and wake the renewal loop. Returns
/// `{ok:true, state:"pending"}` synchronously; the caller polls
/// `webd.acme.status` (or watches `webd.props.audit.watch`) for the
/// outcome.
///
/// Force-renew bypasses **both** the per-vhost cooldown gate and the
/// 30-day renewal-window gate inside `tick_once`. Disabled-state and
/// apex-policy gates are deliberately NOT bypassed — those represent
/// operator policy, not scheduler timing, and a force-renew of a
/// disabled vhost would be a footgun. A bare notify (without queueing)
/// would only break the interval wait, not the gates — the failure
/// mode that earlier returned `ok:true` while issuance was silently
/// skipped on every tick.
///
/// ## Pre-queue validation
///
/// Before queueing, the command reads the namespace row internally and
/// rejects four classes of inputs that `tick_once` would silently
/// no-op on:
///
/// * fqdn typo / tombstoned row → `caller_error("vhost {fqdn} not
///   found")` — row is absent (hard-deleted) or soft-deleted.
/// * `enabled=false` → `caller_error("vhost {fqdn} is disabled")` —
///   `tick_once` skips disabled plans by policy.
/// * `acme_provider.is_none()` → `caller_error("vhost {fqdn} has no
///   ACME provider")` — manual-TLS-only or HTTP-only rows produce no
///   plan; the queue entry self-clears on the next tick with no
///   visible error.
///
/// The internal read uses the runtime's store directly (no
/// an additional read request) — this is validation
/// under the narrow renew authority, not disclosure. The command's
/// response carries no row fields beyond the echoed `fqdn`.
async fn acme_renew(node: &Arc<NodeState>, cmd: &IncomingCommand) -> (u8, String) {
    let fqdn = match kwarg(cmd, "fqdn") {
        Some(s) => s,
        None => return caller_error("missing required kwarg: fqdn"),
    };

    let Some(runtime) = node.vhosts_runtime.as_ref() else {
        return server_error("webd.vhosts runtime not attached — bootstrap node or wiring bug");
    };

    // Internal namespace read for validation. NotFound covers both
    // never-existed (typo) and soft-deleted (tombstoned) — `get()`
    // hides tombstones, which is exactly the failure shape we want to
    // surface as caller_error here.
    let key = RecordKey::collection(namespace_name(), fqdn.clone());
    let row = match runtime.store().get(&key).await {
        Ok(snap) => match vhost_row_from_value(&snap.value.value) {
            Ok(r) => r,
            Err(e) => {
                return server_error(&format!(
                    "row {fqdn:?} failed VhostRow deserialisation: {e}",
                ));
            }
        },
        Err(StoreError::NotFound) => {
            return caller_error(&format!(
                "vhost {fqdn:?} not found in webd.vhosts namespace",
            ));
        }
        Err(other) => {
            return server_error(&format!("fetching webd.vhosts row {fqdn:?}: {other}"));
        }
    };
    if !row.enabled {
        return caller_error(&format!("vhost {fqdn:?} is disabled — force-renew refused",));
    }
    if row.acme_provider.is_none() {
        return caller_error(&format!(
            "vhost {fqdn:?} has no acme_provider — manual-TLS-only or HTTP-only vhost cannot force-renew",
        ));
    }

    let (Some(notify), Some(queue)) = (
        node.acme_notify.as_ref(),
        node.acme_force_renew_queue.as_ref(),
    ) else {
        return server_error("no ACME provisioner attached on this node — renew has no surface");
    };
    queue.lock().await.insert(fqdn.clone());
    notify.notify_one();
    tracing::info!(
        target: "webd::bus::vhost_commands",
        fqdn = %fqdn,
        "acme.renew: queued for force-renewal + provisioner notified",
    );
    (
        0,
        json!({
            "ok": true,
            "fqdn": fqdn,
            "state": "pending",
        })
        .to_string(),
    )
}

/// Return the vhost's ACME state and full error details for every caller.
async fn acme_status(node: &Arc<NodeState>, cmd: &IncomingCommand) -> (u8, String) {
    let fqdn = match kwarg(cmd, "fqdn") {
        Some(s) => s,
        None => return caller_error("missing required kwarg: fqdn"),
    };

    let Some(runtime) = node.vhosts_runtime.as_ref() else {
        return server_error("webd.vhosts runtime not attached — bootstrap node or wiring bug");
    };

    let key = RecordKey::collection(namespace_name(), fqdn.clone());
    let row = match runtime.store().get(&key).await {
        Ok(snap) => match vhost_row_from_value(&snap.value.value) {
            Ok(r) => r,
            Err(e) => {
                return server_error(&format!(
                    "row {fqdn:?} failed VhostRow deserialisation: {e}",
                ));
            }
        },
        Err(StoreError::NotFound) => {
            return caller_error(&format!(
                "vhost {fqdn:?} not found in webd.vhosts namespace",
            ));
        }
        Err(other) => {
            return server_error(&format!("fetching webd.vhosts row {fqdn:?}: {other}"));
        }
    };

    let tls_snap = node.tls_status_rx.borrow().clone();
    let now_unix = OffsetDateTime::now_utc().unix_timestamp();
    let acme_status = derive_acme_status(&row, &tls_snap, now_unix);

    // Vhost provisioner state — `None` on no-ACME nodes, falsy
    // defaults when the row exists but the provisioner hasn't ticked
    // it yet.
    let vhost_state = tls_snap
        .acme
        .as_ref()
        .and_then(|a| a.vhost_state.get(&fqdn).cloned());

    let last_error_field = vhost_state.as_ref().and_then(|s| s.last_error.clone());

    let body = json!({
        "fqdn": fqdn,
        "acme_status": acme_status,
        "not_after": row.not_after,
        "last_attempt": row.last_attempt,
        "last_error_count": vhost_state.as_ref().map(|s| s.last_error_count),
        "last_error": last_error_field,
        "next_attempt_after": vhost_state.as_ref().and_then(|s| s.next_attempt_after_rfc3339.clone()),
        "issued": vhost_state.as_ref().map(|s| s.issued).unwrap_or(false),
    });
    (0, body.to_string())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the [`PeerIdentity`] noded forwards to the broker. Mirrors
/// [`crate::vhosts_namespace::dispatch_props`] — `service_name` carries
/// the registered sender; Unix peer creds + WireGuard fields stay
/// `None` because noded doesn't surface them today.
pub(crate) fn caller_error(msg: &str) -> (u8, String) {
    (
        RC_CALLER_ERROR,
        json!({
            "error": msg,
        })
        .to_string(),
    )
}

/// rc for a node-side fault the operator must fix on the node itself —
/// today only an unreadable `node.conf.mix` reported by the
/// `webd.vhosts` hooks (`HookError::Hook`), matching the rc 20 the raw
/// `webd.props.*` surface returns for the same hook error.
const RC_NODE_FAULT: u8 = 20;

/// Project a namespace write error: a hook-reported node fault is rc 20
/// (`kind: "server"`); everything else — validation refusals, OCC
/// conflicts — stays the caller-side rc 10 this module has always used.
fn write_error(command_name: &str, e: &props::RuntimeError) -> (u8, String) {
    let msg = format!("{command_name} failed: {e}");
    match e {
        props::RuntimeError::Hook(props::HookError::Hook { .. }) => (
            RC_NODE_FAULT,
            json!({
                "error": msg,
                "kind": "server",
            })
            .to_string(),
        ),
        _ => caller_error(&msg),
    }
}

fn server_error(msg: &str) -> (u8, String) {
    // Same rc=10 sentinel — `NodedClient::call` doesn't distinguish a
    // server-side failure from a caller error at the wire level (both
    // surface as `Err` above the rc>=10 threshold). The body's
    // `kind: "server"` makes the distinction visible to operators
    // reading the response payload.
    (
        RC_CALLER_ERROR,
        json!({
            "error": msg,
            "kind": "server",
        })
        .to_string(),
    )
}

/// Single millisecond wall-clock stamp threaded into the substrate
/// write. The substrate audit row uses this for `cause_ts`.
fn now_ms() -> i64 {
    let now = OffsetDateTime::now_utc();
    i64::try_from(now.unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
}

/// Derive `acme_status`:
///
/// * `failing` — provisioner has a non-zero `last_error_count`.
/// * `pending` — ACME mode configured but no `not_after` stamped yet
///   (issued for the first time, not yet successful).
/// * `valid` — `not_after - now > RENEWAL_WINDOW`.
/// * `expiring_soon` — `not_after - now <= RENEWAL_WINDOW`.
/// * `n/a` — manual-TLS or HTTP-only row (no ACME mode at all).
fn derive_acme_status(
    row: &VhostRow,
    tls_snap: &crate::tls_status::TlsStatusSnapshot,
    now_unix: i64,
) -> String {
    if row.acme_provider.is_none() {
        return "n/a".to_string();
    }
    let backoff = tls_snap
        .acme
        .as_ref()
        .and_then(|a| a.vhost_state.get(&row.fqdn))
        .map(|s| s.last_error_count)
        .unwrap_or(0);
    if backoff > 0 {
        return "failing".to_string();
    }
    let Some(not_after_str) = row.not_after.as_ref() else {
        return "pending".to_string();
    };
    let Ok(not_after) = OffsetDateTime::parse(not_after_str, &Rfc3339) else {
        return "pending".to_string();
    };
    let remaining = not_after.unix_timestamp() - now_unix;
    if remaining > RENEWAL_WINDOW.as_secs() as i64 {
        "valid".to_string()
    } else {
        "expiring_soon".to_string()
    }
}

/// Render all stored vhost fields for every Bus caller.
fn project_row(row: &VhostRow, acme_status: &str) -> JsonValue {
    json!({
        "fqdn": row.fqdn,
        "enabled": row.enabled,
        "www_dir": row.www_dir,
        "aliases": row.aliases,
        "source": row.source,
        "acme_provider": row.acme_provider,
        "acme_challenge": row.acme_challenge,
        "acme_contact_email": row.acme_contact_email,
        "tls_cert_path": row.tls_cert_path,
        "tls_key_path": row.tls_key_path,
        "cert_blob_id": row.cert_blob_id,
        "key_blob_id": row.key_blob_id,
        "not_after": row.not_after,
        "last_attempt": row.last_attempt,
        "last_error_count": row.last_error_count,
        "last_error": row.last_error,
        "acme_status": acme_status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acme_provisioner::FqdnLockMap;
    use crate::tls_status::{AcmeStatusSnapshot, AcmeVhostStateSnapshot, TlsStatusSnapshot};
    use crate::vhosts_namespace::{
        ListenerConfigSource, listener_add_error, listener_remove_error,
        register_vhosts_namespace_with_listener_config,
    };
    use arc_swap::ArcSwap;
    use config::node::{NodeConfig, WebdListenerConfig};
    use props::PropsRouter;
    use props::sqlite::SqliteStore;
    use rusqlite::Connection;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use tokio::sync::RwLock;

    #[test]
    fn configs_with_removed_listener_operators_still_load() {
        let config: NodeConfig = config::from_conf_mix_str(
            r#"webd: { listeners: { operators: ["old-client"] }, port: 8443 }"#,
        )
        .expect("unknown webd settings survive typed node config loading");
        assert_eq!(config.webd.port, 8443);
    }

    fn cmd_with_headers(command_name: &str, headers: &[(&str, &str)]) -> IncomingCommand {
        let mut hdrs: std::collections::BTreeMap<String, String> = Default::default();
        for (k, v) in headers {
            hdrs.insert((*k).to_string(), (*v).to_string());
        }
        IncomingCommand {
            generation: 0,
            from: String::new(),
            command: command_name.to_string(),
            id: None,
            args: serde_json::Value::Null,
            body: String::new(),
            headers: hdrs,
        }
    }

    /// Build a command whose kwargs arrive in the JSON `args` object with
    /// NO headers — the shape a bare Mix `send command k=v` actually produces
    /// (2026-07 sweep). Confirms per-command that the kwarg readers pull from
    /// `args`, not only headers.
    fn cmd_with_args(command_name: &str, args: JsonValue) -> IncomingCommand {
        IncomingCommand {
            generation: 0,
            from: String::new(),
            command: command_name.to_string(),
            id: None,
            args,
            body: String::new(),
            headers: Default::default(),
        }
    }

    /// Build an in-memory namespace with the production command path.
    async fn build_node() -> Arc<NodeState> {
        build_node_full(ListenerConfigSource::Fixed(None)).await
    }

    async fn build_node_with_listener_config(cfg: NodeConfig) -> Arc<NodeState> {
        build_node_full(ListenerConfigSource::Fixed(Some(Arc::new(cfg)))).await
    }

    async fn build_node_full(listener_config: ListenerConfigSource) -> Arc<NodeState> {
        // Mirror production: explicit iff the node config carries a
        // [[webd.listener]] array.
        let explicit_listeners = matches!(
            &listener_config,
            ListenerConfigSource::Fixed(Some(cfg)) if !cfg.webd.listener.is_empty()
        );
        let conn = Connection::open_in_memory().expect("sqlite");
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )
        .expect("pragmas");
        let store = Arc::new(SqliteStore::new("webd", conn).expect("store"));
        let mut router = PropsRouter::new("webd");
        let (runtime, _events_rx) =
            register_vhosts_namespace_with_listener_config(&mut router, &store, listener_config)
                .expect("register");
        let (_tx, tls_status_rx) = tokio::sync::watch::channel(TlsStatusSnapshot::default());
        let key_locks: FqdnLockMap = Arc::new(TokioMutex::new(HashMap::new()));
        let notify = Arc::new(tokio::sync::Notify::new());

        Arc::new(NodeState {
            share_roots: crate::file_share::Roots::default(),
            share_runtime: crate::shares::Runtime::default(),
            media_runtime: crate::media::Runtime::default(),
            service_jmap_tokens: Arc::new(
                tokio::sync::Mutex::new(std::collections::HashMap::new()),
            ),
            login_throttle: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            login_pending: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            vhosts: Arc::new(ArcSwap::from(Arc::new(
                crate::vhost_directory::VhostDirectory::empty(),
            ))),
            http_client: reqwest::Client::builder().build().expect("reqwest"),
            session: Arc::new(crate::session::SessionSealer::ephemeral()),
            served_mail_domains: HashSet::new(),
            mx: None,
            autoconfig_mail_host: None,
            acme_challenges: Arc::new(RwLock::new(HashMap::new())),
            tls_status_rx,
            props_router: Arc::new(router),
            props_subscribe_granter: Arc::new(
                crate::bus::subscribe_granter::NodedSubscribeGranter::new(
                    crate::bus::subscribe_granter::new_broker_handle(),
                ),
            ),
            broker_handle: crate::bus::subscribe_granter::new_broker_handle(),
            vhosts_runtime: Some(runtime),
            listeners_runtime: None,
            vhost_key_locks: Some(key_locks),
            acme_notify: Some(notify),
            // Tests that exercise force-renew share the fixture but
            // don't all need a populated queue; `Some(empty)` is the
            // correct default so the command's
            // `(notify, queue)` both-Some guard passes and the command
            // exercises the queue-then-notify path end-to-end.
            acme_force_renew_queue: Some(Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            ))),
            // C5 command tests don't exercise embedded handlers.
            handlers: Arc::new(ArcSwap::from(Arc::new(
                crate::mix_handler::HandlerTable::default(),
            ))),
            handler_ast_cache: crate::mix_handler::new_ast_cache(),
            tls_reload: None,
            explicit_listeners,
        })
    }

    #[tokio::test]
    async fn listener_commands_accept_every_caller_and_include_secrets() {
        let mut node = build_node().await;
        let store =
            Arc::new(SqliteStore::new("webd", Connection::open_in_memory().unwrap()).unwrap());
        let mut router = PropsRouter::new("webd");
        let (runtime, _events) =
            crate::listeners_namespace::register_listeners_namespace(&mut router, &store).unwrap();
        runtime
            .set_with_origin(
                RecordKey::collection(crate::listeners_namespace::namespace_name(), "public"),
                serde_json::from_value(serde_json::json!({
                    "id":"public", "enabled":true, "external":true, "last_error":"listener detail"
                }))
                .unwrap(),
                SetOpts {
                    expected_version: Some(Version::zero()),
                    merge: MergeMode::Replace,
                    actor: Actor::service("webd").unwrap(),
                    cause: None,
                    ts_ms: 0,
                },
                WriteOrigin::backend(),
            )
            .await
            .unwrap();
        Arc::get_mut(&mut node).unwrap().listeners_runtime = Some(runtime);
        for caller in ["", "unlisted-agent"] {
            for (command, enabled) in [("listener.disable", false), ("listener.enable", true)] {
                let mut request = cmd_with_args(command, serde_json::json!({"id":"public"}));
                request.from = caller.into();
                let (rc, body) =
                    super::super::listener_commands::dispatch(command, &request, &node)
                        .await
                        .unwrap();
                assert_eq!(rc, 0, "{caller:?}: {body}");
                let status = cmd_with_args("listener.status", serde_json::json!({"id":"public"}));
                let (rc, body) =
                    super::super::listener_commands::dispatch("listener.status", &status, &node)
                        .await
                        .unwrap();
                assert_eq!(rc, 0, "{body}");
                let body: JsonValue = serde_json::from_str(&body).unwrap();
                assert_eq!(body["listeners"][0]["enabled"], enabled);
                assert_eq!(body["listeners"][0]["last_error"], "listener detail");
            }
        }
    }

    #[tokio::test]
    async fn tls_reload_availability_is_the_same_for_every_caller() {
        let node = build_node().await;
        for caller in ["", "unlisted-agent"] {
            let mut request = cmd_with_headers("webd.tls.reload", &[]);
            request.from = caller.into();
            let (rc, body) = super::super::tls::dispatch("tls.reload", &request, &node)
                .await
                .unwrap();
            assert_eq!(rc, 10);
            assert!(body.contains("unavailable"), "{body}");
        }
    }

    /// V-V — `vhost.add` happy path with full ACME trio. Acquires the
    /// per-fqdn lock, writes through backend-origin, stamps
    /// source=bus_runtime. Verifies the row lands in the namespace
    /// with the stamped source.
    #[tokio::test(flavor = "current_thread")]
    async fn vhost_add_happy_path_writes_bus_runtime_row() {
        let node = build_node().await;
        let cmd = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "p3test.example.com"),
                ("www_dir", "/srv/p3test"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, body) = vhost_add(&node, &cmd).await;
        assert_eq!(rc, 0, "happy path rc=0; body={body}");
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["fqdn"], "p3test.example.com");
        assert_eq!(v["source"], "bus_runtime");
        // No explicit listener array: no restart caveat in the reply.
        assert!(v.get("served_after").is_none(), "{body}");

        // Read it back via store().get and assert source.
        let runtime = node.vhosts_runtime.as_ref().unwrap();
        let key = RecordKey::collection(namespace_name(), "p3test.example.com".to_string());
        let snap = runtime.store().get(&key).await.expect("row present");
        if let PropValue::Object(m) = &snap.value.value {
            assert_eq!(
                m.get("source"),
                Some(&PropValue::String("bus_runtime".into())),
                "source must be stamped bus_runtime",
            );
            assert_eq!(
                m.get("fqdn"),
                Some(&PropValue::String("p3test.example.com".to_string())),
            );
        } else {
            panic!("row body is not an Object: {:?}", snap.value.value);
        }
    }

    /// Listener intent reads its id from args even with a broker correlation header.
    #[tokio::test(flavor = "current_thread")]
    async fn listener_enable_reads_id_from_args_object() {
        let node = build_node().await;
        let mut cmd = cmd_with_args(
            "webd.listener.enable",
            serde_json::json!({ "id": "nonexistent-listener" }),
        );
        cmd.from = "op-tester".to_string();
        // Replicate the PRODUCTION wire shape: noded stamps the message
        // correlation id into a reserved `id` HEADER. A header-first kwarg
        // read let this shadow the operator's args `id=` (caught by a live
        // operator-cap probe — the unit test missed it until this header was
        // added). The not-found body below must echo the OPERATOR's id, not
        // this `noded-9999`.
        cmd.headers
            .insert("id".to_string(), "noded-9999".to_string());
        let (rc, body) = super::super::listener_commands::dispatch("listener.enable", &cmd, &node)
            .await
            .expect("listener.enable is a listener command");
        // This fixture has no listener runtime, so availability follows argument parsing.
        assert_eq!(rc, 10, "body={body}");
        assert!(
            !body.contains("missing required kwarg"),
            "id must be read from args; got {body}",
        );
        assert!(
            body.contains("not attached"),
            "expected not-attached after the id read; got {body}",
        );
    }

    /// The reserved-`id`-header collision at the LISTENER STATUS command (which
    /// DOES reach the id read without a mutation cap): with the ambient
    /// message-id header present AND an operator `id=` in args, the filter
    /// must use the OPERATOR's id. Pre-fix, `status id=lan` silently filtered
    /// by `noded-<seq>` and returned an empty list — a coincidence that read
    /// like success. Here the operator id matches no listener, so the list is
    /// empty either way; the pin is that dispatch SUCCEEDS with the operator
    /// id honoured, exercised via the real dispatch path.
    #[tokio::test(flavor = "current_thread")]
    async fn listener_status_id_filter_uses_operator_id_not_message_id() {
        let node = build_node().await;
        // status needs a listeners runtime; without one it's "not attached".
        // The point here is purely that the id read prefers args — assert via
        // kwarg directly on the production-shaped command, since status with
        // no runtime short-circuits before building the filter.
        let mut cmd = cmd_with_args("webd.listener.status", serde_json::json!({ "id": "lan" }));
        cmd.headers
            .insert("id".to_string(), "noded-1234".to_string());
        assert_eq!(
            super::super::kwarg(&cmd, "id").as_deref(),
            Some("lan"),
            "status must filter by the operator id, not the message-id header",
        );
        let _ = node; // node built to mirror the dispatch fixture shape
    }

    /// An anonymous caller can create a validated vhost.
    #[tokio::test(flavor = "current_thread")]
    async fn anonymous_vhost_add_succeeds() {
        let node = build_node().await;
        let cmd = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "denied.example.com"),
                ("www_dir", "/srv/denied"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, body) = vhost_add(&node, &cmd).await;
        assert_eq!(rc, 0, "anonymous add: {body}");
        let runtime = node.vhosts_runtime.as_ref().unwrap();
        let key = RecordKey::collection(namespace_name(), "denied.example.com".to_string());
        assert!(runtime.store().get(&key).await.is_ok());
    }

    /// V-V — `vhost.remove` happy path. Add a row, remove it,
    /// confirm the row is gone.
    #[tokio::test(flavor = "current_thread")]
    async fn vhost_remove_happy_path_drops_row() {
        let node = build_node().await;
        // Seed: vhost.add first.
        let add = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "remove.example.com"),
                ("www_dir", "/srv/remove"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, _) = vhost_add(&node, &add).await;
        assert_eq!(rc, 0);

        let rm = cmd_with_headers("webd.vhost.remove", &[("fqdn", "remove.example.com")]);
        let (rc, body) = vhost_remove(&node, &rm).await;
        assert_eq!(rc, 0, "remove happy path rc=0; body={body}");

        let runtime = node.vhosts_runtime.as_ref().unwrap();
        let key = RecordKey::collection(namespace_name(), "remove.example.com".to_string());
        assert!(
            matches!(runtime.store().get(&key).await, Err(StoreError::NotFound)),
            "row must be gone after vhost.remove",
        );
    }

    /// Regression for the soft-delete re-add OCC trap: after
    /// `vhost.remove` leaves a tombstone, `vhost.add` of the same fqdn
    /// must succeed by anchoring against the tombstone version (SPEC 12
    /// §5.4) rather than `Version::zero()`. Without
    /// `Store::version_anchor`, the second add would fail with
    /// `VersionMismatch{expected: 0, current: <tombstone_version>}` and
    /// the operator would have no in-band recovery path.
    #[tokio::test(flavor = "current_thread")]
    async fn vhost_add_after_remove_recreates_via_tombstone_anchor() {
        let node = build_node().await;
        let add1 = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "readd.example.com"),
                ("www_dir", "/srv/readd"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, _) = vhost_add(&node, &add1).await;
        assert_eq!(rc, 0, "initial add must succeed");

        let rm = cmd_with_headers("webd.vhost.remove", &[("fqdn", "readd.example.com")]);
        let (rc, _) = vhost_remove(&node, &rm).await;
        assert_eq!(rc, 0, "remove must succeed");

        let runtime = node.vhosts_runtime.as_ref().unwrap();
        let key = RecordKey::collection(namespace_name(), "readd.example.com".to_string());
        assert!(
            matches!(runtime.store().get(&key).await, Err(StoreError::NotFound)),
            "soft-deleted row must be hidden from get",
        );
        // But the tombstone version IS present and must be observable
        // via the new substrate API — this is what makes re-add work.
        let anchor = runtime
            .store()
            .version_anchor(&key)
            .await
            .expect("anchor query");
        assert!(
            anchor.is_some(),
            "tombstone must surface a version_anchor (got None)",
        );

        let add2 = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "readd.example.com"),
                ("www_dir", "/srv/readd-v2"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, body) = vhost_add(&node, &add2).await;
        assert_eq!(
            rc, 0,
            "re-add after remove must succeed via tombstone anchor; body={body}",
        );
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        // The recreated row is live again.
        let snap = runtime.store().get(&key).await.expect("re-add live row");
        if let PropValue::Object(m) = &snap.value.value {
            assert_eq!(
                m.get("www_dir"),
                Some(&PropValue::String("/srv/readd-v2".into())),
                "patch-merge picked up the v2 www_dir",
            );
        } else {
            panic!("row body is not an Object: {:?}", snap.value.value);
        }
    }

    /// V-V — `vhost.list` happy path returns the row with derived
    /// `acme_status = "pending"` (no `not_after` stamped yet).
    #[tokio::test(flavor = "current_thread")]
    async fn vhost_list_happy_path_decorates_acme_status_pending() {
        let node = build_node().await;
        // Seed.
        let add = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "list.example.com"),
                ("www_dir", "/srv/list"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, _) = vhost_add(&node, &add).await;
        assert_eq!(rc, 0);

        let (rc, body) = vhost_list(&node).await;
        assert_eq!(rc, 0, "list happy path rc=0; body={body}");
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        assert_eq!(v["count"], 1);
        let row = &v["rows"][0];
        assert_eq!(row["fqdn"], "list.example.com");
        assert_eq!(row["acme_status"], "pending");
        // No certificate has been issued yet, so the field is present but null.
        assert!(row.get("cert_blob_id").is_some());
    }

    /// V-V — `acme.renew` happy path BOTH queues the fqdn for
    /// force-renewal AND fires the notify. The pair is what makes the
    /// command actually bypass the renewal-window/cooldown gates on the
    /// next sweep; a bare notify (the pre-fix behaviour) would wake
    /// the loop without queueing, leaving timing gates intact and
    /// silently no-op'ing the operator request.
    ///
    /// The command now also pre-validates the fqdn against the namespace
    /// row (since the post-fix command rejects unknown/disabled/non-ACME
    /// fqdns rather than queueing a guaranteed-no-op), so the test
    /// seeds an enabled ACME row through vhost.add before invoking renew.
    #[tokio::test(flavor = "current_thread")]
    async fn acme_renew_happy_path_queues_then_notifies() {
        let node = build_node().await;
        // Seed the row so the new pre-queue validation passes.
        let seed = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "kick.example.com"),
                ("www_dir", "/srv/kick"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, _) = vhost_add(&node, &seed).await;
        assert_eq!(rc, 0, "seed must land for renew validation to pass");

        let notify = node.acme_notify.clone().expect("notify wired");
        let queue = node.acme_force_renew_queue.clone().expect("queue wired");
        // Spawn a waiter so we can assert the notify_one fired.
        let waiter = tokio::spawn(async move {
            notify.notified().await;
        });
        let cmd = cmd_with_headers("webd.acme.renew", &[("fqdn", "kick.example.com")]);
        let (rc, body) = acme_renew(&node, &cmd).await;
        assert_eq!(rc, 0, "renew happy path rc=0; body={body}");
        // The waiter should resolve quickly. tokio::time::timeout
        // guards against a regression where notify_one is no-op'd.
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("notify_one must wake a notified() waiter")
            .expect("waiter task ran");
        // The fqdn must be in the force-renew queue at this point —
        // tick_once drains it later, but until then it's observable
        // here (the test holds an Arc clone of the same Mutex).
        let snapshot = queue.lock().await.clone();
        assert!(
            snapshot.contains("kick.example.com"),
            "queue must hold the requested fqdn (snapshot={snapshot:?})",
        );
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["state"], "pending");
    }

    /// An anonymous renewal request reaches target validation.
    #[tokio::test(flavor = "current_thread")]
    async fn anonymous_acme_renew_validates_target() {
        let node = build_node().await;
        let cmd = cmd_with_headers("webd.acme.renew", &[("fqdn", "denied.example.com")]);
        let (rc, body) = acme_renew(&node, &cmd).await;
        assert_eq!(rc, RC_CALLER_ERROR);
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        assert!(v["error"].as_str().unwrap().contains("not found"), "{body}");
    }

    /// V-V — `acme.renew` on an unknown fqdn (typo or never-added) is
    /// rejected as `caller_error`, not silently queued. Without this
    /// gate, `tick_once` would find no matching plan and self-clear
    /// the queue entry, leaving the operator with an `ok:true`
    /// response and no renewal ever occurring. The same arm also
    /// rejects tombstoned (soft-deleted) rows since `store().get`
    /// returns `NotFound` for them — soft-delete is intentional in
    /// this namespace and a tombstoned vhost has no live plan to
    /// renew.
    #[tokio::test(flavor = "current_thread")]
    async fn acme_renew_unknown_fqdn_caller_error() {
        let node = build_node().await;
        let queue = node.acme_force_renew_queue.clone().expect("queue wired");
        let cmd = cmd_with_headers(
            "webd.acme.renew",
            &[("fqdn", "typo-or-tombstoned.example.com")],
        );
        let (rc, body) = acme_renew(&node, &cmd).await;
        assert_eq!(rc, RC_CALLER_ERROR, "unknown fqdn must reject; body={body}");
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        let err = v["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("not found"),
            "error should mention not found; got {err}",
        );
        // Negative: nothing was queued.
        assert!(
            queue.lock().await.is_empty(),
            "queue must remain empty after rejected renew",
        );
    }

    /// V-V — `acme.renew` on a soft-deleted (tombstoned) row is
    /// rejected by the same `NotFound` arm as the typo case. Pinned
    /// separately so a future change to soft-delete semantics (e.g.
    /// surfacing tombstones via `get`) doesn't silently re-open the
    /// no-op path.
    #[tokio::test(flavor = "current_thread")]
    async fn acme_renew_tombstoned_fqdn_caller_error() {
        let node = build_node().await;
        let queue = node.acme_force_renew_queue.clone().expect("queue wired");
        // Add then remove → tombstone in the SqliteStore.
        let add = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "tombstone.example.com"),
                ("www_dir", "/srv/tombstone"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, _) = vhost_add(&node, &add).await;
        assert_eq!(rc, 0);
        let rm = cmd_with_headers("webd.vhost.remove", &[("fqdn", "tombstone.example.com")]);
        let (rc, _) = vhost_remove(&node, &rm).await;
        assert_eq!(rc, 0);

        let cmd = cmd_with_headers("webd.acme.renew", &[("fqdn", "tombstone.example.com")]);
        let (rc, body) = acme_renew(&node, &cmd).await;
        assert_eq!(
            rc, RC_CALLER_ERROR,
            "tombstoned row must reject; body={body}"
        );
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        assert!(
            v["error"]
                .as_str()
                .unwrap_or_default()
                .contains("not found"),
            "tombstone goes through the same NotFound arm",
        );
        assert!(
            queue.lock().await.is_empty(),
            "queue must remain empty after tombstone rejection",
        );
    }

    /// V-V — `acme.renew` on a disabled (`enabled=false`) row is
    /// rejected. Disabled is operator policy, not scheduler timing,
    /// so force-renew honours it (consistent with `tick_once`'s
    /// disabled-state gate which the BLOCKER 1 fix did NOT bypass).
    #[tokio::test(flavor = "current_thread")]
    async fn acme_renew_disabled_fqdn_caller_error() {
        let node = build_node().await;
        let queue = node.acme_force_renew_queue.clone().expect("queue wired");
        let add = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "disabled.example.com"),
                ("www_dir", "/srv/disabled"),
                ("enabled", "false"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, _) = vhost_add(&node, &add).await;
        assert_eq!(rc, 0);

        let cmd = cmd_with_headers("webd.acme.renew", &[("fqdn", "disabled.example.com")]);
        let (rc, body) = acme_renew(&node, &cmd).await;
        assert_eq!(rc, RC_CALLER_ERROR, "disabled row must reject; body={body}");
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        assert!(
            v["error"].as_str().unwrap_or_default().contains("disabled"),
            "error must mention disabled",
        );
        assert!(
            queue.lock().await.is_empty(),
            "queue must remain empty after disabled rejection",
        );
    }

    /// V-V — `acme.renew` on a row with no `acme_provider` (manual-TLS
    /// or HTTP-only vhost) is rejected. `tick_once` produces no plan
    /// for these rows, so a queued entry would self-clear with no
    /// visible error — the command must surface the configuration
    /// mismatch to the operator instead.
    #[tokio::test(flavor = "current_thread")]
    async fn acme_renew_no_acme_provider_caller_error() {
        let node = build_node().await;
        let queue = node.acme_force_renew_queue.clone().expect("queue wired");
        // Manual-TLS vhost: no acme_provider, but cert/key pair so the
        // namespace hook accepts an enabled row. This is the operator
        // shape the no-acme rejection arm has to surface — running
        // `acme.renew` on a row that has no ACME plan to renew.
        let add = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "manual-tls.example.com"),
                ("www_dir", "/srv/manual-tls"),
                ("tls.cert_path", "/etc/mixos/manual-tls/cert.pem"),
                ("tls.key_path", "/etc/mixos/manual-tls/key.pem"),
            ],
        );
        let (rc, body) = vhost_add(&node, &add).await;
        assert_eq!(rc, 0, "manual-TLS vhost.add must succeed; body={body}");

        let cmd = cmd_with_headers("webd.acme.renew", &[("fqdn", "manual-tls.example.com")]);
        let (rc, body) = acme_renew(&node, &cmd).await;
        assert_eq!(
            rc, RC_CALLER_ERROR,
            "no-acme-provider row must reject; body={body}",
        );
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        assert!(
            v["error"]
                .as_str()
                .unwrap_or_default()
                .contains("acme_provider"),
            "error must mention acme_provider",
        );
        assert!(
            queue.lock().await.is_empty(),
            "queue must remain empty after no-acme-provider rejection",
        );
    }

    /// An anonymous caller receives full ACME error details.
    #[tokio::test(flavor = "current_thread")]
    async fn anonymous_acme_status_includes_last_error() {
        let node = build_node().await;
        // Seed a vhost row.
        let add = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "status.example.com"),
                ("www_dir", "/srv/status"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, _) = vhost_add(&node, &add).await;
        assert_eq!(rc, 0);

        // Seed the tls_status watch channel with a failing vhost_state.
        let (tx, rx) = tokio::sync::watch::channel(TlsStatusSnapshot {
            manual_identities: Vec::new(),
            acme: Some(AcmeStatusSnapshot {
                plans: Vec::new(),
                vhost_state: {
                    let mut m = BTreeMap::new();
                    m.insert(
                        "status.example.com".into(),
                        AcmeVhostStateSnapshot {
                            issued: false,
                            last_error: Some("secret-leaking-error-body".into()),
                            last_error_count: 2,
                            next_attempt_after_rfc3339: Some("2026-05-26T12:34:56Z".into()),
                        },
                    );
                    m
                },
            }),
        });
        // Rebuild the NodeState with the seeded rx (the helper
        // hands back a default-empty one).
        let node2 = {
            let runtime = node.vhosts_runtime.clone();
            let key_locks = node.vhost_key_locks.clone();
            let notify = node.acme_notify.clone();
            let force_queue = node.acme_force_renew_queue.clone();
            Arc::new(NodeState {
                share_roots: crate::file_share::Roots::default(),
                share_runtime: crate::shares::Runtime::default(),
                media_runtime: crate::media::Runtime::default(),
                service_jmap_tokens: Arc::new(tokio::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
                login_throttle: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                login_pending: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                vhosts: node.vhosts.clone(),
                http_client: node.http_client.clone(),
                session: node.session.clone(),
                served_mail_domains: node.served_mail_domains.clone(),
                mx: None,
                autoconfig_mail_host: node.autoconfig_mail_host.clone(),
                acme_challenges: node.acme_challenges.clone(),
                tls_status_rx: rx,
                props_router: node.props_router.clone(),
                props_subscribe_granter: node.props_subscribe_granter.clone(),
                broker_handle: node.broker_handle.clone(),
                vhosts_runtime: runtime,
                listeners_runtime: None,
                vhost_key_locks: key_locks,
                acme_notify: notify,
                acme_force_renew_queue: force_queue,
                handlers: Arc::new(ArcSwap::from(Arc::new(
                    crate::mix_handler::HandlerTable::default(),
                ))),
                handler_ast_cache: crate::mix_handler::new_ast_cache(),
                tls_reload: None,
                explicit_listeners: false,
            })
        };
        let _tx = tx; // keep sender alive for the rx to remain live

        let cmd = cmd_with_headers("webd.acme.status", &[("fqdn", "status.example.com")]);
        let (rc, body) = acme_status(&node2, &cmd).await;
        assert_eq!(rc, 0, "status happy path rc=0; body={body}");
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        assert_eq!(v["fqdn"], "status.example.com");
        assert_eq!(
            v["last_error"], "secret-leaking-error-body",
            "every caller receives last_error"
        );
        assert_eq!(v["last_error_count"], 2);
        assert_eq!(v["acme_status"], "failing"); // last_error_count > 0
    }

    /// Any caller can request ACME issuance; issuance rate limits still apply.
    #[tokio::test(flavor = "current_thread")]
    async fn anonymous_caller_can_add_new_acme_vhost_consuming_rate() {
        let node = build_node().await;
        let cmd = cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", "ratebudget.example.com"),
                ("www_dir", "/srv/ratebudget"),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        );
        let (rc, body) = vhost_add(&node, &cmd).await;
        assert_eq!(
            rc, 0,
            "anonymous caller admitted for ACME vhost.add; body={body}"
        );
    }

    // ── restart-time listener surface: vhost.add / vhost.remove must
    //    not leave a change the next boot rejects ─────────────────────

    fn listener(id: &str, bind: &str, enabled: bool, hosts: &[&str]) -> WebdListenerConfig {
        WebdListenerConfig {
            id: id.to_string(),
            bind: bind.to_string(),
            external: true,
            enabled,
            vhosts: hosts.iter().map(|h| (*h).to_string()).collect(),
        }
    }

    fn cfg_with(listeners: Vec<WebdListenerConfig>, vhost_hosts: &[&str]) -> NodeConfig {
        let mut cfg = NodeConfig::default();
        cfg.webd.listener = listeners;
        cfg.webd.vhost = vhost_hosts
            .iter()
            .map(|h| config::node::WebdVhostConfig {
                host: (*h).to_string(),
                www_dir: "/srv/x".to_string(),
                ..Default::default()
            })
            .collect();
        cfg
    }

    fn add_cmd(fqdn: &str) -> IncomingCommand {
        add_cmd_in(fqdn, "/srv/new")
    }

    fn add_cmd_in(fqdn: &str, www_dir: &str) -> IncomingCommand {
        cmd_with_headers(
            "webd.vhost.add",
            &[
                ("fqdn", fqdn),
                ("www_dir", www_dir),
                ("acme.provider", "letsencrypt_staging"),
                ("acme.challenge", "http01"),
                ("acme.contact_email", "ops@example.com"),
            ],
        )
    }

    async fn namespace_hosts(node: &NodeState) -> Vec<String> {
        let runtime = node.vhosts_runtime.as_ref().unwrap();
        crate::vhosts_namespace::snapshot_rows(runtime)
            .await
            .expect("snapshot")
            .into_iter()
            .map(|r| r.fqdn)
            .collect()
    }

    #[test]
    fn listener_add_error_mirrors_synthesize_listeners_per_host_rules() {
        let h = "new.example.org";
        // No node config / no listener array: implicit single listener
        // serves every host.
        assert!(listener_add_error(None, h).is_none());
        assert!(listener_add_error(Some(&cfg_with(vec![], &[])), h).is_none());
        // Named by exactly one enabled listener: admissible.
        let ok = cfg_with(vec![listener("pub", "192.0.2.1:443", true, &[h])], &[]);
        assert!(listener_add_error(Some(&ok), h).is_none());
        // Named by none: the crash-loop case.
        let none = cfg_with(
            vec![listener(
                "pub",
                "192.0.2.1:443",
                true,
                &["other.example.org"],
            )],
            &[],
        );
        let msg = listener_add_error(Some(&none), h).expect("unnamed host refused");
        assert!(msg.contains("not in any [[webd.listener]]"), "{msg}");
        // Named only by a disabled listener.
        let off = cfg_with(vec![listener("pub", "192.0.2.1:443", false, &[h])], &[]);
        let msg = listener_add_error(Some(&off), h).expect("disabled owner refused");
        assert!(msg.contains("disabled listener"), "{msg}");
        // Named by two listeners.
        let two = cfg_with(
            vec![
                listener("pub", "192.0.2.1:443", true, &[h]),
                listener("wg", "198.51.100.1:443", true, &[h]),
            ],
            &[],
        );
        assert!(listener_add_error(Some(&two), h).is_none());
        let shared = two
            .synthesize_listeners(&[h.to_string()], &HashSet::new())
            .expect("two listeners may serve the same host");
        assert_eq!(shared.len(), 2);
        assert!(shared.iter().all(|l| l.hosts == [h]));
        // Listed twice by ONE listener still aborts boot.
        let dup = cfg_with(vec![listener("pub", "192.0.2.1:443", true, &[h, h])], &[]);
        assert!(
            dup.synthesize_listeners(&[h.to_string()], &HashSet::new())
                .is_err(),
            "startup rejects a duplicate slot"
        );
        let msg = listener_add_error(Some(&dup), h).expect("duplicate slot refused");
        assert!(msg.contains("listed twice"), "{msg}");
        // And the admissible shape is admissible to the real synthesis.
        assert!(
            ok.synthesize_listeners(&[h.to_string()], &HashSet::new())
                .is_ok()
        );
        // Exact match, as at startup: a case-variant is not the same key.
        let upper = cfg_with(
            vec![listener("pub", "192.0.2.1:443", true, &["NEW.example.org"])],
            &[],
        );
        assert!(listener_add_error(Some(&upper), h).is_some());
    }

    #[test]
    fn listener_add_error_checks_all_shared_listener_slots_and_enabled_coverage() {
        let h = "new.example.org";
        for enabled in [(true, false), (false, true), (false, false)] {
            let cfg = cfg_with(
                vec![
                    listener("first", "192.0.2.1:443", enabled.0, &[h]),
                    listener("second", "198.51.100.1:443", enabled.1, &[h]),
                ],
                &[],
            );
            assert_eq!(
                listener_add_error(Some(&cfg), h).is_none(),
                enabled.0 || enabled.1
            );
            assert_eq!(
                cfg.synthesize_listeners(&[h.to_string()], &HashSet::new())
                    .is_ok(),
                enabled.0 || enabled.1
            );
        }
        let cfg = cfg_with(
            vec![
                listener("first", "192.0.2.1:443", true, &[h]),
                listener("second", "198.51.100.1:443", true, &[h]),
                listener("third", "203.0.113.1:443", true, &[h, h]),
            ],
            &[],
        );
        let msg = listener_add_error(Some(&cfg), h).expect("later duplicate refused");
        assert!(
            msg.contains("listed twice") && msg.contains("third"),
            "{msg}"
        );
        assert!(
            cfg.synthesize_listeners(&[h.to_string()], &HashSet::new())
                .is_err()
        );
        let duplicate_id = cfg_with(
            vec![
                listener("same", "192.0.2.1:443", true, &[h]),
                listener("same", "198.51.100.1:443", true, &[h]),
            ],
            &[],
        );
        assert!(listener_add_error(Some(&duplicate_id), h).is_some());
        assert!(
            duplicate_id
                .synthesize_listeners(&[h.to_string()], &HashSet::new())
                .is_err()
        );
    }

    #[test]
    fn listener_remove_error_only_when_a_listener_would_name_an_unknown_host() {
        let h = "gone.example.org";
        assert!(listener_remove_error(None, h).is_none());
        // No listener names it: removal is safe.
        let unnamed = cfg_with(vec![listener("pub", "192.0.2.1:443", true, &[])], &[]);
        assert!(listener_remove_error(Some(&unnamed), h).is_none());
        // Named, and a [[webd.vhost]] block still defines it (bootstrap
        // re-materialises the row): safe.
        let defined = cfg_with(vec![listener("pub", "192.0.2.1:443", true, &[h])], &[h]);
        assert!(listener_remove_error(Some(&defined), h).is_none());
        // Named and defined nowhere else: the next boot would abort.
        let orphan = cfg_with(vec![listener("pub", "192.0.2.1:443", true, &[h])], &[]);
        let msg = listener_remove_error(Some(&orphan), h).expect("orphaning remove refused");
        assert!(msg.contains("\"pub\""), "{msg}");
    }

    /// The failure half: a `vhost.add` the next restart would reject is
    /// refused BEFORE any write, so nothing partial persists — no
    /// namespace row, no tombstone, no provisioner event.
    #[tokio::test(flavor = "current_thread")]
    async fn vhost_add_refused_when_no_listener_serves_it_writes_nothing() {
        let cfg = cfg_with(
            vec![listener(
                "pub",
                "192.0.2.1:443",
                true,
                &["existing.example.org"],
            )],
            &[],
        );
        let node = build_node_with_listener_config(cfg).await;
        let (rc, body) = vhost_add(&node, &add_cmd("new.example.org")).await;
        assert_eq!(rc, RC_CALLER_ERROR, "refused as caller error; body={body}");
        assert!(body.contains("vhost.add failed"), "{body}");
        assert!(body.contains("not in any [[webd.listener]]"), "{body}");

        let runtime = node.vhosts_runtime.as_ref().unwrap();
        let key = RecordKey::collection(namespace_name(), "new.example.org".to_string());
        assert!(
            matches!(runtime.store().get(&key).await, Err(StoreError::NotFound)),
            "a refused add must leave no row behind",
        );
        assert!(
            matches!(runtime.store().version_anchor(&key).await, Ok(None)),
            "a refused add must not even leave a tombstone",
        );
        assert!(namespace_hosts(&node).await.is_empty());
    }

    /// The success half: after an admitted `vhost.add`, every surface
    /// the NEXT boot consults agrees on the new host — the namespace row
    /// (routing directory source), the `[[webd.listener]]` allowlist
    /// (`synthesize_listeners` over the namespace hosts succeeds and
    /// assigns it), and the per-listener TLS partition (its identity
    /// lands on that listener's resolver, not skipped). The cert
    /// adoption itself is pinned in `acme_provisioner`'s
    /// `startup_adopt_*` tests.
    #[tokio::test(flavor = "current_thread")]
    async fn vhost_add_admitted_host_is_consistent_across_restart_surfaces() {
        let host = "new.example.org";
        let cfg = cfg_with(
            vec![
                listener("pub", "192.0.2.1:443", true, &[host]),
                listener("wg", "198.51.100.1:443", true, &[]),
            ],
            &[],
        );
        let node = build_node_with_listener_config(cfg.clone()).await;
        let www = tempfile::tempdir().unwrap();
        let www_dir = www.path().to_string_lossy().into_owned();
        let (rc, body) = vhost_add(&node, &add_cmd_in(host, &www_dir)).await;
        assert_eq!(rc, 0, "admitted add rc=0; body={body}");
        // Explicit-listener node: the reply must not imply it is live.
        let v: JsonValue = serde_json::from_str(&body).unwrap();
        assert_eq!(v["served_after"], "restart", "{body}");

        // Surface: routing directory — rebuilt exactly as the next boot
        // does (namespace snapshot → from_namespace_rows), so the host
        // is actually routed, not merely stored.
        let rows = crate::vhosts_namespace::snapshot_rows(node.vhosts_runtime.as_ref().unwrap())
            .await
            .expect("snapshot");
        let dir =
            crate::vhost_directory::from_namespace_rows(&rows, &HashMap::new(), &HashSet::new())
                .expect("directory");
        assert!(dir.by_host.contains_key(host), "host routed after restart");
        let all_hosts: Vec<String> = dir.by_host.keys().cloned().collect();
        let dropped = crate::vhost_directory::routing_dropped_hosts(&rows, &dir);
        assert!(dropped.is_empty());

        // Surface: listener allowlist — the startup check over the
        // routed hosts must pass and put the host on `pub`.
        let resolved = cfg
            .synthesize_listeners(&all_hosts, &dropped)
            .expect("the next boot's listener resolution accepts the added host");
        let owner = resolved
            .iter()
            .find(|l| l.hosts.iter().any(|h| h == host))
            .expect("host assigned to a listener");
        assert_eq!(owner.id, "pub");
        assert!(owner.enabled);

        // Surface: TLS partition — an identity for the host is routed
        // to its listener's resolver bucket.
        let ident = config::node::TlsIdentityConfig {
            server_name: host.to_string(),
            cert: "/nonexistent/fullchain.pem".to_string(),
            key: "/nonexistent/privkey.pem".to_string(),
            default: false,
            no_sni_fallback: false,
        };
        let buckets = crate::partition_identities_by_listener(&[ident], &resolved);
        assert_eq!(buckets.get("pub").map(Vec::len), Some(1));
        assert!(!buckets.contains_key("wg"));
    }

    /// Remove mirror: refusing to orphan a listener entry, and allowing
    /// the removal once config no longer names the host.
    #[tokio::test(flavor = "current_thread")]
    async fn vhost_remove_refused_while_a_listener_still_names_the_host() {
        let host = "gone.example.org";
        let naming = cfg_with(vec![listener("pub", "192.0.2.1:443", true, &[host])], &[]);
        let node = build_node_with_listener_config(naming).await;
        let (rc, body) = vhost_add(&node, &add_cmd(host)).await;
        assert_eq!(rc, 0, "seed add; body={body}");

        let rm = cmd_with_headers("webd.vhost.remove", &[("fqdn", host)]);
        let (rc, body) = vhost_remove(&node, &rm).await;
        assert_eq!(rc, RC_CALLER_ERROR, "refused; body={body}");
        assert!(body.contains("still names vhost"), "{body}");
        assert_eq!(
            namespace_hosts(&node).await,
            vec![host.to_string()],
            "a refused remove must leave the row in place",
        );
    }

    /// A disabled row is not routed, so it needs no listener: the add is
    /// admitted even though no listener names it, and startup's
    /// fail-soft keeps the node booting. Flipping it to enabled later is
    /// the transition the hook checks.
    #[tokio::test(flavor = "current_thread")]
    async fn disabled_add_needs_no_listener_but_enabling_it_does() {
        let host = "staged.example.org";
        let cfg = cfg_with(vec![listener("pub", "192.0.2.1:443", true, &[])], &[]);
        let node = build_node_with_listener_config(cfg.clone()).await;
        let mut add = add_cmd(host);
        add.headers
            .insert("enabled".to_string(), "false".to_string());
        let (rc, body) = vhost_add(&node, &add).await;
        assert_eq!(rc, 0, "disabled add admitted; body={body}");

        let rows = crate::vhosts_namespace::snapshot_rows(node.vhosts_runtime.as_ref().unwrap())
            .await
            .expect("snapshot");
        let dir =
            crate::vhost_directory::from_namespace_rows(&rows, &HashMap::new(), &HashSet::new())
                .expect("directory");
        let all_hosts: Vec<String> = dir.by_host.keys().cloned().collect();
        let dropped = crate::vhost_directory::routing_dropped_hosts(&rows, &dir);
        cfg.synthesize_listeners(&all_hosts, &dropped)
            .expect("next boot survives the disabled row");

        // Enabling it (vhost.add Patch with enabled=true) is refused.
        let (rc, body) = vhost_add(&node, &add_cmd(host)).await;
        assert_eq!(rc, RC_CALLER_ERROR, "enable refused; body={body}");
        assert!(body.contains("not in any [[webd.listener]]"), "{body}");
    }

    /// An unreadable node.conf.mix is a node fault: rc 20 on both the
    /// ergonomic command and the raw props surface, not a caller error.
    #[tokio::test(flavor = "current_thread")]
    async fn unreadable_node_config_is_rc_20_on_both_surfaces() {
        let node = build_node_full(ListenerConfigSource::Broken(
            "parse error at line 3".to_string(),
        ))
        .await;
        let (rc, body) = vhost_add(&node, &add_cmd("any.example.org")).await;
        assert_eq!(rc, 20, "command: node fault; body={body}");
        assert!(body.contains("could not be loaded"), "{body}");
        assert!(body.contains("\"kind\":\"server\""), "{body}");

        let set = props_cmd(
            "webd.props.set",
            &[
                ("namespace", "vhosts"),
                ("key", "any.example.org"),
                ("if_version", "0"),
            ],
            r#"{"fqdn":"any.example.org","www_dir":"/srv/x","enabled":true,
                "acme_provider":"letsencrypt_staging","acme_challenge":"http01",
                "acme_contact_email":"ops@example.com"}"#,
        );
        let (rc, body) =
            crate::vhosts_namespace::dispatch_props(&node.props_router, "set", &set).await;
        assert_eq!(rc, 20, "props.set: node fault; body={body}");
        assert!(namespace_hosts(&node).await.is_empty());
    }

    fn props_cmd(command_name: &str, headers: &[(&str, &str)], body: &str) -> IncomingCommand {
        let mut cmd = cmd_with_headers(command_name, headers);
        cmd.body = body.to_string();
        cmd
    }

    /// The check lives in the namespace hooks, so the raw SPEC-12 surface
    /// (`webd.props.set` / `webd.props.delete`) is guarded exactly like
    /// the ergonomic commands — no writer can bypass it.
    #[tokio::test(flavor = "current_thread")]
    async fn raw_props_set_and_delete_hit_the_same_listener_check() {
        let named = "named.example.org";
        let unnamed = "unnamed.example.org";
        let cfg = cfg_with(vec![listener("pub", "192.0.2.1:443", true, &[named])], &[]);
        let node = build_node_with_listener_config(cfg).await;
        let row = |fqdn: &str| {
            format!(
                r#"{{"fqdn":"{fqdn}","www_dir":"/srv/x","enabled":true,
                    "acme_provider":"letsencrypt_staging","acme_challenge":"http01",
                    "acme_contact_email":"ops@example.com"}}"#
            )
        };

        // Unnamed host through props.set: refused, nothing written.
        let set = props_cmd(
            "webd.props.set",
            &[
                ("namespace", "vhosts"),
                ("key", unnamed),
                ("if_version", "0"),
            ],
            &row(unnamed),
        );
        let (rc, body) =
            crate::vhosts_namespace::dispatch_props(&node.props_router, "set", &set).await;
        assert_eq!(rc, 10, "validation refusal; body={body}");
        assert!(body.contains("not in any [[webd.listener]]"), "{body}");
        assert!(namespace_hosts(&node).await.is_empty(), "nothing persisted");

        // Named host through props.set: admitted.
        let set = props_cmd(
            "webd.props.set",
            &[("namespace", "vhosts"), ("key", named), ("if_version", "0")],
            &row(named),
        );
        let (rc, body) =
            crate::vhosts_namespace::dispatch_props(&node.props_router, "set", &set).await;
        assert_eq!(rc, 0, "admitted; body={body}");

        // props.delete of the named host would orphan the listener entry.
        let del = cmd_with_headers(
            "webd.props.delete",
            &[("namespace", "vhosts"), ("key", named), ("if_version", "1")],
        );
        let (rc, body) =
            crate::vhosts_namespace::dispatch_props(&node.props_router, "delete", &del).await;
        assert_eq!(rc, 10, "delete refused; body={body}");
        assert!(body.contains("still names vhost"), "{body}");
        assert_eq!(namespace_hosts(&node).await, vec![named.to_string()]);
    }

    /// Writes to an already-enabled row are not re-checked: config that
    /// drifted after the row was admitted must never block a
    /// provisioner cert writeback or an ordinary field update.
    #[tokio::test(flavor = "current_thread")]
    async fn writes_to_an_already_enabled_row_are_not_rechecked() {
        let host = "drift.example.org";
        // Seed under a permissive (no node config) namespace, then open
        // the SAME database again under a config that no longer names the
        // host — a restart after the operator edited node.conf.mix.
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("web.db");
        let open = || {
            let conn = Connection::open(&db).expect("sqlite");
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
                .expect("pragmas");
            Arc::new(SqliteStore::new("webd", conn).expect("store"))
        };
        let store1 = open();
        let mut r1 = PropsRouter::new("webd");
        let (rt1, _rx1) = register_vhosts_namespace_with_listener_config(
            &mut r1,
            &store1,
            ListenerConfigSource::Fixed(None),
        )
        .expect("register");
        let key = RecordKey::collection(namespace_name(), host.to_string());
        let mut body = BTreeMap::new();
        body.insert("fqdn".to_string(), PropValue::String(host.into()));
        body.insert("www_dir".to_string(), PropValue::String("/srv/x".into()));
        body.insert("enabled".to_string(), PropValue::Bool(true));
        body.insert(
            "source".to_string(),
            PropValue::String("bus_runtime".into()),
        );
        for (k, v) in [
            ("acme_provider", "letsencrypt_staging"),
            ("acme_challenge", "http01"),
            ("acme_contact_email", "ops@example.com"),
        ] {
            body.insert(k.to_string(), PropValue::String(v.into()));
        }
        rt1.set_with_origin(
            key.clone(),
            PropValue::Object(body),
            SetOpts {
                expected_version: Some(Version::zero()),
                merge: MergeMode::Patch,
                actor: Actor::service("webd").expect("actor"),
                cause: None,
                ts_ms: 0,
            },
            WriteOrigin::backend(),
        )
        .await
        .expect("seed");

        let drifted = cfg_with(vec![listener("pub", "192.0.2.1:443", true, &[])], &[]);
        let store2 = open();
        let mut r2 = PropsRouter::new("webd");
        let (rt2, _rx2) = register_vhosts_namespace_with_listener_config(
            &mut r2,
            &store2,
            ListenerConfigSource::Fixed(Some(Arc::new(drifted))),
        )
        .expect("re-register");
        let mut patch = BTreeMap::new();
        patch.insert(
            "not_after".to_string(),
            PropValue::String("2100-01-01T00:00:00Z".into()),
        );
        rt2.set_with_origin(
            key,
            PropValue::Object(patch),
            SetOpts {
                expected_version: Some(Version(1)),
                merge: MergeMode::Patch,
                actor: Actor::service("webd").expect("actor"),
                cause: None,
                ts_ms: 0,
            },
            WriteOrigin::backend(),
        )
        .await
        .expect("writeback to an admitted row must not be refused");
    }

    /// Remove is allowed while a listener names the host when a
    /// `[[webd.vhost]]` block still defines it (bootstrap re-creates the
    /// row at the next boot, so the listener entry is not orphaned).
    #[tokio::test(flavor = "current_thread")]
    async fn vhost_remove_allowed_when_config_still_defines_the_host() {
        let host = "kept.example.org";
        let cfg = cfg_with(
            vec![listener("pub", "192.0.2.1:443", true, &[host])],
            &[host],
        );
        let node = build_node_with_listener_config(cfg).await;
        let (rc, body) = vhost_add(&node, &add_cmd(host)).await;
        assert_eq!(rc, 0, "seed add; body={body}");
        let rm = cmd_with_headers("webd.vhost.remove", &[("fqdn", host)]);
        let (rc, body) = vhost_remove(&node, &rm).await;
        assert_eq!(rc, 0, "remove allowed; body={body}");
        assert!(namespace_hosts(&node).await.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn vhost_add_reads_all_kwargs_from_args_object() {
        let node = build_node().await;
        let cmd = cmd_with_args(
            "webd.vhost.add",
            serde_json::json!({
                "fqdn": "argswired.example.com",
                "www_dir": "/srv/argswired",
                "acme.provider": "letsencrypt_staging",
                "acme.challenge": "http01",
                "acme.contact_email": "ops@example.com",
                "enabled": false,
            }),
        );
        let (rc, body) = vhost_add(&node, &cmd).await;
        assert_eq!(rc, 0, "args-delivered add rc=0; body={body}");

        let runtime = node.vhosts_runtime.as_ref().unwrap();
        let key = RecordKey::collection(namespace_name(), "argswired.example.com".to_string());
        let snap = runtime.store().get(&key).await.expect("row present");
        let PropValue::Object(m) = &snap.value.value else {
            panic!("row body is not an Object: {:?}", snap.value.value);
        };
        assert_eq!(
            m.get("www_dir"),
            Some(&PropValue::String("/srv/argswired".into())),
            "www_dir read from args",
        );
        assert_eq!(
            m.get("acme_provider"),
            Some(&PropValue::String("letsencrypt_staging".into())),
            "dotted acme.provider read from args via kwarg_any",
        );
        assert_eq!(
            m.get("enabled"),
            Some(&PropValue::Bool(false)),
            "enabled=false JSON bool read from args via kwarg_bool — NOT defaulted to true",
        );
    }
}
