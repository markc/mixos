// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Source checkout discovery is separate from the installed five roots.
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, Copy)]
pub enum Dir {
    Bin,
}
pub fn mixos_path(_: Dir) -> PathBuf {
    ::paths::bin("")
}
pub fn mixos_root() -> Option<PathBuf> {
    locate_root(
        std::env::var_os("MIXOS_SRC").map(PathBuf::from),
        std::env::current_exe().ok().as_deref(),
    )
}
pub fn mixos_src() -> PathBuf {
    mixos_root().unwrap_or_else(|| {
        default_root(&std::env::var_os("HOME").map(PathBuf::from).expect("HOME"))
    })
}
pub fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}
pub fn default_root(home: &Path) -> PathBuf {
    home.join("Projects/mixos")
}
pub fn locate_root(explicit: Option<PathBuf>, exe: Option<&Path>) -> Option<PathBuf> {
    explicit.or_else(|| {
        exe?.ancestors()
            .skip(1)
            .take(6)
            .find(|p| p.join("AGENTS.md").is_file() && p.join("Cargo.toml").is_file())
            .map(Path::to_path_buf)
    })
}
pub(crate) struct EtcEnvironment(config::Environment);
impl EtcEnvironment {
    pub(crate) fn capture() -> Self {
        Self(config::Environment::current())
    }
    pub(crate) fn resolve(self) -> (PathBuf, bool) {
        (
            config::Dirs::resolve(&self.0)
                .get(config::Dir::Etc)
                .to_owned(),
            true,
        )
    }
}
