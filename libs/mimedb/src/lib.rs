// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! mimedb: the freedesktop shared-mime-info database, read from the same
//! files the desktop reads (`globs2`, `magic`, `aliases`, `subclasses` under
//! each `$XDG_DATA_HOME`/`$XDG_DATA_DIRS` `mime` directory), and a file's
//! type decided the way GLib decides it:
//!
//! 1. a non-regular file is its `inode/*` type, and an empty one is
//!    `application/x-zerosize`;
//! 2. one glob match is the answer;
//! 3. otherwise the content is sniffed: magic, or `text/plain` when it looks
//!    like text;
//! 4. with no glob match the sniffed type is the answer; with several, a
//!    sniffed type of priority 80 or more wins, else the first glob type that
//!    is a subclass of the sniffed type, else the first glob type, uncertain.
//!
//! Glob matching follows GLib's order: a literal name, then the longest
//! `*.suffix`, then full patterns.
//!
//! **One known limit.** Full patterns use this crate's own `fnmatch`, not
//! glibc's. Wildcards, ASCII ranges and escapes match exactly. POSIX named
//! classes use Unicode's predicates rather than a locale's tables, and
//! collating symbols cannot end a range, so a pattern using those can be
//! decided differently from GIO for rare characters. The stock database
//! uses none of them.

mod apps;
mod glob;
mod magic;

pub use apps::{apps_in, cache_paths, mimeapps_paths};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How a type was decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By {
    /// A directory or other non-regular file.
    Stat,
    /// The file name.
    Glob,
    /// A magic rule.
    Magic,
    /// An empty file, or the text check.
    Content,
    /// Nothing matched: `application/octet-stream`.
    Fallback,
}

impl By {
    pub fn as_str(self) -> &'static str {
        match self {
            By::Stat => "stat",
            By::Glob => "glob",
            By::Magic => "magic",
            By::Content => "content",
            By::Fallback => "fallback",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guess {
    pub mime: String,
    pub by: By,
    pub uncertain: bool,
}

/// A file's type, from [`Database::for_path`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathGuess {
    pub guess: Guess,
    /// Whether the path existed (a missing one is typed by name).
    pub exists: bool,
}

pub const OCTET_STREAM: &str = "application/octet-stream";
pub const TEXT_PLAIN: &str = "text/plain";
pub const ZEROSIZE: &str = "application/x-zerosize";
/// GLib sniffs at most this many bytes (its buffer in GLocalFile).
pub const SNIFF_MAX: usize = 16384;

/// A database file larger than this is skipped: the stock database is a few
/// hundred KiB, and a hostile one must not exhaust memory.
pub const MAX_FILE: u64 = 16 * 1024 * 1024;

/// The files read from each `mime` directory.
const FILES: [&str; 5] = ["globs2", "globs", "magic", "aliases", "subclasses"];

/// What a directory looked like when read: its own timestamp and each
/// file's timestamp and size, so a file rewritten in place is noticed.
type Fingerprint = Vec<Option<(SystemTime, u64)>>;

/// The loaded database.
#[derive(Clone, Debug, Default)]
pub struct Database {
    /// The `mime` directories, highest precedence first, as read.
    dirs: Vec<(PathBuf, Fingerprint)>,
    /// Each directory's globs, highest precedence first.
    globs: Vec<Vec<glob::Glob>>,
    magic: Vec<magic::Section>,
    extent: usize,
    aliases: HashMap<String, String>,
    parents: HashMap<String, Vec<String>>,
}

/// The `mime` directories the desktop reads, highest precedence first:
/// `$XDG_DATA_HOME` (else `~/.local/share`), then `$XDG_DATA_DIRS` (else
/// `/usr/local/share:/usr/share`). Relative entries are ignored, as the XDG
/// spec requires.
pub fn xdg_mime_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        Some(home) if home.is_absolute() => dirs.push(home),
        _ => {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    match std::env::var_os("XDG_DATA_DIRS") {
        Some(v) if !v.is_empty() => {
            dirs.extend(std::env::split_paths(&v).filter(|p| p.is_absolute()))
        }
        _ => dirs.extend([
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]),
    }
    dirs.into_iter().map(|d| d.join("mime")).collect()
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn fingerprint(dir: &Path) -> Fingerprint {
    std::iter::once(stamp(dir))
        .chain(FILES.iter().map(|f| stamp(&dir.join(f))))
        .collect()
}

/// A database file's bytes, or nothing when it is missing or too large.
///
/// It is opened without blocking and must be a regular file once open: a
/// FIFO or device planted in a database directory is skipped, not waited on.
fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_FILE).then_some(bytes)
}

