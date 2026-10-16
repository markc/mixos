// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Which applications open a type, as the freedesktop MIME Applications
//! spec decides it, and a type's usual file extension.
//!
//! For the type and then each supertype (never `application/octet-stream`:
//! "any file" is not an association), the candidates are, in order: every
//! `mimeapps.list`'s `[Default Applications]`, then their `[Added
//! Associations]`, then each `mimeinfo.cache`, less any `[Removed
//! Associations]`. A `media/*` key matches the whole media type. The
//! `mimeapps.list` files are read highest precedence first: the user's
//! config, the system config directories, then the data directories'
//! `applications/`. Desktop-specific `$desktop-mimeapps.list` files are not
//! read.
//!
//! The answers are desktop ids without `.desktop` (`firefox`), the form the
//! session's launcher (`apps.launch`) takes.

use std::path::PathBuf;

use crate::{Database, OCTET_STREAM, read_text};

/// `mimeapps.list` files, highest precedence first: `$XDG_CONFIG_HOME`
/// (else `~/.config`), `$XDG_CONFIG_DIRS` (else `/etc/xdg`), then each data
/// directory's `applications/`.
pub fn mimeapps_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    match std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        Some(home) if home.is_absolute() => out.push(home),
        _ => {
            if let Some(home) = std::env::var_os("HOME") {
                out.push(PathBuf::from(home).join(".config"));
            }
        }
    }
    match std::env::var_os("XDG_CONFIG_DIRS") {
        Some(v) if !v.is_empty() => {
            out.extend(std::env::split_paths(&v).filter(|p| p.is_absolute()))
        }
        _ => out.push(PathBuf::from("/etc/xdg")),
    }
    let mut paths: Vec<PathBuf> = out.into_iter().map(|d| d.join("mimeapps.list")).collect();
    paths.extend(applications_dirs().into_iter().map(|d| d.join("mimeapps.list")));
    paths
}

/// Each data directory's `applications/mimeinfo.cache`, highest first.
pub fn cache_paths() -> Vec<PathBuf> {
    applications_dirs()
        .into_iter()
        .map(|d| d.join("mimeinfo.cache"))
        .collect()
}

/// The data directories' `applications/`, from the same list the `mime`
/// directories come from.
fn applications_dirs() -> Vec<PathBuf> {
    crate::xdg_mime_dirs()
        .into_iter()
        .filter_map(|mime| mime.parent().map(|d| d.join("applications")))
        .collect()
}

/// One file's `[section]` keys, in order: `type=a.desktop;b.desktop;`.
fn section<'a>(text: &'a str, name: &str) -> Vec<(&'a str, Vec<&'a str>)> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == format!("[{name}]");
            continue;
        }
        if !inside || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let ids = value
                .split(';')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .collect();
            out.push((key.trim(), ids));
        }
    }
    out
}

/// Whether association key `key` covers `mime` (exactly, or `media/*`).
fn covers(key: &str, mime: &str) -> bool {
    key == mime
        || key
            .strip_suffix("/*")
            .is_some_and(|media| mime.split('/').next() == Some(media))
}

/// The applications for `types` (a type, then its supertypes), from the
/// texts of the `mimeapps.list` files (highest precedence first) and of the
/// `mimeinfo.cache` files. Pure: [`Database::apps_for`] reads the files.
pub fn apps_in(lists: &[String], caches: &[String], types: &[String]) -> Vec<String> {
    let removed: Vec<(&str, Vec<&str>)> = lists
        .iter()
        .flat_map(|t| section(t, "Removed Associations"))
        .collect();
    let gone = |mime: &str, id: &str| {
        removed
            .iter()
            .any(|(key, ids)| covers(key, mime) && ids.contains(&id))
    };
    let mut out: Vec<String> = Vec::new();
    let mut push = |id: &str| {
        let id = id.strip_suffix(".desktop").unwrap_or(id);
        if !id.is_empty() && !out.iter().any(|seen| seen == id) {
            out.push(id.to_owned());
        }
    };
    for mime in types {
        for (texts, name) in [
            (lists, "Default Applications"),
            (lists, "Added Associations"),
            (caches, "MIME Cache"),
        ] {
            for text in texts {
                for (key, ids) in section(text, name) {
                    if covers(key, mime) {
                        ids.iter()
                            .filter(|id| !gone(mime, id))
                            .for_each(|id| push(id));
                    }
                }
            }
        }
    }
    out
}

