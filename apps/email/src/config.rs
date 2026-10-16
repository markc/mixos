// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Native `.conf.mix` persistence for email's window shape and its cache of
//! the accounts settingsd last held, under the poison-pill law (dopus's
//! config.rs): a malformed or unsupported-schema file loads defaults and is
//! never overwritten.
//!
//! Schema 2 holds no secret. The accounts themselves live in settingsd
//! (`email.*`) and their passwords in [`crate::secrets`]; the copy here only
//! lets the window open the accounts it last saw while settingsd is absent.
//!
//! A schema-1 file (one `account` with its password inline) is read for
//! migration only: [`ConfigFile`] keeps it, and keeps writing that form with
//! the account verbatim, until settingsd has taken the account; then the
//! file is rewritten as schema 2 ([`ConfigFile::migrated`]). Nothing is lost
//! if the migration never completes, and it cannot run twice.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jmap::Account;
use serde::{Deserialize, Serialize};

use crate::state::{Mode, State};

pub const CURRENT_SCHEMA: u32 = 2;
/// The single-account schema this app wrote before settingsd held accounts.
pub const LEGACY_SCHEMA: u32 = 1;

/// What is persisted: the window's shape and the accounts cache.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct EmailConfig {
    pub schema_version: u32,
    pub ui: UiConfig,
    /// The `email.*` settings last read from settingsd.
    #[serde(skip_serializing_if = "settings::email::Email::is_default")]
    pub cache: settings::email::Email,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            ui: UiConfig::default(),
            cache: settings::email::Email::default(),
        }
    }
}

/// The schema-1 file, read for its account only.
#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
struct LegacyConfig {
    schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<Account>,
    ui: UiConfig,
}

impl Default for LegacyConfig {
    fn default() -> Self {
        Self {
            schema_version: LEGACY_SCHEMA,
            account: None,
            ui: UiConfig::default(),
        }
    }
}

/// The window's remembered shape: two panel splits, the last mode and
/// account, and which tree nodes are collapsed.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct UiConfig {
    /// The tree panel's fraction of the window width.
    pub places: f32,
    /// The reader panel's fraction of what remains after the tree.
    pub reader: f32,
    pub mode: Mode,
    /// The account last selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Tree nodes by key (`crate::state::Node::key`); absent is expanded.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub expanded: BTreeMap<String, bool>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            places: 0.18,
            reader: 0.42,
            mode: Mode::Mail,
            account: None,
            expanded: BTreeMap::new(),
        }
    }
}

impl UiConfig {
    fn normalised(self) -> Self {
        Self {
            places: if self.places.is_finite() {
                self.places.clamp(0.1, 0.35)
            } else {
                Self::default().places
            },
            reader: if self.reader.is_finite() {
                self.reader.clamp(0.25, 0.6)
            } else {
                Self::default().reader
            },
            ..self
        }
    }
}

/// What [`ConfigFile::load`] found.
pub struct Loaded {
    pub config: EmailConfig,
    /// A schema-1 account still to migrate.
    pub legacy: Option<Account>,
}

/// Persistence target plus the poison pill.
pub struct ConfigFile {
    pub path: PathBuf,
    pub allow_save: bool,
    /// The schema-1 account, kept (and written back verbatim) until the
    /// migration completes.
    legacy: Option<Account>,
}