fn read_text(path: &Path) -> Option<String> {
    read_bounded(path).map(|b| String::from_utf8_lossy(&b).into_owned())
}

impl Database {
    /// The desktop's database ([`xdg_mime_dirs`]).
    pub fn load() -> Self {
        Self::load_from(&xdg_mime_dirs())
    }

    /// The database in `dirs`, highest precedence first. Missing directories
    /// and files are skipped.
    pub fn load_from(dirs: &[PathBuf]) -> Self {
        let mut db = Database {
            dirs: dirs.iter().map(|d| (d.clone(), fingerprint(d))).collect(),
            ..Default::default()
        };
        // Lowest precedence first, so a higher directory clears or overrides.
        let mut glob_layers: Vec<Vec<glob::Glob>> = Vec::new();
        let mut magic_layers: Vec<Vec<magic::Section>> = Vec::new();
        // Parent edges already seen: a hostile fan-out stays linear.
        let mut edges: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for dir in dirs.iter().rev() {
            let (mut globs, cleared) = match read_text(&dir.join("globs2")) {
                Some(text) => glob::parse(&text, true),
                None => match read_text(&dir.join("globs")) {
                    Some(text) => glob::parse(&text, false),
                    None => (Vec::new(), Vec::new()),
                },
            };
            for mime in &cleared {
                for layer in &mut glob_layers {
                    layer.retain(|g| &g.mime != mime);
                }
            }
            glob::merge_case(&mut globs);
            glob_layers.push(globs);

            let sections = read_bounded(&dir.join("magic"))
                .map(|b| magic::parse(&b))
                .unwrap_or_default();
            for section in sections.iter().filter(|s| s.cleared) {
                for layer in &mut magic_layers {
                    layer.retain(|s| s.mime != section.mime);
                }
            }
            magic_layers.push(sections.into_iter().filter(|s| s.has_rules()).collect());

            if let Some(text) = read_text(&dir.join("aliases")) {
                for line in text.lines() {
                    let mut words = line.split_whitespace();
                    if let (Some(alias), Some(canonical)) = (words.next(), words.next()) {
                        db.aliases.insert(alias.to_owned(), canonical.to_owned());
                    }
                }
            }
            if let Some(text) = read_text(&dir.join("subclasses")) {
                for line in text.lines() {
                    let mut words = line.split_whitespace();
                    if let (Some(child), Some(parent)) = (words.next(), words.next())
                        && edges.insert((child.to_owned(), parent.to_owned()))
                    {
                        db.parents
                            .entry(child.to_owned())
                            .or_default()
                            .push(parent.to_owned());
                    }
                }
            }
        }
        // Highest precedence first; magic by priority, stable.
        glob_layers.reverse();
        db.globs = glob_layers;
        db.magic = magic_layers.into_iter().rev().flatten().collect();
        db.magic.sort_by_key(|s| std::cmp::Reverse(s.priority));
        db.extent = db.magic.iter().map(|s| s.extent()).max().unwrap_or(0);
        db
    }

    /// Whether anything read has changed since loading: a directory, or any
    /// of its files (timestamp or size).
    pub fn is_stale(&self) -> bool {
        self.dirs
            .iter()
            .any(|(dir, seen)| fingerprint(dir) != *seen)
    }

    /// How many leading bytes to sniff: what the magic rules can reach,
    /// capped as GLib caps it (`SNIFF_MAX`), which also bounds the read.
    pub fn sniff_len(&self) -> usize {
        if self.extent == 0 || self.extent > SNIFF_MAX {
            SNIFF_MAX
        } else {
            self.extent
        }
    }

    /// The `mime` directories this database was loaded from.
    pub fn dirs(&self) -> Vec<PathBuf> {
        self.dirs.iter().map(|(d, _)| d.clone()).collect()
    }

    /// The canonical name of `mime`.
    pub fn unalias<'a>(&'a self, mime: &'a str) -> &'a str {
        self.aliases.get(mime).map(String::as_str).unwrap_or(mime)
    }

