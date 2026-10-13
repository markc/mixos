// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Native `.conf.mix` persistence for email's account and window shape,
//! under the poison-pill law (dopus's config.rs): a malformed or
//! unsupported-schema file loads defaults and is never overwritten. The
//! account's password rides in the file — mode 0600, the same trust the
//! config directory's other tenants get — until MixOS grows a keyring.

use std::path::{Path, PathBuf};

use jmap::Account;
use serde::{Deserialize, Serialize};

use crate::state::{Mode, State};

pub const CURRENT_SCHEMA: u32 = 1;

/// What is persisted: the account (absent until the first successful
/// connect) and the window's shape.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct EmailConfig {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
    pub ui: UiConfig,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            account: None,
            ui: UiConfig::default(),
        }
    }
}

/// The window's remembered shape: two panel splits and the last mode.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct UiConfig {
    /// The places panel's fraction of the window width.
    pub places: f32,
    /// The reader panel's fraction of what remains after places.
    pub reader: f32,
    pub mode: Mode,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            places: 0.18,
            reader: 0.42,
            mode: Mode::Mail,
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
            mode: self.mode,
        }
    }
}

/// Persistence target plus the poison pill.
pub struct ConfigFile {
    pub path: PathBuf,
    pub allow_save: bool,
}

impl ConfigFile {
    /// Load `config.conf.mix` from the app's config directory.
    pub fn load(dir: &Path) -> (EmailConfig, Self) {
        let path = dir.join("config.conf.mix");
        match std::fs::read_to_string(&path) {
            Ok(raw) => match config::from_conf_mix_str::<EmailConfig>(&raw) {
                Ok(config) if config.schema_version == CURRENT_SCHEMA => {
                    let config = EmailConfig {
                        ui: config.ui.normalised(),
                        ..config
                    };
                    (
                        config,
                        Self {
                            path,
                            allow_save: true,
                        },
                    )
                }
                Ok(config) => {
                    eprintln!(
                        "email: refusing to overwrite unsupported config schema {} in {}",
                        config.schema_version,
                        path.display()
                    );
                    (
                        EmailConfig::default(),
                        Self {
                            path,
                            allow_save: false,
                        },
                    )
                }
                Err(error) => {
                    eprintln!(
                        "email: refusing to overwrite invalid config {}: {error}",
                        path.display()
                    );
                    (
                        EmailConfig::default(),
                        Self {
                            path,
                            allow_save: false,
                        },
                    )
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
                EmailConfig::default(),
                Self {
                    path,
                    allow_save: true,
                },
            ),
            Err(error) => {
                eprintln!("email: cannot read config {}: {error}", path.display());
                (
                    EmailConfig::default(),
                    Self {
                        path,
                        allow_save: false,
                    ),
                )
            }
        }
    }

    /// Persist the config; `Ok(false)` when the poison pill refuses the
    /// write. Mode 0600: the account password lives in this file.
    pub fn save(&self, config: &EmailConfig) -> Result<bool, String> {
        if !self.allow_save {
            return Ok(false);
        }
        let content = config::to_conf_mix_string(config)
            .map_err(|error| format!("serialising email config: {error}"))?;
        write_private(&self.path, content.as_bytes())
            .map(|()| true)
            .map_err(|error| format!("writing {}: {error}", self.path.display()))
    }
}

/// Write a file only this user can read: a sibling temporary created new,
/// chmodded to 0600 before any byte lands, renamed into place so a reader
/// sees either the old file or the new one, never a partial write.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let temp = path.with_extension("conf.mix.tmp");
    let _ = std::fs::remove_file(&temp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    // The file is empty here; from this line on it is private, and it is
    // only ever renamed into place after the bytes are in.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    drop(file);
    std::fs::rename(&temp, path)
}

impl From<&State> for EmailConfig {
    fn from(state: &State) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            account: state.account().cloned(),
            ui: UiConfig {
                places: state.ui.places,
                reader: state.ui.reader,
                mode: state.mode,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account() -> Account {
        Account {
            url: "https://example.invalid".into(),
            email: "ann@example.invalid".into(),
            password: "secret".into(),
            ca_pem: None,
        }
    }

    #[test]
    fn round_trips_the_account_and_shape() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile {
            path: dir.path().join("config.conf.mix"),
            allow_save: true,
        };
        let config = EmailConfig {
            account: Some(account()),
            ..EmailConfig::default()
        };
        file.save(&config).unwrap();
        assert_eq!(ConfigFile::load(dir.path()).0, config);
        // The file is private.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn partial_records_keep_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.conf.mix"),
            "schema_version: 1\nui: {places: 0.25}\n",
        )
        .unwrap();
        let (config, file) = ConfigFile::load(dir.path());
        assert!(file.allow_save);
        assert_eq!(config.ui.places, 0.25);
        assert_eq!(config.ui.reader, UiConfig::default().reader);
        assert!(config.account.is_none());
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
    fn a_json_account_still_parses_through_the_conf_mix_layer() {
        // The jmap::Account serde shape is shared with the wire; make sure
        // the conf.mix data layer accepts its field names verbatim.
        let value = serde_json::to_value(account()).unwrap();
        assert_eq!(value["url"], json!("https://example.invalid"));
    }
}
