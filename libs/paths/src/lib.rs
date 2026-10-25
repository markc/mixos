// SPDX-License-Identifier: MIT OR Apache-2.0
//! The five MixOS roots. No dependency on another workspace component.
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Root {
    Prefix,
    Config,
    State,
    Cache,
    Runtime,
}

impl Root {
    pub const ALL: [Self; 5] = [Self::Prefix, Self::Config, Self::State, Self::Cache, Self::Runtime];

    pub fn name(self) -> &'static str {
        match self {
            Self::Prefix => "prefix",
            Self::Config => "config",
            Self::State => "state",
            Self::Cache => "cache",
            Self::Runtime => "runtime",
        }
    }

    pub fn parse(name: &str) -> Result<Self> {
        Self::ALL.into_iter().find(|r| r.name() == name)
            .ok_or_else(|| format!("unknown MixOS root: {name}"))
    }

    pub fn env(self) -> String {
        format!("MIXOS_{}", self.name().to_ascii_uppercase())
    }
}

/// Explicit inputs keep tests independent of process environment and uid.
#[derive(Clone, Debug)]
pub struct Layout(BTreeMap<Root, PathBuf>);

impl Layout {
    /// Installation defaults, independent of the interpreter's location,
    /// MIXOS_* overrides and any existing layout document.
    pub fn defaults(tier: &str, prefix: Option<&Path>, env: &BTreeMap<String, String>) -> Result<Self> {
        let prefix = match (prefix, tier) {
            (Some(p), "system" | "user") => p.to_path_buf(),
            (None, "system") => PathBuf::from("/opt/mixos"),
            (None, "user") => return Err("user installation defaults require --prefix".into()),
            _ => return Err("tier must be system or user".into()),
        };
        let mut inputs = env.clone();
        for r in Root::ALL { inputs.remove(&r.env()); }
        Self::configured(&prefix, &inputs, "", Some(tier))
    }

    pub fn discover() -> Result<Self> {
        let exe = std::fs::read_link("/proc/self/exe")
            .or_else(|_| std::env::current_exe()).map_err(|e| e.to_string())?;
        Self::from_environment(&exe, &std::env::vars().collect())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<PathBuf>) -> Result<Self> {
        let mut vars = BTreeMap::new();
        for key in ["HOME", "MIXOS_PREFIX", "MIXOS_CONFIG", "MIXOS_STATE", "MIXOS_CACHE", "MIXOS_RUNTIME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR"] {
            if let Some(value) = get(key) {
                vars.insert(key.to_string(), value.into_os_string().into_string().map_err(|_| "layout input is not UTF-8")?);
            }
        }
        Self::from_environment(&std::env::current_exe().map_err(|e| e.to_string())?, &vars)
    }

