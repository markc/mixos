// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! JMAP EmailSubmission methods (RFC 8621 §7).

use std::sync::Arc;

use anyhow::Result;
use serde::Deserialize;
use uuid::Uuid;

use super::types::*;
use crate::db::{self, Db};
use mds::ItemId;

use crate::mailstore::{MailStore, MailboxRole, SqliteMailStore};

/// EmailSubmission/set — queue messages for outbound delivery.
pub async fn set(
    db: &Db,
    mailstore: &Arc<SqliteMailStore>,
    aliases_runtime: &Arc<::props::Runtime>,
    account_id: i32,
    args: serde_json::Value,
) -> Result<serde_json::Value> {
    let acct = account_id.to_string();
    // Wontfix-by-design: EmailSubmission state stream stays on
    // db::changelog (mds tracks Mail items only). EmailSubmission
    // is its own JMAP type and has no MailStore-side surface.
    let old_state = db::changelog::current_state(&db.conn, account_id, "EmailSubmission").await?;

    let mut created_map = std::collections::HashMap::new();

    // Handle create
    if let Some(create) = args.get("create").and_then(|v| v.as_object()) {
        for (client_id, obj) in create {
            let legacy_sent_move = ["onSuccessUpdateEmail", "onSuccessDestroyEmail"]
                .iter()
                .all(|key| args.get(key).is_none_or(serde_json::Value::is_null));
            let result = create_submission(
                db,
                mailstore,
                aliases_runtime,
                account_id,
                obj,
                false,
                legacy_sent_move,
            )
            .await;
            match result {
                Ok(submission_id) => {
                    created_map.insert(
                        client_id.clone(),
                        serde_json::json!({
                            "id": submission_id.to_string(),
                            "sendAt": chrono::Utc::now().to_rfc3339(),
                        }),
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "EmailSubmission create failed");
                }
            }
        }
    }

    // Wontfix-by-design: see comment on `old_state` above.
    let new_state = db::changelog::current_state(&db.conn, account_id, "EmailSubmission").await?;

    let resp = SetResponse {
        account_id: acct,
        old_state,
        new_state,
        created: if created_map.is_empty() {
            None
        } else {
            Some(created_map)
        },
        updated: None,
        destroyed: None,
        not_created: None,
        not_updated: None,
        not_destroyed: None,
    };

    Ok(serde_json::to_value(resp)?)
}

/// Create a single email submission — load the email blob and queue it.
pub(crate) async fn create_submission_strict(
    db: &Db,
    mailstore: &Arc<SqliteMailStore>,
    aliases_runtime: &Arc<::props::Runtime>,
    account_id: i32,
    obj: &serde_json::Value,
) -> Result<Uuid> {
    create_submission(db, mailstore, aliases_runtime, account_id, obj, true, true).await
}