    /// The types `name` matches by glob, best first.
    pub fn globs(&self, name: &str) -> Vec<String> {
        glob::candidates(&self.globs, name)
            .into_iter()
            .map(|(m, _)| self.unalias(&m).to_owned())
            .fold(Vec::new(), |mut out, m| {
                if !out.contains(&m) {
                    out.push(m);
                }
                out
            })
    }

    /// The highest-priority magic match for `data`, with its priority.
    pub fn sniff(&self, data: &[u8]) -> Option<(String, u32)> {
        self.magic
            .iter()
            .find(|s| s.priority > 0 && s.matches(data))
            .map(|s| (self.unalias(&s.mime).to_owned(), s.priority))
    }

    /// Whether `mime` is `base` or one of its subtypes, including the
    /// spec's implicit ones: `media/*`, `text/*` under `text/plain`, and
    /// every non-`inode/*` type under `application/octet-stream`. Each type
    /// is visited once, so a cycle in `subclasses` costs nothing extra.
    pub fn is_subclass(&self, mime: &str, base: &str) -> bool {
        let base = self.unalias(base);
        let implied = |m: &str| {
            m == base
                || base
                    .strip_suffix("/*")
                    .is_some_and(|media| m.split('/').next() == Some(media))
                || (base == TEXT_PLAIN && m.starts_with("text/"))
                || (base == OCTET_STREAM && !m.starts_with("inode/"))
        };
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut queue: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
        queue.push_back(self.unalias(mime));
        while let Some(next) = queue.pop_front() {
            if !seen.insert(next) {
                continue;
            }
            if implied(next) {
                return true;
            }
            if let Some(parents) = self.parents.get(next) {
                queue.extend(parents.iter().map(|p| self.unalias(p)));
            }
        }
        false
    }

    /// Every supertype of `mime`, nearest first: declared parents breadth
    /// first, then `text/plain` when the type or any ancestor is `text/*`,
    /// then `application/octet-stream` for anything but `inode/*`.
    pub fn parents(&self, mime: &str) -> Vec<String> {
        let start = self.unalias(mime).to_owned();
        let mut out: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        seen.insert(start.clone());
        let mut queue: std::collections::VecDeque<String> =
            self.parents.get(&start).cloned().unwrap_or_default().into();
        while let Some(next) = queue.pop_front() {
            let next = self.unalias(&next).to_owned();
            if !seen.insert(next.clone()) {
                continue;
            }
            if let Some(more) = self.parents.get(&next) {
                queue.extend(more.iter().cloned());
            }
            out.push(next);
        }
        let textual = std::iter::once(&start)
            .chain(out.iter())
            .any(|m| m.starts_with("text/"));
        if textual && start != TEXT_PLAIN && !out.iter().any(|p| p == TEXT_PLAIN) {
            out.push(TEXT_PLAIN.to_owned());
        }
        if !start.starts_with("inode/")
            && start != OCTET_STREAM
            && !out.iter().any(|p| p == OCTET_STREAM)
        {
            out.push(OCTET_STREAM.to_owned());
        }
        out
    }

    /// Decide a type from a file name and, when given, its leading bytes.
    pub fn guess(&self, name: Option<&str>, data: Option<&[u8]>) -> Guess {
        let names = name.map(|n| self.globs(n)).unwrap_or_default();
        if names.len() == 1 {
            return Guess {
                mime: names[0].clone(),
                by: By::Glob,
                uncertain: false,
            };
        }
        // An empty buffer, when one is given, is a zero-size file.
        if data.is_some_and(<[u8]>::is_empty) {
            return Guess {
                mime: ZEROSIZE.to_owned(),
                by: By::Content,
                uncertain: false,
            };
        }
        let mut sniffed: Option<(String, u32, By)> = None;
        if let Some(data) = data {
            sniffed = match self.sniff(data) {
                Some((mime, prio)) => Some((mime, prio, By::Magic)),
                None if text_fallback(data) || looks_like_text(data) => {
                    Some((TEXT_PLAIN.to_owned(), 0, By::Content))
                }
                None => None,
            };
            // A desktop entry never hides under another name.
            if name.is_some()
                && let Some((mime, _, by)) = sniffed.as_mut()
                && mime == "application/x-desktop"
            {
                *mime = TEXT_PLAIN.to_owned();
                *by = By::Content;
            }
        }
        if names.is_empty() {
            return match sniffed {
                Some((mime, _, by)) => Guess {
                    mime,
                    by,
                    uncertain: false,
                },
                None => Guess {
                    mime: OCTET_STREAM.to_owned(),
                    by: By::Fallback,
                    uncertain: true,
                },
            };
        }
        if let Some((mime, prio, by)) = &sniffed {
            if *prio >= 80 {
                return Guess {
                    mime: mime.clone(),
                    by: *by,
                    uncertain: false,
                };
            }
            if let Some(found) = names.iter().find(|n| self.is_subclass(n, mime)) {
                return Guess {
                    mime: found.clone(),
                    by: By::Glob,
                    uncertain: false,
                };
            }
        }
        Guess {
            mime: names[0].clone(),
            by: By::Glob,
            uncertain: true,
        }
    }