    pub fn from_environment(exe: &Path, env: &BTreeMap<String, String>) -> Result<Self> {
        // /proc/self/exe follows the managed entry symlink. Canonicalise explicit
        // executables too (useful to installers and tests).
        let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
        let derived = exe.parent().and_then(Path::parent)
            .ok_or("executable has no prefix parent")?;
        let prefix = env.get("MIXOS_PREFIX").map(PathBuf::from)
            .unwrap_or_else(|| derived.to_path_buf());
        absolute(&prefix)?;
        let file = prefix.join("layout.mx");
        let source = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", file.display())),
        };
        Self::from_source(&prefix, env, &source)
    }

    pub fn from_source(prefix: &Path, env: &BTreeMap<String, String>, source: &str) -> Result<Self> {
        Self::configured(prefix, env, source, None)
    }

    fn configured(prefix: &Path, env: &BTreeMap<String, String>, source: &str, tier: Option<&str>) -> Result<Self> {
        let settings = document::parse(source)?;
        let choose = |r: Root| env.get(&r.env()).map(PathBuf::from)
            .or_else(|| settings.get(&r).cloned());
        let prefix = choose(Root::Prefix).unwrap_or_else(|| prefix.to_path_buf());
        absolute(&prefix)?;
        let home = env.get("HOME").map(PathBuf::from);
        let user = tier.map(|t| t == "user").unwrap_or_else(||
            home.as_ref().is_some_and(|h| h.is_absolute() && prefix.starts_with(h)));
        let xdg = |key: &str, suffix: &str| -> Result<PathBuf> {
            if let Some(p) = env.get(key).map(PathBuf::from).filter(|p| p.is_absolute()) {
                return Ok(p.join("mixos"));
            }
            home.as_ref().map(|h| h.join(suffix).join("mixos"))
                .ok_or_else(|| format!("{key} or HOME is required for a user layout"))
        };
        let mut roots = BTreeMap::new();
        roots.insert(Root::Prefix, prefix);
        for r in [Root::Config, Root::State, Root::Cache, Root::Runtime] {
            let p = if let Some(p) = choose(r) { p } else if user {
                match r {
                    Root::Config => xdg("XDG_CONFIG_HOME", ".config")?,
                    Root::State => xdg("XDG_STATE_HOME", ".local/state")?,
                    Root::Cache => xdg("XDG_CACHE_HOME", ".cache")?,
                    Root::Runtime => match env.get("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
                        Some(p) => p.join("mixos"),
                        None => continue,
                    },
                    Root::Prefix => unreachable!(),
                }
            } else {
                // The only system defaults table. Runtime resolution derives
                // prefix from self-location; installers request it explicitly.
                PathBuf::from(match r {
                    Root::Config => "/etc/mixos",
                    Root::State => "/var/lib/mixos",
                    Root::Cache => "/var/cache/mixos",
                    Root::Runtime => "/run/mixos",
                    Root::Prefix => unreachable!(),
                })
            };
            absolute(&p)?;
            roots.insert(r, p);
        }
        Ok(Self(roots))
    }

    pub fn try_root(&self, r: Root) -> Result<PathBuf> {
        self.0.get(&r).cloned().ok_or_else(|| format!("{} root requires an override or XDG_RUNTIME_DIR", r.name()))
    }
    pub fn root(&self, r: Root) -> PathBuf { self.try_root(r).expect("unresolved MixOS root") }

    pub fn path(&self, r: Root, rel: impl AsRef<Path>) -> Result<PathBuf> {
        let mut tail = PathBuf::new();
        for c in rel.as_ref().components() {
            match c {
                Component::Normal(s) => tail.push(s),
                Component::CurDir => {},
                Component::ParentDir if tail.pop() => {},
                _ => return Err("MixOS relative path escapes its root".into()),
            }
        }
        Ok(self.try_root(r)?.join(tail))
    }

    pub fn resolve(&self, value: impl AsRef<str>) -> Result<PathBuf> {
        let value = value.as_ref();
        if let Some(logical) = value.strip_prefix("mixos:") {
            let (name, rel) = logical.split_once('/').unwrap_or((logical, ""));
            self.path(Root::parse(name)?, rel)
        } else { Ok(PathBuf::from(value)) }
    }
}

fn absolute(p: &Path) -> Result<()> {
    if !p.is_absolute() || p.components().any(|c| c == Component::ParentDir)
        || p.to_string_lossy().chars().any(char::is_control) {
        Err(format!("MixOS root must be an absolute path without '..' or controls: {}", p.display()))
    } else { Ok(()) }
}

pub fn try_root(r: Root) -> Result<PathBuf> { Layout::discover()?.try_root(r) }
pub fn root(r: Root) -> PathBuf { try_root(r).expect("invalid MixOS layout") }
pub fn bin(rel: impl AsRef<Path>) -> PathBuf {
    Layout::discover().and_then(|l| l.path(Root::Prefix, Path::new("bin").join(rel))).expect("invalid MixOS binary path")
}
pub fn share(rel: impl AsRef<Path>) -> PathBuf {
    Layout::discover().and_then(|l| l.path(Root::Prefix, Path::new("share").join(rel))).expect("invalid MixOS shared path")
}
pub fn resolve(value: impl AsRef<str>) -> Result<PathBuf> { Layout::discover()?.resolve(value) }

mod document;