async fn create_submission(
    db: &Db,
    mailstore: &Arc<SqliteMailStore>,
    aliases_runtime: &Arc<::props::Runtime>,
    account_id: i32,
    obj: &serde_json::Value,
    _strict_local: bool,
    legacy_sent_move: bool,
) -> Result<Uuid> {
    #[derive(Deserialize)]
    struct SubmissionCreate {
        #[serde(rename = "emailId")]
        email_id: String,
        #[serde(rename = "identityId")]
        _identity_id: Option<String>,
        envelope: Option<Envelope>,
    }

    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "mailFrom")]
        mail_from: EnvelopeAddr,
        #[serde(rename = "rcptTo")]
        rcpt_to: Vec<EnvelopeAddr>,
    }

    #[derive(Deserialize)]
    struct EnvelopeAddr {
        email: String,
    }

    let sub: SubmissionCreate = serde_json::from_value(obj.clone())?;
    let email_uuid: Uuid = sub.email_id.parse()?;

    // : full migration onto MailStore. `get_email` is
    // account-scoped — only the submitter's own emails resolve, so
    // the prior `db::blob::id_owned_by_account` gate is subsumed.
    // CAS hashes are content-addressed (any bytes that hash to `h`
    // *are* the bytes for `h`), so there is no per-account UUID
    // smuggling vector to defend against either; the legacy
    // ownership gate has no analogue in the CAS world.
    let ms_for_get = mailstore.clone();
    let record =
        tokio::task::spawn_blocking(move || ms_for_get.get_email(account_id, ItemId(email_uuid)))
            .await
            .map_err(|e| anyhow::anyhow!("get_email spawn_blocking failed: {e}"))?
            .map_err(|e| anyhow::anyhow!("get_email failed for {}: {e}", sub.email_id))?;

    // Determine envelope
    let (from_addr, to_addrs) = if let Some(env) = sub.envelope {
        (
            env.mail_from.email,
            env.rcpt_to.into_iter().map(|a| a.email).collect(),
        )
    } else {
        // Auto-detect from substrate `EmailEnvelope`. RFC 8621
        // §7.1.2 specifies `rcptTo` is the union of To, Cc, and
        // Bcc, but BCC fan-out is intentionally deferred: the
        // substrate stores the message bytes once, and shipping
        // the same blob to BCC recipients while To/Cc recipients
        // also receive a copy would expose the BCC list (the
        // `Bcc:` header is in the bytes). Proper BCC-aware
        // delivery requires either header stripping for the
        // To/Cc copy or per-recipient blob generation, neither of
        // which is in this task's scope. `EmailEnvelope.bcc` is
        // still populated by `build_envelope` so the
        // data is available for that follow-up work.
        // Empty strings are filtered out so a missing header
        // doesn't enqueue an empty rcpt.
        let from = record.envelope.from.clone();
        let mut tos: Vec<String> = Vec::new();
        for list in [record.envelope.to.as_slice(), record.envelope.cc.as_slice()] {
            for addr in list {
                if !addr.is_empty() {
                    tos.push(addr.clone());
                }
            }
        }
        (from, tos)
    };

    if to_addrs.is_empty() {
        anyhow::bail!("No recipients");
    }

    // RFC 6409 §6.1 — the envelope sender MUST be the authenticated
    // identity (or an enabled alias of it). JMAP submission previously
    // queued ANY envelope sender (an explicit `envelope.mailFrom` or a
    // composed `From:`) with no check, letting an authenticated user
    // spoof another sender — the SMTP submission path enforces this gate
    // (smtp/session.rs MAIL FROM) but JMAP did not. Resolve the account's
    // identity and authorize the sender the same way.
    let authed_email = db::account::get_by_id(&db.conn, account_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("submitting account {account_id} not found"))?
        .email;
    if from_addr.is_empty()
        || !crate::props::aliases::sender_authorized(aliases_runtime, &from_addr, &authed_email)
            .await
    {
        anyhow::bail!("envelope sender {from_addr:?} is not authorized for this account");
    }
    // Reject control chars in any address before they reach the outbound
    // SMTP `MAIL FROM` / `RCPT TO` command line (command injection).
    for addr in std::iter::once(&from_addr).chain(to_addrs.iter()) {
        if addr.chars().any(|c| matches!(c, '\r' | '\n' | '\0')) {
            anyhow::bail!("address contains an illegal control character");
        }
    }

    let blob_hash = record.blob_hash;
    let submission_id = Uuid::new_v4();

    // Separate local vs remote recipients
    let mut remote_addrs = Vec::new();
    let mut recipients_seen = std::collections::BTreeSet::new();
    let mut routes = Vec::new();
    for addr in &to_addrs {
        if recipients_seen.insert(addr) {
            let accounts =
                crate::props::aliases::recipient_accounts(aliases_runtime, &db.conn, addr).await?;
            routes.push((addr.clone(), accounts));
        }
    }
    // One physical copy per mailbox, with a visible outcome for each envelope
    // recipient/target pair. A failed target gets a durable local queue entry.
    let mut local_outcomes =
        std::collections::BTreeMap::<i32, (Option<i64>, String, String)>::new();
    for (addr, accounts) in routes {
        if accounts.is_empty() {
            remote_addrs.push(addr);
            continue;
        }
        for account in accounts {
            let outcome = if let Some(outcome) = local_outcomes.get(&account.id) {
                outcome.clone()
            } else {
                let outcome = match crate::smtp::queue::deliver_local(
                    db, mailstore, account.id, blob_hash, &from_addr, &addr,
                )
                .await
                {
                    Ok(()) => (
                        None,
                        "delivered".to_string(),
                        "250 2.0.0 Local delivery completed".to_string(),
                    ),
                    Err(error) => {
                        let reply = format!("451 4.3.0 Local delivery deferred: {error}");
                        tracing::error!(recipient=%addr,target=%account.email,error=%error,"Local delivery deferred for retry");
                        let queue_id = crate::smtp::queue::enqueue_local(
                            &db.conn,
                            &from_addr,
                            &addr,
                            &account.email,
                            account.id,
                            blob_hash,
                            submission_id,
                            &reply,
                        )
                        .await?;
                        (Some(queue_id), "deferred".to_string(), reply)
                    }
                };
                local_outcomes.insert(account.id, outcome.clone());
                outcome
            };
            crate::smtp::queue::record_local(
                &db.conn,
                submission_id,
                &addr,
                &account.email,
                outcome.0,
                &outcome.1,
                &outcome.2,
            )
            .await?;
        }
    }
    if !remote_addrs.is_empty() {
        let queue_id = crate::smtp::queue::enqueue_submission(
            &db.conn,
            &from_addr,
            &remote_addrs,
            blob_hash,
            submission_id,
        )
        .await?;
        tracing::info!(queue_id,from=%from_addr,to=?remote_addrs,"EmailSubmission queued for remote delivery");
    }

    // Move to Sent mailbox if one exists. The legacy path was
    // `if let Ok(Some(sent_id)) = db::mailbox::get_by_role(...)` then
    // a `let _ =` on the update — both lookup and move were
    // best-effort; remote SMTP enqueue and local delivery have
    // already happened above and a retry would duplicate sends. Treat
    // every step here as best-effort: log and continue on lookup
    // errors, missing role, and `move_email` errors (notably
    // `"ambiguous source"` for multi-mailbox emails — legacy
    // `update_mailboxes` would silently overwrite that case; the
    // multi-mailbox migration moves under).
    if legacy_sent_move {
        let ms = mailstore.clone();
        let move_outcome = tokio::task::spawn_blocking(move || {
            let sent_id = ms.mailbox_by_role(account_id, MailboxRole::Sent)?;
            Ok::<_, anyhow::Error>(
                sent_id.map(|id| ms.move_email(account_id, ItemId(email_uuid), id)),
            )
        })
        .await;
        match move_outcome {
            Ok(Ok(Some(Err(e)))) => {
                tracing::warn!(error = %e, email_id = %email_uuid, "Sent-mailbox move skipped");
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, email_id = %email_uuid, "Sent-mailbox lookup failed");
            }
            Err(e) => {
                tracing::warn!(error = %e, email_id = %email_uuid, "Sent-mailbox spawn_blocking failed");
            }
            Ok(Ok(None)) | Ok(Ok(Some(Ok(())))) => {}
        }
    }

    // Persist submission record
    let to_json = serde_json::to_string(&to_addrs)?;
    let sub_conn = db.conn.clone();
    let sub_id_str = submission_id.to_string();
    let email_id_str = email_uuid.to_string();
    let from_clone = from_addr.clone();
    tokio::task::spawn_blocking(move || {
        let conn = sub_conn.lock().map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        conn.execute(
            "INSERT INTO email_submissions (id, account_id, email_id, envelope_from, envelope_to, undo_status) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'final')",
            rusqlite::params![sub_id_str, account_id, email_id_str, from_clone, to_json],
        )?;
        Ok::<_, anyhow::Error>(())
    }).await??;

    // Wontfix-by-design: EmailSubmission state stream stays on
    // db::changelog (mds tracks Mail items only). Phase 6 cleanup
    // preserves the EmailSubmission-typed changelog channel.
    db::changelog::record(
        &db.conn,
        account_id,
        "EmailSubmission",
        submission_id,
        "created",
    )
    .await?;

    Ok(submission_id)
}

