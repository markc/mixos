// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Unsent messages, kept on this machine so nothing typed is lost: one
//! file per draft in the app's state directory
//! (`$XDG_STATE_HOME/mixos/apps/email/state/drafts/<id>.json`, or the
//! isolated desk's equivalent), written atomically (a temporary file, then
//! a rename) with mode 0600 in a 0700 directory. Drafts are not settings:
//! they never go to settingsd or Prefs.
//!
//! The state decides when a draft is saved or removed ([`DraftOp`]); the
//! shell carries the operations out on a [`DraftStore`]. A draft is removed
//! only after its message was sent or when the person discards it. Tests
//! use [`MemoryStore`], so they run without a disk.
//!
//! A file that does not parse, or has an unknown schema, is left alone and
//! never overwritten (the poison-pill law the config follows).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA: u32 = 1;

/// The directory's name under the app's state directory.
pub const DIR: &str = "drafts";

/// One unsent message, as typed.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct SavedDraft {
    pub schema_version: u32,
    /// The draft's own id: its file name. Lowercase hex only.
    pub id: String,
    /// The settings account it sends from.
    pub account: String,
    pub from: String,
    /// The recipient fields as typed.
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    /// Paths only; attachment contents are read when sending, never saved here.
    pub attachments: Vec<PathBuf>,
    pub in_reply_to: Option<String>,
    /// The sender identity it was typed under, and that identity's display
    /// name and Reply-To, so a restored draft sends as it was. Empty for a
    /// draft kept before identities existed.
    pub identity: String,
    pub from_name: String,
    pub reply_to: Option<String>,
    /// When it was last saved (RFC 3339, UTC).
    pub saved_at: String,
    /// What earlier sends of it did on the server, so a send after a
    /// restart never creates or submits a second copy.
    pub attempt: jmap::SendAttempt,
}

/// What the state asks of the store.
#[derive(Clone, Debug, PartialEq)]
pub enum DraftOp {
    Save(Box<SavedDraft>),
    /// Remove the draft with this id (sent, or discarded).
    Remove(String),
}

/// Where drafts are kept.
pub trait DraftStore {
    /// Write `draft` durably, replacing an older copy of it.
    fn save(&mut self, draft: &SavedDraft) -> Result<(), String>;
    /// Remove the draft `id`; removing one that is not there succeeds.
    fn remove(&mut self, id: &str) -> Result<(), String>;
    /// Every draft held, newest first.
    fn list(&self) -> Vec<SavedDraft>;
}

/// Carry out `ops` on `store`, in order; the errors, for the status line.
pub fn apply(store: &mut dyn DraftStore, ops: Vec<DraftOp>) -> Vec<String> {
    ops.into_iter()
        .filter_map(|op| match op {
            DraftOp::Save(draft) => store.save(&draft).err(),
            DraftOp::Remove(id) => store.remove(&id).err(),
        })
        .collect()
}

/// A draft id is used as a file name: lowercase hex, bounded.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Newest first, then by id, so the order is stable.
fn newest_first(list: &mut [SavedDraft]) {
    list.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then_with(|| a.id.cmp(&b.id)));
}

/// Drafts held in memory: the tests' store, and the fallback when there is
/// no state directory.
#[derive(Debug, Default)]
pub struct MemoryStore {
    pub drafts: BTreeMap<String, SavedDraft>,
}

impl DraftStore for MemoryStore {
    fn save(&mut self, draft: &SavedDraft) -> Result<(), String> {
        if !valid_id(&draft.id) {
            return Err(format!("draft id {:?} is not valid", draft.id));
        }
        self.drafts.insert(draft.id.clone(), draft.clone());
        Ok(())
    }

    fn remove(&mut self, id: &str) -> Result<(), String> {
        self.drafts.remove(id);
        Ok(())
    }

    fn list(&self) -> Vec<SavedDraft> {
        let mut list: Vec<_> = self.drafts.values().cloned().collect();
        newest_first(&mut list);
        list
    }
}

/// Drafts as files in a private directory.
#[derive(Debug)]
pub struct DirStore {
    pub dir: PathBuf,
}

impl DirStore {
    /// The store under the app's state directory `state`.
    pub fn new(state: &Path) -> Self {
        Self {
            dir: state.join(DIR),
        }
    }

