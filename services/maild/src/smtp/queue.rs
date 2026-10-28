// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Outbound SMTP queue — SQLite-backed message queue.
//!
//! : queue rows now carry `blob_hash` (hex-encoded
//! `mds::BlobHash`) into the shared CAS. Pre-Task-5.4b rows
//! still carry the legacy `blob_id` UUID; the delivery worker prefers
//! `blob_hash` and falls back to `blob_id` so in-flight entries keep
//! draining across the upgrade.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use mds::BlobHash;
use rusqlite::{Connection, params};
use uuid::Uuid;

#[derive(Debug)]
pub struct QueueEntry {
    pub id: i64,
    pub from_addr: String,
    pub to_addrs: Vec<String>,
    /// Legacy per-account UUID. `Some` only for pre-Task-5.4b rows
    /// that still need `db::blob::load` to fetch bytes.
    pub blob_id: Option<Uuid>,
    /// CAS hash. `Some` for all + rows; the delivery worker
    /// uses this in preference to `blob_id`.
    pub blob_hash: Option<BlobHash>,
    pub attempts: i32,
    pub next_retry: String,
    pub last_error: Option<String>,
    pub local_account_id: Option<i32>,
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueEntry> {
    let to_addrs_json: String = row.get(2)?;
    let to_addrs: Vec<String> = serde_json::from_str(&to_addrs_json).unwrap_or_default();
    let blob_id_str: Option<String> = row.get(3)?;
    let blob_id = blob_id_str
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(|s| Uuid::parse_str(s).ok());
    let blob_hash_hex: Option<String> = row.get(4)?;
    let blob_hash = blob_hash_hex
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(mds::blob::from_hex);

    Ok(QueueEntry {
        id: row.get(0)?,
        from_addr: row.get(1)?,
        to_addrs,
        blob_id,
        blob_hash,
        attempts: row.get(5)?,
        next_retry: row.get(6)?,
        last_error: row.get(7)?,
        local_account_id: row.get(8)?,
    })
}

const SELECT_COLS: &str = "id, from_addr, to_addrs, blob_id, blob_hash, attempts, next_retry, last_error, local_account_id";

/// Enqueue a message for outbound delivery, addressed by CAS hash.
pub async fn enqueue(
    conn: &Arc<Mutex<Connection>>,
    from_addr: &str,
    to_addrs: &[String],
    blob_hash: BlobHash,
) -> Result<i64> {
    enqueue_entry(conn, from_addr, to_addrs, blob_hash, None, None, None).await
}

pub async fn enqueue_submission(
    conn: &Arc<Mutex<Connection>>,
    from_addr: &str,
    to_addrs: &[String],
    blob_hash: BlobHash,
    submission: Uuid,
) -> Result<i64> {
    enqueue_entry(
        conn,
        from_addr,
        to_addrs,
        blob_hash,
        Some(submission),
        None,
        None,
    )
    .await
}

pub(crate) async fn enqueue_local(
    conn: &Arc<Mutex<Connection>>,
    from_addr: &str,
    recipient: &str,
    target: &str,
    account_id: i32,
    blob_hash: BlobHash,
    submission: Uuid,
    error: &str,
) -> Result<i64> {
    enqueue_entry(
        conn,
        from_addr,
        &[recipient.to_string()],
        blob_hash,
        Some(submission),
        Some((account_id, target.to_string())),
        Some(error.to_string()),
    )
    .await
}

async fn enqueue_entry(
    conn: &Arc<Mutex<Connection>>,
    from_addr: &str,
    to_addrs: &[String],
    blob_hash: BlobHash,
    submission: Option<Uuid>,
    local: Option<(i32, String)>,
    error: Option<String>,
) -> Result<i64> {
    let conn = conn.clone();
    let from_addr = from_addr.to_string();
    let to_addrs_json = serde_json::to_string(to_addrs)?;
    let blob_hash_hex = mds::blob::hex(&blob_hash);
    tokio::task::spawn_blocking(move || {
        let mut conn = conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO smtp_queue (from_addr,to_addrs,blob_hash,local_account_id,next_retry,last_error,attempts) VALUES (?1,?2,?3,?4,CASE WHEN ?5 IS NULL THEN datetime('now') ELSE datetime('now','+1 minute') END,?5,CASE WHEN ?5 IS NULL THEN 0 ELSE 1 END)",
            params![from_addr, to_addrs_json, blob_hash_hex,local.as_ref().map(|l| l.0),error],
        )?;
        let id = tx.last_insert_rowid();
        for recipient in serde_json::from_str::<Vec<String>>(&to_addrs_json)? {
            tx.execute(
                "INSERT INTO smtp_delivery(queue_id,submission_id,recipient,target,status,last_reply) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(queue_id,recipient,target) DO NOTHING",
                params![id,submission.map(|s| s.to_string()),recipient,local.as_ref().map(|l| l.1.as_str()).unwrap_or(""),if error.is_some() {"deferred"} else {"queued"},error],
            )?;
        }
        tx.commit()?;
        // Key values inline: the journald sink emits structured fields
        // as journal FIELDS, invisible in default `journalctl` output.
        tracing::info!(
            queue_id = id,
            from = from_addr.as_str(),
            "Message queued for delivery: queue_id={id} from=<{}>",
            crate::maillog::sanitize(&from_addr),
        );
        Ok(id)
    })
    .await?
}

