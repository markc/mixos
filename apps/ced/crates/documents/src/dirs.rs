// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! App-owned filenames over config's shared per-application directories.
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

pub const COMPONENT: &str = "ced";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs(config::AppDirs);

impl std::ops::Deref for AppDirs {
    type Target = config::AppDirs;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AppDirs {
    pub fn resolve(component: &str) -> Option<Self> {
        config::AppDirs::resolve(component).map(Self)
    }
    pub fn resolve_with(component: &str, get: impl Fn(&str) -> Option<PathBuf>) -> Option<Self> {
        config::AppDirs::resolve_with(component, get).map(Self)
    }
    pub fn config_file(&self) -> PathBuf {
        self.config().join("ced.conf.mix")
    }
    /// Per-app theme override (layered over the shared `theme.conf.mix`).
    pub fn theme_override(&self) -> PathBuf {
        self.config().join("theme.conf.mix")
    }
    pub fn session_file(&self) -> PathBuf {
        self.state().join("session.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directories_follow_the_shared_layout() {
        let d = AppDirs::resolve_with("ced", |key| match key {
            "MIXOS_PREFIX" => Some("/srv/tree".into()),
            "MIXOS_CONFIG" => Some("/isolated/config".into()),
            "MIXOS_STATE" => Some("/isolated/state".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(d.root(), Path::new("/isolated/state/apps/ced"));
        assert_eq!(d.config(), Path::new("/isolated/config/apps/ced"));
        assert!(AppDirs::resolve_with("../escape", |_| None).is_none());
    }
}
