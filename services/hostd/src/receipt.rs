// SPDX-License-Identifier: MIT OR Apache-2.0
//! Receipts and the circuit breaker. Every write keeps the previous content
//! of each file it replaces, under a receipt id, so `hostd.undo` can put it
//! back. Two rollbacks in a row under one grant suspend that grant until
//! local root thaws hostd.
use crate::caller::Origin;
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

/// Rollbacks in a row that suspend a grant.
pub const BREAKER: u32 = 2;

/// A receipt's durable state. `Applying` and `Undoing` are in flight: a
/// receipt left in either after a stop or crash is finished at the next
/// start, and writes wait until it is.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Applying,
    Applied,
    RolledBack,
    Undoing,
    Undone,
    /// Interrupted, and the file no longer held either hostd's content or
    /// the backup: someone else's change, left alone.
    Abandoned,
}

impl Outcome {
    pub fn in_flight(&self) -> bool {
        matches!(self, Self::Applying | Self::Undoing)
    }
}

/// What a file holds now, relative to one change.
#[derive(Debug, PartialEq, Eq)]
pub enum Holds {
    /// What this change installed.
    Installed,
    /// The backup: the change was never written, or was already put back.
    Previous,
    /// Anything else: a later change, or someone's edit.
    Other,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileChange {
    pub path: PathBuf,
    /// `None` only when the file provably didn't exist before.
    pub previous: Option<String>,
    /// The digest of what hostd wrote. Undo refuses unless the file still
    /// holds exactly this: a later change, or anyone's edit, wins.
    #[serde(default)]
    pub installed: Option<String>,
    /// The mode the file is written and restored with.
    #[serde(default = "default_mode")]
    pub mode: u32,
    /// A group the file is given (a WireGuard key: `systemd-network`), so
    /// the reader it is for can read it and nobody else can.
    #[serde(default)]
    pub group: Option<String>,
    /// An existing file's owner and group (numeric), recorded at backup and
    /// given back on restore, so a file networkd reads keeps its reader.
    #[serde(default)]
    pub owner: Option<(u32, u32)>,
}

fn default_mode() -> u32 {
    0o644
}

impl FileChange {
    /// Reads the file's current content as the backup. Only a file that
    /// provably doesn't exist backs up as `None`; any other failure to read
    /// it stops the change before anything is written.
    ///
    /// An existing file keeps its mode, owner and group on restore (read
    /// from the same open file as its content). A new file is written with
    /// the `mode` (and `group`, if set) the verb chooses.
    pub fn backup(path: &Path) -> anyhow::Result<Self> {
        use std::{io::Read, os::unix::fs::MetadataExt};
        let context = || format!("back up {}", path.display());
        let (previous, mode, owner) = match std::fs::File::open(path) {
            Ok(mut file) => {
                let meta = file.metadata().with_context(context)?;
                let mut bytes = vec![];
                file.read_to_end(&mut bytes).with_context(context)?;
                let text = String::from_utf8(bytes)
                    .with_context(|| format!("{} is not UTF-8 text", path.display()))?;
                (
                    Some(text),
                    meta.mode() & 0o7777,
                    Some((meta.uid(), meta.gid())),
                )
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, default_mode(), None),
            Err(e) => return Err(e).with_context(context),
        };
        Ok(Self {
            path: path.into(),
            previous,
            installed: None,
            mode,
            group: None,
            owner,
        })
    }

    /// What the file holds now. A read error is an error, never a guess.
    pub fn holds(&self) -> anyhow::Result<Holds> {
        let current = match std::fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| self.path.display().to_string()),
        };
        let previous = self.previous.as_ref().map(|p| p.as_bytes());
        Ok(match (&current, &self.installed) {
            (Some(bytes), Some(installed)) if &digest(bytes) == installed => Holds::Installed,
            _ if current.as_deref() == previous => Holds::Previous,
            _ => Holds::Other,
        })
    }
}

pub fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub id: String,
    pub verb: String,
    pub grant: String,
    pub origin: Origin,
    pub at: String,
    /// The non-secret arguments, as the verb chose to record them.
    pub summary: serde_json::Value,
    pub files: Vec<FileChange>,
    pub outcome: Outcome,
    #[serde(default)]
    pub reason: Option<String>,
}

