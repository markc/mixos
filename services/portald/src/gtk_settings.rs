// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! GTK caption settings in the desk's private config directory.
use design::CaptionSide;
use serde::Serialize;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

const MAX_BYTES: u64 = 64 * 1024;
const LAYOUT: &str = "gtk-decoration-layout";
const ICON_THEME: &str = "gtk-icon-theme-name";
const THEME: &str = "mixos";

/// New text, or `None` when both keys already have the requested values.
pub fn merge(existing: Option<&str>, side: CaptionSide) -> Option<String> {
    let text = existing.unwrap_or_default();
    let layout = match side {
        CaptionSide::Right => "menu:minimize,maximize,close",
        CaptionSide::Left => "close,minimize,maximize:menu",
    };
    let keys = [(LAYOUT, layout), (ICON_THEME, THEME)];
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::new();
    let mut in_settings = false;
    let mut found_settings = false;
    let mut seen = [false; 2];
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let trimmed = body.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_settings {
                append_missing(&mut out, &keys, &seen, newline);
            }
            in_settings = trimmed == "[Settings]";
            found_settings |= in_settings;
            seen = [false; 2];
        }
        let mut replacement = None;
        if in_settings && let Some((key, value)) = body.split_once('=') {
            for (index, (name, wanted)) in keys.iter().enumerate() {
                if key.trim() == *name {
                    seen[index] = true;
                    if value.trim() != *wanted {
                        replacement = Some(format!("{key}={wanted}{}", &line[body.len()..]));
                    }
                    break;
                }
            }
        }
        out.push_str(replacement.as_deref().unwrap_or(line));
    }
    if in_settings {
        append_missing(&mut out, &keys, &seen, newline);
    } else if !found_settings {
        end_line(&mut out, newline);
        out.push_str("[Settings]");
        out.push_str(newline);
        append_missing(&mut out, &keys, &[false; 2], newline);
    }
    (out != text).then_some(out)
}

fn end_line(text: &mut String, newline: &str) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push_str(newline);
    }
}

fn append_missing(text: &mut String, keys: &[(&str, &str); 2], seen: &[bool; 2], newline: &str) {
    for (index, (key, value)) in keys.iter().enumerate() {
        if !seen[index] {
            end_line(text, newline);
            text.push_str(&format!("{key}={value}{newline}"));
        }
    }
}

/// Environment values are supplied so policy tests do not change process state.
pub fn destination(
    config_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, &'static str> {
    let config = config_home.ok_or("XDG_CONFIG_HOME is unset")?;
    if !config.is_absolute() {
        return Err("XDG_CONFIG_HOME is not absolute");
    }
    if config.components().any(|part| part == Component::ParentDir) {
        return Err("XDG_CONFIG_HOME contains a parent segment");
    }
    if home.is_some_and(|home| config == home.join(".config")) {
        return Err("XDG_CONFIG_HOME is HOME/.config");
    }
    Ok(config.join("gtk-3.0/settings.ini"))
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub path: Option<PathBuf>,
    pub outcome: &'static str,
    pub reason: Option<String>,
}

pub struct Exporter {
    target: Result<PathBuf, &'static str>,
}

impl Exporter {
    pub fn new(config_home: Option<&Path>, home: Option<&Path>) -> Self {
        let target = destination(config_home, home);
        if let Err(reason) = &target {
            tracing::info!(reason, "GTK settings export skipped");
        }
        Self { target }
    }

    pub fn export(&self, side: Option<CaptionSide>) -> Report {
        let path = match &self.target {
            Ok(path) => path,
            Err(reason) => {
                return Report {
                    path: None,
                    outcome: "skipped",
                    reason: Some((*reason).into()),
                };
            }
        };
        let mut report = Report {
            path: Some(path.clone()),
            outcome: "skipped",
            reason: None,
        };
        let Some(side) = side else {
            report.reason = Some("awaiting an accepted appearance projection".into());
            return report;
        };
        match write(path, side) {
            Ok(written) => report.outcome = if written { "written" } else { "unchanged" },
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "GTK settings export failed");
                report.reason = Some(error.to_string());
            }
        }
        report
    }
}

