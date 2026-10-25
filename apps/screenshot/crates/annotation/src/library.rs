// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Where shots live and how finished ones are published.
//!
//! - **Captures** land in the app's private state directory
//!   (`captures/`, 0700, owned by the user, never a symlink): the raw frames
//!   compd writes, before anyone decides to keep them.
//! - **Saved shots** are published into the person's screenshots folder,
//!   `XDG_PICTURES_DIR/Screenshots` (`~/Pictures/Screenshots`), named by
//!   local time, `20261012_140322.png` (Spectacle's `yyyyMMdd_HHmmss`),
//!   with `_2`, `_3`… when a name is taken. Nothing is ever overwritten.
//!   Nothing reads a name back: the ledger records whole paths, so shots
//!   saved under the earlier `Screenshot 2026-10-12 14-03-22.png` form
//!   stay listed.
//! - **The ledger** (`saved.jsonl` in the state directory) records every
//!   save, newest last, so an agent can ask for the latest shots
//!   (`screenshot.recent`) without scanning a folder or guessing which file
//!   is the annotated one.
use crate::paint::{PUBLIC, publish_png, sync_dir};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Seek, Write};
use std::path::{Path, PathBuf};

/// The app's own name under the MixOS state directories.
pub const APP: &str = "screenshot";
/// The folder under Pictures that saved shots go to.
pub const FOLDER: &str = "Screenshots";
/// The ledger's file name in the state directory.
pub const LEDGER: &str = "saved.jsonl";
/// The most entries `screenshot.recent` returns.
pub const RECENT_LIMIT: usize = 100;
/// Past this size the ledger is rewritten with its newest entries only.
const LEDGER_COMPACT_BYTES: u64 = 512 * 1024;
const LEDGER_KEEP: usize = 500;
/// How many `_n` suffixes a save tries before giving up.
const SUFFIXES: usize = 999;

fn absolute_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// The app's state directory: `MIXOS_APP_HOME`, else
/// `MIXOS_APPS_HOME/screenshot`, `MIXOS_VAR/apps/screenshot`,
/// `XDG_STATE_HOME/mixos/apps/screenshot` or
/// `~/.local/state/mixos/apps/screenshot`.
pub fn state_dir() -> Result<PathBuf, String> {
    absolute_env("MIXOS_APP_HOME")
        .or_else(|| absolute_env("MIXOS_APPS_HOME").map(|p| p.join(APP)))
        .or_else(|| absolute_env("MIXOS_VAR").map(|p| p.join("apps").join(APP)))
        .or_else(|| absolute_env("XDG_STATE_HOME").map(|p| p.join("mixos/apps").join(APP)))
        .or_else(|| absolute_env("HOME").map(|p| p.join(".local/state/mixos/apps").join(APP)))
        .ok_or_else(|| "no absolute application state directory".into())
}

/// The private captures directory, created and secured.
pub fn captures_dir() -> Result<PathBuf, String> {
    let directory = state_dir()?.join("captures");
    secure_directory(&directory)?;
    Ok(directory)
}

/// Create `directory` (0700) or secure an existing one: the opened
/// directory itself is checked, a final symlink is refused, and one owned
/// by another account is refused.
pub fn secure_directory(directory: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(|e| e.to_string())?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory)
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    // SAFETY: geteuid has no arguments or memory preconditions.
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err("the captures directory belongs to another account".into());
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())
}

/// `XDG_PICTURES_DIR` from a `user-dirs.dirs` file's `content`, with
/// `$HOME` expanded; `None` when it is not set there (or is the home
/// directory itself, which xdg-user-dirs uses to mean "disabled").
pub fn pictures_from_user_dirs(content: &str, home: &Path) -> Option<PathBuf> {
    let value = content.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("XDG_PICTURES_DIR=")?;
        Some(rest.trim().trim_matches('"').to_owned())
    })?;
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    (path.is_absolute() && path != home).then_some(path)
}

