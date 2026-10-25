// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Configuration consumers use the shared five-root resolver.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir { Etc, Var, Run, Share }

pub fn path(dir: Dir) -> PathBuf {
    match dir {
        Dir::Etc => paths::root(paths::Root::Config),
        Dir::Var => paths::root(paths::Root::State),
        Dir::Run => paths::root(paths::Root::Runtime),
        Dir::Share => paths::share(""),
    }
}

/// Snapshot on the main thread; filesystem resolution can happen later.
#[derive(Debug, Clone)]
pub struct Environment {
    pub vars: BTreeMap<String, String>,
    pub exe: PathBuf,
}
impl Environment {
    pub fn current() -> Self {
        Self { vars: std::env::vars().collect(), exe: std::env::current_exe().expect("self location") }
    }
}
#[derive(Debug, Clone)]
pub struct Dirs { etc: PathBuf, var: PathBuf, run: PathBuf, share: PathBuf }
impl Dirs {
    pub fn resolve(env: &Environment) -> Self {
        let l = paths::Layout::from_environment(&env.exe, &env.vars).expect("invalid MixOS layout");
        Self {
            etc: l.root(paths::Root::Config), var: l.root(paths::Root::State),
            run: l.root(paths::Root::Runtime), share: l.root(paths::Root::Prefix).join("share"),
        }
    }
    pub fn get(&self, dir: Dir) -> &Path {
        match dir { Dir::Etc => &self.etc, Dir::Var => &self.var, Dir::Run => &self.run, Dir::Share => &self.share }
    }
}