/// Native queue insert for callers that must atomically commit related mail.db
/// state. The caller owns the transaction and reports success after commit.
pub(crate) fn enqueue_on(
    conn: &Connection,
    from: &str,
    to: &[String],
    hash: BlobHash,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO smtp_queue (from_addr,to_addrs,blob_hash) VALUES(?1,?2,?3)",
        params![from, serde_json::to_string(to)?, mds::blob::hex(&hash)],
    )?;
    let id = conn.last_insert_rowid();
    for recipient in to {
        conn.execute("INSERT OR IGNORE INTO smtp_delivery(queue_id,recipient) VALUES(?1,?2)", params![id, recipient])?;
    }
    Ok(id)
}

/// Recipients still owed this message. Terminal results are never retried.
pub async fn pending(conn: &Arc<Mutex<Connection>>, id: i64) -> Result<Vec<String>> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let mut stmt = conn.prepare("SELECT DISTINCT recipient FROM smtp_delivery WHERE queue_id=?1 AND status IN ('queued','deferred') ORDER BY rowid")?;
        Ok(stmt.query_map([id], |row| row.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }).await?
}

pub async fn recipient_result(conn: &Arc<Mutex<Connection>>, id: i64, recipient: &str, status: &str, reply: &str) -> Result<()> {
    let conn = conn.clone();
    let recipient = recipient.to_string();
    let status = status.to_string();
    let reply = reply.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        conn.execute("UPDATE smtp_delivery SET status=?3,last_reply=?4,updated_at=datetime('now') WHERE queue_id=?1 AND recipient=?2 AND status IN ('queued','deferred')", params![id, recipient, status, reply])?;
        Ok(())
    }).await?
}

/// Fetch messages ready for delivery (next_retry <= now, attempts < 10).
pub async fn fetch_ready(conn: &Arc<Mutex<Connection>>, limit: i64) -> Result<Vec<QueueEntry>> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM smtp_queue \
             WHERE next_retry <= datetime('now') AND attempts < 10 \
             ORDER BY next_retry LIMIT ?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit], row_to_entry)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    })
    .await?
}

/// Mark a queue entry as successfully delivered (delete it).
pub async fn mark_delivered(conn: &Arc<Mutex<Connection>>, id: i64) -> Result<()> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM smtp_delivery WHERE queue_id=?1 AND submission_id IS NULL",
            [id],
        )?;
        tx.execute("DELETE FROM smtp_queue WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    })
    .await?
}

