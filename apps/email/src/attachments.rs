// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Attachments: what the popup shows for each one, and how its bytes reach
//! the disk.
//!
//! The pure half decides names and refusals: [`sanitise`] makes a sender's
//! file name safe to create (no separators, no `..`, no control characters,
//! no leading dots, at most [`NAME_MAX`] bytes), [`numbered`] is the name a
//! collision falls back to (`name (1).ext`), and [`refuses_open`] keeps
//! programs and scripts from being opened (they can only be saved).
//!
//! The disk half runs on the shell's blocking pool, never the UI thread:
//! [`save_in`] writes a new 0600 file atomically (a temporary file, synced,
//! then renamed into place without replacing anything), [`download_dir`]
//! is the XDG download directory, and [`stage`] puts a file to open in
//! `$XDG_RUNTIME_DIR/email-attach/` (0700).

use std::ffi::CString;
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use jmap::{Attachment, Email};
use serde_json::{Value, json};
use toolkit::Icon;

/// The longest name written, in bytes.
pub const NAME_MAX: usize = 200;

/// The runtime directory's subdirectory files to open are staged in.
pub const STAGE_DIR: &str = "email-attach";

/// Types never opened: native programs and libraries, scripts, desktop
/// launchers and AppImages.
const PROGRAM_TYPES: &[&str] = &[
    "application/x-executable",
    "application/x-pie-executable",
    "application/x-sharedlib",
    "application/x-elf",
    "application/x-object",
    "application/x-core",
    "application/x-mach-binary",
    "application/x-msdownload",
    "application/x-ms-dos-executable",
    "application/x-ms-shortcut",
    "application/x-msi",
    "application/vnd.microsoft.portable-executable",
    "application/x-shellscript",
    "application/x-sh",
    "application/x-csh",
    "text/x-shellscript",
    "text/x-sh",
    "application/x-desktop",
    "application/x-appimage",
    "application/vnd.appimage",
    "application/x-iso9660-appimage",
    "application/x-perl",
    "text/x-perl",
    "application/x-python",
    "text/x-python",
    "text/x-python3",
    "application/x-ruby",
    "application/x-bat",
    "application/x-java-archive",
    "application/java-archive",
];

/// Extensions never opened, whatever type the sender claimed.
const PROGRAM_EXTENSIONS: &[&str] = &[
    "sh", "bash", "zsh", "csh", "ksh", "fish", "mix", "desktop", "appimage", "run", "bin", "elf",
    "so", "out", "exe", "com", "scr", "msi", "bat", "cmd", "ps1", "vbs", "jar", "py", "pl", "rb",
    "command", "lnk",
];

/// Whether an attachment of type `mime` named `name` must not be opened:
/// a program, a library or a script, by its type or by its extension.
pub fn refuses_open(mime: &str, name: Option<&str>) -> bool {
    let mime = mime.trim().to_ascii_lowercase();
    let by_type = PROGRAM_TYPES.contains(&mime.as_str())
        || mime.contains("executable")
        || (mime.ends_with("script") && mime != "application/postscript")
        || mime.ends_with("-sharedlib");
    let by_name = name
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .is_some_and(|ext| PROGRAM_EXTENSIONS.contains(&ext.as_str()));
    by_type || by_name
}

/// Whether downloaded `data` is a program whatever its label said: an ELF
/// image, a script with a `#!` line, a Windows executable, or a desktop
/// launcher.
pub fn looks_like_program(data: &[u8]) -> bool {
    data.starts_with(b"\x7fELF")
        || data.starts_with(b"#!")
        || data.starts_with(b"MZ")
        || data.starts_with(b"[Desktop Entry]")
}