fn receipts(state: &Path) -> PathBuf {
    state.join("receipts")
}

/// Receipt ids are uuid v7 strings; anything else is refused before it
/// becomes a path.
pub fn valid_id(id: &str) -> bool {
    uuid::Uuid::try_parse(id).is_ok_and(|u| u.hyphenated().to_string() == id)
}

impl Receipt {
    pub fn new(verb: &str, grant: &str, origin: &Origin, summary: serde_json::Value) -> Self {
        Self {
            id: uuid::Uuid::now_v7().hyphenated().to_string(),
            verb: verb.into(),
            grant: grant.into(),
            origin: origin.clone(),
            at: chrono::Utc::now().to_rfc3339(),
            summary,
            files: vec![],
            outcome: Outcome::Applying,
            reason: None,
        }
    }

    pub fn save(&self, state: &Path) -> anyhow::Result<()> {
        let dir = receipts(state);
        std::fs::create_dir_all(&dir)?;
        write_private(
            &dir.join(format!("{}.json", self.id)),
            &serde_json::to_vec(self)?,
        )
    }

    pub fn load(state: &Path, id: &str) -> anyhow::Result<Self> {
        if !valid_id(id) {
            bail!("not a receipt id");
        }
        let path = receipts(state).join(format!("{id}.json"));
        let text = std::fs::read(&path).with_context(|| format!("receipt {id}"))?;
        Ok(serde_json::from_slice(&text)?)
    }
}

/// Every receipt still `applying`: a transaction a stop or crash cut short.
/// Oldest first (receipt ids are time-ordered), so recovery unwinds in order.
///
/// Fails closed: a receipt file that can't be read or parsed is an error,
/// never skipped, because it may be an interrupted change.
pub fn unfinished(state: &Path) -> anyhow::Result<Vec<Receipt>> {
    Ok(all(state)?
        .into_iter()
        .filter(|r| r.outcome.in_flight())
        .collect())
}

/// Every receipt, oldest first. Fails closed like `unfinished`.
pub fn all(state: &Path) -> anyhow::Result<Vec<Receipt>> {
    let entries = match std::fs::read_dir(receipts(state)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).context("read receipts"),
    };
    let mut found = vec![];
    for entry in entries {
        let name = entry?.file_name();
        let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
            continue; // temporary files from an atomic write
        };
        found.push(Receipt::load(state, id).with_context(|| format!("receipt {id}"))?);
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

/// Writes `bytes` to `path` atomically, readable by root only.
pub fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_atomic(path, bytes, 0o600)
}

/// Flushes a directory's entries (a rename or removal in it) to disk.
pub fn sync_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .with_context(|| format!("sync {}", dir.display()))
}

/// Writes through a temporary file in the same directory and renames it
/// over `path`, so readers see the old or the new file, never a part. Both
/// the file and the rename are synced before this returns.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> anyhow::Result<()> {
    write_as(path, bytes, mode, None)
}

/// `write_atomic`, giving the file exactly `mode` (not masked by the
/// umask) and, if named, `group`. Mode and group are set on the temporary
/// file before a byte is written, so the content is never readable by
/// anyone it isn't for, even briefly.
pub fn write_as(path: &Path, bytes: &[u8], mode: u32, group: Option<&str>) -> anyhow::Result<()> {
    let owner = match group {
        Some(group) => Owner::Group(group),
        None => Owner::Default,
    };
    write_owned(path, bytes, mode, owner)
}

/// Who a written file belongs to.
pub enum Owner<'a> {
    /// hostd (root).
    Default,
    /// root, and this group by name.
    Group(&'a str),
    /// Exactly this owner and group, as a backup recorded them.
    Ids(u32, u32),
}

