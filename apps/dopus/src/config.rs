// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Session config: the core's `ConfigFile` pointed at dopus's config
//! directory. The file, schema and poison-pill law all live in the core
//! (`dopus_core::config`); this module resolves the directory and renders
//! the resolved config for `--print-config`.

use std::path::Path;

use dopus_core::{ConfigFile, DOpusConfig};

/// The narrowest and widest the left pane may be, as a share of both.
pub const SPLIT_MIN: f32 = 0.1;
pub const SPLIT_MAX: f32 = 0.9;

/// Load `config.conf.mix` from `dir` (the core handles missing files,
/// malformed files and foreign schemas). Without a directory there is
/// nothing to load and nothing to save into. The directory is created when
/// missing, because the core's atomic write does not create parents.
pub fn load(dir: Option<&Path>) -> (DOpusConfig, Option<ConfigFile>) {
    match dir {
        Some(dir) => {
            if let Err(error) = std::fs::create_dir_all(dir) {
                eprintln!("dopus: cannot create config dir {}: {error}", dir.display());
            }
            let (mut config, file) = ConfigFile::load(dir);
            // A stale or hand-edited ratio clamps at load to the divider's
            // range, so `dopus.state` reports what the window draws.
            config.split_ratio = config.split_ratio.clamp(SPLIT_MIN, SPLIT_MAX);
            (config, Some(file))
        }
        None => (DOpusConfig::default(), None),
    }
}

/// The resolved configuration as pretty JSON (`--print-config`).
pub fn to_json(config: &DOpusConfig) -> String {
    serde_json::to_string_pretty(config).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_directory_is_defaults_without_a_file() {
        let (config, file) = load(None);
        assert_eq!(config, DOpusConfig::default());
        assert!(file.is_none());
    }

    #[test]
    fn boot_split_ratio_clamps_to_the_divider_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.conf.mix");
        std::fs::write(&path, b"schema_version: 1\nsplit_ratio: 5.0\n").unwrap();
        let (config, _) = load(Some(dir.path()));
        assert_eq!(config.split_ratio, SPLIT_MAX);
        std::fs::write(&path, b"schema_version: 1\nsplit_ratio: 0.01\n").unwrap();
        let (config, _) = load(Some(dir.path()));
        assert_eq!(config.split_ratio, SPLIT_MIN);
    }
}