/// The screenshots folder: `MIXOS_SCREENSHOTS_DIR` when set, else
/// `Screenshots` in the person's Pictures directory (`user-dirs.dirs`, else
/// `~/Pictures`). Not created here.
pub fn screenshots_dir() -> Result<PathBuf, String> {
    if let Some(dir) = absolute_env("MIXOS_SCREENSHOTS_DIR") {
        return Ok(dir);
    }
    let home = absolute_env("HOME").ok_or("HOME is not an absolute path")?;
    let config = absolute_env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
    let pictures = std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|content| pictures_from_user_dirs(&content, &home))
        .unwrap_or_else(|| home.join("Pictures"));
    Ok(pictures.join(FOLDER))
}

/// The file name a shot saved at `at` takes, before any suffix:
/// `YYYYMMDD_HHMMSS`, Spectacle's `yyyyMMdd_HHmmss`.
pub fn shot_stem<Tz: chrono::TimeZone>(at: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    at.format("%Y%m%d_%H%M%S").to_string()
}

/// The names a shot called `stem` may take in `dir`, in order:
/// `stem.png`, then `stem_2.png`, `stem_3.png`…
pub fn candidates(dir: &Path, stem: &str) -> impl Iterator<Item = PathBuf> {
    let dir = dir.to_path_buf();
    let stem = stem.to_owned();
    (1..=SUFFIXES).map(move |n| {
        if n == 1 {
            dir.join(format!("{stem}.png"))
        } else {
            dir.join(format!("{stem}_{n}.png"))
        }
    })
}

/// Whether `name` has the form a folder save gives a file:
/// `YYYYMMDD_HHMMSS.png`, or `YYYYMMDD_HHMMSS_n.png` for n of 2 or more.
pub fn is_shot_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".png") else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let mut parts = stem.split('_');
    let (Some(date), Some(time)) = (parts.next(), parts.next()) else {
        return false;
    };
    let suffix_ok = match parts.next() {
        None => true,
        Some(n) => digits(n) && !n.starts_with('0') && n != "1",
    };
    date.len() == 8
        && digits(date)
        && time.len() == 6
        && digits(time)
        && suffix_ok
        && parts.next().is_none()
}

/// The first of `stem`'s names in `dir` that nothing holds yet, as Save As
/// proposes it; `stem.png` when every one is taken.
pub fn free_name(dir: &Path, stem: &str) -> PathBuf {
    candidates(dir, stem)
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .unwrap_or_else(|| dir.join(format!("{stem}.png")))
}

/// One save, as the ledger and `screenshot.saved` report it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Drawn on or cropped: not the bare capture.
    pub annotated: bool,
    /// Local time with its offset (RFC 3339).
    pub saved_at: String,
}

/// Publish `image` into `dir` (created if missing) under the next free
/// name for `stem`.
pub fn save_into(dir: &Path, stem: &str, image: &RgbaImage) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !std::fs::metadata(dir).map_err(|e| e.to_string())?.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    publish_png(image, candidates(dir, stem), PUBLIC)
}

/// The newest captures kept in the private captures directory, and the
/// oldest one kept at all: older or surplus raw frames are deleted.
pub const CAPTURES_KEPT: usize = 20;
pub const CAPTURES_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// Whether `path` is a raw capture this app wrote into `dir`.
pub fn is_capture(dir: &Path, path: &Path) -> bool {
    path.parent() == Some(dir)
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("capture-") && n.ends_with(".png"))
}

/// Make the raw capture at `path` private (0600), whatever mode compd
/// created it with. A symlink is refused.
pub fn secure_capture(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())
}

/// Delete the raw capture at `path` if it is one of `dir`'s.
pub fn forget_capture(dir: &Path, path: &Path) {
    if is_capture(dir, path) {
        let _ = std::fs::remove_file(path);
    }
}

/// Keep the newest [`CAPTURES_KEPT`] raw captures in `dir` that are younger
/// than [`CAPTURES_MAX_AGE`]; delete the rest. Returns how many went.
pub fn prune_captures(dir: &Path) -> usize {
    prune_captures_at(
        dir,
        CAPTURES_KEPT,
        CAPTURES_MAX_AGE,
        std::time::SystemTime::now(),
    )
}

