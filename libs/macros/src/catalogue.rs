// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

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
                let folder = match folder {
                    Ok(folder) => folder,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        eprintln!("macro discovery: entry vanished: {error}");
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let name = folder.file_name().to_string_lossy().into_owned();
                let canonical = match fs::canonicalize(folder.path()) {
                    Ok(path) => path,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        eprintln!(
                            "macro discovery: {} vanished: {error}",
                            folder.path().display()
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if valid_destination(&name)
                    && canonical.is_dir()
                    && canonical.starts_with(fs::canonicalize(root)?)
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

    /// A bounded transport read, through the same confined descriptor checks.
    pub fn read_bounded(&self, entry: &Entry, limit: usize) -> io::Result<String> {
        let root = if entry.tier.is_user() {
            &self.user
        } else {
            &self.builtin
        };
        let mut bytes = Vec::new();
        checked_file(root, &entry.path)?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "macro exceeds text limit",
            ));
        }
        String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn user_target(&self, entry: &Entry) -> io::Result<PathBuf> {
        if !entry.tier.is_user() {
            return Err(invalid("built-in macros are read-only"));
        }
        checked_file(&self.user, &entry.path)?;
        Ok(entry.path.clone())
    }

    /// Atomic replacement through an opened root; macro symlinks are refused.
    pub fn write(&self, entry: &Entry, text: &str) -> io::Result<()> {
        self.user_target(entry)?;
        self.write_path(&entry.path, text.as_bytes())
    }

    pub fn write_path(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.write_path_outcome(path, bytes).map(|_| ())
    }

    /// Save succeeded even if the returned warning reports uncertain durability.
    pub fn write_path_outcome(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> io::Result<crate::MutationOutcome> {
        crate::storage::Target::open(&self.user, path)?.write(bytes)
    }

    pub fn read_path(&self, path: &Path) -> io::Result<Vec<u8>> {
        let root = if path.starts_with(&self.user) {
            &self.user
        } else {
            &self.builtin
        };
        crate::storage::Target::open(root, path)?.read()
    }

    pub fn remove(&self, entry: &Entry) -> io::Result<()> {
        self.user_target(entry)?;
        crate::storage::Target::open(&self.user, &entry.path)?.remove()
    }

    /// Exclusive destination allocation refuses clashes, including dangling links.
    pub fn move_to(&self, entry: &Entry, tier: Tier, app: &str) -> io::Result<PathBuf> {
        self.move_to_outcome(entry, tier, app).map(|(path, _)| path)
    }

    pub fn move_to_outcome(
        &self,
        entry: &Entry,
        tier: Tier,
        app: &str,
    ) -> io::Result<(PathBuf, crate::MutationOutcome)> {
        self.user_target(entry)?;
        if fs::symlink_metadata(&entry.path)?.file_type().is_symlink() {
            return Err(invalid("moving a macro symlink is not supported"));
        }
        if !valid_id(&entry.id) {
            return Err(invalid("invalid macro id"));
        }
        let destination = self
            .ensure_user_folder(app, tier)?
            .join(format!("{}.mix", entry.id));
        let outcome = crate::storage::Target::open(&self.user, &entry.path)?
            .move_to(&crate::storage::Target::open(&self.user, &destination)?)?;
        Ok((destination, outcome))
    }
    pub fn resolve() -> Option<Self> {
        Self::resolve_with(|key| std::env::var_os(key).map(PathBuf::from))
    }

    pub fn resolve_with(get: impl Fn(&str) -> Option<PathBuf>) -> Option<Self> {
        let absolute = |p: PathBuf| p.is_absolute().then_some(p);
        let home = get("HOME").and_then(absolute)?;
        let layout = paths::Layout::from_lookup(&get).ok()?;
        Some(Self {
            builtin: get("MIXOS_SHARE")
                .and_then(absolute)
                .unwrap_or_else(|| layout.root(paths::Root::Share))
                .join("macros"),
            user: get("XDG_CONFIG_HOME")
                .and_then(absolute)
                .unwrap_or_else(|| home.join(".config"))
                .join("mixos/macros"),
            mix: get("MIXOS_MIX")
                .and_then(absolute)
                .unwrap_or_else(|| layout.root(paths::Root::Prefix).join("bin/mix")),
            home,
        })
    }

    pub fn folder(&self, app: &str, tier: Tier) -> io::Result<PathBuf> {
        if !valid_app(app) || (tier.is_app() && !valid_destination(app)) {
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
        let file = crate::storage::Target::open(&self.user, &path)?.create()?;
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

/// Workshop's disk library keeps app ids scoped, independently of globals.
#[derive(Clone, Debug, Default)]
pub struct Library {
    pub globals: Catalogue,
    pub apps: BTreeMap<String, Catalogue>,
}

impl Library {
    pub fn scan(roots: &Roots) -> io::Result<Self> {
        let mut apps = BTreeMap::new();
        for app in roots.apps()? {
            apps.insert(app.clone(), Catalogue::scan(roots, &app)?);
        }
        Ok(Self {
            globals: Catalogue::scan_globals(roots)?,
            apps,
        })
    }
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
            let mut titled = false;
            let mut targets = None;
            let mut malformed = false;
            for line in BufReader::new(file.take(8192)).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => break,
                };
                let line = line.trim();
                if let Some(value) = line.strip_prefix("-- title:") {
                    if !titled && !value.trim().is_empty() {
                        title = value.trim().into();
                    }
                    titled = true;
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

/// App catalogue keys exclude the Library's global and Workshop namespaces.
pub fn valid_destination(app: &str) -> bool {
    valid_app(app) && !matches!(app, "global" | "workshop" | "any" | "docs" | "v")
}

fn checked_file(root: &Path, path: &Path) -> io::Result<fs::File> {
    crate::storage::Target::open(root, path)?.file()
}

/// Editor preflight without following links or touching an existing file.
pub fn check_editable(path: &Path) -> io::Result<PathBuf> {
    crate::storage::Target::open(Path::new("/"), path)?.check()?;
    Ok(path.to_path_buf())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