/// EmailSubmission/get — query submission status.
pub async fn get(db: &Db, account_id: i32, args: serde_json::Value) -> Result<serde_json::Value> {
    let acct = account_id.to_string();

    let ids: Option<Vec<String>> = args
        .get("ids")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let conn = db.conn.clone();
    let submissions = tokio::task::spawn_blocking(move || {
        let conn = conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let mut results = Vec::new();

        if let Some(ids) = ids {
            for id in &ids {
                let result: rusqlite::Result<(String, String, String, String, String, String)> =
                    conn.query_row(
                        "SELECT id, email_id, envelope_from, envelope_to, send_at, undo_status \
                     FROM email_submissions WHERE id = ?1 AND account_id = ?2",
                        rusqlite::params![id, account_id],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                            ))
                        },
                    );
                if let Ok((id, email_id, from, to_json, send_at, undo_status)) = result {
                    let to_addrs: Vec<String> = serde_json::from_str(&to_json).unwrap_or_default();
                    let rcpt_to: Vec<serde_json::Value> = to_addrs
                        .iter()
                        .map(|e| serde_json::json!({"email": e}))
                        .collect();
                    results.push(serde_json::json!({
                        "id": id,
                        "emailId": email_id,
                        "envelope": {
                            "mailFrom": {"email": from},
                            "rcptTo": rcpt_to,
                        },
                        "sendAt": send_at,
                        "undoStatus": undo_status,
                        "deliveryStatus": {},
                    }));
                }
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, email_id, envelope_from, envelope_to, send_at, undo_status \
                 FROM email_submissions WHERE account_id = ?1 ORDER BY created_at DESC LIMIT 100",
            )?;
            let rows = stmt.query_map(rusqlite::params![account_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?;
            for row in rows {
                let (id, email_id, from, to_json, send_at, undo_status) = row?;
                let to_addrs: Vec<String> = serde_json::from_str(&to_json).unwrap_or_default();
                let rcpt_to: Vec<serde_json::Value> = to_addrs
                    .iter()
                    .map(|e| serde_json::json!({"email": e}))
                    .collect();
                results.push(serde_json::json!({
                    "id": id,
                    "emailId": email_id,
                    "envelope": {
                        "mailFrom": {"email": from},
                        "rcptTo": rcpt_to,
                    },
                    "sendAt": send_at,
                    "undoStatus": undo_status,
                    "deliveryStatus": {},
                }));
            }
        }
        Ok::<_, anyhow::Error>(results)
    })
    .await??;

    // Wontfix-by-design: EmailSubmission state stream stays on
    // db::changelog (mds tracks Mail items only).
    let state = db::changelog::current_state(&db.conn, account_id, "EmailSubmission").await?;

    Ok(serde_json::json!({
        "accountId": acct,
        "state": state,
        "list": submissions,
        "notFound": [],
    }))
}

