// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Resolution is pure; missing/relative HOME cannot supply a user home.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roots {
    pub builtin: PathBuf,
    pub user: PathBuf,
    pub home: PathBuf,
    pub mix: PathBuf,
}

impl Roots {
    /// Discover app folders using the same confinement rules as catalogue scans.
    pub fn apps(&self) -> io::Result<Vec<String>> {
        let mut apps = std::collections::BTreeSet::new();
        for root in [&self.user, &self.builtin] {
            let folders = match fs::read_dir(root) {
                Ok(folders) => folders,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for folder in folders {
                let folder = folder?;
                let name = folder.file_name().to_string_lossy().into_owned();
                if valid_app(&name)
                    && folder.path().is_dir()
                    && fs::canonicalize(folder.path())?.starts_with(fs::canonicalize(root)?)
                {
                    apps.insert(name);
                }
            }
        }
        Ok(apps.into_iter().collect())
    }

    pub fn read(&self, entry: &Entry) -> io::Result<String> {
        let mut text = String::new();
        checked_file(
            if entry.tier.is_user() {
                &self.user
            } else {
                &self.builtin
            },
            &entry.path,
        )?
        .read_to_string(&mut text)?;
        Ok(text)
    }

    fn user_target(&self, entry: &Entry) -> io::Result<PathBuf> {
        if !entry.tier.is_user() {
            return Err(invalid("built-in macros are read-only"));
        }
        checked_file(&self.user, &entry.path)?;
        fs::canonicalize(&entry.path)
    }

    /// Replace the resolved regular file atomically, preserving an in-root symlink.
    pub fn write(&self, entry: &Entry, text: &str) -> io::Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        self.user_target(entry)?;
        let target = check_editable(&entry.path)?;
        let parent = target
            .parent()
            .ok_or_else(|| invalid("macro has no parent"))?;
        let temporary = parent.join(format!(
            ".macro-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.set_permissions(fs::metadata(&target)?.permissions())?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &target)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn remove(&self, entry: &Entry) -> io::Result<()> {
        self.user_target(entry)?;
        fs::remove_file(&entry.path)
    }

    /// Exclusive destination allocation refuses clashes, including dangling links.
    pub fn move_to(&self, entry: &Entry, tier: Tier, app: &str) -> io::Result<PathBuf> {
        self.user_target(entry)?;
        if fs::symlink_metadata(&entry.path)?.file_type().is_symlink() {
            return Err(invalid("moving a macro symlink is not supported"));
        }
        let (destination, file) = self.new_file(app, tier, &entry.id)?;
        drop(file);
        if let Err(error) = fs::rename(&entry.path, &destination) {
            let _ = fs::remove_file(&destination);
            return Err(error);
        }
        Ok(destination)
    }
    pub fn resolve() -> Option<Self> {
        Self::resolve_with(|key| std::env::var_os(key).map(PathBuf::from))
    }

    pub fn resolve_with(get: impl Fn(&str) -> Option<PathBuf>) -> Option<Self> {
        let absolute = |p: PathBuf| p.is_absolute().then_some(p);
        let home = get("HOME").and_then(absolute)?;
        Some(Self {
            builtin: get("MIXOS_SHARE")
                .and_then(absolute)
                .unwrap_or_else(|| PathBuf::from("/opt/mixos/share"))
                .join("macros"),
            user: get("XDG_CONFIG_HOME")
                .and_then(absolute)
                .unwrap_or_else(|| home.join(".config"))
                .join("mixos/macros"),
            mix: get("MIXOS_MIX")
                .and_then(absolute)
                .unwrap_or_else(|| PathBuf::from("/opt/mixos/bin/mix")),
            home,
        })
    }

    pub fn folder(&self, app: &str, tier: Tier) -> io::Result<PathBuf> {
        if !valid_app(app) {
            return Err(invalid("invalid application component name"));
        }
        let root = if tier.is_user() {
            &self.user
        } else {
            &self.builtin
        };
        Ok(if tier.is_app() {
            root.join(app)
        } else {
            root.clone()
        })
    }

    /// Create only user directories; refuse a per-app symlink outside the root.
    pub fn ensure_user_folder(&self, app: &str, tier: Tier) -> io::Result<PathBuf> {
        if !tier.is_user() {
            return Err(invalid("built-in macros are read-only"));
        }
        let folder = self.folder(app, tier)?;
        fs::create_dir_all(&self.user)?;
        // Validate an existing parent before creating anything beneath it.
        if fs::symlink_metadata(&folder).is_ok()
            && !fs::canonicalize(&folder)?.starts_with(fs::canonicalize(&self.user)?)
        {
            return Err(invalid("macro folder escapes its root"));
        }
        fs::create_dir_all(&folder)?;
        if !fs::canonicalize(&folder)?.starts_with(fs::canonicalize(&self.user)?) {
            return Err(invalid("macro folder escapes its root"));
        }
        Ok(folder)
    }

    pub fn create(&self, app: &str, tier: Tier, id: &str, title: &str) -> io::Result<PathBuf> {
        if title.contains(['\n', '\r']) {
            return Err(invalid("title must be one line"));
        }
        let text = format!(
            "-- title: {title}\n-- SPDX-License-Identifier: MIT OR Apache-2.0\n\nprint(\"Hello from a Mix macro\")\n"
        );
        self.write_new(app, tier, id, text.as_bytes())
    }

    /// Slug a title and allocate a fresh name using the same exclusive create
    /// path as `create`. Only a name clash is retried.
    pub fn create_from_title(&self, app: &str, title: &str) -> io::Result<PathBuf> {
        self.create_from_title_in(app, Tier::UserApp, title)
    }

    pub fn create_from_title_in(&self, app: &str, tier: Tier, title: &str) -> io::Result<PathBuf> {
        if title.trim().is_empty() || title.contains(['\n', '\r']) {
            return Err(invalid("title must be a non-empty single line"));
        }
        let mut slug = String::new();
        let mut separator = false;
        for ch in title.chars() {
            if ch.is_ascii_alphanumeric() {
                if separator && !slug.is_empty() {
                    slug.push('-');
                }
                slug.push(ch.to_ascii_lowercase());
                separator = false;
            } else {
                separator = true;
            }
        }
        if slug.is_empty() {
            slug.push_str("macro");
        }
        let folder = self.folder(app, tier)?;
        for n in 1u64.. {
            let id = if n == 1 {
                slug.clone()
            } else {
                format!("{slug}-{n}")
            };
            match self.create(app, tier, &id, title) {
                // AlreadyExists can also come from create_dir_all when a
                // parent is a regular file. Retry only an occupied candidate,
                // including a dangling symlink that exclusive create refuses.
                Err(err)
                    if err.kind() == io::ErrorKind::AlreadyExists
                        && folder.join(format!("{id}.mix")).symlink_metadata().is_ok() =>
                {
                    continue;
                }
                result => return result,
            }
        }
        unreachable!("exhausted macro names")
    }

    pub fn copy_to_user(&self, app: &str, entry: &Entry) -> io::Result<PathBuf> {
        if entry.tier.is_user() {
            return Err(invalid("macro is already in a user tier"));
        }
        let mut source = checked_file(&self.builtin, &entry.path)?;
        let tier = if entry.tier.is_app() {
            Tier::UserApp
        } else {
            Tier::UserGlobal
        };
        let (path, mut destination) = self.new_file(app, tier, &entry.id)?;
        io::copy(&mut source, &mut destination)?;
        Ok(path)
    }

    fn write_new(&self, app: &str, tier: Tier, id: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        let (path, mut file) = self.new_file(app, tier, id)?;
        file.write_all(bytes)?;
        Ok(path)
    }

    fn new_file(&self, app: &str, tier: Tier, id: &str) -> io::Result<(PathBuf, fs::File)> {
        if !valid_id(id) {
            return Err(invalid("invalid macro id"));
        }
        let path = self
            .ensure_user_folder(app, tier)?
            .join(format!("{id}.mix"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok((path, file))
    }
}

/// Ascending precedence; stable strings are also passed to macro processes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    BuiltinGlobal,
    BuiltinApp,
    UserGlobal,
    UserApp,
}

impl Tier {
    pub const ALL: [Self; 4] = [
        Self::BuiltinGlobal,
        Self::BuiltinApp,
        Self::UserGlobal,
        Self::UserApp,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BuiltinGlobal => "builtin-global",
            Self::BuiltinApp => "builtin-app",
            Self::UserGlobal => "user-global",
            Self::UserApp => "user-app",
        }
    }
    pub fn is_user(self) -> bool {
        matches!(self, Self::UserGlobal | Self::UserApp)
    }
    pub fn is_app(self) -> bool {
        matches!(self, Self::BuiltinApp | Self::UserApp)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub tier: Tier,
    pub path: PathBuf,
    pub shadows: Vec<Tier>,
    pub targets: Option<Targets>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Targets {
    Any,
    List(Vec<String>),
}

impl Targets {
    pub fn allows(&self, app: &str) -> bool {
        valid_app(app)
            && match self {
                Self::Any => true,
                Self::List(apps) => apps.iter().any(|name| name == app),
            }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct Catalogue {
    pub entries: Vec<Entry>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Catalogue {
    pub fn scan(roots: &Roots, app: &str) -> io::Result<Self> {
        Self::scan_tiers(roots, app, &[Tier::BuiltinApp, Tier::UserApp])
    }

    /// Global macros belong to Workshop, independently of app catalogues.
    pub fn scan_globals(roots: &Roots) -> io::Result<Self> {
        Self::scan_tiers(roots, "workshop", &[Tier::BuiltinGlobal, Tier::UserGlobal])
    }

    fn scan_tiers(roots: &Roots, app: &str, tiers: &[Tier]) -> io::Result<Self> {
        let mut result = Self::default();
        let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
        for &tier in tiers {
            let folder = roots.folder(app, tier)?;
            let root = if tier.is_user() {
                &roots.user
            } else {
                &roots.builtin
            };
            if let Err(err) = result.scan_tier(root, &folder, tier, &mut entries)
                && err.kind() != io::ErrorKind::NotFound
            {
                result.diagnose(folder, err.to_string());
            }
        }
        result.entries = entries.into_values().collect();
        result.diagnostics.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(result)
    }

    fn diagnose(&mut self, path: PathBuf, message: String) {
        self.diagnostics.push(Diagnostic { path, message });
    }

    fn scan_tier(
        &mut self,
        root: &Path,
        folder: &Path,
        tier: Tier,
        entries: &mut BTreeMap<String, Entry>,
    ) -> io::Result<()> {
        let canonical_root = fs::canonicalize(root)?;
        if !fs::canonicalize(folder)?.starts_with(&canonical_root) {
            return Err(invalid("macro folder escapes its root"));
        }
        let mut paths = fs::read_dir(folder)?
            .map(|r| r.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()?;
        paths.sort();
        for path in paths {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            if path.extension().is_some_and(|e| e == "mx") {
                self.diagnose(path, ".mx documents are not Mix macros".into());
                continue;
            }
            if path.extension().is_none_or(|e| e != "mix") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !valid_id(id) {
                self.diagnose(path, "invalid macro id".into());
                continue;
            }
            let file = match checked_file(root, &path) {
                Ok(file) => file,
                Err(err) => {
                    self.diagnose(path, err.to_string());
                    continue;
                }
            };
            // Bound reads even for maliciously long headers; never evaluate content.
            let mut title = id.to_owned();
            let mut targets = None;
            let mut malformed = false;
            for line in BufReader::new(file.take(8192)).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => break,
                };
                let line = line.trim();
                if let Some(value) = line.strip_prefix("-- title:") {
                    if !value.trim().is_empty() {
                        title = value.trim().into();
                    }
                    continue;
                }
                if let Some(value) = line.strip_prefix("-- target:") {
                    let names: Vec<_> = value.split(',').map(str::trim).collect();
                    if targets.is_some()
                        || value.trim().is_empty()
                        || (value.trim() != "any"
                            && names.iter().any(|name| !valid_app(name) || *name == "any"))
                    {
                        malformed = true;
                    } else {
                        targets = Some(if value.trim() == "any" {
                            Targets::Any
                        } else {
                            Targets::List(names.into_iter().map(str::to_owned).collect())
                        });
                    }
                }
                if !line.is_empty() && !line.starts_with("--") {
                    break;
                }
            }
            if malformed {
                self.diagnose(path, "malformed target header".into());
                continue;
            }
            let mut entry = Entry {
                id: id.into(),
                title,
                tier,
                path: path.clone(),
                shadows: Vec::new(),
                targets,
            };
            if let Some(previous) = entries.remove(id) {
                entry.shadows = previous.shadows;
                entry.shadows.push(previous.tier);
            }
            entries.insert(id.into(), entry);
        }
        Ok(())
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
}

fn valid_app(app: &str) -> bool {
    app.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        && app
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn checked_file(root: &Path, path: &Path) -> io::Result<fs::File> {
    let target = fs::canonicalize(path)?;
    if !target.starts_with(fs::canonicalize(root)?) {
        return Err(invalid("macro symlink escapes its root"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = options.open(target)?;
    // Inspect the opened descriptor, not a path that can be swapped for a FIFO.
    if !file.metadata()?.is_file() {
        return Err(invalid("macro is not a regular file"));
    }
    Ok(file)
}

/// Shared editor preflight: check the resolved regular file and probe only a
/// fresh scratch file in the directory where atomic replacement will occur.
pub fn check_editable(path: &Path) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let target = fs::canonicalize(path)?;
    let metadata = fs::metadata(&target)?;
    if !metadata.is_file() {
        return Err(invalid("macro is not a regular file"));
    }
    if metadata.permissions().readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "macro is read-only",
        ));
    }
    let parent = target
        .parent()
        .ok_or_else(|| invalid("macro has no parent"))?;
    loop {
        let probe = parent.join(format!(
            ".macro-save-check-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(file) => {
                drop(file);
                fs::remove_file(probe)?;
                return Ok(target);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