/// `write_as` with any `Owner`.
pub fn write_owned(path: &Path, bytes: &[u8], mode: u32, owner: Owner) -> anyhow::Result<()> {
    use std::{io::Write, os::fd::AsRawFd, os::unix::fs::PermissionsExt};
    let dir = path.parent().context("path has no directory")?;
    let tmp = dir.join(format!(
        ".{}.hostd-tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("create {}", tmp.display()))?;
    // Order matters under hostd's capabilities (CAP_CHOWN, no CAP_FOWNER):
    // set the mode while hostd still owns the file, then change the owner,
    // then write the content, so it is never readable by anyone it isn't
    // for and no step needs a capability hostd lacks.
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    let ids = match owner {
        Owner::Default => None,
        // -1 (u32::MAX) keeps the owner.
        Owner::Group(group) => Some((u32::MAX, gid_of(group)?)),
        Owner::Ids(uid, gid) => Some((uid, gid)),
    };
    if let Some((uid, gid)) = ids {
        // SAFETY: fchown on a file descriptor we own.
        if unsafe { libc::fchown(file.as_raw_fd(), uid, gid) } != 0 {
            let e = std::io::Error::last_os_error();
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("set the owner of {}", tmp.display()));
        }
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    sync_dir(dir)
}

/// A group's id, from the group database.
pub fn gid_of(group: &str) -> anyhow::Result<u32> {
    let name = std::ffi::CString::new(group)?;
    // SAFETY: getgrnam with a valid C string; the result is read at once and
    // not kept. hostd's writes run one at a time, under the writer lock.
    let entry = unsafe { libc::getgrnam(name.as_ptr()) };
    if entry.is_null() {
        anyhow::bail!("no group {group}");
    }
    // SAFETY: non-null, from getgrnam.
    Ok(unsafe { (*entry).gr_gid })
}

/// Puts each file back as the receipt found it, with the mode and group it
/// was written with.
pub fn restore(files: &[FileChange]) -> anyhow::Result<()> {
    for change in files.iter().rev() {
        match &change.previous {
            Some(text) => {
                let owner = match (change.owner, change.group.as_deref()) {
                    (Some((uid, gid)), _) => Owner::Ids(uid, gid),
                    (None, Some(group)) => Owner::Group(group),
                    (None, None) => Owner::Default,
                };
                write_owned(&change.path, text.as_bytes(), change.mode, owner)?
            }
            None => match std::fs::remove_file(&change.path) {
                Ok(()) => sync_dir(change.path.parent().context("no directory")?)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context(change.path.display().to_string()),
            },
        }
    }
    Ok(())
}

#[derive(Default, Serialize, Deserialize)]
struct Breaker {
    /// Rollbacks in a row, per grant.
    streak: BTreeMap<String, u32>,
    /// Suspended grants and why.
    suspended: BTreeMap<String, String>,
}

fn breaker_path(state: &Path) -> PathBuf {
    state.join("breaker.json")
}

fn load_breaker(state: &Path) -> Breaker {
    std::fs::read(breaker_path(state))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_breaker(state: &Path, breaker: &Breaker) -> anyhow::Result<()> {
    std::fs::create_dir_all(state)?;
    write_private(&breaker_path(state), &serde_json::to_vec(breaker)?)
}

pub fn suspended(state: &Path, grant: &str) -> Option<String> {
    load_breaker(state).suspended.get(grant).cloned()
}

/// Records one write's result. Returns true when this rollback suspended
/// the grant.
pub fn record(state: &Path, grant: &str, rolled_back: bool) -> anyhow::Result<bool> {
    let mut breaker = load_breaker(state);
    let streak = breaker.streak.entry(grant.to_owned()).or_default();
    *streak = if rolled_back { *streak + 1 } else { 0 };
    let tripped = rolled_back && *streak >= BREAKER;
    if tripped {
        breaker.suspended.insert(
            grant.to_owned(),
            format!(
                "{BREAKER} rollbacks in a row at {}",
                chrono::Utc::now().to_rfc3339()
            ),
        );
    }
    save_breaker(state, &breaker)?;
    Ok(tripped)
}

/// Clears every suspension and streak (`hostd.thaw`).
pub fn clear(state: &Path) -> anyhow::Result<Vec<String>> {
    let breaker = load_breaker(state);
    let cleared = breaker.suspended.keys().cloned().collect();
    save_breaker(state, &Breaker::default())?;
    Ok(cleared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipts_round_trip_and_ids_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let r = Receipt::new(
            "net.link.apply",
            "g",
            &Origin::Bus,
            serde_json::json!({"link":"x"}),
        );
        r.save(dir.path()).unwrap();
        assert_eq!(
            Receipt::load(dir.path(), &r.id).unwrap().verb,
            "net.link.apply"
        );
        assert!(Receipt::load(dir.path(), "../breaker").is_err());
        assert!(!valid_id("../x"));
        assert!(valid_id(&r.id));
    }

    #[test]
    fn restore_puts_files_back_or_removes_new_ones() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.network");
        let new = dir.path().join("new.network");
        std::fs::write(&old, "changed").unwrap();
        std::fs::write(&new, "created").unwrap();
        restore(&[
            FileChange {
                path: old.clone(),
                previous: Some("original".into()),
                installed: None,
                mode: 0o644,
                group: None,
                owner: None,
            },
            FileChange {
                path: new.clone(),
                previous: None,
                installed: None,
                mode: 0o644,
                group: None,
                owner: None,
            },
        ])
        .unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "original");
        assert!(!new.exists());
    }

    /// A restored file gets back its mode and its owner and group, not
    /// root's defaults (a 0640 file networkd reads must stay readable).
    #[test]
    fn restore_keeps_mode_owner_and_group() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.netdev");
        std::fs::write(&path, "original").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let change = FileChange::backup(&path).unwrap();
        assert_eq!(change.mode, 0o640);
        assert_eq!(change.owner, Some((meta.uid(), meta.gid())));
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "replaced").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        restore(std::slice::from_ref(&change)).unwrap();
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        assert_eq!(after.mode() & 0o7777, 0o640);
        assert_eq!((after.uid(), after.gid()), (meta.uid(), meta.gid()));
    }

    #[test]
    fn backup_is_none_only_for_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            FileChange::backup(&dir.path().join("absent"))
                .unwrap()
                .previous
                .is_none()
        );
        let text = dir.path().join("text");
        std::fs::write(&text, "x").unwrap();
        assert_eq!(
            FileChange::backup(&text).unwrap().previous.as_deref(),
            Some("x")
        );
        // A directory, or bytes that aren't text: refused, never "absent".
        assert!(FileChange::backup(dir.path()).is_err());
        let binary = dir.path().join("binary");
        std::fs::write(&binary, b"\xff\xfe").unwrap();
        assert!(FileChange::backup(&binary).is_err());
    }

    #[test]
    fn holds_tells_installed_previous_and_other_apart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        // A change that created the file (no previous).
        let mut created = FileChange::backup(&path).unwrap();
        created.installed = Some(digest(b"ours"));
        assert_eq!(created.holds().unwrap(), Holds::Previous);
        std::fs::write(&path, "ours").unwrap();
        assert_eq!(created.holds().unwrap(), Holds::Installed);
        std::fs::write(&path, "someone else").unwrap();
        assert_eq!(created.holds().unwrap(), Holds::Other);
        // A change that replaced "old".
        std::fs::write(&path, "old").unwrap();
        let mut replaced = FileChange::backup(&path).unwrap();
        replaced.installed = Some(digest(b"new"));
        assert_eq!(replaced.holds().unwrap(), Holds::Previous);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(replaced.holds().unwrap(), Holds::Other);
    }

    #[test]
    fn unfinished_fails_closed_on_a_broken_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Receipt::new("net.link.apply", "g", &Origin::Bus, serde_json::json!({}));
        r.save(dir.path()).unwrap();
        assert_eq!(unfinished(dir.path()).unwrap().len(), 1);
        r.outcome = Outcome::Applied;
        r.save(dir.path()).unwrap();
        assert!(unfinished(dir.path()).unwrap().is_empty());
        let broken = uuid::Uuid::now_v7().hyphenated().to_string();
        std::fs::write(receipts(dir.path()).join(format!("{broken}.json")), "{").unwrap();
        assert!(unfinished(dir.path()).is_err());
    }

    #[test]
    fn two_rollbacks_in_a_row_suspend_and_thaw_clears() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!record(dir.path(), "g", true).unwrap());
        assert!(!record(dir.path(), "g", false).unwrap());
        assert!(!record(dir.path(), "g", true).unwrap());
        assert!(suspended(dir.path(), "g").is_none());
        assert!(record(dir.path(), "g", true).unwrap());
        assert!(suspended(dir.path(), "g").is_some());
        assert_eq!(clear(dir.path()).unwrap(), vec!["g".to_string()]);
        assert!(suspended(dir.path(), "g").is_none());
    }
}
