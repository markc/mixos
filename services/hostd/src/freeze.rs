// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The kill switch. A frozen hostd serves reads and refuses every write
//! until a local caller thaws it. The state is a file in hostd's state
//! directory, written durably, so it survives restarts, crashes and reboots.
//!
//! It fails closed: only a missing file means "not frozen". A record that
//! can't be read, for any other reason, counts as frozen.
use crate::{caller::Origin, receipt};
use serde_json::{Value, json};
use std::path::Path;

const FILE: &str = "frozen";

/// `Ok(None)` only when there is no freeze record at all.
pub fn state(dir: &Path) -> std::io::Result<Option<Value>> {
    match std::fs::read(dir.join(FILE)) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"record": "unreadable"})),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether writes must be refused: frozen, or the record can't be read.
pub fn blocks_writes(dir: &Path) -> bool {
    !matches!(state(dir), Ok(None))
}

pub fn freeze(dir: &Path, by: &Origin, reason: Option<&str>) -> anyhow::Result<Value> {
    match state(dir) {
        Ok(Some(existing)) => return Ok(existing),
        Ok(None) => {}
        // Unreadable already blocks writes; replacing it is safe and
        // gives the next reader a clean record.
        Err(_) => {}
    }
    let record = json!({
        "since": chrono::Utc::now().to_rfc3339(),
        "by": by,
        "reason": reason,
    });
    std::fs::create_dir_all(dir)?;
    receipt::write_private(&dir.join(FILE), record.to_string().as_bytes())?;
    Ok(record)
}

/// Returns whether hostd was frozen. The removal is synced before this
/// returns.
pub fn thaw(dir: &Path) -> anyhow::Result<bool> {
    match std::fs::remove_file(dir.join(FILE)) {
        Ok(()) => {
            receipt::sync_dir(dir)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> Origin {
        Origin::Local(crate::caller::Caller {
            uid: 0,
            gid: 0,
            pid: Some(1),
            cgroup: None,
            verified: false,
            host_ns: false,
        })
    }

    #[test]
    fn freeze_is_sticky_and_thaw_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        assert!(state(dir.path()).unwrap().is_none());
        assert!(!blocks_writes(dir.path()));
        let first = freeze(dir.path(), &root(), Some("testing")).unwrap();
        assert_eq!(first["reason"], "testing");
        // A second freeze keeps the first record.
        let again = freeze(dir.path(), &root(), Some("other")).unwrap();
        assert_eq!(again["reason"], "testing");
        assert!(blocks_writes(dir.path()));
        assert!(thaw(dir.path()).unwrap());
        assert!(!thaw(dir.path()).unwrap());
        assert!(!blocks_writes(dir.path()));
    }

    #[test]
    fn an_unreadable_record_blocks_writes() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the record should be: read fails, not NotFound.
        std::fs::create_dir(dir.path().join(FILE)).unwrap();
        assert!(state(dir.path()).is_err());
        assert!(blocks_writes(dir.path()));
        // Garbage content still counts as frozen.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), b"\xff\xfe").unwrap();
        assert!(blocks_writes(dir.path()));
    }
}