impl ConfigFile {
    /// Load `config.conf.mix` from the app's config directory.
    pub fn load(dir: &Path) -> (Loaded, Self) {
        let path = dir.join("config.conf.mix");
        let refused = |path: PathBuf| {
            (
                Loaded {
                    config: EmailConfig::default(),
                    legacy: None,
                },
                Self {
                    path,
                    allow_save: false,
                    legacy: None,
                },
            )
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (
                    Loaded {
                        config: EmailConfig::default(),
                        legacy: None,
                    },
                    Self {
                        path,
                        allow_save: true,
                        legacy: None,
                    },
                );
            }
            Err(error) => {
                eprintln!("email: cannot read config {}: {error}", path.display());
                return refused(path);
            }
        };
        // The schema decides the shape; a file that is not strict data at
        // all is refused either way.
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            schema_version: Option<u32>,
        }
        let schema = match config::from_conf_mix_str::<Probe>(&raw) {
            Ok(probe) => probe.schema_version.unwrap_or(CURRENT_SCHEMA),
            Err(error) => {
                eprintln!(
                    "email: refusing to overwrite invalid config {}: {}",
                    path.display(),
                    diagnostic(&error.to_string())
                );
                return refused(path);
            }
        };
        let parsed = match schema {
            CURRENT_SCHEMA => config::from_conf_mix_str::<EmailConfig>(&raw).map(|config| Loaded {
                config: EmailConfig {
                    ui: config.ui.normalised(),
                    ..config
                },
                legacy: None,
            }),
            LEGACY_SCHEMA => config::from_conf_mix_str::<LegacyConfig>(&raw).map(|old| Loaded {
                config: EmailConfig {
                    schema_version: CURRENT_SCHEMA,
                    ui: old.ui.normalised(),
                    cache: settings::email::Email::default(),
                },
                legacy: old.account,
            }),
            other => {
                eprintln!(
                    "email: refusing to overwrite unsupported config schema {other} in {}",
                    path.display()
                );
                return refused(path);
            }
        };
        match parsed {
            Ok(loaded) => {
                let legacy = loaded.legacy.clone();
                (
                    loaded,
                    Self {
                        path,
                        allow_save: true,
                        legacy,
                    },
                )
            }
            Err(error) => {
                eprintln!(
                    "email: refusing to overwrite invalid config {}: {}",
                    path.display(),
                    diagnostic(&error.to_string())
                );
                refused(path)
            }
        }
    }

    /// Whether a schema-1 account is still waiting to migrate.
    pub fn legacy(&self) -> bool {
        self.legacy.is_some()
    }

    /// settingsd holds the legacy account now: later saves write schema 2.
    pub fn migrated(&mut self) {
        self.legacy = None;
    }

    /// Persist the config; `Ok(false)` when the poison pill refuses the
    /// write. Before the migration completes the schema-1 form is written,
    /// its account untouched, so a crash or an absent settingsd loses
    /// nothing; afterwards, schema 2 (no secret). Mode 0600 either way.
    pub fn save(&self, config: &EmailConfig) -> Result<bool, String> {
        if !self.allow_save {
            return Ok(false);
        }
        let content = match &self.legacy {
            Some(account) => config::to_conf_mix_string(&LegacyConfig {
                schema_version: LEGACY_SCHEMA,
                account: Some(account.clone()),
                ui: config.ui.clone(),
            }),
            None => config::to_conf_mix_string(config),
        }
        .map_err(|_| "serialising the email config failed".to_owned())?;
        write_private(&self.path, content.as_bytes()).map(|()| true)
    }
}

/// A parse diagnostic safe to print. A schema-1 file or the secrets file
/// holds a password and the parser quotes the token it stopped at, so only
/// the position and the kind of failure are kept.
pub(crate) fn diagnostic(message: &str) -> String {
    let position = message.split_once("at line ").and_then(|(_, rest)| {
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == ':'))
            .unwrap_or(rest.len());
        let position = rest[..end].trim_end_matches(':');
        (!position.is_empty()).then(|| position.to_owned())
    });
    match position {
        Some(position) => format!("not valid strict data at line {position} (details withheld)"),
        None => "does not match the config schema (details withheld)".to_owned(),
    }
}

/// Write a file only this user can read through `config::atomic`: the
/// directory is created private (0700) on first run, the temporary is created
/// 0600 before any byte lands, and the file and the directory are synced
/// around the rename, so a crash leaves the old file or the new one.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{}: no directory", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{}: no file name", path.display()))?;
    // Keep the provisioned descriptor: reopening by path could find a
    // replacement directory whose entry was never synced.
    let held =
        config::atomic::create_directory(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    config::atomic::replace_in(&held, name, bytes).map_err(|e| {
        if e.may_have_replaced {
            format!(
                "writing {}: {e} (the new file may be in place)",
                path.display()
            )
        } else {
            format!("writing {}: {e}", path.display())
        }
    })
}