/// EmailSubmission/changes — returns changes since a given state.
pub async fn changes(
    db: &Db,
    account_id: i32,
    args: serde_json::Value,
) -> Result<serde_json::Value> {
    let acct = account_id.to_string();
    let since_state = args
        .get("sinceState")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);

    let max = args
        .get("maxChanges")
        .and_then(|v| v.as_i64())
        .unwrap_or(500);

    // Wontfix-by-design: EmailSubmission state stream stays on
    // db::changelog (mds tracks Mail items only).
    let result =
        db::changelog::changes_since(&db.conn, account_id, "EmailSubmission", since_state, max)
            .await?;

    let resp = ChangesResponse {
        account_id: acct,
        old_state: since_state.to_string(),
        new_state: result.new_state,
        has_more_changes: result.has_more_changes,
        created: result.created,
        updated: result.updated,
        destroyed: result.destroyed,
        updated_properties: None,
    };

    Ok(serde_json::to_value(resp)?)
}

/// Identity/get — list the account and its authorized sender aliases.
pub async fn identity_get(
    db: &Db,
    domains_runtime: std::sync::Arc<::props::Runtime>,
    aliases_runtime: &Arc<::props::Runtime>,
    account_id: i32,
    args: serde_json::Value,
) -> Result<serde_json::Value> {
    let mut response = account_identity_get(db, domains_runtime, account_id, args).await?;
    let Some(primary) = response["list"]
        .as_array()
        .and_then(|list| list.first())
        .cloned()
    else {
        return Ok(response);
    };
    let account = crate::props::aliases::canon(primary["email"].as_str().unwrap_or_default());
    let snapshot = match aliases_runtime
        .store()
        .list(&crate::props::aliases::namespace_name())
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            // Preserve the account identity on a store fault, but never
            // advertise an alias whose authorization cannot be checked.
            tracing::warn!(%error, "Identity/get could not list sender aliases");
            return Ok(response);
        }
    };
    let mut addresses = std::collections::BTreeSet::new();
    for record in snapshot.value {
        let address = crate::props::aliases::canon(&record.key.key);
        if address != account
            && crate::props::aliases::sender_authorized(aliases_runtime, &address, &account).await
        {
            addresses.insert(address);
        }
    }
    for address in addresses {
        let mut identity = primary.clone();
        // JMAP ids use URL-safe characters and must stay short even for
        // long email addresses. Keep the primary account's existing id.
        identity["id"] = serde_json::json!(format!(
            "{account_id}-alias-{}",
            blake3::hash(address.as_bytes()).to_hex()
        ));
        identity["email"] = serde_json::json!(address);
        response["list"]
            .as_array_mut()
            .expect("primary identity list")
            .push(identity);
    }
    response["state"] = serde_json::json!(
        blake3::hash(serde_json::to_string(&response["list"])?.as_bytes())
            .to_hex()
            .to_string()
    );
    Ok(response)
}

