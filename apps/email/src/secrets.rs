// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The account passwords: `secrets.conf.mix` in the app's config directory,
//! written only by this app (directory 0700, file 0600, atomic replace) and
//! never sent anywhere but the account's own server. settingsd holds every
//! other account setting; it publishes retained snapshots on the Bus, so a
//! password never enters it (docs/docs/decisions/2026-10-16-email-accounts-in-settings.md).
//!
//! The poison-pill law holds as for `config.conf.mix`: a malformed or
//! unsupported-schema file loads empty and is never overwritten. Nothing
//! here prints a password: [`Secrets`]' `Debug` names the account ids only,
//! and a parse diagnostic keeps only the position.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA: u32 = 1;

/// The file's name in the config directory.
pub const FILE: &str = "secrets.conf.mix";

/// Every account's password, by settings account id.
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Secrets {
    pub schema_version: u32,
    pub passwords: BTreeMap<String, String>,
}

impl Default for Secrets {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            passwords: BTreeMap::new(),
        }
    }
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("schema_version", &self.schema_version)
            .field("accounts", &self.passwords.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Secrets {
    /// The password of account `id`, when one is held.
    pub fn password(&self, id: &str) -> Option<&str> {
        self.passwords
            .get(id)
            .map(String::as_str)
            .filter(|p| !p.is_empty())
    }

    pub fn has(&self, id: &str) -> bool {
        self.password(id).is_some()
    }

    /// Hold `password` for `id`; an empty one forgets it. Whether anything
    /// changed.
    pub fn set(&mut self, id: &str, password: &str) -> bool {
        if password.is_empty() {
            return self.passwords.remove(id).is_some();
        }
        if self.passwords.get(id).map(String::as_str) == Some(password) {
            return false;
        }
        self.passwords.insert(id.to_owned(), password.to_owned());
        true
    }

    /// Forget every account not in `keep`. Whether anything changed.
    pub fn retain(&mut self, keep: &[&str]) -> bool {
        let before = self.passwords.len();
        self.passwords.retain(|id, _| keep.contains(&id.as_str()));
        self.passwords.len() != before
    }
}

/// Persistence target plus the poison pill.
#[derive(Debug)]
pub struct SecretsFile {
    pub path: PathBuf,
    pub allow_save: bool,
}

impl SecretsFile {
    /// Load `secrets.conf.mix` from the app's config directory.
    pub fn load(dir: &Path) -> (Secrets, Self) {
        let path = dir.join(FILE);
        let refuse = |path: PathBuf| {
            (
                Secrets::default(),
                Self {
                    path,
                    allow_save: false,
                },
            )
        };
        match std::fs::read_to_string(&path) {
            Ok(raw) => match config::from_conf_mix_str::<Secrets>(&raw) {
                Ok(secrets) if secrets.schema_version == CURRENT_SCHEMA => (
                    secrets,
                    Self {
                        path,
                        allow_save: true,
                    },
                ),
                Ok(secrets) => {
                    eprintln!(
                        "email: refusing to overwrite unsupported secrets schema {} in {}",
                        secrets.schema_version,
                        path.display()
                    );
                    refuse(path)
                }
                Err(error) => {
                    eprintln!(
                        "email: refusing to overwrite invalid secrets {}: {}",
                        path.display(),
                        crate::config::diagnostic(&error.to_string())
                    );
                    refuse(path)
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
                Secrets::default(),
                Self {
                    path,
                    allow_save: true,
                },
            ),
            Err(error) => {
                eprintln!("email: cannot read secrets {}: {error}", path.display());
                refuse(path)
            }
        }
    }

    /// Persist the passwords; `Ok(false)` when the poison pill refuses the
    /// write. The error never carries a password.
    pub fn save(&self, secrets: &Secrets) -> Result<bool, String> {
        if !self.allow_save {
            return Ok(false);
        }
        let content = config::to_conf_mix_string(secrets)
            .map_err(|_| "serialising the email secrets failed".to_owned())?;
        crate::config::write_private(&self.path, content.as_bytes()).map(|()| true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn round_trips_privately() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config");
        let (mut secrets, file) = SecretsFile::load(&dir);
        assert!(file.allow_save && secrets.passwords.is_empty());
        assert!(secrets.set("acct-1", "hunter2"));
        assert!(!secrets.set("acct-1", "hunter2"), "unchanged");
        assert!(file.save(&secrets).unwrap());
        let (loaded, _) = SecretsFile::load(&dir);
        assert_eq!(loaded.password("acct-1"), Some("hunter2"));
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&file.path), 0o600);
    }

    #[test]
    fn debug_never_shows_a_password() {
        let mut secrets = Secrets::default();
        secrets.set("acct-1", "hunter2");
        let shown = format!("{secrets:?}");
        assert!(
            shown.contains("acct-1") && !shown.contains("hunter2"),
            "{shown}"
        );
    }

    #[test]
    fn a_malformed_file_is_never_overwritten_or_quoted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let raw = "schema_version: 1\npasswords: {\"acct-1\" \"hunter2\"}\n";
        std::fs::write(&path, raw).unwrap();
        let (secrets, file) = SecretsFile::load(dir.path());
        assert!(!file.allow_save && secrets.passwords.is_empty());
        assert!(!file.save(&Secrets::default()).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[test]
    fn an_empty_password_forgets_and_retain_drops_removed_accounts() {
        let mut secrets = Secrets::default();
        secrets.set("a", "1");
        secrets.set("b", "2");
        assert!(secrets.set("a", ""));
        assert!(!secrets.has("a"));
        assert!(secrets.retain(&["c"]));
        assert!(secrets.passwords.is_empty());
    }
}