impl Database {
    /// The applications that open `mime`, best first, as desktop ids.
    pub fn apps_for(&self, mime: &str) -> Vec<String> {
        let read = |paths: Vec<PathBuf>| -> Vec<String> {
            paths.iter().filter_map(|p| read_text(p)).collect()
        };
        let types: Vec<String> = std::iter::once(self.unalias(mime).to_owned())
            .chain(self.parents(mime))
            .filter(|m| m != OCTET_STREAM)
            .collect();
        apps_in(&read(mimeapps_paths()), &read(cache_paths()), &types)
    }

    /// `mime`'s usual extension, without the dot (`pdf`): its heaviest
    /// plain `*.ext` glob, the first one listed among equals.
    pub fn extension(&self, mime: &str) -> Option<String> {
        let mime = self.unalias(mime);
        let mut best: Option<(u32, &str)> = None;
        for glob in self.globs.iter().flatten() {
            let crate::glob::Kind::Suffix(suffix) = &glob.kind else {
                continue;
            };
            let Some(ext) = suffix.strip_prefix('.') else {
                continue;
            };
            let plain = !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric());
            if plain
                && self.unalias(&glob.mime) == mime
                && best.is_none_or(|(weight, _)| glob.weight > weight)
            {
                best = Some((glob.weight, ext));
            }
        }
        best.map(|(_, ext)| ext.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "\
[Default Applications]
application/pdf=org.gnome.Evince.desktop;firefox.desktop;

[Added Associations]
image/*=org.kde.gwenview.desktop;
text/plain=org.kde.kate.desktop;

[Removed Associations]
image/png=firefox.desktop;
";

    const CACHE: &str = "\
[MIME Cache]
application/pdf=firefox.desktop;
image/png=firefox.desktop;ristretto.desktop;
text/plain=dev.mixos.ced.desktop;
";

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn defaults_then_added_then_the_cache_less_the_removed() {
        let lists = [USER.to_owned()];
        let caches = [CACHE.to_owned()];
        assert_eq!(
            apps_in(&lists, &caches, &types(&["application/pdf"])),
            ["org.gnome.Evince", "firefox"]
        );
        // A media wildcard matches; the removed association is dropped.
        assert_eq!(
            apps_in(&lists, &caches, &types(&["image/png"])),
            ["org.kde.gwenview", "ristretto"]
        );
        // A subtype falls back to its parent's applications.
        assert_eq!(
            apps_in(&lists, &caches, &types(&["text/csv", "text/plain"])),
            ["org.kde.kate", "dev.mixos.ced"]
        );
        assert!(apps_in(&lists, &caches, &types(&["audio/flac"])).is_empty());
    }

    #[test]
    fn a_higher_file_comes_first() {
        let lists = [
            "[Default Applications]\ntext/html=zen.desktop\n".to_owned(),
            "[Default Applications]\ntext/html=firefox.desktop;zen.desktop\n".to_owned(),
        ];
        assert_eq!(apps_in(&lists, &[], &types(&["text/html"])), ["zen", "firefox"]);
    }

    #[test]
    fn the_heaviest_plain_suffix_is_the_extension() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("globs2"),
            "50:application/pdf:*.pdf\n\
             40:application/pdf:*.PDF.bak\n\
             50:image/jpeg:*.jpg\n\
             60:image/jpeg:*.jpeg\n\
             50:application/x-compressed-tar:*.tar.gz\n\
             50:text/plain:README\n",
        )
        .unwrap();
        let db = Database::load_from(std::slice::from_ref(&dir));
        assert_eq!(db.extension("application/pdf").as_deref(), Some("pdf"));
        assert_eq!(db.extension("image/jpeg").as_deref(), Some("jpeg"));
        // Only multi-dot or literal globs: no plain extension.
        assert_eq!(db.extension("application/x-compressed-tar"), None);
        assert_eq!(db.extension("text/plain"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mimedb-apps-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