fn write(path: &Path, side: CaptionSide) -> anyhow::Result<bool> {
    let parent = path.parent().expect("GTK settings has a parent");
    // Refuse links even above the config root: a nested desk must not write
    // through a link into the host's config tree.
    for directory in parent.ancestors() {
        match std::fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => {}
            Ok(_) => anyhow::bail!("GTK config path is not a plain directory"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let existing = match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file() && !metadata.is_symlink(),
                "GTK settings is not a plain file"
            );
            anyhow::ensure!(metadata.len() <= MAX_BYTES, "GTK settings exceeds 64 KiB");
            let file = std::fs::File::open(path)?;
            let mut text = String::new();
            file.take(MAX_BYTES + 1).read_to_string(&mut text)?;
            anyhow::ensure!(
                text.len() as u64 <= MAX_BYTES,
                "GTK settings exceeds 64 KiB"
            );
            Some(text)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let Some(text) = merge(existing.as_deref(), side) else {
        return Ok(false);
    };
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(text.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_missing_and_empty_files_for_both_sides() {
        for (side, layout) in [
            (CaptionSide::Right, "menu:minimize,maximize,close"),
            (CaptionSide::Left, "close,minimize,maximize:menu"),
        ] {
            let expected =
                format!("[Settings]\ngtk-decoration-layout={layout}\ngtk-icon-theme-name=mixos\n");
            assert_eq!(merge(None, side).as_deref(), Some(expected.as_str()));
            assert_eq!(merge(Some(""), side).as_deref(), Some(expected.as_str()));
            assert_eq!(merge(Some(&expected), side), None);
        }
    }

    #[test]
    fn preserves_other_sections_comments_and_crlf() {
        let text = "# keep\r\n[Other]\r\ngtk-icon-theme-name=host\r\n\r\n[Settings]\r\n; keep too\r\ngtk-font-name=Sans 11\r\n[Last]\r\nx=y";
        let expected = text.replace("[Last]", "gtk-decoration-layout=close,minimize,maximize:menu\r\ngtk-icon-theme-name=mixos\r\n[Last]");
        assert_eq!(merge(Some(text), CaptionSide::Left), Some(expected.clone()));
        assert_eq!(merge(Some(&expected), CaptionSide::Left), None);
    }

    #[test]
    fn replaces_only_the_owned_keys_in_settings() {
        let text = "[Settings]\n gtk-decoration-layout =old\n# comment\ngtk-icon-theme-name=other\ngtk-decoration-layout=duplicate\n[Other]\ngtk-icon-theme-name=keep\n";
        let expected = "[Settings]\n gtk-decoration-layout =menu:minimize,maximize,close\n# comment\ngtk-icon-theme-name=mixos\ngtk-decoration-layout=menu:minimize,maximize,close\n[Other]\ngtk-icon-theme-name=keep\n";
        assert_eq!(
            merge(Some(text), CaptionSide::Right).as_deref(),
            Some(expected)
        );
        assert_eq!(merge(Some(expected), CaptionSide::Right), None);
        let left = merge(Some(expected), CaptionSide::Left).unwrap();
        assert_eq!(merge(Some(&left), CaptionSide::Left), None);
    }

    #[test]
    fn appends_a_missing_section_and_handles_no_final_newline() {
        let text = "# keep\n[Other]\nx=y";
        let added = merge(Some(text), CaptionSide::Right).unwrap();
        assert!(added.starts_with("# keep\n[Other]\nx=y\n[Settings]\n"));
        assert_eq!(merge(Some(&added), CaptionSide::Right), None);
        let text = "[Settings]\ngtk-decoration-layout=old";
        let added = merge(Some(text), CaptionSide::Left).unwrap();
        assert_eq!(
            added,
            "[Settings]\ngtk-decoration-layout=close,minimize,maximize:menu\ngtk-icon-theme-name=mixos\n"
        );
    }

    #[test]
    fn skips_unset_relative_and_host_config_without_changing_the_environment() {
        let home = Some(Path::new("/home/user"));
        for config in [
            None,
            Some(Path::new("")),
            Some(Path::new("config")),
            Some(Path::new("/home/user/.config")),
            Some(Path::new("/home/user/./.config/")),
            Some(Path::new("/home/user/desk/../.config")),
        ] {
            assert!(destination(config, home).is_err());
        }
        assert_eq!(
            destination(Some(Path::new("/var/lib/mixos/desk/config")), home).unwrap(),
            Path::new("/var/lib/mixos/desk/config/gtk-3.0/settings.ini")
        );
    }

    #[test]
    fn writes_atomically_and_refuses_large_or_linked_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let exporter = Exporter::new(Some(&config), None);
        assert_eq!(exporter.export(None).outcome, "skipped");
        let report = exporter.export(Some(CaptionSide::Left));
        assert_eq!(report.outcome, "written");
        let path = report.path.unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            exporter.export(Some(CaptionSide::Left)).outcome,
            "unchanged"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
        std::fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert_eq!(exporter.export(Some(CaptionSide::Right)).outcome, "skipped");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), MAX_BYTES + 1);
        std::fs::remove_file(&path).unwrap();
        let host = dir.path().join("host.ini");
        std::fs::write(&host, &bytes).unwrap();
        std::os::unix::fs::symlink(&host, &path).unwrap();
        assert_eq!(exporter.export(Some(CaptionSide::Right)).outcome, "skipped");
        assert_eq!(std::fs::read(&host).unwrap(), bytes);
    }
}