/// The account's primary identity under the domain availability policy.
///
/// Phase 3: cross-product the account's row against the
/// `maild.domains` substrate. The account-domain must be present and
/// `enabled = true` for the identity to surface; if the namespace is
/// empty or the substrate read errors, fall back to the legacy
/// single-identity behavior so an Identity/get failure mid-session
/// can't lock the user out of submission UI.
async fn account_identity_get(
    db: &Db,
    domains_runtime: std::sync::Arc<::props::Runtime>,
    account_id: i32,
    _args: serde_json::Value,
) -> Result<serde_json::Value> {
    let acct = account_id.to_string();

    let account = db::account::get_by_id(&db.conn, account_id).await?;
    let Some(a) = account else {
        return Ok(serde_json::json!({
            "accountId": acct,
            "state": "0",
            "list": [],
            "notFound": [],
        }));
    };

    fn identity_json(a: &crate::db::account::Account) -> serde_json::Value {
        serde_json::json!({
            "id": a.id.to_string(),
            "name": a.name.clone().unwrap_or_default(),
            "email": a.email,
            "replyTo": null,
            "bcc": null,
            "textSignature": "",
            "htmlSignature": "",
            "mayDelete": false,
        })
    }

    let legacy_single = || vec![identity_json(&a)];

    let account_domain = match a.email.rsplit_once('@') {
        Some((_, d)) if !d.is_empty() => d.to_ascii_lowercase(),
        _ => {
            // Account email without an `@` — defensive; treat as
            // legacy fallback rather than block submission.
            return Ok(serde_json::json!({
                "accountId": acct,
                "state": "0",
                "list": legacy_single(),
                "notFound": [],
            }));
        }
    };

    let list_result = domains_runtime
        .store()
        .list(&crate::props::domains::namespace_name())
        .await;
    let namespace_empty = match list_result {
        Ok(snap) => snap.value.is_empty(),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "maild.domains list error in identity_get — falling back to legacy single identity"
            );
            return Ok(serde_json::json!({
                "accountId": acct,
                "state": "0",
                "list": legacy_single(),
                "notFound": [],
            }));
        }
    };

    if namespace_empty {
        return Ok(serde_json::json!({
            "accountId": acct,
            "state": "0",
            "list": legacy_single(),
            "notFound": [],
        }));
    }

    let key = ::props::RecordKey::collection(
        crate::props::domains::namespace_name(),
        account_domain.clone(),
    );
    let record_result = domains_runtime.store().get(&key).await;

    let list = match record_result {
        Ok(snap) => match crate::props::domains::view(&snap.value) {
            Ok(view) if view.enabled => vec![identity_json(&a)],
            Ok(_) => vec![],
            Err(e) => {
                tracing::warn!(
                    domain = %account_domain,
                    error = %e,
                    "maild.domains view projection failed in identity_get — falling back to legacy single identity"
                );
                legacy_single()
            }
        },
        Err(::props::store::StoreError::NotFound) => vec![],
        Err(e) => {
            tracing::warn!(
                domain = %account_domain,
                error = %e,
                "maild.domains get error in identity_get — falling back to legacy single identity"
            );
            legacy_single()
        }
    };

    Ok(serde_json::json!({
        "accountId": acct,
        "state": "0",
        "list": list,
        "notFound": [],
    }))
}