impl From<&State> for EmailConfig {
    fn from(state: &State) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            ui: UiConfig {
                places: state.ui.places,
                reader: state.ui.reader,
                mode: state.mode,
                account: state.selected.clone(),
                expanded: state.ui.expanded.clone(),
            },
            cache: state.cache(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// The schema a written file declares.
    fn schema(raw: &str) -> f64 {
        config::from_conf_mix_str::<serde_json::Value>(raw).unwrap()["schema_version"]
            .as_f64()
            .unwrap()
    }

    fn entry() -> settings::email::AccountEntry {
        settings::email::AccountEntry {
            id: "acct-1".into(),
            name: "Home".into(),
            url: "https://example.invalid".into(),
            email: "ann@example.invalid".into(),
            ca_path: None,
            enabled: true,
            signature: String::new(),
        }
    }

    #[test]
    fn round_trips_the_shape_and_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let (_, file) = ConfigFile::load(dir.path());
        let mut config = EmailConfig::default();
        config.ui.expanded.insert("acct-1".into(), false);
        config.ui.account = Some("acct-1".into());
        config.cache.accounts.push(entry());
        assert!(file.save(&config).unwrap());
        let (loaded, again) = ConfigFile::load(dir.path());
        assert_eq!(loaded.config, config);
        assert!(loaded.legacy.is_none() && !again.legacy());
        let mode = std::fs::metadata(&file.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let raw = std::fs::read_to_string(&file.path).unwrap();
        assert_eq!(schema(&raw), 2.0, "{raw}");
    }

    #[test]
    fn partial_records_keep_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.conf.mix"),
            "schema_version: 2\nui: {places: 0.25}\n",
        )
        .unwrap();
        let (loaded, file) = ConfigFile::load(dir.path());
        assert!(file.allow_save);
        assert_eq!(loaded.config.ui.places, 0.25);
        assert_eq!(loaded.config.ui.reader, UiConfig::default().reader);
        assert!(loaded.config.cache.accounts.is_empty());
    }

    #[test]
    fn malformed_config_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.conf.mix");
        let raw = "schema_version: $executable\n";
        std::fs::write(&path, raw).unwrap();
        let (_, file) = ConfigFile::load(dir.path());
        assert!(!file.allow_save);
        file.save(&EmailConfig::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[test]
    fn an_unknown_schema_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.conf.mix");
        let raw = "schema_version: 9\nui: {}\n";
        std::fs::write(&path, raw).unwrap();
        let (_, file) = ConfigFile::load(dir.path());
        assert!(!file.allow_save);
        assert!(!file.save(&EmailConfig::default()).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    /// The live install's shape: a schema-1 file with one account and its
    /// password. It is read for the account, saved back verbatim until the
    /// migration completes, then rewritten as schema 2 with no secret.
    #[test]
    fn a_schema_one_file_is_kept_until_migrated_then_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.conf.mix");
        std::fs::write(
            &path,
            "schema_version: 1\naccount: {url: \"https://mail.example.invalid\", email: \"admin@example.invalid\", password: \"pw-1\"}\nui: {places: 0.2, mode: \"calendar\"}\n",
        )
        .unwrap();
        let (loaded, mut file) = ConfigFile::load(dir.path());
        let legacy = loaded.legacy.clone().expect("the schema-1 account");
        assert_eq!(legacy.email, "admin@example.invalid");
        assert_eq!(legacy.password, "pw-1");
        assert_eq!(loaded.config.ui.mode, Mode::Calendar);
        assert!(file.legacy());
        // A save before the migration keeps the account and its password.
        let mut shaped = loaded.config.clone();
        shaped.ui.places = 0.3;
        assert!(file.save(&shaped).unwrap());
        let (again, _) = ConfigFile::load(dir.path());
        assert_eq!(
            again.legacy.as_ref().map(|a| a.password.as_str()),
            Some("pw-1")
        );
        assert_eq!(again.config.ui.places, 0.3);
        // Once settingsd has it: schema 2, no password anywhere.
        file.migrated();
        assert!(file.save(&shaped).unwrap());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(schema(&raw) == 2.0 && !raw.contains("pw-1"), "{raw}");
        let (after, file) = ConfigFile::load(dir.path());
        assert!(after.legacy.is_none() && !file.legacy());
    }

    #[test]
    fn the_first_save_creates_a_private_config_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("dev.mixos.email");
        let (_, file) = ConfigFile::load(&dir);
        assert!(file.save(&EmailConfig::default()).unwrap());
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let mode = std::fs::metadata(&file.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_invalid_config_diagnostic_never_quotes_the_password() {
        let raw =
            "schema_version: 1\naccount: {url: \"https://x\", email: \"a\", password \"secret\"}\n";
        let error = config::from_conf_mix_str::<LegacyConfig>(raw)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("secret"),
            "the raw parser error quotes it: {error}"
        );
        let shown = diagnostic(&error);
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("line 2:"), "{shown}");
        let schema = diagnostic("invalid type: string \"secret\", expected u32");
        assert!(!schema.contains("secret"), "{schema}");
        // The loader refuses the file without printing it.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.conf.mix"), raw).unwrap();
        let (loaded, file) = ConfigFile::load(dir.path());
        assert!(!file.allow_save && loaded.legacy.is_none());
    }
}