/// `name` made safe to create in a directory: separators, `..`, control
/// characters and leading dots and spaces stripped, trailing spaces and
/// dots too, and at most [`NAME_MAX`] bytes (the extension kept when it is
/// short). `None` when nothing is left.
pub fn sanitise(name: &str) -> Option<String> {
    let mut clean: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .collect();
    while clean.contains("..") {
        clean = clean.replace("..", ".");
    }
    let clean = clean
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' ']);
    if clean.is_empty() {
        return None;
    }
    if clean.len() <= NAME_MAX {
        return Some(clean.to_owned());
    }
    let (stem, ext) = split(clean);
    let ext = ext.filter(|e| e.len() < 16).unwrap_or("");
    let room = NAME_MAX - if ext.is_empty() { 0 } else { ext.len() + 1 };
    let stem = truncate(stem, room);
    Some(if ext.is_empty() {
        stem.to_owned()
    } else {
        format!("{stem}.{ext}")
    })
}

/// The longest prefix of `text` of at most `max` bytes, on a character
/// boundary.
fn truncate(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A name's stem and extension; a leading dot is not an extension's.
fn split(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => (&name[..dot], Some(&name[dot + 1..])),
        _ => (name, None),
    }
}

/// The name for the `n`th attachment (from 1) when it has none, with `ext`
/// (the type's usual extension) when one is known.
pub fn fallback(n: usize, ext: Option<&str>) -> String {
    match ext.filter(|e| !e.is_empty()) {
        Some(ext) => format!("attachment-{n}.{ext}"),
        None => format!("attachment-{n}"),
    }
}

/// `name`'s `k`th alternative for a collision: `report (1).pdf`; the
/// whole stays within [`NAME_MAX`].
pub fn numbered(name: &str, k: u32) -> String {
    if k == 0 {
        return name.to_owned();
    }
    let (stem, ext) = split(name);
    let tail = match ext {
        Some(ext) => format!(" ({k}).{ext}"),
        None => format!(" ({k})"),
    };
    let stem = truncate(stem, NAME_MAX.saturating_sub(tail.len()));
    format!("{stem}{tail}")
}

/// The name an attachment is written under: its own name, sanitised, else
/// `attachment-<n>` with `ext`.
pub fn file_name(name: Option<&str>, n: usize, ext: Option<&str>) -> String {
    name.and_then(sanitise).unwrap_or_else(|| fallback(n, ext))
}

/// A size as a person reads it: `512 B`, `48 KB`, `1.2 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// The icon for a type.
pub fn icon(mime: &str, name: Option<&str>) -> Icon {
    let (media, sub) = mime.split_once('/').unwrap_or((mime, ""));
    if refuses_open(mime, name) {
        return Icon::FileTerminal;
    }
    match media {
        "image" => Icon::FileImage,
        "audio" => Icon::FileMusic,
        "video" => Icon::FileVideoCamera,
        _ if sub.contains("zip")
            || sub.contains("tar")
            || sub.contains("compressed")
            || sub.contains("rar")
            || sub.contains("7z")
            || sub.contains("gzip")
            || sub.contains("bzip")
            || sub.contains("xz") =>
        {
            Icon::FileArchive
        }
        _ if sub.contains("spreadsheet") || sub == "csv" || sub.contains("excel") => {
            Icon::FileSpreadsheet
        }
        _ if sub == "json"
            || sub == "xml"
            || sub.contains("javascript")
            || sub.ends_with("+xml") =>
        {
            Icon::FileCode
        }
        "text" => Icon::FileText,
        _ if sub == "pdf" || sub.contains("document") || sub.contains("word") || sub == "rtf" => {
            Icon::FileText
        }
        _ => Icon::File,
    }
}

/// `email`'s attachments, as the server listed them.
pub fn of(email: &Email) -> &[Attachment] {
    email.attachments.as_deref().unwrap_or(&[])
}

/// `email.attachments`'s answer for `email`.
pub fn describe(email: &Email) -> Value {
    let list: Vec<Value> = of(email)
        .iter()
        .map(|a| {
            let mime = a.mime();
            json!({
                "part": a.part_id,
                "name": a.name,
                "type": mime,
                "size": a.size,
                "disposition": a.disposition,
                "cid": a.cid,
                "downloadable": a.downloadable(),
                "openable": a.downloadable() && !refuses_open(&mime, a.name.as_deref()),
            })
        })
        .collect();
    json!({ "id": email.id, "attachments": list })
}