    /// The type of the file at `path`, following symlinks. With `content`
    /// false the file is never opened. A missing path is typed by its name;
    /// so is one this process may not read, uncertain.
    pub fn for_path(&self, path: &Path, content: bool) -> PathGuess {
        use std::io::Read;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let meta = std::fs::metadata(path).ok();
        let exists = meta.is_some();
        if let Some(meta) = meta.as_ref() {
            if !meta.is_file() {
                return PathGuess {
                    guess: Guess {
                        mime: inode_type(meta).to_owned(),
                        by: By::Stat,
                        uncertain: false,
                    },
                    exists,
                };
            }
            if meta.len() == 0 {
                return PathGuess {
                    guess: Guess {
                        mime: ZEROSIZE.to_owned(),
                        by: By::Content,
                        uncertain: false,
                    },
                    exists,
                };
            }
        }
        let mut data = Vec::new();
        let mut unreadable = false;
        if content && exists {
            let read = std::fs::File::open(path)
                .and_then(|f| f.take(self.sniff_len() as u64).read_to_end(&mut data));
            if read.is_err() {
                data.clear();
                unreadable = true;
            }
        }
        let sniff =
            (content && exists && !unreadable && !data.is_empty()).then_some(data.as_slice());
        let mut guess = self.guess(name.as_deref(), sniff);
        if unreadable {
            guess.uncertain = true;
        }
        PathGuess { guess, exists }
    }
}

/// xdgmime's answer when no magic rule matches: text when the first 128
/// bytes hold no control byte but tab, line feed and carriage return.
pub fn text_fallback(data: &[u8]) -> bool {
    data.iter()
        .take(128)
        .all(|&b| b >= 32 || matches!(b, b'\t' | b'\n' | b'\r'))
}

/// GLib's second test, over everything sniffed: no control character other
/// than whitespace and backspace.
pub fn looks_like_text(data: &[u8]) -> bool {
    data.iter()
        .all(|&b| !b.is_ascii_control() || b.is_ascii_whitespace() || b == 0x08 || b == 0x0b)
}

