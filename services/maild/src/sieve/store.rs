// SPDX-License-Identifier: MIT OR Apache-2.0
//! Versioned scripts in mail.db. Every mutation is account-scoped and atomic.
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use super::{MAX_SCRIPT_BYTES, MAX_SCRIPTS, compile};
use crate::db::Db;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sieve_scripts (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    version INTEGER NOT NULL,
    source TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(account_id, name)
);
CREATE UNIQUE INDEX IF NOT EXISTS sieve_one_active ON sieve_scripts(account_id) WHERE active=1;
CREATE TABLE IF NOT EXISTS sieve_versions (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    version INTEGER NOT NULL,
    source TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(account_id, name, version)
);
CREATE TABLE IF NOT EXISTS sieve_replies (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    sender TEXT NOT NULL,
    handle TEXT NOT NULL,
    sent_at INTEGER NOT NULL,
    PRIMARY KEY(account_id, sender, handle)
);
"#;

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub version: i64,
    pub active: bool,
    pub size: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct StoredScript {
    pub name: String,
    pub version: i64,
    pub source: String,
    pub active: bool,
}

pub fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.chars().count() <= 128
            && name.nfc().eq(name.chars())
            && !name.chars().any(|c| c.is_control()
                || matches!(c, '\u{2028}' | '\u{2029}')
                || (0xfdd0..=0xfdef).contains(&(c as u32))
                || c as u32 & 0xffff >= 0xfffe),
        "invalid script name (1..128 Unicode characters, NFC, no controls or separators)"
    );
    Ok(())
}

async fn access<T: Send + 'static>(
    db: &Db,
    f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
) -> Result<T> {
    let conn = db.conn.clone();
    tokio::task::spawn_blocking(move || {
        let mut guard = conn
            .lock()
            .map_err(|_| anyhow::anyhow!("sieve database lock poisoned"))?;
        f(&mut guard)
    })
    .await?
}

fn read(conn: &Connection, account: i32, name: &str) -> Result<Option<StoredScript>> {
    Ok(conn
        .query_row(
            "SELECT name,version,source,active FROM sieve_scripts WHERE account_id=?1 AND name=?2",
            params![account, name],
            |r| {
                Ok(StoredScript {
                    name: r.get(0)?,
                    version: r.get(1)?,
                    source: r.get(2)?,
                    active: r.get(3)?,
                })
            },
        )
        .optional()?)
}
pub async fn list(db: &Db, account: i32) -> Result<Vec<Entry>> {
    access(db, move |conn| {
        let mut q = conn.prepare("SELECT name,version,active,length(CAST(source AS BLOB)) FROM sieve_scripts WHERE account_id=?1 ORDER BY name")?;
        Ok(q.query_map([account], |r| Ok(Entry { name:r.get(0)?,version:r.get(1)?,active:r.get(2)?,size:r.get(3)? }))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }).await
}
pub async fn get(db: &Db, account: i32, name: &str) -> Result<StoredScript> {
    let name = name.to_string();
    access(db, move |conn| {
        read(conn, account, &name)?.ok_or_else(|| anyhow::anyhow!("NONEXISTENT: script not found"))
    })
    .await
}
pub async fn active(db: &Db, account: i32) -> Result<Option<StoredScript>> {
    access(db, move |conn| {
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM sieve_scripts WHERE account_id=?1 AND active=1",
                [account],
                |r| r.get(0),
            )
            .optional()?;
        name.map(|n| read(conn, account, &n))
            .transpose()
            .map(Option::flatten)
    })
    .await
}
pub async fn have_space(db: &Db, account: i32, name: &str, size: usize) -> Result<()> {
    validate_name(name)?;
    ensure!(size <= MAX_SCRIPT_BYTES, "QUOTA/MAXSIZE: script too large");
    let name = name.to_string();
    access(db, move |conn| quota(conn, account, &name)).await
}
fn quota(conn: &Connection, account: i32, name: &str) -> Result<()> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sieve_scripts WHERE account_id=?1 AND name<>?2",
        params![account, name],
        |r| r.get(0),
    )?;
    ensure!(
        count < MAX_SCRIPTS as i64,
        "QUOTA/MAXSCRIPTS: too many scripts"
    );
    Ok(())
}
/// The optional version is an optimistic concurrency guard for Bus editors.
/// ManageSieve specifies last-writer-wins and passes None.
pub async fn put(
    db: &Db,
    account: i32,
    name: &str,
    source: &str,
    expected_version: Option<i64>,
) -> Result<i64> {
    validate_name(name)?;
    let name = name.to_string();
    let source = source.to_string();
    access(db, move |conn| {
        compile(&source)?;
        let tx = conn.transaction()?;
        quota(&tx,account,&name)?;
        let old = read(&tx,account,&name)?;
        let current = old.as_ref().map(|s| s.version).unwrap_or(0);
        ensure!(expected_version.is_none_or(|v| v == current), "version conflict");
        // Account-wide sequence remains monotonic when old names are pruned.
        let highest: i64 = tx.query_row("SELECT COALESCE(max(version),0) FROM sieve_versions WHERE account_id=?1", [account], |r| r.get(0))?;
        let version = highest + 1;
        tx.execute("INSERT INTO sieve_scripts(account_id,name,version,source) VALUES(?1,?2,?3,?4) ON CONFLICT(account_id,name) DO UPDATE SET version=excluded.version,source=excluded.source", params![account,name,version,source])?;
        tx.execute("INSERT INTO sieve_versions(account_id,name,version,source) VALUES(?1,?2,?3,?4)",params![account,name,version,source])?;
        prune(&tx,account,&name)?;
        tx.commit()?; Ok(version)
    }).await
}
fn prune(conn: &Connection, account: i32, name: &str) -> Result<()> {
    conn.execute("DELETE FROM sieve_versions WHERE account_id=?1 AND name=?2 AND version NOT IN (SELECT version FROM sieve_versions WHERE account_id=?1 AND name=?2 ORDER BY version DESC LIMIT 16)",params![account,name])?;
    // Churning deleted/renamed names cannot escape the storage quota.
    conn.execute("DELETE FROM sieve_versions WHERE account_id=?1 AND version NOT IN (SELECT version FROM sieve_versions WHERE account_id=?1 ORDER BY version DESC LIMIT 512)",[account])?;
    Ok(())
}

