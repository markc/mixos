// SPDX-License-Identifier: MIT OR Apache-2.0
//! Main's uid/XDG directory rule, with a relocatable installation prefix.
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

pub type Result<T> = std::result::Result<T, String>;
pub const ROOT_VAR: &str = "MIXOS";
pub const DEFAULT_PREFIX: &str = "/opt/mixos";
/// Historical assertion constant; runtime Share derives from the executable.
#[cfg(test)]
pub const DEFAULT_SHARE: &str = "/opt/mixos/share";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum Root {
    Prefix,
    Share,
    Etc,
    Var,
    Run,
    Cache,
}
impl Root {
    pub const ALL: [Self; 6] = [
        Self::Prefix,
        Self::Share,
        Self::Etc,
        Self::Var,
        Self::Run,
        Self::Cache,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Prefix => "prefix",
            Self::Share => "share",
            Self::Etc => "etc",
            Self::Var => "var",
            Self::Run => "run",
            Self::Cache => "cache",
        }
    }
    pub const fn env_var(self) -> &'static str {
        match self {
            Self::Prefix => "MIXOS_PREFIX",
            Self::Share => "MIXOS_SHARE",
            Self::Etc => "MIXOS_ETC",
            Self::Var => "MIXOS_VAR",
            Self::Run => "MIXOS_RUN",
            Self::Cache => "MIXOS_CACHE",
        }
    }
    pub fn parse(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|r| r.name() == name)
            .ok_or_else(|| format!("unknown MixOS root: {name}"))
    }
}

/// Original four-directory API. ALL and env_var retain main's contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir {
    Etc,
    Var,
    Run,
    Share,
}
impl Dir {
    pub const ALL: [Self; 4] = [Self::Etc, Self::Var, Self::Run, Self::Share];
    pub const fn env_var(self) -> &'static str {
        self.root().env_var()
    }
    pub const fn root(self) -> Root {
        match self {
            Self::Etc => Root::Etc,
            Self::Var => Root::Var,
            Self::Run => Root::Run,
            Self::Share => Root::Share,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    System,
    User,
}
impl Tier {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "system" => Ok(Self::System),
            "user" => Ok(Self::User),
            _ => Err("tier must be system or user".into()),
        }
    }
    pub const fn uid(self) -> u32 {
        match self {
            Self::System => 0,
            Self::User => 1000,
        }
    }
}