/// The XDG download directory: `XDG_DOWNLOAD_DIR` from
/// `user-dirs.dirs` (under `$XDG_CONFIG_HOME`, else `~/.config`), else
/// `~/Downloads`.
pub fn download_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let named = std::fs::read_to_string(config.join("user-dirs.dirs"))
        .ok()
        .and_then(|text| user_download_dir(&text, &home));
    Some(named.unwrap_or_else(|| home.join("Downloads")))
}

/// `XDG_DOWNLOAD_DIR` from a `user-dirs.dirs` text: `"$HOME/…"` or an
/// absolute path, as xdg-user-dirs writes it. `$HOME` itself (the spec's
/// "disabled") and anything else are not a download directory.
pub fn user_download_dir(text: &str, home: &Path) -> Option<PathBuf> {
    let line = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix("XDG_DOWNLOAD_DIR="))?;
    let value = line.trim().strip_prefix('"')?.strip_suffix('"')?;
    let path = match value.strip_prefix("$HOME") {
        Some(rest) if rest.is_empty() || rest == "/" => return None,
        Some(rest) => home.join(rest.strip_prefix('/')?),
        None if value.starts_with('/') => PathBuf::from(value),
        None => return None,
    };
    Some(path)
}

/// Rename `from` to `to` only if `to` does not exist (`renameat2` with
/// `RENAME_NOREPLACE`; a hard link and unlink where the filesystem has no
/// such rename). `Ok(false)` when `to` exists.
fn rename_new(from: &Path, to: &Path) -> std::io::Result<bool> {
    let cstr = |p: &Path| {
        CString::new(p.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    };
    let (a, b) = (cstr(from)?, cstr(to)?);
    // SAFETY: both are valid NUL-terminated paths for the call's duration.
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EEXIST) => Ok(false),
        Some(libc::EINVAL | libc::ENOSYS) => match std::fs::hard_link(from, to) {
            Ok(()) => {
                std::fs::remove_file(from)?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e),
        },
        _ => Err(error),
    }
}

/// Write `data` as a new 0600 file named `name` in `dir`, atomically and
/// never over an existing file (`name (1).ext`, `name (2).ext`, … instead);
/// the path written.
pub fn save_in(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf, String> {
    let shown = |e: std::io::Error, p: &Path| format!("{}: {e}", p.display());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temp = dir.join(format!(".email-{}-{stamp}.part", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| shown(e, &temp))?;
    let written = file.write_all(data).and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(shown(e, &temp));
    }
    for k in 0..10_000 {
        let path = dir.join(numbered(name, k));
        match rename_new(&temp, &path) {
            Ok(true) => return Ok(path),
            Ok(false) => continue,
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                return Err(shown(e, &path));
            }
        }
    }
    let _ = std::fs::remove_file(&temp);
    Err(format!(
        "{}: {name}: every alternative name is taken",
        dir.display()
    ))
}

/// The usual extension of `mime`, from the desktop's MIME database (loaded
/// once).
pub fn extension(mime: &str) -> Option<String> {
    static DB: std::sync::OnceLock<mimedb::Database> = std::sync::OnceLock::new();
    DB.get_or_init(mimedb::Database::load).extension(mime)
}

/// The applications to open staged file `path` with, best first (desktop
/// ids): those of the sender's type `mime`, or of the file's own type when
/// the sender's says nothing. A file whose own type is a program is
/// removed and refused. Blocking.
pub fn open_with(path: &Path, mime: &str) -> Result<Vec<String>, String> {
    let db = mimedb::Database::load();
    let mime = if mime.is_empty() || mime == mimedb::OCTET_STREAM {
        db.for_path(path, true).guess.mime
    } else {
        mime.to_owned()
    };
    let name = path.file_name().and_then(|n| n.to_str());
    if refuses_open(&mime, name) {
        let _ = std::fs::remove_file(path);
        return Err(crate::label_with(
            "attachment-program",
            &[("name", name.unwrap_or(""))],
        ));
    }
    Ok(db.apps_for(&mime))
}