pub async fn activate(db: &Db, account: i32, name: &str) -> Result<()> {
    let name = name.to_string();
    access(db, move |conn| {
        let tx = conn.transaction()?;
        if !name.is_empty() {
            let stored = read(&tx, account, &name)?
                .ok_or_else(|| anyhow::anyhow!("NONEXISTENT: script not found"))?;
            compile(&stored.source)?;
        }
        tx.execute(
            "UPDATE sieve_scripts SET active=0 WHERE account_id=?1",
            [account],
        )?;
        if !name.is_empty() {
            tx.execute(
                "UPDATE sieve_scripts SET active=1 WHERE account_id=?1 AND name=?2",
                params![account, name],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
    .await
}
pub async fn delete(db: &Db, account: i32, name: &str) -> Result<()> {
    let name = name.to_string();
    access(db, move |conn| {
        let tx = conn.transaction()?;
        let stored = read(&tx, account, &name)?
            .ok_or_else(|| anyhow::anyhow!("NONEXISTENT: script not found"))?;
        ensure!(!stored.active, "ACTIVE: deactivate before deleting");
        tx.execute(
            "DELETE FROM sieve_scripts WHERE account_id=?1 AND name=?2",
            params![account, name],
        )?;
        tx.commit()?;
        Ok(())
    })
    .await
}
pub async fn rename(db: &Db, account: i32, old: &str, new: &str) -> Result<()> {
    validate_name(new)?;
    let old = old.to_string();
    let new = new.to_string();
    access(db, move |conn| {
        let tx = conn.transaction()?;
        ensure!(
            read(&tx, account, &new)?.is_none(),
            "ALREADYEXISTS: destination exists"
        );
        let stored = read(&tx, account, &old)?
            .ok_or_else(|| anyhow::anyhow!("NONEXISTENT: script not found"))?;
        let version: i64 = tx.query_row(
            "SELECT COALESCE(max(version),0)+1 FROM sieve_versions WHERE account_id=?1",
            [account],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE sieve_scripts SET name=?3,version=?4 WHERE account_id=?1 AND name=?2",
            params![account, old, new, version],
        )?;
        tx.execute(
            "INSERT INTO sieve_versions(account_id,name,version,source) VALUES(?1,?2,?3,?4)",
            params![account, new, version, stored.source],
        )?;
        prune(&tx, account, &new)?;
        tx.commit()?;
        Ok(())
    })
    .await
}

pub async fn reply_due(
    db: &Db,
    account: i32,
    sender: &str,
    handle: &str,
    days: u32,
) -> Result<bool> {
    let sender = sender.to_ascii_lowercase();
    let handle = handle.to_string();
    access(db, move |conn| {
        // One atomic compare-and-stamp across concurrent deliveries and restarts.
        Ok(conn.execute("INSERT INTO sieve_replies(account_id,sender,handle,sent_at) VALUES(?1,?2,?3,unixepoch()) ON CONFLICT(account_id,sender,handle) DO UPDATE SET sent_at=unixepoch() WHERE sent_at<=unixepoch()-?4",params![account,sender,handle,i64::from(days)*86400])? > 0)
    }).await
}
