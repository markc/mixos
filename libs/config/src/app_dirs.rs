// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Per-application directories beneath the five shared roots.
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs { state: PathBuf, config: PathBuf, cache: PathBuf }
impl AppDirs {
    pub fn resolve(component: &str) -> Option<Self> {
        Self::resolve_with(component, |key| std::env::var_os(key).map(PathBuf::from))
    }
    pub fn resolve_with(component: &str, get: impl Fn(&str) -> Option<PathBuf>) -> Option<Self> {
        let mut parts = Path::new(component).components();
        if component.contains('\\') || !matches!((parts.next(), parts.next()), (Some(Component::Normal(_)), None)) {
            return None;
        }
        let layout = paths::Layout::from_lookup(get).ok()?;
        let app = |r| layout.root(r).join("apps").join(component);
        Some(Self { state: app(paths::Root::State), config: app(paths::Root::Config), cache: app(paths::Root::Cache) })
    }
    pub fn root(&self) -> &Path { &self.state }
    pub fn config(&self) -> PathBuf { self.config.clone() }
    pub fn state(&self) -> PathBuf { self.state.clone() }
    pub fn cache(&self) -> PathBuf { self.cache.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_roots_remain_separate_and_components_cannot_escape() {
        let get = |key| match key {
            "MIXOS_PREFIX" => Some("/srv/tree".into()),
            "MIXOS_STATE" => Some("/isolated/state".into()),
            "MIXOS_CONFIG" => Some("/isolated/config".into()),
            "MIXOS_CACHE" => Some("/isolated/cache".into()),
            _ => None,
        };
        let dirs = AppDirs::resolve_with("ced", get).unwrap();
        assert_eq!(dirs.state(), Path::new("/isolated/state/apps/ced"));
        assert_eq!(dirs.config(), Path::new("/isolated/config/apps/ced"));
        assert_eq!(dirs.cache(), Path::new("/isolated/cache/apps/ced"));
        for name in ["", ".", "..", "/ced", "../ced", "ced/../other", "ced\\other"] {
            assert!(AppDirs::resolve_with(name, get).is_none(), "{name}");
        }
    }
}