fn prune_captures_at(
    dir: &Path,
    keep: usize,
    max_age: std::time::Duration,
    now: std::time::SystemTime,
) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut captures: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| is_capture(dir, p))
        .filter_map(|p| {
            let meta = std::fs::symlink_metadata(&p).ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, p))
        })
        .collect();
    captures.sort_by_key(|c| std::cmp::Reverse(c.0));
    let mut removed = 0;
    for (index, (modified, path)) in captures.into_iter().enumerate() {
        let old = now.duration_since(modified).is_ok_and(|age| age > max_age);
        if (index >= keep || old) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The record of saved shots, in the state directory.
#[derive(Debug, Clone)]
pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    /// The ledger in `state` (the app's state directory).
    pub fn new(state: &Path) -> Self {
        Self {
            path: state.join(LEDGER),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// One interprocess lock (`saved.jsonl.lock`) held while the value
    /// lives: exclusive for an append and its compaction, shared for a read.
    fn lock(&self, exclusive: bool) -> Result<std::fs::File, String> {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.path.with_extension("jsonl.lock"))
            .map_err(|e| e.to_string())?;
        let operation = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        };
        loop {
            // SAFETY: the descriptor is open for this call; flock has no
            // memory preconditions.
            if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
                return Ok(file);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(format!("lock the ledger: {error}"));
            }
        }
    }

    /// Record `saved`, newest last. The ledger is compacted to its newest
    /// entries once it grows past a bound.
    pub fn append(&self, saved: &Saved) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(parent) = self.path.parent() {
            secure_directory(parent)?;
        }
        let mut line = serde_json::to_string(saved).map_err(|e| e.to_string())?;
        line.push('\n');
        // Every instance shares the ledger: one lock covers the append and
        // any compaction, and the append descriptor is closed before the
        // file can be replaced.
        let _lock = self.lock(true)?;
        let created = std::fs::symlink_metadata(&self.path).is_err();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.path)
            .map_err(|e| e.to_string())?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.sync_data())
            .map_err(|e| e.to_string())?;
        let length = file.metadata().map_err(|e| e.to_string())?.len();
        drop(file);
        // A new ledger's name lasts only once its directory is synced.
        if created && let Some(dir) = self.path.parent() {
            sync_dir(dir)?;
        }
        if length > LEDGER_COMPACT_BYTES {
            self.compact()?;
        }
        Ok(())
    }

    /// Every readable entry, oldest first, from the ledger's last
    /// `LEDGER_COMPACT_BYTES` at most.
    fn entries(&self) -> Vec<Saved> {
        use std::os::unix::fs::OpenOptionsExt;
        let Ok(mut file) = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&self.path)
        else {
            return Vec::new();
        };
        let length = file.metadata().map_or(0, |m| m.len());
        let skip = length.saturating_sub(LEDGER_COMPACT_BYTES * 2);
        if file.seek(std::io::SeekFrom::Start(skip)).is_err() {
            return Vec::new();
        }
        let mut reader = std::io::BufReader::new(file.take(LEDGER_COMPACT_BYTES * 2));
        let mut entries = Vec::new();
        let mut line = String::new();
        let mut first = skip > 0;
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            // A read starting mid-file drops its partial first line.
            if std::mem::take(&mut first) {
                continue;
            }
            if let Ok(saved) = serde_json::from_str::<Saved>(line.trim()) {
                entries.push(saved);
            }
        }
        entries
    }

    /// The newest `limit` saves whose files still exist, newest first, one
    /// entry per path.
    pub fn recent(&self, limit: usize) -> Vec<Saved> {
        // A read without the lock still sees whole lines; the lock only
        // keeps it off a compaction midway.
        let _lock = self.lock(false).ok();
        let mut seen = std::collections::HashSet::new();
        self.entries()
            .into_iter()
            .rev()
            .filter(|s| seen.insert(s.path.clone()))
            .filter(|s| s.path.is_file())
            .take(limit.min(RECENT_LIMIT))
            .collect()
    }

    fn compact(&self) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        // The newest entries, up to half the bound, so appends run a while
        // before the next compaction.
        let mut lines = Vec::new();
        let mut bytes = 0u64;
        for saved in self.entries().iter().rev().take(LEDGER_KEEP) {
            let line = serde_json::to_string(saved).map_err(|e| e.to_string())?;
            bytes += line.len() as u64 + 1;
            if bytes > LEDGER_COMPACT_BYTES / 2 {
                break;
            }
            lines.push(line);
        }
        lines.reverse();
        let temporary = self
            .path
            .with_file_name(format!(".{LEDGER}.{}", uuid::Uuid::now_v7()));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(|e| e.to_string())?;
            for line in &lines {
                writeln!(file, "{line}").map_err(|e| e.to_string())?;
            }
            file.sync_all().map_err(|e| e.to_string())?;
            std::fs::rename(&temporary, &self.path).map_err(|e| e.to_string())?;
            match self.path.parent() {
                Some(dir) => sync_dir(dir),
                None => Ok(()),
            }
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}

