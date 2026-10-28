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
    })
}

const SELECT_COLS: &str =
    "id, from_addr, to_addrs, blob_id, blob_hash, attempts, next_retry, last_error";

/// Enqueue a message for outbound delivery, addressed by CAS hash.
pub async fn enqueue(
    conn: &Arc<Mutex<Connection>>,
    from_addr: &str,
    to_addrs: &[String],
    blob_hash: BlobHash,
) -> Result<i64> {
    let conn = conn.clone();
    let from_addr = from_addr.to_string();
    let to_addrs = to_addrs.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut conn = conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let tx = conn.transaction()?;
        let id = enqueue_on(&tx, &from_addr, &to_addrs, blob_hash)?;
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
        conn.execute("INSERT INTO smtp_delivery(queue_id,recipient) VALUES(?1,?2)", params![id, recipient])?;
    }
    Ok(id)
}

/// Recipients still owed this message. Terminal results are never retried.
pub async fn pending(conn: &Arc<Mutex<Connection>>, id: i64) -> Result<Vec<String>> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow::anyhow!("lock error: {e}"))?;
        let mut stmt = conn.prepare("SELECT recipient FROM smtp_delivery WHERE queue_id=?1 AND status IN ('queued','deferred') ORDER BY rowid")?;
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
        conn.execute("DELETE FROM smtp_queue WHERE id = ?1", params![id])?;
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
        conn.execute("UPDATE smtp_delivery SET status='bounced',last_reply=?2,updated_at=datetime('now') WHERE queue_id=?1 AND status IN ('queued','deferred')", params![id,error])?;
        conn.execute("DELETE FROM smtp_queue WHERE id = ?1", params![id])?;
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