/// Mark a queue entry as failed — increment attempts, set next retry with backoff.
pub async fn mark_failed(conn: &Arc<Mutex<Connection>>, id: i64, error: &str) -> Result<()> {
    retry(conn, id, Some(error)).await
}

/// Schedule another pass without overwriting the individual SMTP replies.
pub async fn retry_pending(conn: &Arc<Mutex<Connection>>, id: i64) -> Result<()> {
    retry(conn, id, None).await
}

async fn retry(conn: &Arc<Mutex<Connection>>, id: i64, error: Option<&str>) -> Result<()> {
    let conn = conn.clone();
    let error = error.map(str::to_string);
    tokio::task::spawn_blocking(move || {
        let conn = conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        // Exponential backoff: multiply by power of 3
        if let Some(error) = &error {
            conn.execute("UPDATE smtp_delivery SET status='deferred',last_reply=?2,updated_at=datetime('now') WHERE queue_id=?1 AND status IN ('queued','deferred')", params![id,error])?;
        }
        // SQLite doesn't have POWER, so we compute the interval in Rust
        let attempts: i32 = conn
            .query_row(
                "SELECT attempts FROM smtp_queue WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let backoff_minutes = 3i64.pow(attempts.min(8) as u32);
        conn.execute(
            "UPDATE smtp_queue SET \
             attempts = attempts + 1, \
             last_error = ?2, \
             next_retry = datetime('now', '+' || ?3 || ' minutes') \
             WHERE id = ?1",
            params![id, error.unwrap_or_else(|| "one or more recipients deferred".into()), backoff_minutes],
        )?;
        Ok(())
    })
    .await?
}

/// Mark a permanently failed entry (attempts >= 10) — remove from queue.
pub async fn mark_permanent_failure(
    conn: &Arc<Mutex<Connection>>,
    id: i64,
    error: &str,
) -> Result<()> {
    tracing::error!(
        queue_id = id,
        error = error,
        "Permanent delivery failure — removing from queue"
    );
    let conn = conn.clone();
    let error = error.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        tx.execute("UPDATE smtp_delivery SET status='bounced',last_reply=?2,updated_at=datetime('now') WHERE queue_id=?1 AND status IN ('queued','deferred')", params![id,error])?;
        tx.execute("DELETE FROM smtp_delivery WHERE queue_id=?1 AND submission_id IS NULL", [id])?;
        tx.execute("DELETE FROM smtp_queue WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    })
    .await?
}

/// List queue entries for admin inspection.
pub async fn list(conn: &Arc<Mutex<Connection>>, limit: i64) -> Result<Vec<QueueEntry>> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let sql = format!("SELECT {SELECT_COLS} FROM smtp_queue ORDER BY id LIMIT ?1");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit], row_to_entry)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    })
    .await?
}

/// Flush (retry now) all queued messages.
pub async fn flush(conn: &Arc<Mutex<Connection>>) -> Result<u64> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let changes = conn.execute(
            "UPDATE smtp_queue SET next_retry = datetime('now') WHERE attempts < 10",
            [],
        )?;
        Ok(changes as u64)
    })
    .await?
}

pub(crate) async fn record_local(
    conn: &Arc<Mutex<Connection>>,
    submission: Uuid,
    recipient: &str,
    target: &str,
    queue_id: Option<i64>,
    status: &str,
    reply: &str,
) -> Result<()> {
    let conn = conn.clone();
    let (recipient, target, status, reply) = (
        recipient.to_string(),
        target.to_string(),
        status.to_string(),
        reply.to_string(),
    );
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        conn.execute("INSERT INTO smtp_delivery(queue_id,submission_id,recipient,target,status,last_reply) VALUES(?1,?2,?3,?4,COALESCE((SELECT status FROM smtp_delivery WHERE queue_id=?1 LIMIT 1),?5),COALESCE((SELECT last_reply FROM smtp_delivery WHERE queue_id=?1 LIMIT 1),?6)) ON CONFLICT(submission_id,recipient,target) DO NOTHING",
            params![queue_id,submission.to_string(),recipient,target,status,reply])?;
        Ok(())
    }).await?
}

