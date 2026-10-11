// SPDX-License-Identifier: MIT OR Apache-2.0
//! App-owned filenames over config's shared per-application directories.

use std::path::{Path, PathBuf};

pub const COMPONENT: &str = "dopus";

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

    /// The directory the core's `ConfigFile::load` reads `config.conf.mix`
    /// from (the core owns the file name).
    pub fn config_dir(&self) -> PathBuf {
        self.config()
    }
}

/// Expand a leading `~` (or `~/…`) to the user's home: the location bar and
/// `dopus.open` accept both spellings. `~user` is left as it is; a file
/// manager's address field does no passwd lookup. The core's
/// `home_directory` is the one source of the home path.
pub fn expand_tilde(value: &str) -> PathBuf {
    expand_tilde_with(&dopus_core::home_directory(), value)
}

/// [`expand_tilde`] with the home given, so tests need not set `HOME`.
pub fn expand_tilde_with(home: &Path, value: &str) -> PathBuf {
    if value == "~" {
        return home.to_path_buf();
    }
    match value.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<PathBuf> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| PathBuf::from(v))
        }
    }

    #[test]
    fn resolution_order() {
        let all = &[
            ("MIXOS_APP_HOME", "/a"),
            ("MIXOS_APPS_HOME", "/b"),
            ("XDG_STATE_HOME", "/c"),
            ("HOME", "/h"),
        ];
        assert_eq!(
            AppDirs::resolve_with("dopus", env(all)).unwrap().root(),
            Path::new("/a")
        );
        let one = &[("HOME", "/h")];
        let d = AppDirs::resolve_with("dopus", env(one)).unwrap();
        assert_eq!(d.root(), Path::new("/h/.local/state/mixos/apps/dopus"));
        assert_eq!(
            d.config_dir(),
            Path::new("/h/.local/state/mixos/apps/dopus/config")
        );
    }

    #[test]
    fn tilde_expands_only_a_bare_or_slashed_tilde() {
        let home = Path::new("/h");
        assert_eq!(expand_tilde_with(home, "~"), PathBuf::from("/h"));
        assert_eq!(expand_tilde_with(home, "~/docs"), PathBuf::from("/h/docs"));
        assert_eq!(expand_tilde_with(home, "~user/x"), PathBuf::from("~user/x"));
        assert_eq!(expand_tilde_with(home, "/abs"), PathBuf::from("/abs"));
    }
}