/// The `inode/*` type of a non-regular file.
fn inode_type(meta: &std::fs::Metadata) -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    let t = meta.file_type();
    if t.is_dir() {
        "inode/directory"
    } else if t.is_fifo() {
        "inode/fifo"
    } else if t.is_socket() {
        "inode/socket"
    } else if t.is_char_device() {
        "inode/chardevice"
    } else if t.is_block_device() {
        "inode/blockdevice"
    } else {
        OCTET_STREAM
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_fallback_reads_128_bytes_and_the_second_test_reads_all() {
        let mut data = vec![b'a'; 130];
        data.push(0);
        assert!(text_fallback(&data));
        assert!(!looks_like_text(&data));
        data[5] = 0x08;
        assert!(!text_fallback(&data));
        data.pop();
        assert!(looks_like_text(&data));
    }

    /// A fresh `<tmp>/<name>/mime` holding `files`.
    fn mime_dir(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mimedb-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("mime");
        std::fs::create_dir_all(&dir).unwrap();
        for (file, bytes) in files {
            std::fs::write(dir.join(file), bytes).unwrap();
        }
        dir
    }

    #[test]
    fn cycles_end_and_text_ancestry_is_inherited() {
        let dir = mime_dir(
            "cycles",
            &[(
                "subclasses",
                b"application/x-a application/x-b\napplication/x-b application/x-a\napplication/x-child text/x-parent\n",
            )],
        );
        let db = Database::load_from(&[dir]);
        assert!(!db.is_subclass("application/x-a", "image/png"));
        assert!(db.is_subclass("application/x-a", "application/x-b"));
        assert!(db.is_subclass("application/x-child", TEXT_PLAIN));
        assert_eq!(
            db.parents("application/x-child"),
            vec!["text/x-parent", TEXT_PLAIN, OCTET_STREAM]
        );
        assert_eq!(
            db.parents("application/x-a"),
            vec!["application/x-b", OCTET_STREAM]
        );
    }

    #[test]
    fn a_diamond_of_cycles_is_cheap() {
        let mut lines = String::new();
        for i in 0..40 {
            for j in [i + 1, i + 2] {
                lines.push_str(&format!("application/x-{i} application/x-{}\n", j % 40));
            }
        }
        let dir = mime_dir("diamond", &[("subclasses", lines.as_bytes())]);
        let db = Database::load_from(&[dir]);
        assert!(!db.is_subclass("application/x-0", "image/png"));
        assert!(db.is_subclass("application/x-0", "application/x-39"));
    }

    #[test]
    fn a_wide_fan_out_and_a_long_chain_stay_linear() {
        let mut lines = String::new();
        for i in 0..50_000 {
            lines.push_str(&format!("application/x-child application/x-p{i}\n"));
        }
        for i in 0..5_000 {
            lines.push_str(&format!("application/x-c{i} application/x-c{}\n", i + 1));
        }
        let dir = mime_dir("fanout", &[("subclasses", lines.as_bytes())]);
        let started = std::time::Instant::now();
        let db = Database::load_from(&[dir]);
        assert_eq!(db.parents("application/x-child").len(), 50_001);
        assert_eq!(db.parents("application/x-c0").len(), 5_001);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn priority_zero_magic_never_wins() {
        let dir = mime_dir(
            "zero",
            &[("magic", b"MIME-Magic\0\n[0:image/x-zero]\n>0=\0\x01A\n")],
        );
        let db = Database::load_from(&[dir]);
        assert_eq!(db.sniff(b"A"), None);
        assert_eq!(db.guess(None, Some(b"A")).mime, TEXT_PLAIN);
        assert_eq!(db.guess(None, Some(b"A\0")).mime, OCTET_STREAM);
    }

    #[test]
    fn a_same_size_rewrite_is_noticed() {
        let dir = mime_dir("rewrite", &[("globs2", b"50:image/x-a:*.aaa\n")]);
        let db = Database::load_from(std::slice::from_ref(&dir));
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("globs2"), b"50:image/x-b:*.aaa\n").unwrap();
        assert!(db.is_stale());
    }

    #[test]
    fn higher_directories_clear_lower_ones_and_rewrites_are_noticed() {
        let low = mime_dir(
            "low",
            &[
                ("globs2", b"50:image/x-low:*.low\n50:image/x-gone:*.gone\n"),
                ("magic", b"MIME-Magic\0\n[50:image/x-m]\n>0=\0\x01M\n"),
            ],
        );
        let high = mime_dir(
            "high",
            &[
                ("globs2", b"0:image/x-gone:__NOGLOBS__\n"),
                (
                    "magic",
                    b"MIME-Magic\0\n[50:image/x-m]\n>0=\0\x0b__NOMAGIC__\n>0=\0\x01N\n",
                ),
            ],
        );
        let db = Database::load_from(&[high.clone(), low.clone()]);
        assert!(db.globs("a.gone").is_empty());
        assert_eq!(db.globs("a.low"), vec!["image/x-low"]);
        assert_eq!(db.sniff(b"M"), None);
        assert_eq!(db.sniff(b"N"), Some(("image/x-m".to_owned(), 50)));
        assert!(!db.is_stale());
        std::fs::write(low.join("globs2"), b"50:image/x-low:*.low\n").unwrap();
        assert!(db.is_stale());
    }

    #[test]
    fn an_empty_buffer_is_zero_size_and_none_is_names_only() {
        let db = Database::default();
        let guess = db.guess(None, Some(&[]));
        assert_eq!((guess.mime.as_str(), guess.uncertain), (ZEROSIZE, false));
        let guess = db.guess(Some("x"), None);
        assert_eq!(
            (guess.mime.as_str(), guess.by),
            (OCTET_STREAM, By::Fallback)
        );
    }

    #[test]
    fn an_oversized_file_is_skipped() {
        let mut big = b"50:image/x-big:*.big\n".to_vec();
        big.resize(MAX_FILE as usize + 1, b'#');
        let dir = mime_dir("big", &[("globs2", &big)]);
        assert!(Database::load_from(&[dir]).globs("a.big").is_empty());
    }
}
