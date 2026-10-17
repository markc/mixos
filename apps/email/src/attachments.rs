// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Saving and opening an email's attachments: the file-name rules (pure,
//! tested) and the one write that puts bytes on disk.
//!
//! A name from an email is never trusted: [`file_name`] keeps its last
//! path component, drops control characters, `..` and leading dots, and
//! caps it at [`NAME_MAX_BYTES`]; a part with no usable name gets
//! `attachment-<n>.<ext>` from its type. [`save`] never overwrites: a name
//! that is taken becomes `name (1).ext`, `name (2).ext`, …, claimed with a
//! hard link from a private temporary file, so the file appears whole and
//! no existing file is ever replaced. Files are 0600.
//!
//! Executables and scripts ([`refuses_open`]) are saved, never opened.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The longest name written, in bytes.
pub const NAME_MAX_BYTES: usize = 200;

/// How many `name (n).ext` candidates are tried before giving up.
const COLLISIONS_MAX: u32 = 999;

/// The extension for a type, for an attachment that arrived without a name.
pub fn extension(mime: Option<&str>) -> &'static str {
    let mime = mime
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "application/pdf" => "pdf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "text/plain" => "txt",
        "text/html" => "html",
        "text/markdown" => "md",
        "text/csv" => "csv",
        "text/calendar" => "ics",
        "text/vcard" | "text/x-vcard" => "vcf",
        "message/rfc822" => "eml",
        "application/zip" => "zip",
        "application/gzip" => "gz",
        "application/x-tar" => "tar",
        "application/json" => "json",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "video/mp4" => "mp4",
        "application/vnd.oasis.opendocument.text" => "odt",
        "application/vnd.oasis.opendocument.spreadsheet" => "ods",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        _ => "bin",
    }
}

/// `text` cut to at most `max` bytes on a character boundary.
fn cap(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A safe file name for attachment `index` (1-based) of type `mime`, named
/// `name` by its sender.
pub fn file_name(name: Option<&str>, index: usize, mime: Option<&str>) -> String {
    let raw = name.unwrap_or("");
    // The last component of either separator: a name is never a path.
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.replace("..", "");
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    if cleaned.is_empty() {
        return format!("attachment-{index}.{}", extension(mime));
    }
    // Keep the extension when the cap cuts a long name.
    let capped = if cleaned.len() > NAME_MAX_BYTES {
        match cleaned.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && ext.len() <= 16 => {
                format!("{}.{ext}", cap(stem, NAME_MAX_BYTES - ext.len() - 1))
            }
            _ => cap(cleaned, NAME_MAX_BYTES).to_owned(),
        }
    } else {
        cleaned.to_owned()
    };
    capped.trim_end().to_owned()
}

/// The `n`th alternative to a taken `name`: `plan (n).pdf`, `notes (n)`.
pub fn numbered(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

/// Executables, libraries, installers and scripts: saved, never opened.
pub fn refuses_open(name: &str, mime: Option<&str>) -> bool {
    const TYPES: &[&str] = &[
        "application/x-executable",
        "application/x-sharedlib",
        "application/x-pie-executable",
        "application/x-msdownload",
        "application/x-ms-dos-executable",
        "application/x-msi",
        "application/x-sh",
        "application/x-shellscript",
        "application/x-csh",
        "application/x-perl",
        "application/x-python",
        "text/x-python",
        "text/x-shellscript",
        "application/x-ruby",
        "application/x-desktop",
        "application/java-archive",
        "application/x-java-archive",
        "application/vnd.debian.binary-package",
        "application/x-rpm",
        "application/x-appimage",
        "application/javascript",
        "text/javascript",
    ];
    const EXTENSIONS: &[&str] = &[
        "sh", "bash", "zsh", "csh", "fish", "mix", "py", "pl", "rb", "php", "js", "mjs", "jar",
        "exe", "com", "bat", "cmd", "msi", "ps1", "vbs", "scr", "desktop", "run", "appimage",
        "bin", "so", "dll", "deb", "rpm", "elf", "out",
    ];
    let mime = mime
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    TYPES.contains(&mime.as_str()) || EXTENSIONS.contains(&ext.as_str())
}

/// A size as people read it: `512 B`, `1.2 KB`, `3.4 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if size < 10.0 {
        format!("{size:.1} {}", UNITS[unit])
    } else {
        format!("{size:.0} {}", UNITS[unit])
    }
}

/// The downloads directory from `$XDG_DOWNLOAD_DIR` (`env`), else the
/// `XDG_DOWNLOAD_DIR` line of `user-dirs.dirs` (`user_dirs`, its text),
/// else `~/Downloads`.
pub fn download_dir(env: Option<&str>, user_dirs: Option<&str>, home: &Path) -> PathBuf {
    let expand = |value: &str| -> Option<PathBuf> {
        let value = value.trim().trim_matches('"');
        if value.is_empty() {
            return None;
        }
        let path = match value.strip_prefix("$HOME") {
            Some(rest) => home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(value),
        };
        path.is_absolute().then_some(path)
    };
    if let Some(dir) = env.and_then(expand) {
        return dir;
    }
    if let Some(text) = user_dirs {
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=")
                && let Some(dir) = expand(value)
            {
                return dir;
            }
        }
    }
    home.join("Downloads")
}