/// Only consumed keys are read, using var_os. Empty variables are unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environment {
    pub root: Option<PathBuf>,
    pub etc: Option<PathBuf>,
    pub var: Option<PathBuf>,
    pub run: Option<PathBuf>,
    pub share: Option<PathBuf>,
    pub prefix: Option<PathBuf>,
    pub cache: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub xdg_runtime_dir: Option<PathBuf>,
    pub xdg_cache_home: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub uid: u32,
    /// Explicit executable input for pure tests. None preserves the historical
    /// install prefix in the legacy Dirs::resolve compatibility API.
    pub exe: Option<PathBuf>,
}
impl Environment {
    pub fn from_lookup(
        get: impl Fn(&str) -> Option<PathBuf>,
        uid: u32,
        exe: Option<PathBuf>,
    ) -> Self {
        let var = |key: &str| -> Option<PathBuf> { get(key).filter(|p| !p.as_os_str().is_empty()) };
        Self {
            root: var(ROOT_VAR),
            etc: var("MIXOS_ETC"),
            var: var("MIXOS_VAR"),
            run: var("MIXOS_RUN"),
            share: var("MIXOS_SHARE"),
            prefix: var("MIXOS_PREFIX"),
            cache: var("MIXOS_CACHE"),
            xdg_config_home: var("XDG_CONFIG_HOME"),
            xdg_data_home: var("XDG_DATA_HOME"),
            xdg_runtime_dir: var("XDG_RUNTIME_DIR"),
            xdg_cache_home: var("XDG_CACHE_HOME"),
            home: var("HOME"),
            uid,
            exe,
        }
    }
    pub fn current() -> Self {
        Self::from_lookup(
            |name| std::env::var_os(name).map(PathBuf::from),
            current_uid(),
            std::fs::read_link("/proc/self/exe")
                .or_else(|_| std::env::current_exe())
                .ok(),
        )
    }
}
pub fn current_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no arguments, cannot fail, and does not mutate state.
    unsafe { getuid() }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    roots: BTreeMap<Root, PathBuf>,
}
pub type Layout = Dirs;
impl Dirs {
    /// Main's pure API. Invalid new prefix/layout input is a startup error;
    /// fallible consumers should use from_source instead of this expect.
    pub fn resolve(env: &Environment) -> Self {
        Self::from_source(env, "").expect("invalid MixOS startup layout")
    }
    /// Pure rule: override > MIXOS > layout > uid tier. Share ignores MIXOS
    /// and accepts only an absolute override, exactly as main did.
    pub fn from_source(env: &Environment, source: &str) -> Result<Self> {
        let settings = parse_layout(source)?;
        let prefix = match &env.prefix {
            Some(p) => p.clone(),
            None => match &env.exe {
                Some(exe) => prefix_from_exe(exe)?,
                None => PathBuf::from(DEFAULT_PREFIX),
            },
        };
        absolute(&prefix)?;
        let home = env.home.clone().unwrap_or_else(|| PathBuf::from("/root"));
        let xdg = |base: &Option<PathBuf>, default: &str| {
            base.clone()
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(default))
                .join("mixos")
        };
        let choose = |root: Root, direct: &Option<PathBuf>, suffix: &str, fallback: PathBuf| {
            direct
                .clone()
                .or_else(|| env.root.as_ref().map(|r| r.join(suffix)))
                .or_else(|| settings.get(&root).cloned())
                .unwrap_or(fallback)
        };
        let user = env.uid != 0;
        let etc = choose(
            Root::Etc,
            &env.etc,
            "etc",
            if user {
                xdg(&env.xdg_config_home, ".config")
            } else {
                "/etc/mixos".into()
            },
        );
        let var = choose(
            Root::Var,
            &env.var,
            "var",
            if user {
                xdg(&env.xdg_data_home, ".local/share")
            } else {
                "/var/lib/mixos".into()
            },
        );
        let run = choose(
            Root::Run,
            &env.run,
            "run",
            if user {
                env.xdg_runtime_dir
                    .clone()
                    .filter(|p| p.is_absolute())
                    .map(|p| p.join("mixos"))
                    .unwrap_or_else(|| "/tmp/mixos-run".into())
            } else {
                "/run/mixos".into()
            },
        );
        let cache = choose(
            Root::Cache,
            &env.cache,
            "cache",
            if user {
                xdg(&env.xdg_cache_home, ".cache")
            } else {
                "/var/cache/mixos".into()
            },
        );
        let share = env
            .share
            .clone()
            .filter(|p| p.is_absolute())
            .or_else(|| settings.get(&Root::Share).cloned())
            .unwrap_or_else(|| prefix.join("share"));
        Ok(Self {
            roots: BTreeMap::from([
                (Root::Prefix, prefix),
                (Root::Share, share),
                (Root::Etc, etc),
                (Root::Var, var),
                (Root::Run, run),
                (Root::Cache, cache),
            ]),
        })
    }
    /// Explicit env, uid and executable inputs. from_source does no file I/O.
    pub fn from_environment(env: &Environment) -> Result<Self> {
        let mut env = env.clone();
        if let Some(exe) = &env.exe {
            env.exe = Some(exe.canonicalize().unwrap_or_else(|_| exe.clone()));
        }
        let prefix = Self::from_source(&env, "")?.try_root(Root::Prefix)?;
        Self::from_source(&env, &read_layout(&prefix)?)
    }
    pub fn from_lookup(get: impl Fn(&str) -> Option<PathBuf>) -> Result<Self> {
        let exe = std::fs::read_link("/proc/self/exe")
            .or_else(|_| std::env::current_exe())
            .map_err(|e| e.to_string())?;
        Self::from_environment(&Environment::from_lookup(get, current_uid(), Some(exe)))
    }
    /// Installer defaults ignore self-location, all MIXOS overrides and files.
    pub fn defaults(tier: &str, prefix: Option<&Path>, inputs: &Environment) -> Result<Self> {
        let uid = Tier::parse(tier)?.uid();
        let prefix = match (prefix, uid) {
            (Some(p), _) => p.to_path_buf(),
            (None, 0) => DEFAULT_PREFIX.into(),
            _ => return Err("user installation defaults require --prefix".into()),
        };
        Self::from_source(
            &Environment {
                uid,
                prefix: Some(prefix),
                home: inputs.home.clone(),
                xdg_config_home: inputs.xdg_config_home.clone(),
                xdg_data_home: inputs.xdg_data_home.clone(),
                xdg_runtime_dir: inputs.xdg_runtime_dir.clone(),
                xdg_cache_home: inputs.xdg_cache_home.clone(),
                ..Environment::default()
            },
            "",
        )
    }
    pub fn get(&self, dir: Dir) -> &Path {
        &self.roots[&dir.root()]
    }
    pub fn try_root(&self, root: Root) -> Result<PathBuf> {
        self.roots
            .get(&root)
            .cloned()
            .ok_or_else(|| format!("unresolved root: {}", root.name()))
    }
    /// Startup convenience, with the same explicit expect contract as path().
    pub fn root(&self, root: Root) -> PathBuf {
        self.try_root(root).expect("unresolved MixOS startup root")
    }
    pub fn path(&self, root: Root, rel: impl AsRef<Path>) -> Result<PathBuf> {
        let mut tail = PathBuf::new();
        for part in rel.as_ref().components() {
            match part {
                Component::Normal(s) => tail.push(s),
                Component::CurDir => {}
                Component::ParentDir if tail.pop() => {}
                _ => return Err("MixOS relative path escapes its root".into()),
            }
        }
        Ok(self.try_root(root)?.join(tail))
    }
    pub fn resolve_path(&self, value: &str) -> Result<PathBuf> {
        if let Some(logical) = value.strip_prefix("mixos:") {
            let (root, rel) = logical.split_once('/').unwrap_or((logical, ""));
            self.path(Root::parse(root)?, rel)
        } else {
            Ok(value.into())
        }
    }
}