/// Where an attachment goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Saved in this directory (the download directory when `None`).
    Save(Option<PathBuf>),
    /// Staged in the runtime directory and opened in its default app.
    Open,
}

/// Put downloaded `data` where `target` says, named from `name` (or the
/// `n`th attachment's fallback with `mime`'s extension). Blocking. An
/// Open of something that turns out to be a program is refused here, from
/// its bytes, before anything is written.
pub fn place(
    target: &Target,
    name: Option<&str>,
    n: usize,
    mime: &str,
    data: &[u8],
) -> Result<PathBuf, String> {
    let file = file_name(name, n, extension(mime).as_deref());
    match target {
        Target::Save(dir) => {
            let dir = match dir {
                Some(dir) => dir.clone(),
                None => {
                    let dir = download_dir().ok_or_else(|| crate::label("no-download-dir"))?;
                    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                    dir
                }
            };
            save_in(&dir, &file, data)
        }
        Target::Open => {
            if looks_like_program(data) {
                return Err(crate::label_with("attachment-program", &[("name", &file)]));
            }
            let dir = crate::print::runtime_dir(STAGE_DIR)?;
            crate::print::prune(&dir, crate::print::KEEP);
            save_in(&dir, &file, data)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_name_loses_separators_dots_and_control_characters() {
        assert_eq!(sanitise("report.pdf").as_deref(), Some("report.pdf"));
        assert_eq!(sanitise("../../etc/passwd").as_deref(), Some("etcpasswd"));
        assert_eq!(sanitise("a\\b/c.txt").as_deref(), Some("abc.txt"));
        assert_eq!(sanitise(".bashrc").as_deref(), Some("bashrc"));
        assert_eq!(sanitise("...hidden..txt").as_deref(), Some("hidden.txt"));
        assert_eq!(
            sanitise("bad\u{0}na\u{7}me\n.txt").as_deref(),
            Some("badname.txt")
        );
        assert_eq!(sanitise("  spaced.txt. ").as_deref(), Some("spaced.txt"));
        assert_eq!(sanitise(".."), None);
        assert_eq!(sanitise("/"), None);
        assert_eq!(sanitise(""), None);
    }

    #[test]
    fn a_long_name_is_capped_keeping_its_extension() {
        let long = format!("{}.pdf", "x".repeat(400));
        let capped = sanitise(&long).unwrap();
        assert_eq!(capped.len(), NAME_MAX);
        assert!(capped.ends_with("x.pdf"));
        // Never through the middle of a character.
        let wide = format!("{}.txt", "é".repeat(150));
        let capped = sanitise(&wide).unwrap();
        assert!(capped.len() <= NAME_MAX && capped.ends_with(".txt"));
    }

    #[test]
    fn a_nameless_attachment_is_numbered_with_its_types_extension() {
        assert_eq!(file_name(None, 2, Some("pdf")), "attachment-2.pdf");
        assert_eq!(file_name(Some("../"), 3, None), "attachment-3");
        assert_eq!(file_name(Some("notes.md"), 1, Some("txt")), "notes.md");
    }

    #[test]
    fn a_collision_takes_the_next_number() {
        assert_eq!(numbered("report.pdf", 0), "report.pdf");
        assert_eq!(numbered("report.pdf", 1), "report (1).pdf");
        assert_eq!(numbered("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(numbered("README", 3), "README (3)");
        let long = "y".repeat(NAME_MAX - 4) + ".txt";
        assert!(numbered(&long, 12).len() <= NAME_MAX);
        let dir = tempfile::tempdir().unwrap();
        let first = save_in(dir.path(), "report.pdf", b"one").unwrap();
        let second = save_in(dir.path(), "report.pdf", b"two").unwrap();
        let third = save_in(dir.path(), "report.pdf", b"three").unwrap();
        assert_eq!(first, dir.path().join("report.pdf"));
        assert_eq!(second, dir.path().join("report (1).pdf"));
        assert_eq!(third, dir.path().join("report (2).pdf"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one", "never overwritten");
        assert_eq!(std::fs::read(&third).unwrap(), b"three");
        let mode = std::fs::metadata(&second).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn programs_and_scripts_are_never_opened() {
        for (mime, name) in [
            ("application/x-executable", None),
            ("application/x-pie-executable", Some("tool")),
            ("application/x-sharedlib", Some("libx.so.1")),
            ("application/x-shellscript", Some("setup")),
            ("text/x-shellscript", None),
            ("application/x-desktop", Some("evil.desktop")),
            ("application/vnd.appimage", Some("App.AppImage")),
            ("application/x-msdownload", Some("setup.exe")),
            ("application/octet-stream", Some("install.sh")),
            ("text/plain", Some("run.desktop")),
            ("application/octet-stream", Some("Thing.AppImage")),
        ] {
            assert!(refuses_open(mime, name), "{mime} {name:?}");
        }
        for (mime, name) in [
            ("application/pdf", Some("report.pdf")),
            ("image/png", None),
            ("text/plain", Some("notes.txt")),
            ("application/zip", Some("photos.zip")),
        ] {
            assert!(!refuses_open(mime, name), "{mime} {name:?}");
        }
        assert!(looks_like_program(b"\x7fELF\x02\x01"));
        assert!(looks_like_program(b"#!/bin/sh\nrm -rf ~\n"));
        assert!(looks_like_program(b"[Desktop Entry]\nExec=x\n"));
        assert!(!looks_like_program(b"%PDF-1.7"));
    }

    #[test]
    fn an_open_of_a_program_is_refused_from_its_bytes() {
        let refused = place(&Target::Open, Some("photo.png"), 1, "image/png", b"\x7fELF");
        assert!(refused.is_err());
    }

    #[test]
    fn the_download_dir_comes_from_user_dirs() {
        let home = Path::new("/home/ann");
        assert_eq!(
            user_download_dir("# c\nXDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\n", home),
            Some(PathBuf::from("/home/ann/Downloads"))
        );
        assert_eq!(
            user_download_dir("XDG_DOWNLOAD_DIR=\"/srv/in\"\n", home),
            Some(PathBuf::from("/srv/in"))
        );
        assert_eq!(
            user_download_dir("XDG_DOWNLOAD_DIR=\"$HOME/\"\n", home),
            None
        );
        assert_eq!(
            user_download_dir("XDG_MUSIC_DIR=\"$HOME/Music\"\n", home),
            None
        );
    }

    #[test]
    fn sizes_and_icons() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(48_213), "47 KB");
        assert_eq!(human_size(1_300_000), "1.2 MB");
        assert_eq!(icon("image/png", None), Icon::FileImage);
        assert_eq!(icon("application/pdf", Some("a.pdf")), Icon::FileText);
        assert_eq!(icon("application/zip", None), Icon::FileArchive);
        assert_eq!(icon("application/x-executable", None), Icon::FileTerminal);
        assert_eq!(icon("application/x-unknown", None), Icon::File);
    }

    #[test]
    fn the_bus_answer_lists_each_attachment() {
        let email: Email = serde_json::from_value(json!({
            "id": "E5",
            "attachments": [
                { "partId": "2", "blobId": "B2", "size": 10, "name": "a.pdf", "type": "application/pdf" },
                { "partId": "3", "blobId": "B3", "size": 10, "name": "run.sh", "type": "application/x-shellscript" },
                { "partId": "4", "size": 10, "type": "image/png", "undecodable": true }
            ]
        }))
        .unwrap();
        let value = describe(&email);
        let list = value["attachments"].as_array().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0]["part"], "2");
        assert_eq!(list[0]["openable"], true);
        assert_eq!(list[1]["openable"], false);
        assert_eq!(list[1]["downloadable"], true);
        assert_eq!(list[2]["downloadable"], false);
        assert!(
            value["attachments"].to_string().find("B2").is_none(),
            "no blob ids"
        );
    }
}