/// The local time now, as a ledger timestamp.
pub fn now_local() -> (chrono::DateTime<chrono::Local>, String) {
    let now = chrono::Local::now();
    let stamp = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    (now, stamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn existing_capture_directory_is_secured_and_symlinks_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("captures");
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
        secure_directory(&directory).unwrap();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let link = root.path().join("linked");
        std::os::unix::fs::symlink(&directory, &link).unwrap();
        assert!(secure_directory(&link).is_err());
        let ordinary_file = root.path().join("file");
        std::fs::write(&ordinary_file, "not a directory").unwrap();
        assert!(secure_directory(&ordinary_file).is_err());
    }

    #[test]
    fn pictures_follow_user_dirs() {
        let home = Path::new("/home/user");
        let content =
            "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_PICTURES_DIR=\"$HOME/Bilder\"\n";
        assert_eq!(
            pictures_from_user_dirs(content, home),
            Some(PathBuf::from("/home/user/Bilder"))
        );
        assert_eq!(
            pictures_from_user_dirs("XDG_PICTURES_DIR=\"/srv/pics\"", home),
            Some(PathBuf::from("/srv/pics"))
        );
        // Disabled (the home itself) and missing both fall back.
        assert_eq!(
            pictures_from_user_dirs("XDG_PICTURES_DIR=\"$HOME/\"", home),
            None
        );
        assert_eq!(pictures_from_user_dirs("", home), None);
        assert_eq!(
            pictures_from_user_dirs("XDG_PICTURES_DIR=\"relative\"", home),
            None
        );
    }

    #[test]
    fn names_are_local_time_and_never_collide() {
        let at = chrono::FixedOffset::east_opt(10 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 10, 12, 14, 3, 22)
            .unwrap();
        let stem = shot_stem(&at);
        assert_eq!(stem, "20261012_140322");
        // Single digits are zero-padded, so names sort by time.
        let early = chrono::FixedOffset::east_opt(10 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
            .unwrap();
        assert_eq!(shot_stem(&early), "20260102_030405");
        let dir = tempfile::tempdir().unwrap();
        let shots = dir.path().join("Pictures/Screenshots");
        let image = RgbaImage::from_pixel(3, 2, image::Rgba([9, 9, 9, 255]));
        assert_eq!(free_name(&shots, &stem), shots.join("20261012_140322.png"));
        let first = save_into(&shots, &stem, &image).unwrap();
        assert_eq!(
            free_name(&shots, &stem),
            shots.join("20261012_140322_2.png")
        );
        let second = save_into(&shots, &stem, &image).unwrap();
        let third = save_into(&shots, &stem, &image).unwrap();
        assert_eq!(first, shots.join("20261012_140322.png"));
        assert_eq!(second, shots.join("20261012_140322_2.png"));
        assert_eq!(third, shots.join("20261012_140322_3.png"));
        for name in [
            "20261012_140322.png",
            "20261012_140322_2.png",
            "20261012_140322_12.png",
        ] {
            assert!(is_shot_name(name), "{name}");
        }
        for name in [
            "Screenshot 2026-10-12 14-03-22.png",
            "20261012_140322_1.png",
            "20261012_140322_02.png",
            "20261012_1403.png",
            "20261012_140322.jpg",
            "20261012_140322_2_3.png",
        ] {
            assert!(!is_shot_name(name), "{name}");
        }
        assert_eq!(image::open(&second).unwrap().to_rgba8(), image);
        // The umask decides, as for any file an application creates: the
        // same mode an ordinary 0666 creation gets here.
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let reference = dir.path().join("reference");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o666)
            .open(&reference)
            .unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&first), mode(&reference));
    }

    #[test]
    fn captures_are_private_bounded_and_forgotten_only_in_their_folder() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("captures");
        secure_directory(&dir).unwrap();
        let now = std::time::SystemTime::now();
        let day = std::time::Duration::from_secs(24 * 3600);
        let make = |name: &str, age: std::time::Duration| {
            let path = dir.join(name);
            std::fs::write(&path, b"png").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(now - age)
                .unwrap();
            path
        };
        let fresh: Vec<_> = (0..4)
            .map(|i| make(&format!("capture-{i}.png"), day * i))
            .collect();
        let stale = make("capture-old.png", day * 9);
        let other = make("notes.txt", day * 30);
        assert_eq!(prune_captures_at(&dir, 3, CAPTURES_MAX_AGE, now), 2);
        assert!(
            fresh[..3].iter().all(|p| p.exists()),
            "the newest three stay"
        );
        assert!(!fresh[3].exists(), "beyond the count");
        assert!(!stale.exists(), "beyond the age");
        assert!(other.exists(), "only captures are pruned");
        secure_capture(&fresh[0]).unwrap();
        assert_eq!(
            std::fs::metadata(&fresh[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let link = dir.join("capture-link.png");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(secure_capture(&link).is_err(), "a symlink is refused");
        forget_capture(&dir, &other);
        assert!(other.exists(), "not a capture");
        forget_capture(&root.path().join("elsewhere"), &fresh[0]);
        assert!(fresh[0].exists(), "not in that folder");
        forget_capture(&dir, &fresh[0]);
        assert!(!fresh[0].exists());
    }

    #[test]
    fn the_ledger_lists_existing_saves_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let ledger = Ledger::new(&state);
        assert!(ledger.recent(10).is_empty());
        let file = |name: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, b"png").unwrap();
            path
        };
        let saved = |path: PathBuf, annotated| Saved {
            path,
            width: 4,
            height: 3,
            annotated,
            saved_at: "2026-10-12T14:03:22+10:00".into(),
        };
        // A shot saved under the earlier name form still lists.
        let a = file("Screenshot 2026-10-12 14-03-22.png");
        let b = file("20261012_140323.png");
        ledger.append(&saved(a.clone(), false)).unwrap();
        ledger.append(&saved(b.clone(), true)).unwrap();
        ledger
            .append(&saved(dir.path().join("gone.png"), true))
            .unwrap();
        // A save again of the same path counts once, as its newest.
        ledger.append(&saved(a.clone(), true)).unwrap();
        let recent = ledger.recent(10);
        assert_eq!(
            recent.iter().map(|s| s.path.clone()).collect::<Vec<_>>(),
            [a.clone(), b]
        );
        assert!(recent[0].annotated);
        assert_eq!(ledger.recent(1).len(), 1);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(ledger.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn a_large_ledger_is_compacted_to_its_newest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::new(dir.path());
        let shot = dir.path().join("shot.png");
        std::fs::write(&shot, b"png").unwrap();
        let long = "x".repeat(1500);
        for i in 0..400 {
            ledger
                .append(&Saved {
                    path: dir.path().join(format!("{long}-{i}.png")),
                    width: 1,
                    height: 1,
                    annotated: false,
                    saved_at: String::new(),
                })
                .unwrap();
        }
        assert!(std::fs::metadata(ledger.path()).unwrap().len() <= LEDGER_COMPACT_BYTES + 2048);
        ledger
            .append(&Saved {
                path: shot.clone(),
                width: 1,
                height: 1,
                annotated: false,
                saved_at: String::new(),
            })
            .unwrap();
        assert_eq!(ledger.recent(5)[0].path, shot);
    }
}