/// The same local delivery operation serves initial submission and queue retries.
pub(crate) async fn deliver_local(
    store: &Arc<crate::mailstore::SqliteMailStore>,
    account_id: i32,
    blob_hash: BlobHash,
) -> Result<()> {
    use crate::mailstore::{MailStore, MailboxRole};
    use mds::{Flags, Mds, Tags};
    let store = store.clone();
    tokio::task::spawn_blocking(move || {
        let data = store.mds().get_blob(&blob_hash)?;
        let inbox = store
            .mailbox_by_role(account_id, MailboxRole::Inbox)?
            .ok_or_else(|| anyhow::anyhow!("recipient {account_id} has no Inbox"))?;
        let parsed = mail_parser::MessageParser::default()
            .parse(data.as_slice())
            .ok_or_else(|| anyhow::anyhow!("failed to parse message for envelope"))?;
        store.create_email(
            account_id,
            inbox,
            blob_hash,
            crate::jmap::email::build_envelope(&parsed),
            &crate::jmap::email::extract_in_reply_to(&parsed),
            Flags(0),
            Tags::new(),
            chrono::Utc::now().timestamp_millis(),
        )?;
        Ok(())
    })
    .await?
}

pub(crate) async fn local_result(
    conn: &Arc<Mutex<Connection>>,
    id: i64,
    status: &str,
    reply: &str,
) -> Result<()> {
    let conn = conn.clone();
    let (status, reply) = (status.to_string(), reply.to_string());
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        conn.execute("UPDATE smtp_delivery SET status=?2,last_reply=?3,updated_at=datetime('now') WHERE queue_id=?1 AND status IN ('queued','deferred')",params![id,status,reply])?;
        Ok(())
    }).await?
}

/// Keep complete active receipts; expire terminal submissions as a whole.
/// Zero disables age-based retention. Unlinked SMTP outcomes are ephemeral.
pub async fn sweep_receipts(conn: &Arc<Mutex<Connection>>, days: u32) -> Result<usize> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let tx = conn.unchecked_transaction()?;
        let mut removed = tx.execute("DELETE FROM smtp_delivery WHERE submission_id IS NULL AND NOT EXISTS(SELECT 1 FROM smtp_queue q WHERE q.id=smtp_delivery.queue_id)",[])?;
        if days > 0 {
            let cutoff = format!("-{days} days");
            removed += tx.execute("DELETE FROM smtp_delivery WHERE status IN ('delivered','bounced') AND updated_at < datetime('now',?1) AND NOT EXISTS(SELECT 1 FROM smtp_queue q WHERE q.id=smtp_delivery.queue_id) AND NOT EXISTS(SELECT 1 FROM smtp_delivery sibling WHERE sibling.submission_id=smtp_delivery.submission_id AND (sibling.status IN ('queued','deferred') OR sibling.updated_at >= datetime('now',?1)))",[&cutoff])?;
            let expired = tx.prepare("SELECT id,account_id FROM email_submissions WHERE created_at < datetime('now',?1) AND undo_status='final' AND NOT EXISTS(SELECT 1 FROM smtp_delivery d WHERE d.submission_id=email_submissions.id) AND NOT EXISTS(SELECT 1 FROM smtp_queue q WHERE q.id=email_submissions.queue_id)")?
                .query_map([&cutoff], |row| Ok((row.get::<_,String>(0)?,row.get::<_,i32>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id,account) in expired {
                crate::db::changelog::record_tx(&tx,account,"EmailSubmission",Uuid::parse_str(&id)?,"destroyed")?;
                removed += tx.execute("DELETE FROM email_submissions WHERE id=?1",[id])?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }).await?
}