/// Pure derivation from an already canonical executable. from_environment
/// canonicalises explicit inputs; current() obtains the real /proc target.
pub fn prefix_from_exe(exe: &Path) -> Result<PathBuf> {
    let prefix = exe
        .parent()
        .and_then(Path::parent)
        .ok_or("executable has no prefix parent")?
        .to_path_buf();
    absolute(&prefix)?;
    Ok(prefix)
}
fn absolute(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.components().any(|c| c == Component::ParentDir)
        || path
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .any(|c| c.is_ascii_control())
    {
        Err(format!(
            "MixOS root must be an absolute path without '..' or controls: {}",
            path.display()
        ))
    } else {
        Ok(())
    }
}
fn read_layout(prefix: &Path) -> Result<String> {
    use std::io::Read;
    let file = prefix.join("layout.mx");
    let mut input = match std::fs::File::open(&file) {
        Ok(f) => f.take(65537),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("layout.mx exceeds 64 KiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "layout.mx is not UTF-8".into())
}

struct Snapshot {
    process: Layout,
    user: Layout,
    system: Layout,
}
static SNAPSHOT: OnceLock<Result<Snapshot>> = OnceLock::new();
fn snapshot() -> Result<&'static Snapshot> {
    SNAPSHOT
        .get_or_init(|| {
            let env = Environment::current();
            if env.prefix.is_none() && env.exe.is_none() {
                return Err("cannot locate the running executable".into());
            }
            let prefix = Layout::from_source(&env, "")?.try_root(Root::Prefix)?;
            let source = read_layout(&prefix)?;
            let process = Layout::from_source(&env, &source)?;
            let user = Layout::from_source(
                &Environment {
                    uid: 1000,
                    ..env.clone()
                },
                &source,
            )?;
            let system = Layout::from_source(&Environment { uid: 0, ..env }, &source)?;
            Ok(Snapshot {
                process,
                user,
                system,
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}
impl Layout {
    pub fn discover() -> Result<Self> {
        Ok(snapshot()?.process.clone())
    }
    pub fn for_tier(tier: Tier) -> Result<Self> {
        let snapshot = snapshot()?;
        Ok(match tier {
            Tier::System => &snapshot.system,
            Tier::User => &snapshot.user,
        }
        .clone())
    }
}
pub fn try_root(root: Root) -> Result<PathBuf> {
    snapshot()?.process.try_root(root)
}
pub fn try_system(root: Root) -> Result<PathBuf> {
    snapshot()?.system.try_root(root)
}
/// A historical fixed FHS fallback, explicitly independent of process
/// overrides. APIs that promised to ignore MIXOS_RUN (native bus fallback)
/// or kept storage at a fixed system path use this rather than moving data.
pub fn system_default(root: Root) -> Result<PathBuf> {
    if matches!(root, Root::Prefix | Root::Share) {
        return Err("installed roots require self-location".into());
    }
    static DEFAULTS: OnceLock<Result<Layout>> = OnceLock::new();
    DEFAULTS
        .get_or_init(|| Layout::defaults("system", None, &Environment::default()))
        .as_ref()
        .map_err(Clone::clone)?
        .try_root(root)
}
/// Startup convenience. Invalid layout is a startup error; fallible operations
/// should use try_root/try_system/resolve instead.
pub fn root(root: Root) -> PathBuf {
    try_root(root).expect("invalid MixOS startup layout")
}
/// Explicit system tier, retaining per-root override/MIXOS/layout precedence.
pub fn system(root: Root) -> PathBuf {
    try_system(root).expect("invalid MixOS system startup layout")
}
pub fn path(dir: Dir) -> PathBuf {
    root(dir.root())
}
pub fn bin(rel: impl AsRef<Path>) -> PathBuf {
    snapshot()
        .and_then(|s| s.process.path(Root::Prefix, Path::new("bin").join(rel)))
        .expect("invalid MixOS startup binary path")
}
pub fn share(rel: impl AsRef<Path>) -> PathBuf {
    snapshot()
        .and_then(|s| s.process.path(Root::Share, rel))
        .expect("invalid MixOS startup shared path")
}
pub fn resolve(value: impl AsRef<str>) -> Result<PathBuf> {
    snapshot()?.process.resolve_path(value.as_ref())
}

mod document;
/// Strict, bounded data, exported for cross-parser contract tests.
pub fn parse_layout(source: &str) -> Result<BTreeMap<Root, PathBuf>> {
    document::parse(source)
}

#[cfg(test)]
mod layout_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> Environment {
        Environment {
            home: Some(PathBuf::from("/home/user")),
            uid: 1000,
            ..Environment::default()
        }
    }

    fn system() -> Environment {
        Environment {
            home: Some(PathBuf::from("/root")),
            uid: 0,
            ..Environment::default()
        }
    }

    #[test]
    fn overrides_win_over_everything() {
        let env = Environment {
            root: Some(PathBuf::from("/srv/mixos")),
            etc: Some(PathBuf::from("/sandbox/etc")),
            var: Some(PathBuf::from("/sandbox/var")),
            run: Some(PathBuf::from("/sandbox/run")),
            share: Some(PathBuf::from("/sandbox/share")),
            ..user()
        };
        let dirs = Dirs::resolve(&env);
        assert_eq!(dirs.get(Dir::Etc), Path::new("/sandbox/etc"));
        assert_eq!(dirs.get(Dir::Var), Path::new("/sandbox/var"));
        assert_eq!(dirs.get(Dir::Run), Path::new("/sandbox/run"));
        assert_eq!(dirs.get(Dir::Share), Path::new("/sandbox/share"));
    }

    #[test]
    fn a_root_places_etc_var_and_run_under_it_for_user_and_system() {
        for mut env in [user(), system()] {
            env.root = Some(PathBuf::from("/srv/mixos"));
            let dirs = Dirs::resolve(&env);
            assert_eq!(dirs.get(Dir::Etc), Path::new("/srv/mixos/etc"));
            assert_eq!(dirs.get(Dir::Var), Path::new("/srv/mixos/var"));
            assert_eq!(dirs.get(Dir::Run), Path::new("/srv/mixos/run"));
            // Share is installed data, not part of the root.
            assert_eq!(dirs.get(Dir::Share), Path::new(DEFAULT_SHARE));
        }
    }

    #[test]
    fn a_user_without_a_root_gets_the_xdg_defaults_under_home() {
        let dirs = Dirs::resolve(&user());
        assert_eq!(dirs.get(Dir::Etc), Path::new("/home/user/.config/mixos"));
        assert_eq!(
            dirs.get(Dir::Var),
            Path::new("/home/user/.local/share/mixos")
        );
        assert_eq!(dirs.get(Dir::Run), Path::new("/tmp/mixos-run"));
        assert_eq!(dirs.get(Dir::Share), Path::new(DEFAULT_SHARE));
    }

    #[test]
    fn a_user_with_xdg_variables_gets_mixos_under_them() {
        let env = Environment {
            xdg_config_home: Some(PathBuf::from("/home/user/cfg")),
            xdg_data_home: Some(PathBuf::from("/home/user/data")),
            xdg_runtime_dir: Some(PathBuf::from("/run/user/1000")),
            ..user()
        };
        let dirs = Dirs::resolve(&env);
        assert_eq!(dirs.get(Dir::Etc), Path::new("/home/user/cfg/mixos"));
        assert_eq!(dirs.get(Dir::Var), Path::new("/home/user/data/mixos"));
        assert_eq!(dirs.get(Dir::Run), Path::new("/run/user/1000/mixos"));
    }

    #[test]
    fn relative_xdg_variables_are_ignored() {
        let env = Environment {
            xdg_config_home: Some(PathBuf::from("cfg")),
            xdg_data_home: Some(PathBuf::from("data")),
            xdg_runtime_dir: Some(PathBuf::from("run")),
            ..user()
        };
        let dirs = Dirs::resolve(&env);
        assert_eq!(dirs.get(Dir::Etc), Path::new("/home/user/.config/mixos"));
        assert_eq!(
            dirs.get(Dir::Var),
            Path::new("/home/user/.local/share/mixos")
        );
        assert_eq!(dirs.get(Dir::Run), Path::new("/tmp/mixos-run"));
    }

    #[test]
    fn root_user_without_a_root_gets_the_system_directories() {
        let dirs = Dirs::resolve(&system());
        assert_eq!(dirs.get(Dir::Etc), Path::new("/etc/mixos"));
        assert_eq!(dirs.get(Dir::Var), Path::new("/var/lib/mixos"));
        assert_eq!(dirs.get(Dir::Run), Path::new("/run/mixos"));
        assert_eq!(dirs.get(Dir::Share), Path::new(DEFAULT_SHARE));
        // XDG variables do not move root's directories.
        let env = Environment {
            xdg_config_home: Some(PathBuf::from("/root/cfg")),
            ..system()
        };
        assert_eq!(Dirs::resolve(&env).get(Dir::Etc), Path::new("/etc/mixos"));
    }

    #[test]
    fn no_home_falls_back_to_root_home() {
        let env = Environment {
            home: None,
            ..user()
        };
        assert_eq!(
            Dirs::resolve(&env).get(Dir::Etc),
            Path::new("/root/.config/mixos")
        );
    }

    #[test]
    fn share_takes_only_an_absolute_override() {
        let absolute = Environment {
            share: Some(PathBuf::from("/srv/resources")),
            ..user()
        };
        assert_eq!(
            Dirs::resolve(&absolute).get(Dir::Share),
            Path::new("/srv/resources")
        );
        let relative = Environment {
            share: Some(PathBuf::from("relative")),
            ..user()
        };
        assert_eq!(
            Dirs::resolve(&relative).get(Dir::Share),
            Path::new(DEFAULT_SHARE)
        );
    }

    #[test]
    fn every_dir_has_its_own_variable() {
        let vars: Vec<&str> = Dir::ALL.iter().map(|d| d.env_var()).collect();
        assert_eq!(vars, ["MIXOS_ETC", "MIXOS_VAR", "MIXOS_RUN", "MIXOS_SHARE"]);
    }

    #[test]
    fn path_resolves_from_the_process_and_is_stable() {
        // Whatever the environment, the cached answer is the rule applied
        // to it, and the same on every call.
        let first = path(Dir::Etc);
        assert_eq!(first, Dirs::resolve(&Environment::current()).get(Dir::Etc));
        assert_eq!(path(Dir::Etc), first);
    }
}