#[cfg(test)]
mod identity_get_tests {
    //! Phase 3 commit 3 — `identity_get` × `maild.domains` matrix:
    //!
    //!   row enabled=true   → one identity
    //!   row enabled=false  → []
    //!   row absent (namespace non-empty) → []
    //!   namespace empty    → legacy single identity (back-compat)
    //!   substrate error    → legacy single identity (best-effort)
    use super::*;
    use crate::db::Db;
    use ::props::namespace::NamespaceName;
    use ::props::record::{AuditEpoch, Nseq, Record, RecordEvent};
    use ::props::store::{
        CompleteCommit, DeleteCommit, PropertyStore, ReconcileCommit, SetCommit, Snapshot,
        StoreError, StoreFuture,
    };
    use ::props::{
        Actor, Hooks, MemoryStore, MergeMode, RecordKey, Runtime, SetOpts, Version,
        value::PropValue,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn build_runtime() -> Arc<Runtime> {
        let store: Arc<dyn PropertyStore> = Arc::new(MemoryStore::new("maild"));
        Arc::new(Runtime::new(
            "maild",
            crate::props::domains::spec(Hooks::new(crate::props::domains::DomainsHooks::new())),
            store,
        ))
    }

    fn build_aliases_runtime() -> Arc<Runtime> {
        Arc::new(Runtime::new(
            "maild",
            crate::props::aliases::spec(Hooks::noop()),
            Arc::new(MemoryStore::new("maild")),
        ))
    }

    async fn seed_alias(runtime: &Arc<Runtime>, address: &str, target: &str, enabled: bool) {
        let value = PropValue::Object(std::collections::BTreeMap::from([
            ("address".into(), PropValue::String(address.into())),
            ("target".into(), PropValue::String(target.into())),
            ("enabled".into(), PropValue::Bool(enabled)),
        ]));
        runtime
            .set(
                RecordKey::collection(crate::props::aliases::namespace_name(), address.to_string()),
                value,
                SetOpts {
                    expected_version: Some(Version::zero()),
                    merge: MergeMode::Replace,
                    actor: Actor::service("identity-tests").expect("valid actor"),
                    cause: None,
                    ts_ms: 0,
                },
            )
            .await
            .expect("seed alias");
    }

    async fn seed(runtime: &Arc<Runtime>, domain: &str, mutate: impl FnOnce(&mut PropValue)) {
        let mut value = crate::props::domains::defaults_value(domain);
        mutate(&mut value);
        runtime
            .set(
                RecordKey::collection(crate::props::domains::namespace_name(), domain.to_string()),
                value,
                SetOpts {
                    expected_version: Some(Version::zero()),
                    merge: MergeMode::Replace,
                    actor: Actor::service("phase3-identity-tests").expect("valid actor"),
                    cause: None,
                    ts_ms: 0,
                },
            )
            .await
            .expect("seed row");
    }

    struct DbFixture {
        db: Db,
        _tmp: tempfile::TempDir,
    }

    async fn make_db_fixture() -> DbFixture {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("maild.sqlite");
        let blob_dir = tmp.path().join("blobs");
        let db = Db::connect(db_path.to_str().unwrap(), blob_dir.to_str().unwrap())
            .await
            .expect("Db::connect");
        db.migrate().await.expect("migrate");
        DbFixture { db, _tmp: tmp }
    }

    fn seed_account(db: &Db, email: &str) -> i32 {
        let conn = db.conn.lock().expect("lock db");
        let hash = bcrypt::hash("changeme", bcrypt::DEFAULT_COST).expect("hash");
        conn.execute(
            "INSERT INTO accounts (email, password, name) VALUES (?, ?, ?)",
            rusqlite::params![email, hash, "Test"],
        )
        .expect("insert account");
        conn.last_insert_rowid() as i32
    }

    fn list_count(v: &serde_json::Value) -> usize {
        v.get("list")
            .and_then(|l| l.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn returns_identity_when_domain_enabled() {
        let rt = build_runtime();
        let fx = make_db_fixture().await;
        let id = seed_account(&fx.db, "user@example.com");
        seed(&rt, "example.com", |_| {}).await; // defaults_value: enabled=true
        let out = identity_get(
            &fx.db,
            rt,
            &build_aliases_runtime(),
            id,
            serde_json::json!({}),
        )
        .await
        .unwrap();
        assert_eq!(list_count(&out), 1);
    }

    #[tokio::test]
    async fn returns_empty_when_domain_disabled() {
        let rt = build_runtime();
        let fx = make_db_fixture().await;
        let id = seed_account(&fx.db, "user@example.com");
        seed(&rt, "example.com", |v| {
            if let PropValue::Object(o) = v {
                o.insert("enabled".into(), PropValue::Bool(false));
            }
        })
        .await;
        let aliases = build_aliases_runtime();
        seed_alias(&aliases, "mc@example.com", "user@example.com", true).await;
        let out = identity_get(&fx.db, rt, &aliases, id, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(list_count(&out), 0);
    }

    #[tokio::test]
    async fn returns_empty_when_domain_not_found() {
        let rt = build_runtime();
        let fx = make_db_fixture().await;
        let id = seed_account(&fx.db, "user@example.com");
        // Seed a *different* domain so the namespace is non-empty but
        // the account's domain is absent.
        seed(&rt, "other.example", |_| {}).await;
        let out = identity_get(
            &fx.db,
            rt,
            &build_aliases_runtime(),
            id,
            serde_json::json!({}),
        )
        .await
        .unwrap();
        assert_eq!(list_count(&out), 0);
    }

    #[tokio::test]
    async fn falls_back_to_legacy_when_namespace_empty() {
        let rt = build_runtime();
        let fx = make_db_fixture().await;
        let id = seed_account(&fx.db, "user@example.com");
        // Don't seed any rows — namespace is empty.
        let out = identity_get(
            &fx.db,
            rt,
            &build_aliases_runtime(),
            id,
            serde_json::json!({}),
        )
        .await
        .unwrap();
        assert_eq!(list_count(&out), 1);
    }

    // -- FaultyStore mirror for the substrate-error best-effort path.

    struct FaultyStore {
        inner: Arc<dyn PropertyStore>,
        fail_get: AtomicBool,
        fail_list: AtomicBool,
    }
    impl FaultyStore {
        fn new(inner: Arc<dyn PropertyStore>) -> Arc<Self> {
            Arc::new(Self {
                inner,
                fail_get: AtomicBool::new(false),
                fail_list: AtomicBool::new(false),
            })
        }
        fn set_fail_get(&self, v: bool) {
            self.fail_get.store(v, Ordering::SeqCst);
        }
    }
    impl PropertyStore for FaultyStore {
        fn get<'a>(&'a self, key: &'a RecordKey) -> StoreFuture<'a, Snapshot<Record>> {
            if self.fail_get.load(Ordering::SeqCst) {
                return Box::pin(async {
                    Err(StoreError::storage("FaultyStore: injected get error"))
                });
            }
            self.inner.get(key)
        }
        fn list<'a>(
            &'a self,
            namespace: &'a NamespaceName,
        ) -> StoreFuture<'a, Snapshot<Vec<Record>>> {
            if self.fail_list.load(Ordering::SeqCst) {
                return Box::pin(async {
                    Err(StoreError::storage("FaultyStore: injected list error"))
                });
            }
            self.inner.list(namespace)
        }
        fn commit_set<'a>(&'a self, op: SetCommit) -> StoreFuture<'a, (Record, RecordEvent)> {
            self.inner.commit_set(op)
        }
        fn commit_delete<'a>(&'a self, op: DeleteCommit) -> StoreFuture<'a, RecordEvent> {
            self.inner.commit_delete(op)
        }
        fn commit_complete<'a>(
            &'a self,
            op: CompleteCommit,
        ) -> StoreFuture<'a, (Record, RecordEvent)> {
            self.inner.commit_complete(op)
        }
        fn commit_reconcile<'a>(
            &'a self,
            op: ReconcileCommit,
        ) -> StoreFuture<'a, (Record, RecordEvent)> {
            self.inner.commit_reconcile(op)
        }
        fn events_since<'a>(
            &'a self,
            namespace: &'a NamespaceName,
            since_nseq: Nseq,
        ) -> StoreFuture<'a, Vec<RecordEvent>> {
            self.inner.events_since(namespace, since_nseq)
        }
        fn audit_epoch<'a>(&'a self, namespace: &'a NamespaceName) -> StoreFuture<'a, AuditEpoch> {
            self.inner.audit_epoch(namespace)
        }
        fn version_anchor<'a>(
            &'a self,
            key: &'a RecordKey,
        ) -> StoreFuture<'a, Option<::props::Version>> {
            self.inner.version_anchor(key)
        }
    }

    #[tokio::test]
    async fn falls_back_to_legacy_on_substrate_get_error() {
        let inner: Arc<dyn PropertyStore> = Arc::new(MemoryStore::new("maild"));
        let faulty = FaultyStore::new(inner);
        let runtime = Arc::new(Runtime::new(
            "maild",
            crate::props::domains::spec(Hooks::new(crate::props::domains::DomainsHooks::new())),
            faulty.clone() as Arc<dyn PropertyStore>,
        ));
        // Seed first so the namespace is non-empty (list succeeds);
        // then flip the get fault.
        seed(&runtime, "example.com", |_| {}).await;
        faulty.set_fail_get(true);

        let fx = make_db_fixture().await;
        let id = seed_account(&fx.db, "user@example.com");
        let out = identity_get(
            &fx.db,
            runtime,
            &build_aliases_runtime(),
            id,
            serde_json::json!({}),
        )
        .await
        .unwrap();
        // Best-effort: legacy single identity preserves submission UI.
        assert_eq!(list_count(&out), 1);
    }

    #[tokio::test]
    async fn advertises_only_enabled_aliases_for_this_account_with_stable_ids() {
        let domains = build_runtime();
        let aliases = build_aliases_runtime();
        let fx = make_db_fixture().await;
        let id = seed_account(&fx.db, "user@example.com");
        seed(&domains, "example.com", |_| {}).await;
        let before = identity_get(&fx.db, domains.clone(), &aliases, id, serde_json::json!({}))
            .await
            .unwrap();
        seed_alias(&aliases, "mc@example.com", "USER@example.com", true).await;
        seed_alias(&aliases, "off@example.com", "user@example.com", false).await;
        seed_alias(&aliases, "other@example.com", "other@example.com", true).await;
        let out = identity_get(&fx.db, domains.clone(), &aliases, id, serde_json::json!({}))
            .await
            .unwrap();
        let repeated = identity_get(&fx.db, domains, &aliases, id, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(out, repeated);
        assert_eq!(list_count(&out), 2);
        assert_eq!(out["list"][0]["id"], id.to_string());
        assert_eq!(out["list"][0]["email"], "user@example.com");
        assert_eq!(out["list"][1]["email"], "mc@example.com");
        assert_ne!(out["list"][1]["id"], out["list"][0]["id"]);
        assert_ne!(out["state"], before["state"]);
        let alias_id = out["list"][1]["id"].as_str().unwrap();
        assert!(alias_id.len() <= 255);
        assert!(
            alias_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        // Every advertised address passes the same gate as EmailSubmission/set.
        for identity in out["list"].as_array().unwrap() {
            assert!(
                crate::props::aliases::sender_authorized(
                    &aliases,
                    identity["email"].as_str().unwrap(),
                    "user@example.com"
                )
                .await
            );
        }
    }

    #[tokio::test]
    async fn alias_store_faults_never_advertise_an_unverified_sender() {
        let domains = build_runtime();
        let faulty = FaultyStore::new(Arc::new(MemoryStore::new("maild")));
        let aliases = Arc::new(Runtime::new(
            "maild",
            crate::props::aliases::spec(Hooks::noop()),
            faulty.clone() as Arc<dyn PropertyStore>,
        ));
        let fx = make_db_fixture().await;
        let id = seed_account(&fx.db, "user@example.com");
        seed(&domains, "example.com", |_| {}).await;
        seed_alias(&aliases, "mc@example.com", "user@example.com", true).await;
        for fail_list in [false, true] {
            faulty.set_fail_get(true);
            faulty.fail_list.store(fail_list, Ordering::SeqCst);
            let out = identity_get(&fx.db, domains.clone(), &aliases, id, serde_json::json!({}))
                .await
                .unwrap();
            assert_eq!(list_count(&out), 1);
            assert_eq!(out["list"][0]["email"], "user@example.com");
        }
    }
}