#[cfg(test)]
mod tests {
    use super::*;
    fn env() -> BTreeMap<String, String> {
        BTreeMap::from([("HOME".into(), "/home/user".into()), ("XDG_RUNTIME_DIR".into(), "/run/user/1000".into())])
    }
    #[test]
    fn defaults_follow_the_prefix_tier_and_move() {
        let system = Layout::from_source(Path::new("/opt/mixos"), &env(), "").unwrap();
        assert_eq!(system.root(Root::Config), Path::new("/etc/mixos"));
        assert_eq!(system.root(Root::State), Path::new("/var/lib/mixos"));
        assert_eq!(system.root(Root::Cache), Path::new("/var/cache/mixos"));
        assert_eq!(system.root(Root::Runtime), Path::new("/run/mixos"));
        for prefix in ["/home/user/first", "/home/user/moved tree"] {
            let layout = Layout::from_source(Path::new(prefix), &env(), "").unwrap();
            assert_eq!(layout.root(Root::Prefix), Path::new(prefix));
            assert_eq!(layout.root(Root::State), Path::new("/home/user/.local/state/mixos"));
            assert_eq!(layout.root(Root::Config), Path::new("/home/user/.config/mixos"));
        }
    }
    #[test]
    fn installer_defaults_ignore_running_location_and_mixos_overrides() {
        let mut inputs = env();
        inputs.insert("MIXOS_PREFIX".into(), "/wrong/interpreter".into());
        inputs.insert("MIXOS_CONFIG".into(), "/wrong/config".into());
        let system = Layout::defaults("system", None, &inputs).unwrap();
        assert_eq!(system.root(Root::Prefix), Path::new("/opt/mixos"));
        assert_eq!(system.root(Root::Config), Path::new("/etc/mixos"));
        let target = Path::new("/srv/target with space");
        let user = Layout::defaults("user", Some(target), &inputs).unwrap();
        assert_eq!(user.root(Root::Prefix), target);
        assert_eq!(user.root(Root::Config), Path::new("/home/user/.config/mixos"));
        assert!(Layout::defaults("user", None, &inputs).is_err());
        assert!(Layout::defaults("unknown", Some(target), &inputs).is_err());
    }
    #[test]
    fn precedence_is_per_root() {
        let mut env = env();
        env.insert("MIXOS_CONFIG".into(), "/env/config".into());
        let layout = Layout::from_source(Path::new("/srv/tree"), &env,
            "config: '/file/config'\nstate: '/file/state'").unwrap();
        assert_eq!(layout.root(Root::Config), Path::new("/env/config"));
        assert_eq!(layout.root(Root::State), Path::new("/file/state"));
        assert_eq!(layout.root(Root::Cache), Path::new("/var/cache/mixos"));
        env.insert("MIXOS_PREFIX".into(), "/env/prefix".into());
        let layout = Layout::from_source(Path::new("/srv/tree"), &env, "prefix: '/file/prefix'").unwrap();
        assert_eq!(layout.root(Root::Prefix), Path::new("/env/prefix"));
    }
    #[test]
    fn an_unset_runtime_does_not_hide_other_roots() {
        let mut env = env();
        env.remove("XDG_RUNTIME_DIR");
        let layout = Layout::from_source(Path::new("/home/user/tree"), &env, "").unwrap();
        assert_eq!(layout.root(Root::Prefix), Path::new("/home/user/tree"));
        assert!(layout.try_root(Root::Runtime).is_err());
    }
    #[test]
    fn logical_paths_are_bounded() {
        let l = Layout::from_source(Path::new("/srv/moved tree"), &env(), "").unwrap();
        assert_eq!(l.resolve("mixos:prefix/share/a/../b").unwrap(), Path::new("/srv/moved tree/share/b"));
        for s in ["mixos:prefix/../a", "mixos:config/a/../../b", "mixos:bin/x", "mixos:prefix//etc"] {
            assert!(l.resolve(s).is_err(), "{s}");
        }
        assert_eq!(l.resolve("/home/user/photo").unwrap(), Path::new("/home/user/photo"));
    }
    #[test]
    fn strict_layout_refuses_unknown_duplicate_or_executable_data() {
        for s in ["other: '/x'", "config: '/x'\nconfig: '/y'", "config: env('X')", "config: \"${X}\"", "config: '/x';", "config: '../x'", "config: 1", "{} junk"] {
            assert!(Layout::from_source(Path::new("/srv/tree"), &env(), s).is_err(), "{s}");
        }
    }

    #[test]
    fn real_layout_file_and_symlink_survive_a_move() {
        let temp = std::env::temp_dir().join(format!("mixos-paths-{}", std::process::id()));
        let first = temp.join("first prefix");
        let moved = temp.join("moved prefix");
        std::fs::create_dir_all(first.join("bin")).unwrap();
        std::fs::write(first.join("bin/mix"), "fixture").unwrap();
        std::fs::write(first.join("layout.mx"), "config: '/file/config'\nstate: '/file/state'").unwrap();
        let mut inputs = env();
        inputs.insert("MIXOS_CONFIG".into(), "/env/config".into());
        let initial = Layout::from_environment(&first.join("bin/mix"), &inputs).unwrap();
        assert_eq!(initial.root(Root::Config), Path::new("/env/config"));
        assert_eq!(initial.root(Root::State), Path::new("/file/state"));
        std::fs::rename(&first, &moved).unwrap();
        let entry = temp.join("entry");
        std::os::unix::fs::symlink(moved.join("bin/mix"), &entry).unwrap();
        let after = Layout::from_environment(&entry, &inputs).unwrap();
        assert_eq!(after.root(Root::Prefix), moved);
        assert_eq!(after.root(Root::State), Path::new("/file/state"));
        inputs.insert("MIXOS_PREFIX".into(), after.root(Root::Prefix).to_string_lossy().into_owned());
        assert_eq!(Layout::from_environment(Path::new("/elsewhere/bin/mix"), &inputs).unwrap().root(Root::State), Path::new("/file/state"));
        std::fs::remove_dir_all(temp).unwrap();
    }
}
