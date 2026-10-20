//! The SSH tab. Each host is one fragment file in `~/.ssh/hosts/`, which
//! `~/.ssh/config` pulls in with `Include ~/.ssh/hosts/*`; a trashed host is
//! the same file renamed `.trashed-<id>`. Keys are the private key files in
//! `~/.ssh/keys/`. The daemon watches those paths and bumps the shared
//! revision when they change, and it runs reachability probes in the
//! background, so a waiting popup sees each result as it lands.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Semaphore, watch};

/// Probes that may run at once.
const PROBE_WORKERS: usize = 4;
/// The longest one probe may take, connect and login included.
const PROBE_LIMIT: Duration = Duration::from_secs(10);
/// A probe logs in without prompting, touches nothing it does not need, and
/// runs `true`. A host seen for the first time has its key accepted.
const PROBE_OPTIONS: &[&str] = &[
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=5",
    "-o",
    "StrictHostKeyChecking=accept-new",
    "-o",
    "ControlMaster=no",
    "-o",
    "ControlPath=none",
    "-o",
    "AddKeysToAgent=no",
    "-o",
    "ClearAllForwardings=yes",
    "-o",
    "UpdateHostKeys=no",
    "-o",
    "PermitLocalCommand=no",
    "-o",
    "RequestTTY=no",
    "-o",
    "RemoteCommand=none",
    "-o",
    "LogLevel=ERROR",
];
const TRASH_PREFIX: &str = ".trashed-";
/// Host fragments hold secrets' paths and users, so they are owner-only.
const HOST_FILE_MODE: u32 = 0o600;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Probe {
    /// `probing`, `ok` or `failed`.
    pub status: String,
    pub ms: u64,
    pub error: String,
    /// Milliseconds since the epoch when the probe finished.
    pub at: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Host {
    pub id: String,
    pub path: String,
    pub hostname: String,
    pub port: u16,
    pub user: String,
    pub identity: String,
    /// Why the fragment cannot be used. A host with an error is shown but
    /// cannot be connected to or probed.
    pub error: String,
    /// Something that may stop a login, such as a missing identity file.
    pub warning: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Key {
    pub id: String,
    pub fingerprint: String,
    pub error: String,
}

type FingerprintCache = HashMap<PathBuf, (SystemTime, u64, Result<String, String>)>;

pub struct Ssh {
    root: PathBuf,
    rev: watch::Sender<u64>,
    probes: Mutex<HashMap<String, Probe>>,
    fingerprints: Mutex<FingerprintCache>,
    workers: Arc<Semaphore>,
    watcher: Mutex<Option<RecommendedWatcher>>,
}

impl Ssh {
    /// The SSH state for `$HOME/.ssh`, sharing the daemon's revision.
    pub fn new(rev: watch::Sender<u64>) -> Arc<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Self::at(home.join(".ssh"), rev)
    }

    pub fn at(root: PathBuf, rev: watch::Sender<u64>) -> Arc<Self> {
        Arc::new(Self {
            root,
            rev,
            probes: Mutex::new(HashMap::new()),
            fingerprints: Mutex::new(HashMap::new()),
            workers: Arc::new(Semaphore::new(PROBE_WORKERS)),
            watcher: Mutex::new(None),
        })
    }

    fn hosts_dir(&self) -> PathBuf {
        self.root.join("hosts")
    }

    fn keys_dir(&self) -> PathBuf {
        self.root.join("keys")
    }

    fn bump(&self) {
        self.rev.send_modify(|r| *r += 1);
    }

    /// Watches `~/.ssh` (for `config`), `hosts/` and `keys/`, and bumps the
    /// revision when one of them changes. `known_hosts` and the mux sockets
    /// change on every login, so they are ignored. A directory created later
    /// is picked up the next time the watch is started, after an action.
    pub fn watch(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let root = self.root.clone();
        let handler = move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            if event.paths.iter().any(|p| relevant(&root, p))
                && let Some(ssh) = weak.upgrade()
            {
                ssh.bump();
            }
        };
        let mut watcher = match notify::recommended_watcher(handler) {
            Ok(watcher) => watcher,
            Err(error) => {
                eprintln!("mixtrayd: ssh watch unavailable: {error}");
                return;
            }
        };
        for dir in [self.root.clone(), self.hosts_dir(), self.keys_dir()] {
            if dir.is_dir()
                && let Err(error) = watcher.watch(&dir, RecursiveMode::NonRecursive)
            {
                eprintln!("mixtrayd: cannot watch {}: {error}", dir.display());
            }
        }
        *lock(&self.watcher) = Some(watcher);
    }

    /// The SSH tab for one snapshot. Runs `ssh-keygen` for keys it has not
    /// seen, so call it off the async runtime.
    pub fn tab(&self) -> Value {
        if !self.root.is_dir() {
            return json!({ "hosts": [], "trash": [], "keys": [], "included": false,
                           "error": format!("{} does not exist", self.root.display()) });
        }
        let included = config_includes_hosts(&self.root);
        let (mut hosts, trash, hosts_error) = self.scan_hosts();
        let (keys, keys_error) = self.scan_keys();
        let probes = lock(&self.probes);
        for host in &mut hosts {
            host.probe = probes.get(&host.id).cloned();
        }
        let probing = probes.values().filter(|p| p.status == "probing").count();
        drop(probes);
        let mut tab = json!({
            "included": included,
            "hosts_dir": self.hosts_dir(),
            "hosts": hosts,
            "trash": trash,
            "keys": keys,
            "probing": probing,
        });
        let errors: Vec<String> = [hosts_error, keys_error].into_iter().flatten().collect();
        if !included {
            tab["warning"] =
                json!("~/.ssh/config does not Include ~/.ssh/hosts/*, so ssh ignores these hosts");
        }
        if !errors.is_empty() {
            tab["error"] = json!(errors.join("; "));
        }
        tab
    }

    /// Live hosts and trashed hosts, each sorted by id.
    fn scan_hosts(&self) -> (Vec<Host>, Vec<Host>, Option<String>) {
        let dir = self.hosts_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Vec::new(), Vec::new(), None);
            }
            Err(error) => {
                return (
                    Vec::new(),
                    Vec::new(),
                    Some(format!("{}: {error}", dir.display())),
                );
            }
        };
        let (mut live, mut trash) = (Vec::new(), Vec::new());
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let (id, trashed) = match name.strip_prefix(TRASH_PREFIX) {
                Some(id) => (id.to_string(), true),
                None => (name.clone(), false),
            };
            // Other dotfiles (shared snippets) are not hosts.
            if canonical_id(&id).is_err() {
                continue;
            }
            let path = entry.path();
            let host = self.read_host(&id, &path);
            if trashed {
                trash.push(host)
            } else {
                live.push(host)
            }
        }
        live.sort_by(|a, b| a.id.cmp(&b.id));
        trash.sort_by(|a, b| a.id.cmp(&b.id));
        (live, trash, None)
    }

    fn read_host(&self, id: &str, path: &Path) -> Host {
        let mut host = Host {
            id: id.to_string(),
            path: path.to_string_lossy().into_owned(),
            hostname: String::new(),
            port: 22,
            user: String::new(),
            identity: String::new(),
            error: String::new(),
            warning: String::new(),
            probe: None,
        };
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) => {
                host.error = error.to_string();
                return host;
            }
        };
        if !meta.file_type().is_file() {
            host.error = "not a regular file".into();
            return host;
        }
        match fs::read_to_string(path) {
            Ok(text) => match parse_host(id, &text) {
                Ok(fields) => {
                    host.hostname = fields.hostname;
                    host.port = fields.port;
                    host.user = fields.user;
                    host.identity = fields.identity;
                }
                Err(error) => host.error = error,
            },
            Err(error) => host.error = error.to_string(),
        }
        if host.error.is_empty() && !host.identity.is_empty() {
            let resolved = expand_home(&host.identity, &self.root);
            if !resolved.exists() {
                host.warning = format!("IdentityFile {} does not exist", host.identity);
            }
        }
        host
    }

    /// The private keys in `keys/`, each with the fingerprint of its `.pub`.
    fn scan_keys(&self) -> (Vec<Key>, Option<String>) {
        let dir = self.keys_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Vec::new(), None);
            }
            Err(error) => return (Vec::new(), Some(format!("{}: {error}", dir.display()))),
        };
        let mut keys = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".pub") || canonical_id(&name).is_err() {
                continue;
            }
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let (fingerprint, error) = match self.fingerprint(&dir.join(format!("{name}.pub"))) {
                Ok(fp) => (fp, String::new()),
                Err(error) => (String::new(), error),
            };
            keys.push(Key {
                id: name,
                fingerprint,
                error,
            });
        }
        keys.sort_by(|a, b| a.id.cmp(&b.id));
        (keys, None)
    }

    /// `ssh-keygen -lf` of one public key, cached until the file changes.
    fn fingerprint(&self, public: &Path) -> Result<String, String> {
        let meta = fs::metadata(public).map_err(|_| "no matching .pub file".to_string())?;
        let stamp = (meta.modified().unwrap_or(UNIX_EPOCH), meta.len());
        if let Some((mtime, size, result)) = lock(&self.fingerprints).get(public)
            && (*mtime, *size) == stamp
        {
            return result.clone();
        }
        let result = match std::process::Command::new("ssh-keygen")
            .arg("-lf")
            .arg(public)
            .output()
        {
            Ok(out) if out.status.success() => {
                Ok(concise_fingerprint(&String::from_utf8_lossy(&out.stdout)))
            }
            Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
            Err(error) => Err(format!("cannot run ssh-keygen: {error}")),
        };
        lock(&self.fingerprints).insert(public.to_path_buf(), (stamp.0, stamp.1, result.clone()));
        result
    }

    /// One `mixtrayd.ssh` request: create, trash, restore, purge or probe.
    pub async fn act(self: &Arc<Self>, args: &Value) -> Result<Value, String> {
        let action = field(args, "action")?;
        let result = match action.as_str() {
            "probe" => {
                let ids: Vec<String> = args
                    .get("ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                self.probe(ids)
            }
            "create" => {
                let port = args.get("port").and_then(Value::as_u64).unwrap_or(22);
                self.create(
                    &field(args, "name")?,
                    &field(args, "hostname")?,
                    port,
                    args.get("user").and_then(Value::as_str).unwrap_or(""),
                    &field(args, "key")?,
                )
            }
            "trash" => self.relocate(&field(args, "id")?, false),
            "restore" => self.relocate(&field(args, "id")?, true),
            "purge" => self.purge(&field(args, "id")?),
            other => Err(format!(
                "unknown ssh action {other:?}; expected create, trash, restore, purge or probe"
            )),
        };
        if result.is_ok() && action != "probe" {
            // Restart the watch so a hosts/ or keys/ directory created since
            // the daemon started is watched too.
            self.watch();
            self.bump();
        }
        result
    }

    /// Starts a probe of each named live host, or of every usable live host
    /// when `ids` is empty. Hosts already being probed are skipped.
    fn probe(self: &Arc<Self>, ids: Vec<String>) -> Result<Value, String> {
        let (live, _, error) = self.scan_hosts();
        if let Some(error) = error {
            return Err(error);
        }
        let usable: Vec<&Host> = live.iter().filter(|h| h.error.is_empty()).collect();
        let targets: Vec<String> = if ids.is_empty() {
            usable.iter().map(|h| h.id.clone()).collect()
        } else {
            for id in &ids {
                if !usable.iter().any(|h| &h.id == id) {
                    return Err(format!("{id} is not a usable live SSH host"));
                }
            }
            ids
        };
        let mut started = Vec::new();
        {
            let mut probes = lock(&self.probes);
            for id in targets {
                if probes.get(&id).is_some_and(|p| p.status == "probing") {
                    continue;
                }
                probes.insert(
                    id.clone(),
                    Probe {
                        status: "probing".into(),
                        ms: 0,
                        error: String::new(),
                        at: 0,
                    },
                );
                started.push(id);
            }
        }
        if !started.is_empty() {
            self.bump();
        }
        for id in &started {
            let (ssh, id) = (Arc::clone(self), id.clone());
            tokio::spawn(async move { ssh.probe_one(id).await });
        }
        Ok(json!({ "probing": started }))
    }

    async fn probe_one(self: Arc<Self>, id: String) {
        let _permit = Arc::clone(&self.workers).acquire_owned().await;
        let started = Instant::now();
        let output = tokio::process::Command::new("ssh")
            .args(PROBE_OPTIONS)
            .args([id.as_str(), "true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output();
        let outcome = match tokio::time::timeout(PROBE_LIMIT, output).await {
            Ok(Ok(out)) if out.status.success() => Ok(()),
            Ok(Ok(out)) => Err(probe_failure(out.status.code(), &out.stderr)),
            Ok(Err(error)) => Err(format!("cannot run ssh: {error}")),
            Err(_) => Err(format!("no answer within {} s", PROBE_LIMIT.as_secs())),
        };
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let probe = match outcome {
            Ok(()) => Probe {
                status: "ok".into(),
                ms,
                error: String::new(),
                at: now_ms(),
            },
            Err(error) => Probe {
                status: "failed".into(),
                ms,
                error,
                at: now_ms(),
            },
        };
        lock(&self.probes).insert(id, probe);
        self.bump();
    }

    fn create(
        &self,
        name: &str,
        hostname: &str,
        port: u64,
        user: &str,
        key: &str,
    ) -> Result<Value, String> {
        let name = validate_identity("name", name)?;
        let hostname = validate_hostname(hostname)?;
        let port = validate_port(port)?;
        let user = validate_user(user)?;
        let key = validate_identity("key", key)?;
        if !config_includes_hosts(&self.root) {
            return Err("~/.ssh/config does not Include ~/.ssh/hosts/*".into());
        }
        if !self.keys_dir().join(&key).is_file() {
            return Err(format!("key {key:?} is not in ~/.ssh/keys"));
        }
        let dir = self.hosts_dir();
        if self.trash_path(&name).exists() {
            return Err(format!("{name} is in the trash; restore or purge it first"));
        }
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(&name);
        let text = host_fragment(&name, &hostname, port, &user, &key);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(HOST_FILE_MODE)
            .open(&path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => format!("SSH host {name} already exists"),
                _ => format!("{}: {e}", path.display()),
            })?;
        file.write_all(text.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(json!({ "created": name, "path": path }))
    }

    fn trash_path(&self, id: &str) -> PathBuf {
        self.hosts_dir().join(format!("{TRASH_PREFIX}{id}"))
    }

    /// Moves a host into the trash, or back out of it. The move is a hard
    /// link then an unlink, so it never replaces a file already there.
    fn relocate(&self, id: &str, restoring: bool) -> Result<Value, String> {
        let id = canonical_id(id)?;
        let (live, trash) = (self.hosts_dir().join(&id), self.trash_path(&id));
        let (from, to) = if restoring {
            (&trash, &live)
        } else {
            (&live, &trash)
        };
        let meta = fs::symlink_metadata(from).map_err(|_| {
            format!(
                "{id} is not {}",
                if restoring {
                    "in the trash"
                } else {
                    "a live SSH host"
                }
            )
        })?;
        if !meta.file_type().is_file() {
            return Err(format!("{} is not a regular file", from.display()));
        }
        fs::hard_link(from, to).map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => format!("{} already exists", to.display()),
            _ => format!("moving {id}: {e}"),
        })?;
        fs::remove_file(from).map_err(|e| format!("moving {id}: {e}"))?;
        lock(&self.probes).remove(&id);
        Ok(json!({ if restoring { "restored" } else { "trashed" }: id }))
    }

    /// Deletes a trashed host for good. Live hosts must be trashed first.
    fn purge(&self, id: &str) -> Result<Value, String> {
        let id = canonical_id(id)?;
        let path = self.trash_path(&id);
        let meta = fs::symlink_metadata(&path).map_err(|_| format!("{id} is not in the trash"))?;
        if !meta.file_type().is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        fs::remove_file(&path).map_err(|e| format!("purging {id}: {e}"))?;
        Ok(json!({ "purged": id }))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn field(args: &Value, name: &str) -> Result<String, String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| format!("missing string field {name:?}"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Whether a changed path matters to the tab: `config`, or anything in
/// `hosts/` or `keys/`.
fn relevant(root: &Path, path: &Path) -> bool {
    let Ok(rest) = path.strip_prefix(root) else {
        return false;
    };
    let mut parts = rest.components();
    match parts
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
    {
        Some(first) => first == "config" || first == "hosts" || first == "keys",
        None => false,
    }
}

/// Whether `~/.ssh/config` has an `Include` that names the hosts directory.
pub fn config_includes_hosts(root: &Path) -> bool {
    let Ok(text) = fs::read_to_string(root.join("config")) else {
        return false;
    };
    includes_hosts(&text)
}

pub fn includes_hosts(text: &str) -> bool {
    text.lines().any(|line| {
        let (keyword, value) = split_directive(line.trim());
        keyword.eq_ignore_ascii_case("include")
            && value
                .split_whitespace()
                .any(|v| matches!(v, "~/.ssh/hosts/*" | "hosts/*") || v.ends_with("/.ssh/hosts/*"))
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct HostFields {
    pub hostname: String,
    pub port: u16,
    pub user: String,
    pub identity: String,
}

/// Reads one host fragment. It must hold exactly one `Host` line naming only
/// `id`. As in ssh, the first value of a repeated directive wins. Other
/// directives are allowed and left alone.
pub fn parse_host(id: &str, text: &str) -> Result<HostFields, String> {
    let mut aliases = Vec::new();
    let (mut hostname, mut port, mut user, mut identity) = (None, None, None, None);
    for (number, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (keyword, value) = split_directive(line);
        let keyword = keyword.to_ascii_lowercase();
        if keyword == "host" {
            aliases.push(value.to_string());
            continue;
        }
        if keyword == "match" {
            return Err(format!(
                "line {}: Match blocks are not supported in a host file",
                number + 1
            ));
        }
        let slot = match keyword.as_str() {
            "hostname" => &mut hostname,
            "user" => &mut user,
            "identityfile" => &mut identity,
            "port" => {
                if port.is_none() {
                    let parsed = value.parse::<u16>().ok().filter(|p| *p != 0);
                    port = Some(
                        parsed
                            .ok_or_else(|| format!("line {}: Port must be 1-65535", number + 1))?,
                    );
                }
                continue;
            }
            _ => continue,
        };
        if slot.is_none() {
            *slot = Some(value.to_string());
        }
    }
    match aliases.as_slice() {
        [alias] if alias == id => {}
        [alias] => return Err(format!("Host line names {alias:?}, expected {id:?}")),
        [] => return Err("no Host line".into()),
        _ => return Err("more than one Host line".into()),
    }
    Ok(HostFields {
        hostname: hostname.unwrap_or_else(|| id.to_string()),
        port: port.unwrap_or(22),
        user: user.unwrap_or_default(),
        identity: identity.unwrap_or_default(),
    })
}

fn split_directive(line: &str) -> (&str, &str) {
    match line.find(|c: char| c.is_whitespace() || c == '=') {
        Some(index) => (
            &line[..index],
            line[index..]
                .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
                .trim(),
        ),
        None => (line, ""),
    }
}

pub fn host_fragment(name: &str, hostname: &str, port: u16, user: &str, key: &str) -> String {
    format!(
        "Host {name}\n  Hostname {hostname}\n  Port {port}\n  User {user}\n  IdentityFile ~/.ssh/keys/{key}\n"
    )
}

/// Resolves a leading `~/`. `~/.ssh/...` is taken against the SSH root, which
/// is `$HOME/.ssh` in the daemon, so the same holds for any root.
fn expand_home(path: &str, ssh_root: &Path) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/.ssh/") {
        return ssh_root.join(rest);
    }
    match path.strip_prefix("~/") {
        Some(rest) => ssh_root.parent().unwrap_or(Path::new("/")).join(rest),
        None => PathBuf::from(path),
    }
}

/// `256 SHA256:abc comment (ED25519)` becomes `SHA256:abc (ED25519)`; the
/// comment is often a user and host, which the tray need not repeat.
fn concise_fingerprint(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    match (words.get(1), words.last()) {
        (Some(hash), Some(kind)) if kind.starts_with('(') && words.len() > 2 => {
            format!("{hash} {kind}")
        }
        (Some(hash), _) => (*hash).to_string(),
        _ => line.trim().to_string(),
    }
}

fn probe_failure(code: Option<i32>, stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let last = text.lines().map(str::trim).rfind(|l| !l.is_empty());
    match (last, code) {
        (Some(line), _) => line.chars().take(300).collect(),
        (None, Some(code)) => format!("ssh exited {code}"),
        (None, None) => "ssh was killed".into(),
    }
}

/// A host or key name: 1-64 characters, alphanumeric first, then only
/// letters, digits, `.`, `_` and `-`, and no `..`.
pub fn canonical_id(id: &str) -> Result<String, String> {
    let bytes = id.as_bytes();
    let ok = !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !id.contains("..");
    if ok {
        Ok(id.to_string())
    } else {
        Err(format!(
            "invalid name {id:?}: use 1-64 of A-Z a-z 0-9 . _ -, starting with a letter or digit"
        ))
    }
}

fn validate_identity(label: &str, value: &str) -> Result<String, String> {
    canonical_id(value.trim()).map_err(|e| format!("{label}: {e}"))
}

pub fn validate_hostname(value: &str) -> Result<String, String> {
    let value = value.trim();
    let ok = !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b':' | b'-'));
    if ok {
        Ok(value.to_string())
    } else {
        Err("hostname: use only A-Z a-z 0-9 . : -, not starting with -".into())
    }
}

fn validate_port(value: u64) -> Result<u16, String> {
    match u16::try_from(value) {
        Ok(0) => Ok(22),
        Ok(port) => Ok(port),
        Err(_) => Err("port must be 1-65535".into()),
    }
}

pub fn validate_user(value: &str) -> Result<String, String> {
    let value = match value.trim() {
        "" => "root",
        v => v,
    };
    let mut bytes = value.bytes();
    let first = bytes.next().unwrap_or(b'-');
    let ok = (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if ok {
        Ok(value.to_string())
    } else {
        Err("user: start with a letter or _, then only A-Z a-z 0-9 _ . -".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mixtrayd-ssh-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("hosts")).unwrap();
        fs::create_dir_all(dir.join("keys")).unwrap();
        fs::write(dir.join("config"), "Include ~/.ssh/hosts/*\n").unwrap();
        dir
    }

    fn ssh_at(root: &Path) -> Arc<Ssh> {
        Ssh::at(root.to_path_buf(), watch::channel(0).0)
    }

    #[test]
    fn parse_host_reads_the_fields_and_defaults_the_rest() {
        let text = "# alpha\nHost alpha\n  Hostname 192.0.2.10\n  Port 2222\n  User admin\n  IdentityFile ~/.ssh/keys/k\n  User other\n  ProxyJump beta\n";
        assert_eq!(
            parse_host("alpha", text).unwrap(),
            HostFields {
                hostname: "192.0.2.10".into(),
                port: 2222,
                user: "admin".into(),
                identity: "~/.ssh/keys/k".into(),
            }
        );
        let bare = parse_host("beta", "Host beta\n").unwrap();
        assert_eq!(
            (bare.hostname.as_str(), bare.port, bare.user.as_str()),
            ("beta", 22, "")
        );
    }

    #[test]
    fn parse_host_refuses_a_wrong_or_repeated_host_line() {
        assert!(parse_host("alpha", "Host beta\n").is_err());
        assert!(parse_host("alpha", "Host alpha\nHost alpha\n").is_err());
        assert!(parse_host("alpha", "Hostname x\n").is_err());
        assert!(parse_host("alpha", "Host alpha\nPort 0\n").is_err());
        assert!(parse_host("alpha", "Host alpha\nMatch all\n").is_err());
    }

    #[test]
    fn includes_hosts_accepts_the_usual_spellings() {
        assert!(includes_hosts("Include ~/.ssh/hosts/*\n"));
        assert!(includes_hosts("include=hosts/*\n"));
        assert!(includes_hosts("Include /home/user/.ssh/hosts/* other/*\n"));
        assert!(!includes_hosts(
            "Include ~/.ssh/conf.d/*\n# Include ~/.ssh/hosts/*\n"
        ));
    }

    #[test]
    fn validators_refuse_option_injection_and_odd_characters() {
        assert!(canonical_id("-oProxyCommand").is_err());
        assert!(canonical_id("a..b").is_err());
        assert!(canonical_id("web-1.example").is_ok());
        assert!(validate_hostname("-oProxyCommand=x").is_err());
        assert!(validate_hostname("2001:db8::1").is_ok());
        assert!(validate_user("").unwrap() == "root");
        assert!(validate_user("a b").is_err());
        assert_eq!(validate_port(0).unwrap(), 22);
        assert!(validate_port(70000).is_err());
    }

    #[test]
    fn concise_fingerprint_drops_the_comment() {
        assert_eq!(
            concise_fingerprint("256 SHA256:abc user@alpha (ED25519)\n"),
            "SHA256:abc (ED25519)"
        );
        assert_eq!(concise_fingerprint("256 SHA256:abc"), "SHA256:abc");
    }

    #[test]
    fn relevant_paths_are_config_hosts_and_keys_only() {
        let root = Path::new("/home/user/.ssh");
        assert!(relevant(root, Path::new("/home/user/.ssh/config")));
        assert!(relevant(root, Path::new("/home/user/.ssh/hosts/alpha")));
        assert!(relevant(root, Path::new("/home/user/.ssh/keys/k.pub")));
        assert!(!relevant(root, Path::new("/home/user/.ssh/known_hosts")));
        assert!(!relevant(root, Path::new("/home/user/.ssh/mux/sock")));
    }

    #[tokio::test]
    async fn create_trash_restore_and_purge_round_trip() {
        let root = scratch("cycle");
        fs::write(root.join("keys/k"), "private").unwrap();
        let ssh = ssh_at(&root);

        let created = ssh
            .act(
                &json!({ "action": "create", "name": "alpha", "hostname": "192.0.2.10",
                          "port": 2222, "user": "", "key": "k" }),
            )
            .await
            .unwrap();
        assert_eq!(created["created"], "alpha");
        let path = root.join("hosts/alpha");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            host_fragment("alpha", "192.0.2.10", 2222, "root", "k")
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let again = ssh
            .act(&json!({ "action": "create", "name": "alpha", "hostname": "192.0.2.11", "key": "k" }))
            .await;
        assert!(again.unwrap_err().contains("already exists"));

        let tab = ssh.tab();
        assert_eq!(tab["hosts"][0]["id"], "alpha");
        assert_eq!(tab["hosts"][0]["port"], 2222);
        assert_eq!(tab["hosts"][0]["warning"], "");
        assert_eq!(tab["included"], true);

        ssh.act(&json!({ "action": "trash", "id": "alpha" }))
            .await
            .unwrap();
        assert!(!path.exists());
        let tab = ssh.tab();
        assert_eq!(tab["hosts"].as_array().unwrap().len(), 0);
        assert_eq!(tab["trash"][0]["id"], "alpha");
        assert!(
            ssh.act(&json!({ "action": "purge", "id": "beta" }))
                .await
                .is_err()
        );

        ssh.act(&json!({ "action": "restore", "id": "alpha" }))
            .await
            .unwrap();
        assert!(path.exists());
        assert!(
            ssh.act(&json!({ "action": "purge", "id": "alpha" }))
                .await
                .is_err(),
            "live hosts are not purged"
        );

        ssh.act(&json!({ "action": "trash", "id": "alpha" }))
            .await
            .unwrap();
        ssh.act(&json!({ "action": "purge", "id": "alpha" }))
            .await
            .unwrap();
        assert!(!root.join("hosts/.trashed-alpha").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn trash_never_replaces_an_existing_trashed_copy() {
        let root = scratch("collide");
        fs::write(root.join("hosts/alpha"), "Host alpha\n").unwrap();
        fs::write(root.join("hosts/.trashed-alpha"), "Host alpha\n  Port 2\n").unwrap();
        let ssh = ssh_at(&root);
        let error = ssh
            .act(&json!({ "action": "trash", "id": "alpha" }))
            .await
            .unwrap_err();
        assert!(error.contains("already exists"), "{error}");
        assert!(root.join("hosts/alpha").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_skips_dotfiles_and_flags_bad_fragments_and_missing_keys() {
        let root = scratch("scan");
        fs::write(root.join("hosts/.shared"), "Host *\n").unwrap();
        fs::write(root.join("hosts/beta"), "Host gamma\n").unwrap();
        fs::write(
            root.join("hosts/alpha"),
            "Host alpha\n  IdentityFile /nonexistent/key\n",
        )
        .unwrap();
        let ssh = ssh_at(&root);
        let tab = ssh.tab();
        let hosts = tab["hosts"].as_array().unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0]["id"], "alpha");
        assert!(
            hosts[0]["warning"]
                .as_str()
                .unwrap()
                .contains("does not exist")
        );
        assert!(hosts[1]["error"].as_str().unwrap().contains("gamma"));
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn create_refuses_without_the_include_or_the_key() {
        let root = scratch("refuse");
        let ssh = ssh_at(&root);
        let missing_key = ssh
            .act(&json!({ "action": "create", "name": "alpha", "hostname": "192.0.2.10", "key": "k" }))
            .await;
        assert!(missing_key.unwrap_err().contains("not in ~/.ssh/keys"));
        fs::write(root.join("keys/k"), "private").unwrap();
        fs::write(root.join("config"), "Host *\n").unwrap();
        let no_include = ssh
            .act(&json!({ "action": "create", "name": "alpha", "hostname": "192.0.2.10", "key": "k" }))
            .await;
        assert!(no_include.unwrap_err().contains("Include"));
        let _ = fs::remove_dir_all(&root);
    }
}