/// The downloads directory of this session.
pub fn downloads() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| crate::label("no-home"))?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|c| !c.is_empty())
        .map_or_else(|| home.join(".config"), PathBuf::from);
    let user_dirs = std::fs::read_to_string(config.join("user-dirs.dirs")).ok();
    let env = std::env::var("XDG_DOWNLOAD_DIR").ok();
    Ok(download_dir(env.as_deref(), user_dirs.as_deref(), &home))
}

/// The private directory an opened attachment is saved in first,
/// `$XDG_RUNTIME_DIR/email-attach` (0700).
pub fn open_dir() -> Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(d).join("email-attach"))
        .ok_or_else(|| crate::label("no-runtime-dir"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| e.to_string())?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Write `bytes` as `name` in `dir` (which must exist), never replacing a
/// file: a taken name is numbered. The file (0600) appears whole.
pub fn save(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::os::unix::fs::OpenOptionsExt;
    if !dir.is_dir() {
        return Err(crate::label_with(
            "not-a-directory",
            &[("path", &dir.display().to_string())],
        ));
    }
    let partial = dir.join(format!(
        ".email-{}-{}.partial",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&partial)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&partial);
        return Err(error.to_string());
    }
    let mut result = Err(crate::label("name-taken"));
    for n in 0..=COLLISIONS_MAX {
        let candidate = if n == 0 {
            name.to_owned()
        } else {
            numbered(name, n)
        };
        let path = dir.join(&candidate);
        // A hard link fails on a taken name: nothing is ever replaced.
        match std::fs::hard_link(&partial, &path) {
            Ok(()) => {
                result = Ok(path);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                result = Err(error.to_string());
                break;
            }
        }
    }
    let _ = std::fs::remove_file(&partial);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_never_paths_or_hidden() {
        assert_eq!(file_name(Some("plan.pdf"), 1, None), "plan.pdf");
        assert_eq!(file_name(Some("../../etc/passwd"), 1, None), "passwd");
        assert_eq!(
            file_name(Some("C:\\Users\\x\\evil.exe"), 1, None),
            "evil.exe"
        );
        assert_eq!(file_name(Some(".bashrc"), 1, None), "bashrc");
        assert_eq!(file_name(Some("a..b.txt"), 1, None), "ab.txt");
        assert_eq!(
            file_name(Some("bell\u{7}\nname.txt"), 1, None),
            "bellname.txt"
        );
        assert_eq!(
            file_name(Some(".."), 2, Some("application/pdf")),
            "attachment-2.pdf"
        );
        assert_eq!(
            file_name(None, 3, Some("image/png; name=x")),
            "attachment-3.png"
        );
        assert_eq!(file_name(Some("   "), 4, None), "attachment-4.bin");
    }

    #[test]
    fn long_names_are_capped_keeping_the_extension() {
        let long = format!("{}.pdf", "é".repeat(300));
        let name = file_name(Some(&long), 1, None);
        assert!(name.len() <= NAME_MAX_BYTES, "{}", name.len());
        assert!(name.ends_with(".pdf"));
        let plain = "x".repeat(500);
        assert_eq!(file_name(Some(&plain), 1, None).len(), NAME_MAX_BYTES);
    }

    #[test]
    fn taken_names_are_numbered() {
        assert_eq!(numbered("plan.pdf", 1), "plan (1).pdf");
        assert_eq!(numbered("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(numbered("README", 3), "README (3)");
        let dir = tempfile::tempdir().unwrap();
        let a = save(dir.path(), "plan.pdf", b"one").unwrap();
        let b = save(dir.path(), "plan.pdf", b"two").unwrap();
        let c = save(dir.path(), "plan.pdf", b"three").unwrap();
        assert_eq!(a.file_name().unwrap(), "plan.pdf");
        assert_eq!(b.file_name().unwrap(), "plan (1).pdf");
        assert_eq!(c.file_name().unwrap(), "plan (2).pdf");
        assert_eq!(std::fs::read(&a).unwrap(), b"one", "never overwritten");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&b).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
        assert!(save(&dir.path().join("missing"), "x", b"").is_err());
    }

    #[test]
    fn executables_and_scripts_are_save_only() {
        for (name, mime) in [
            ("run.sh", None),
            ("tool", Some("application/x-executable")),
            ("libx.so", None),
            ("setup.EXE", None),
            ("app.desktop", None),
            ("x", Some("application/x-shellscript; charset=utf-8")),
            ("Thing.AppImage", None),
            ("build.mix", None),
        ] {
            assert!(refuses_open(name, mime), "{name}");
        }
        for (name, mime) in [
            ("plan.pdf", Some("application/pdf")),
            ("photo.jpg", None),
            ("notes.txt", Some("text/plain")),
        ] {
            assert!(!refuses_open(name, mime), "{name}");
        }
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(20 * 1024), "20 KB");
        assert_eq!(human_size(3 * 1024 * 1024 + 1024 * 400), "3.4 MB");
    }

    #[test]
    fn the_download_dir_follows_xdg() {
        let home = Path::new("/home/ann");
        assert_eq!(
            download_dir(None, None, home),
            PathBuf::from("/home/ann/Downloads")
        );
        let dirs =
            "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Fetched\"\n";
        assert_eq!(
            download_dir(None, Some(dirs), home),
            PathBuf::from("/home/ann/Fetched")
        );
        assert_eq!(
            download_dir(Some("/data/dl"), Some(dirs), home),
            PathBuf::from("/data/dl")
        );
        // A relative value is not a directory to trust.
        assert_eq!(
            download_dir(Some("dl"), None, home),
            PathBuf::from("/home/ann/Downloads")
        );
    }
}