    fn path(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err(format!("draft id {id:?} is not valid"));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    /// Read one file: `None` when it does not parse or has another schema
    /// (it is then never written over).
    fn read(path: &Path) -> Option<SavedDraft> {
        let raw = std::fs::read(path).ok()?;
        let draft: SavedDraft = serde_json::from_slice(&raw).ok()?;
        (draft.schema_version == CURRENT_SCHEMA).then_some(draft)
    }
}

impl DraftStore for DirStore {
    fn save(&mut self, draft: &SavedDraft) -> Result<(), String> {
        let path = self.path(&draft.id)?;
        // A file of this name that is not a draft this build reads is left
        // as it is.
        if path.exists() && Self::read(&path).is_none() {
            return Err(format!(
                "{}: not a draft this version reads; left untouched",
                path.display()
            ));
        }
        let bytes = serde_json::to_vec_pretty(draft).map_err(|e| e.to_string())?;
        crate::config::write_private(&path, &bytes)
    }

    fn remove(&mut self, id: &str) -> Result<(), String> {
        let path = self.path(id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("removing {}: {error}", path.display())),
        }
    }

    fn list(&self) -> Vec<SavedDraft> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut list: Vec<SavedDraft> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let stem = path.file_stem()?.to_str()?.to_owned();
                if path.extension()? != "json" || !valid_id(&stem) {
                    return None;
                }
                Self::read(&path).filter(|draft| draft.id == stem)
            })
            .collect();
        newest_first(&mut list);
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn draft(id: &str, saved_at: &str) -> SavedDraft {
        SavedDraft {
            schema_version: CURRENT_SCHEMA,
            id: id.into(),
            account: "a".into(),
            from: "ann@example.invalid".into(),
            to: "bob@example.invalid".into(),
            subject: "Lunch".into(),
            body: "Noon?".into(),
            saved_at: saved_at.into(),
            ..SavedDraft::default()
        }
    }

    #[test]
    fn drafts_are_private_files_written_whole() {
        let root = tempfile::tempdir().unwrap();
        let mut store = DirStore::new(root.path());
        store.save(&draft("0a", "2026-10-18T01:00:00Z")).unwrap();
        let mut newer = draft("0b", "2026-10-18T02:00:00Z");
        newer.attachments = vec![
            root.path().join("report.pdf"),
            root.path().join("photo.png"),
        ];
        newer.attempt.email_id = Some("E1".into());
        store.save(&newer).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&store.dir), 0o700);
        assert_eq!(mode(&store.dir.join("0a.json")), 0o600);
        let list = store.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], newer, "newest first, paths and send attempt kept");
        // No temporary is left behind.
        let names: Vec<_> = std::fs::read_dir(&store.dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        store.remove("0a").unwrap();
        store.remove("0a").unwrap();
        assert_eq!(store.list(), vec![newer]);
    }

    #[test]
    fn old_schema_one_draft_without_attachments_still_loads() {
        let root = tempfile::tempdir().unwrap();
        let store = DirStore::new(root.path());
        std::fs::create_dir_all(&store.dir).unwrap();
        std::fs::write(
            store.dir.join("0a.json"),
            br#"{"schema_version":1,"id":"0a","body":"kept text"}"#,
        )
        .unwrap();
        let drafts = store.list();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].body, "kept text");
        assert!(drafts[0].attachments.is_empty());
    }

    #[test]
    fn an_unreadable_draft_is_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let mut store = DirStore::new(root.path());
        std::fs::create_dir_all(&store.dir).unwrap();
        std::fs::write(store.dir.join("0c.json"), b"not json").unwrap();
        assert!(store.list().is_empty());
        assert!(store.save(&draft("0c", "2026-10-18T01:00:00Z")).is_err());
        assert_eq!(
            std::fs::read(store.dir.join("0c.json")).unwrap(),
            b"not json"
        );
    }

    #[test]
    fn a_draft_id_is_a_safe_file_name() {
        assert!(valid_id("0123abcdef"));
        for bad in ["", "../x", "AB", "a/b", "a.json", &"a".repeat(65)] {
            assert!(!valid_id(bad), "{bad:?}");
        }
        let root = tempfile::tempdir().unwrap();
        let mut store = DirStore::new(root.path());
        assert!(store.save(&draft("../escape", "")).is_err());
        assert!(
            MemoryStore::default()
                .save(&draft("../escape", ""))
                .is_err()
        );
    }
}
