// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Exactly one profile writer. Accepted state and receipts share one synced
//! replacement. Invalid candidates never occupy the accepted path.
use crate::format4::Record;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use settings::registry::{self, Values};
use settings::*;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The sealed profile's format.
///
/// History: formats 1 to 4 held the typed [`Desktop`] record and are
/// described in [`crate::format4`]. 5 holds key space v2's sparse
/// [`Values`], registry-validated and never at their defaults; a record of
/// an older format is verified against its own seal, then migrated on open
/// ([`crate::migration`]), with the old record kept beside it.
pub const FORMAT: u32 = 5;

pub(crate) const PRIMARY: &str = "desktop.conf.mix";
pub(crate) const BACKUP: &str = "desktop.previous.conf.mix";
pub(crate) const LOCK: &str = "writer.lock";

const MAX_STORE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accepted {
    pub schema: u32,
    /// The sealed record's format ([`FORMAT`]).
    pub format: u32,
    pub binding: Binding,
    pub incarnation: String,
    pub revision: Revision,
    pub design_revision: Revision,
    /// The authored values, in [`canonical`] form.
    pub values: Values,
    pub embedded_source: String,
    pub effective_digest: String,
    pub receipts: Vec<Receipt>,
    pub content_digest: String,
}
impl Accepted {
    /// A first record: revision 1, no receipts, the shipped default source.
    pub fn initial(binding: Binding, values: Values) -> anyhow::Result<Self> {
        let mut accepted = Self {
            schema: SCHEMA,
            format: FORMAT,
            binding,
            incarnation: uuid::Uuid::now_v7().to_string(),
            revision: Revision(1),
            design_revision: Revision(1),
            values: canonical(values),
            embedded_source: settings::EMBEDDED_DEFAULT_SOURCE.into(),
            effective_digest: String::new(),
            receipts: Vec::new(),
            content_digest: String::new(),
        };
        let (effective, _) = accepted.resolve()?;
        accepted.effective_digest = settings::digest(&effective)?;
        accepted.seal()?;
        Ok(accepted)
    }
    fn digest(&self) -> anyhow::Result<String> {
        Ok(settings::digest(&(
            (
                &self.schema,
                &self.binding,
                &self.incarnation,
                &self.revision,
                &self.design_revision,
                &self.values,
                &self.embedded_source,
                &self.effective_digest,
                &self.receipts,
            ),
            self.format,
        ))?)
    }
    pub fn seal(&mut self) -> anyhow::Result<()> {
        self.content_digest = self.digest()?;
        Ok(())
    }
    /// The values resolved with the pinned source, unchecked against the seal.
    pub fn resolve(
        &self,
    ) -> anyhow::Result<(std::collections::BTreeMap<String, Effective>, Accents)> {
        resolve::values::resolve(&self.values, &self.embedded_source)
            .map_err(|e| anyhow::anyhow!("accepted interpretation unsupported: {e:?}"))
    }
    pub fn effective(&self) -> anyhow::Result<std::collections::BTreeMap<String, Effective>> {
        self.interpretation().map(|(effective, _)| effective)
    }
    /// The sealed effective values and, beside them, the in-memory accent per
    /// context. The accents are not sealed: they are re-derived here from the
    /// same compiled design the seal checks.
    pub fn interpretation(
        &self,
    ) -> anyhow::Result<(std::collections::BTreeMap<String, Effective>, Accents)> {
        let (effective, accents) = self.resolve()?;
        anyhow::ensure!(
            self.effective_digest == settings::digest(&effective)?,
            "accepted interpretation changed; explicit migration required"
        );
        Ok((effective, accents))
    }
    pub fn check(&self, binding: &Binding) -> anyhow::Result<()> {
        anyhow::ensure!(self.schema == SCHEMA, "unsupported accepted schema");
        anyhow::ensure!(
            self.format == FORMAT,
            "accepted format {} is not {FORMAT}",
            self.format
        );
        anyhow::ensure!(&self.binding == binding, "wrong stored authority binding");
        anyhow::ensure!(
            self.embedded_source.len() <= MAX_SOURCE_BYTES,
            "pinned source limit exceeded"
        );
        anyhow::ensure!(
            self.content_digest == self.digest()?,
            "accepted content digest mismatch"
        );
        check_lineage(
            binding,
            &self.incarnation,
            self.revision,
            self.design_revision,
            &self.receipts,
        )
    }
}

/// The identity rules every sealed record keeps: binding, incarnation,
/// revisions and receipt history.
pub(crate) fn check_lineage(
    binding: &Binding,
    incarnation: &str,
    revision: Revision,
    design_revision: Revision,
    receipts: &[Receipt],
) -> anyhow::Result<()> {
    binding.validate().map_err(|e| anyhow::anyhow!(e.message))?;
    anyhow::ensure!(
        uuid::Uuid::parse_str(incarnation).is_ok(),
        "invalid store incarnation"
    );
    anyhow::ensure!(
        revision.0 > 0 && design_revision.0 > 0 && design_revision <= revision,
        "invalid accepted revision"
    );
    anyhow::ensure!(receipts.len() <= MAX_RECEIPTS, "receipt limit exceeded");
    let mut ids = std::collections::BTreeSet::new();
    for receipt in receipts {
        anyhow::ensure!(
            receipt.incarnation == incarnation
                && receipt.revision <= revision
                && receipt.revision.0 > 0,
            "receipt lineage mismatch"
        );
        anyhow::ensure!(
            valid_operation_id(&receipt.operation_id) && ids.insert(&receipt.operation_id),
            "invalid or duplicate receipt identity"
        );
        anyhow::ensure!(
            receipt.request_digest.len() == 64
                && receipt
                    .request_digest
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit()),
            "invalid receipt digest"
        );
    }
    Ok(())
}
pub fn valid_operation_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Values in the one form the seal covers. Strict data has a single number
/// type, so a stored `40` reads back as `40.0`: every value is put back in
/// the registry's own normal form (whole numbers for `int` keys, floats for
/// `number` keys), and the app-domain leaves, which the registry keeps as
/// given, have their whole numbers made integers. Applied to every record
/// before it is sealed and after it is read, so the digest survives the
/// round trip. A value the registry refuses is kept as it is, for
/// validation to refuse.
pub fn canonical(values: Values) -> Values {
    values
        .into_iter()
        .map(|(path, value)| {
            if !has_number(&value) {
                return (path, value);
            }
            let domain = registry::lookup(&path)
                .is_ok_and(|m| matches!(m.def.kind, registry::Kind::AppDomain));
            let value = if domain {
                integral(value)
            } else {
                registry::check_value(&path, &value).unwrap_or(value)
            };
            (path, value)
        })
        .collect()
}
fn has_number(value: &Value) -> bool {
    match value {
        Value::Number(_) => true,
        Value::Array(items) => items.iter().any(has_number),
        Value::Object(map) => map.values().any(has_number),
        _ => false,
    }
}
/// `value` with every whole float, at any depth, an integer.
pub fn integral(value: Value) -> Value {
    match value {
        Value::Number(n) if n.is_f64() => match n.as_f64() {
            Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15 => {
                if f < 0.0 {
                    Value::from(f as i64)
                } else {
                    Value::from(f as u64)
                }
            }
            _ => Value::Number(n),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(integral).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, integral(v))).collect())
        }
        other => other,
    }
}

/// A stored record's identity, whatever its format.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Identity {
    pub format: u32,
    pub binding: Binding,
    pub incarnation: String,
    pub revision: Revision,
}

/// A sealed record as stored: format 5, or an older one still to migrate.
#[allow(clippy::large_enum_variant, reason = "one record, read once per open")]
pub enum Stored {
    Current(Accepted),
    Format4(Box<Record>),
}
impl Stored {
    /// The record in `text`, by the format its header names. Nothing is
    /// verified.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct Header {
            #[serde(default = "one")]
            format: u32,
        }
        fn one() -> u32 {
            1
        }
        let header: Header = strict::from_str(text)?;
        anyhow::ensure!(
            header.format <= FORMAT,
            "accepted format {} is newer than {FORMAT}",
            header.format
        );
        Ok(if header.format == FORMAT {
            Self::Current(strict::from_str::<Accepted>(text)?.canonical())
        } else {
            Self::Format4(Box::new(strict::from_str(text)?))
        })
    }
    pub fn check(&self, binding: &Binding) -> anyhow::Result<()> {
        match self {
            Self::Current(record) => record.check(binding),
            Self::Format4(record) => record.check(binding),
        }
    }
    pub fn format(&self) -> u32 {
        match self {
            Self::Current(record) => record.format,
            Self::Format4(record) => record.format,
        }
    }
    /// The record's identity as stored, in its own format.
    pub fn identity(&self) -> Identity {
        let (binding, incarnation, revision) = match self {
            Self::Current(r) => (&r.binding, &r.incarnation, r.revision),
            Self::Format4(r) => (&r.binding, &r.incarnation, r.revision),
        };
        Identity {
            format: self.format(),
            binding: binding.clone(),
            incarnation: incarnation.clone(),
            revision,
        }
    }
    /// The verified record as this build serves it: a format-4 record is
    /// upgraded and migrated in memory. `text` is the record as stored,
    /// whose digest the migration receipt carries.
    pub fn servable(self, text: &str) -> anyhow::Result<Accepted> {
        let accepted = match self {
            Self::Current(record) => record,
            Self::Format4(record) => crate::migration::plan(*record, text)?.accepted,
        };
        accepted.effective()?;
        Ok(accepted)
    }
    fn rebound(self, to: &Binding) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::Current(mut record) => {
                record.binding = to.clone();
                record.seal()?;
                record.check(to)?;
                encode(&record)
            }
            Self::Format4(mut record) => {
                record.binding = to.clone();
                record.seal()?;
                record.check(to)?;
                encode(&record)
            }
        }
    }
}
impl Accepted {
    fn canonical(mut self) -> Self {
        self.values = canonical(std::mem::take(&mut self.values));
        self
    }
}

pub struct Store {
    root: PathBuf,
    directory: File,
    _lock: File,
    pub recovering: bool,
    pub restored: bool,
    /// This open migrated an older record to format 5.
    pub migrated: bool,
}
/// The profile directory, opened without following a link, which must be
/// owned by this user and not writable by others.
pub(crate) fn open_directory(root: &Path) -> anyhow::Result<File> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let metadata = directory.metadata()?;
    anyhow::ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o022 == 0,
        "profile root must be an owned directory not writable by others"
    );
    Ok(directory)
}
/// Create `root` and its missing ancestors, owner-only, syncing each new
/// entry in its parent, outermost first.
pub(crate) fn create_directory(root: &Path) -> anyhow::Result<()> {
    let mut missing = Vec::new();
    let mut cursor = root;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor
            .parent()
            .ok_or_else(|| anyhow::anyhow!("profile root has no existing ancestor"))?;
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)?;
    for created in missing.iter().rev() {
        File::open(
            created
                .parent()
                .ok_or_else(|| anyhow::anyhow!("missing parent"))?,
        )?
        .sync_all()?;
    }
    Ok(())
}
impl Store {
    pub(crate) fn lock(root: &Path, create: bool) -> anyhow::Result<Self> {
        if create {
            create_directory(root)?;
        }
        let directory = open_directory(root)?;
        let lock = open_in(&directory, LOCK, libc::O_RDWR | libc::O_CREAT)?;
        let lock_meta = lock.metadata()?;
        anyhow::ensure!(
            lock_meta.is_file() && lock_meta.uid() == unsafe { libc::geteuid() },
            "writer lock must be an owned regular file"
        );
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            anyhow::bail!(
                "profile already has an active writer: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self {
            root: root.into(),
            directory,
            _lock: lock,
            recovering: false,
            restored: false,
            migrated: false,
        })
    }
    pub fn create(
        root: &Path,
        binding: Binding,
        values: Values,
    ) -> anyhow::Result<(Self, Accepted)> {
        let store = Self::lock(root, true)?;
        Self::initialise(store, binding, values)
    }
    /// Session provisioning is idempotent only for a valid existing primary.
    /// A retained lock inode is evidence of an established or interrupted
    /// profile: never recreate its missing primary, even without a backup.
    pub fn seed(
        root: &Path,
        binding: Binding,
        allow_create: bool,
    ) -> anyhow::Result<(Self, Accepted)> {
        // Automatic startup cannot distinguish first install from a whole lost
        // directory/mount. Only an explicit installer operation permits creation.
        if !allow_create {
            return Self::open(root, &binding);
        }
        if root.try_exists()? {
            // An empty directory pre-provisioned for the account is allowed.
            // Once writer.lock exists, only explicit init can recover an
            // interrupted first creation; normal seed cannot erase history.
            if std::fs::symlink_metadata(root.join(LOCK)).is_ok() {
                return Self::open(root, &binding);
            }
        }
        Self::create(root, binding, Values::new())
    }
    fn initialise(
        store: Self,
        binding: Binding,
        values: Values,
    ) -> anyhow::Result<(Self, Accepted)> {
        anyhow::ensure!(
            !store.path().exists() && !store.backup().exists(),
            "profile already initialised; initialise never overwrites accepted/backup data"
        );
        // Retire is final: a fresh profile never starts beside retired history.
        let retired = std::fs::read_dir(&store.root)?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .find(|name| name.contains(".retired-"));
        if let Some(name) = retired {
            anyhow::bail!("profile root holds retired history ({name}); retire is final");
        }
        let accepted = Accepted::initial(binding, values)
            .map_err(|e| anyhow::anyhow!("invalid initial desktop: {e}"))?;
        accepted.check(&accepted.binding)?;
        crate::authority::snapshot(&accepted, accepted.effective()?)?;
        store.replace(PRIMARY, &encode(&accepted)?)?;
        Ok((store, accepted))
    }
    pub fn open(root: &Path, binding: &Binding) -> anyhow::Result<(Self, Accepted)> {
        let mut store = Self::lock(root, false)?;
        store.check_root()?;
        // Loss/unmount of an established primary must be visible even when an
        // old backup exists. Corrupt present documents have the recovery path.
        let primary = open_in(&store.directory, PRIMARY, libc::O_RDONLY)?;
        let bytes = read_bytes(primary)?;
        let text = std::str::from_utf8(&bytes);
        #[derive(Deserialize)]
        struct Header {
            schema: u32,
            binding: Binding,
            #[serde(default)]
            format: Option<u32>,
        }
        if let Ok(text) = text
            && let Ok(header) = strict::from_str::<Header>(text)
        {
            if header.schema != SCHEMA || &header.binding != binding {
                anyhow::bail!("unsupported schema or wrong stored binding");
            }
            // A newer record is a binary to upgrade, never a backup to restore.
            if let Some(format) = header.format
                && format > FORMAT
            {
                anyhow::bail!("accepted format {format} is newer than {FORMAT}");
            }
        }
        // Header and full record use the exact same bounded bytes. A second
        // read cannot miss a version fence after transient I/O.
        match text
            .map_err(anyhow::Error::from)
            .and_then(Stored::parse)
            .and_then(|data| {
                data.check(binding)?;
                Ok(data)
            }) {
            Ok(Stored::Current(data)) => {
                // An intact accepted document may require a newer compiler or
                // migration. Semantic refusal must preserve it, not roll back.
                data.effective()?;
                Ok((store, data))
            }
            Ok(Stored::Format4(record)) => {
                let text = text.expect("parsed from text");
                let data = crate::migration::on_open(&store, binding, *record, text)?;
                store.migrated = true;
                Ok((store, data))
            }
            Err(primary_error) => {
                if primary_error.downcast_ref::<std::io::Error>().is_some()
                    || primary_error
                        .downcast_ref::<strict::Error>()
                        .is_some_and(|e| {
                            e.kind() == strict::ErrorKind::Deserialize
                                || e.kind() == strict::ErrorKind::Io
                        })
                {
                    // Shape/version changes and I/O faults require diagnosis;
                    // only malformed syntax/integrity enters automatic restore.
                    return Err(primary_error);
                }
                let backup = open_in(&store.directory, BACKUP, libc::O_RDONLY)
                    .and_then(read_bytes)
                    .and_then(|bytes| Ok(String::from_utf8(bytes)?))
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "accepted store unavailable ({primary_error}); backup unavailable ({e})"
                        )
                    })?;
                let previous = Stored::parse(&backup).map_err(|e| {
                    anyhow::anyhow!(
                        "accepted store unavailable ({primary_error}); backup unavailable ({e})"
                    )
                })?;
                previous.check(binding)?;
                // An older backup is migrated in memory; its record is not
                // kept, since the primary it would precede is the corrupt one.
                let mut previous = previous.servable(&backup)?;
                // Preserve user/corrupt evidence before replacing anything.
                store.check_root()?;
                let corrupt = format!("corrupt-{}.conf.mix", uuid::Uuid::now_v7());
                // Preserve the complete corrupt inode before replacement. A
                // hard link leaves the primary present across every crash cut;
                // rename-away would create an unrecoverable missing-store gap.
                store.link(PRIMARY, &corrupt)?;
                previous.incarnation = uuid::Uuid::now_v7().to_string();
                previous.receipts.clear();
                previous.seal()?;
                store.replace(PRIMARY, &encode(&previous)?)?;
                store.restored = true;
                Ok((store, previous))
            }
        }
    }
    /// Move an established profile to a renamed instance (the machine was
    /// renamed, and the instance is its hostname). The primary, and the backup
    /// when present, must each verify as stored under `from`; each is then
    /// re-sealed under `to`, backup first, through the atomic replacement
    /// path. Nothing else changes: format, incarnation, revisions and
    /// receipts stay, so a format-4 profile is rebound as format 4 and
    /// migrates on its next open. The writer lock keeps a serving settingsd
    /// out. A document already under `to` is left as it is, so a run
    /// interrupted between the two writes (backup under `to`, primary under
    /// `from`) is finished by running it again. The primary must also be one
    /// this build serves. Returns the stored primary's identity, as written.
    pub fn rebind(root: &Path, from: &Binding, to: Binding) -> anyhow::Result<(Identity, bool)> {
        to.validate().map_err(|e| anyhow::anyhow!(e.message))?;
        anyhow::ensure!(from != &to, "rebind needs a different binding");
        let store = Self::lock(root, false)?;
        store.check_root()?;
        // Some(record) when it needs re-sealing; None when already under `to`.
        let pending = |name: &str, text: &str| -> anyhow::Result<Option<Stored>> {
            let record = Stored::parse(text).map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
            if record.check(&to).is_ok() {
                return Ok(None);
            }
            record
                .check(from)
                .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
            Ok(Some(record))
        };
        let primary = store
            .read_text(PRIMARY)
            .map_err(|e| anyhow::anyhow!("{PRIMARY}: {e}"))?;
        // Refuse what serve would refuse, before anything is written.
        Stored::parse(&primary)
            .and_then(|record| record.servable(&primary))
            .map_err(|e| anyhow::anyhow!("{PRIMARY}: {e}"))?;
        let primary_pending = pending(PRIMARY, &primary)?;
        let backup_pending = match store.read_text(BACKUP) {
            Ok(text) => pending(BACKUP, &text)?,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                None
            }
            Err(e) => return Err(anyhow::anyhow!("{BACKUP}: {e}")),
        };
        let changed = primary_pending.is_some() || backup_pending.is_some();
        if let Some(backup) = backup_pending {
            store.replace(BACKUP, &backup.rebound(&to)?)?;
        }
        if let Some(record) = primary_pending {
            store.replace(PRIMARY, &record.rebound(&to)?)?;
        }
        store.check_root()?;
        let mut identity = Stored::parse(&primary)?.identity();
        identity.binding = to;
        Ok((identity, changed))
    }
    /// Set aside a profile this node no longer serves (design §8 M1): every
    /// profile document becomes `<name>.retired-<instance>`, the primary
    /// last, and the writer lock stays, so nothing recreates a profile over
    /// the retired history. The primary must verify under `binding`, in
    /// format 4 or 5 (it need not resolve in this build). A run interrupted
    /// part way is finished by running it again; a retired name already in
    /// use is refused, never overwritten.
    pub fn retire(root: &Path, binding: &Binding) -> anyhow::Result<Vec<String>> {
        binding.validate().map_err(|e| anyhow::anyhow!(e.message))?;
        let store = Self::lock(root, false)?;
        store.check_root()?;
        let suffix = format!(".retired-{}", binding.instance);
        let retired = |name: &str| format!("{name}{suffix}");
        let resumed = store.exists(&retired(PRIMARY))?;
        match store.read_text(PRIMARY) {
            Ok(text) => Stored::parse(&text)
                .and_then(|record| record.check(binding))
                .map_err(|e| anyhow::anyhow!("{PRIMARY}: {e}"))?,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
                    && resumed => {}
            Err(e) => return Err(anyhow::anyhow!("{PRIMARY}: {e}")),
        }
        let mut names: Vec<String> = std::fs::read_dir(&store.root)?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| {
                name != LOCK
                    && name != PRIMARY
                    && name != crate::mirror::CACHE
                    && !name.contains(".retired-")
                    && (name.ends_with(".conf.mix") || name.ends_with(".mx"))
            })
            .collect();
        names.sort();
        if store.exists(PRIMARY)? {
            names.push(PRIMARY.into());
        }
        for name in &names {
            let to = retired(name);
            anyhow::ensure!(
                !store.exists(&to)?,
                "{to} already exists; retire never overwrites"
            );
            store.check_root()?;
            std::fs::rename(store.root.join(name), store.root.join(&to))?;
            store.directory.sync_all()?;
        }
        Ok(names.into_iter().map(|name| retired(&name)).collect())
    }
    /// Migrate the backup in place when it is an intact older record (its
    /// seal verified as stored); leave a current, absent or unreadable one
    /// for the next commit to replace.
    pub(crate) fn migrate_backup(&self, binding: &Binding) -> anyhow::Result<()> {
        let Ok(text) = self.read_text(BACKUP) else {
            return Ok(());
        };
        let Ok(previous @ Stored::Format4(_)) = Stored::parse(&text) else {
            return Ok(());
        };
        if previous.check(binding).is_err() {
            return Ok(());
        }
        let previous = previous.servable(&text)?;
        self.replace(BACKUP, &encode(&previous)?)
    }
    fn path(&self) -> PathBuf {
        self.root.join(PRIMARY)
    }
    fn backup(&self) -> PathBuf {
        self.root.join(BACKUP)
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn exists(&self, name: &str) -> anyhow::Result<bool> {
        match open_in(&self.directory, name, libc::O_RDONLY) {
            Ok(_) => Ok(true),
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }
    pub(crate) fn read(&self, name: &str) -> anyhow::Result<Vec<u8>> {
        read_bytes(open_in(&self.directory, name, libc::O_RDONLY)?)
    }
    pub(crate) fn read_text(&self, name: &str) -> anyhow::Result<String> {
        Ok(String::from_utf8(self.read(name)?)?)
    }
    /// Replace `name` with `bytes` through the synced atomic path.
    pub(crate) fn replace(&self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        self.check_root()?;
        config::atomic::replace_in(&self.directory, name.as_ref(), bytes)?;
        self.check_root()
    }
    /// A hard link of `from` as `to`, synced: the inode stays present
    /// across every crash cut.
    pub(crate) fn link(&self, from: &str, to: &str) -> anyhow::Result<()> {
        let (from, to) = (std::ffi::CString::new(from)?, std::ffi::CString::new(to)?);
        if unsafe {
            libc::linkat(
                self.directory.as_raw_fd(),
                from.as_ptr(),
                self.directory.as_raw_fd(),
                to.as_ptr(),
                0,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        self.directory.sync_all()?;
        Ok(())
    }
    /// Write `bytes` as the kept record `name`, which is never rewritten: a
    /// run that finds the same bytes there does nothing, and one that finds
    /// other bytes (an earlier migration or downgrade) first keeps those as
    /// `<stem>.superseded-<uuid><extension>`.
    pub(crate) fn keep(&self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        match self.read(name) {
            Ok(existing) if existing == bytes => return Ok(()),
            Ok(_) => {
                let (stem, extension) = name
                    .split_once('.')
                    .map_or((name, String::new()), |(s, e)| (s, format!(".{e}")));
                let superseded = format!(
                    "{stem}.superseded-{}{extension}",
                    uuid::Uuid::now_v7().simple()
                );
                self.link(name, &superseded)?;
            }
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
            Err(e) => return Err(e),
        }
        self.replace(name, bytes)
    }
    fn check_root(&self) -> anyhow::Result<()> {
        let path = std::fs::symlink_metadata(&self.root)?;
        let held = self.directory.metadata()?;
        anyhow::ensure!(
            path.is_dir() && path.dev() == held.dev() && path.ino() == held.ino(),
            "profile directory replaced or detached; restart required"
        );
        let lock = open_in(&self.directory, LOCK, libc::O_RDONLY)?.metadata()?;
        let held_lock = self._lock.metadata()?;
        anyhow::ensure!(
            lock.dev() == held_lock.dev() && lock.ino() == held_lock.ino(),
            "profile writer lock replaced; restart required"
        );
        Ok(())
    }
    pub fn commit(&mut self, current: &Accepted, next: &Accepted) -> anyhow::Result<()> {
        self.commit_using(current, next, |dir, name, bytes| {
            config::atomic::replace_in(dir, name.as_ref(), bytes)
        })
    }
    fn commit_using(
        &mut self,
        current: &Accepted,
        next: &Accepted,
        mut replace: impl FnMut(&File, &str, &[u8]) -> Result<(), config::atomic::ReplaceError>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.recovering,
            "store outcome unknown; restart/readback recovery required"
        );
        if let Err(error) = self.check_root() {
            self.recovering = true;
            return Err(error);
        }
        next.check(&current.binding)?;
        // Last-good copy is made durable before replacing the accepted document.
        replace(&self.directory, BACKUP, &encode(current)?)?;
        if let Err(error) = replace(&self.directory, PRIMARY, &encode(next)?) {
            self.recovering = error.may_have_replaced;
            return Err(error.into());
        }
        if let Err(error) = self.check_root() {
            self.recovering = true;
            return Err(error);
        }
        Ok(())
    }
}
pub(crate) fn encode<T: Serialize>(data: &T) -> anyhow::Result<Vec<u8>> {
    let bytes = strict::to_string_pretty(data)?.into_bytes();
    anyhow::ensure!(
        bytes.len() <= MAX_STORE_BYTES as usize,
        "accepted document limit exceeded"
    );
    Ok(bytes)
}
pub(crate) fn open_in(directory: &File, name: &str, flags: libc::c_int) -> anyhow::Result<File> {
    anyhow::ensure!(
        !name.contains('/') && name != "." && name != "..",
        "single file name required"
    );
    let name = std::ffi::CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub(crate) fn read_bytes(file: File) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "accepted document is not a regular file"
    );
    let mut bytes = Vec::new();
    file.take(MAX_STORE_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_STORE_BYTES as usize,
        "accepted document limit exceeded"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn binding() -> Binding {
        Binding {
            instance: "test".into(),
            profile: "default".into(),
        }
    }
    /// `current` at the next revision with a spacious density, resealed.
    fn spacious(current: &Accepted) -> Accepted {
        let mut next = current.clone();
        next.revision = Revision(2);
        next.values
            .insert("appearance.density".into(), json!("spacious"));
        next.effective_digest = settings::digest(&next.resolve().unwrap().0).unwrap();
        next.seal().unwrap();
        next
    }
    #[test]
    fn ambiguous_primary_sync_fences_mutations_and_reload_resolves_accepted_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, current) = Store::create(dir.path(), binding(), Values::new()).unwrap();
        let mut next = spacious(&current);
        next.receipts.push(Receipt {
            operation_id: "ambiguous".into(),
            request_digest: "a".repeat(64),
            incarnation: next.incarnation.clone(),
            revision: next.revision,
            outcome: Outcome::Changed,
        });
        next.seal().unwrap();
        let result = store.commit_using(&current, &next, |dir, name, bytes| {
            config::atomic::replace_in(dir, name.as_ref(), bytes)?;
            if name == PRIMARY {
                Err(config::atomic::ReplaceError {
                    stage: config::atomic::Stage::DirectorySync,
                    may_have_replaced: true,
                    source: std::io::Error::other("injected ambiguous sync outcome"),
                })
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert!(store.recovering);
        assert!(store.commit(&current, &next).is_err());
        drop(store);
        let (_, accepted) = Store::open(dir.path(), &binding()).unwrap();
        assert_eq!(accepted.revision, Revision(2));
        assert_eq!(accepted.receipts[0].operation_id, "ambiguous");
    }
    #[test]
    fn rebind_reseals_primary_and_backup_under_the_new_instance() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, current) = Store::create(dir.path(), binding(), Values::new()).unwrap();
        let next = spacious(&current);
        store.commit(&current, &next).unwrap();
        // A serving writer holds the lock: rebind is refused at once.
        let renamed = Binding {
            instance: "renamed".into(),
            profile: "default".into(),
        };
        assert!(Store::rebind(dir.path(), &binding(), renamed.clone()).is_err());
        drop(store);

        let (accepted, changed) = Store::rebind(dir.path(), &binding(), renamed.clone()).unwrap();
        assert!(changed);
        assert_eq!(accepted.binding, renamed);
        assert_eq!(accepted.revision, Revision(2));
        assert_eq!(accepted.incarnation, next.incarnation);
        let backup: Accepted =
            strict::from_str(&std::fs::read_to_string(dir.path().join(BACKUP)).unwrap()).unwrap();
        backup.check(&renamed).unwrap();
        assert!(Store::open(dir.path(), &binding()).is_err());
        let (_, opened) = Store::open(dir.path(), &renamed).unwrap();
        assert_eq!(opened.values["appearance.density"], "spacious");

        // Again is a no-op; a wrong `from` is refused.
        let (_, changed) = Store::rebind(dir.path(), &binding(), renamed.clone()).unwrap();
        assert!(!changed);
        let other = Binding {
            instance: "other".into(),
            profile: "default".into(),
        };
        assert!(Store::rebind(dir.path(), &other, binding()).is_err());
    }
    #[test]
    fn rebind_finishes_a_run_interrupted_after_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, current) = Store::create(dir.path(), binding(), Values::new()).unwrap();
        let mut next = current.clone();
        next.revision = Revision(2);
        next.seal().unwrap();
        store.commit(&current, &next).unwrap();
        drop(store);
        // The state a crash between the two writes leaves: backup under the
        // new binding, primary still under the old one.
        let renamed = Binding {
            instance: "renamed".into(),
            profile: "default".into(),
        };
        let mut backup = current.clone();
        backup.binding = renamed.clone();
        backup.seal().unwrap();
        std::fs::write(dir.path().join(BACKUP), encode(&backup).unwrap()).unwrap();
        assert!(Store::open(dir.path(), &renamed).is_err());

        let (accepted, changed) = Store::rebind(dir.path(), &binding(), renamed.clone()).unwrap();
        assert!(changed);
        assert_eq!(accepted.revision, Revision(2));
        let kept: Accepted =
            strict::from_str(&std::fs::read_to_string(dir.path().join(BACKUP)).unwrap()).unwrap();
        kept.check(&renamed).unwrap();
        assert_eq!(kept.revision, Revision(1));
        let (_, opened) = Store::open(dir.path(), &renamed).unwrap();
        assert_eq!(opened.revision, Revision(2));
    }
    #[test]
    fn rebind_without_a_backup_and_with_a_bad_one() {
        let renamed = Binding {
            instance: "renamed".into(),
            profile: "default".into(),
        };
        // A fresh profile has no backup yet: the primary alone moves.
        let dir = tempfile::tempdir().unwrap();
        drop(Store::create(dir.path(), binding(), Values::new()).unwrap());
        assert!(!dir.path().join(BACKUP).exists());
        let (accepted, changed) = Store::rebind(dir.path(), &binding(), renamed.clone()).unwrap();
        assert!(changed);
        assert_eq!(accepted.binding, renamed);
        assert!(Store::open(dir.path(), &renamed).is_ok());

        // A damaged backup stops the rebind before anything is written, and
        // the error names the file.
        let dir = tempfile::tempdir().unwrap();
        drop(Store::create(dir.path(), binding(), Values::new()).unwrap());
        let before = std::fs::read(dir.path().join(PRIMARY)).unwrap();
        std::fs::write(dir.path().join(BACKUP), "{ broken").unwrap();
        let error = Store::rebind(dir.path(), &binding(), renamed.clone()).unwrap_err();
        assert!(error.to_string().contains(BACKUP), "{error}");
        assert_eq!(std::fs::read(dir.path().join(PRIMARY)).unwrap(), before);
    }
    #[test]
    fn rebind_moves_a_backup_left_under_the_old_binding() {
        // Primary already under the new binding, backup still under the old
        // one: the backup is re-sealed rather than reported unchanged.
        let dir = tempfile::tempdir().unwrap();
        let (mut store, current) = Store::create(dir.path(), binding(), Values::new()).unwrap();
        let mut next = current.clone();
        next.revision = Revision(2);
        next.seal().unwrap();
        store.commit(&current, &next).unwrap();
        drop(store);
        let renamed = Binding {
            instance: "renamed".into(),
            profile: "default".into(),
        };
        let mut primary = next.clone();
        primary.binding = renamed.clone();
        primary.seal().unwrap();
        std::fs::write(dir.path().join(PRIMARY), encode(&primary).unwrap()).unwrap();

        let (_, changed) = Store::rebind(dir.path(), &binding(), renamed.clone()).unwrap();
        assert!(changed);
        let backup: Accepted =
            strict::from_str(&std::fs::read_to_string(dir.path().join(BACKUP)).unwrap()).unwrap();
        backup.check(&renamed).unwrap();
        let (_, changed) = Store::rebind(dir.path(), &binding(), renamed).unwrap();
        assert!(!changed);
    }
    #[test]
    fn writer_lock_and_missing_store_do_not_materialise_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Store::open(dir.path(), &binding()).is_err());
        assert!(!dir.path().join(PRIMARY).exists());
        let (first, _) = Store::create(dir.path(), binding(), Values::new()).unwrap();
        assert!(Store::open(dir.path(), &binding()).is_err());
        drop(first);
        assert!(Store::open(dir.path(), &binding()).is_ok());
    }
    /// Strict data has one number type: whole numbers come back as floats,
    /// and the canonical form puts them back, so the seal survives.
    #[test]
    fn numbers_survive_the_strict_round_trip() {
        let values = canonical(Values::from([
            ("shell.panels.bottom.thickness".into(), json!(56.0)),
            ("accessibility.text_scale".into(), json!(2)),
            ("terminal.padding".into(), json!(4.0)),
            ("terminal.font.size".into(), json!(13.5)),
        ]));
        assert_eq!(values["shell.panels.bottom.thickness"], json!(56));
        assert_eq!(values["accessibility.text_scale"], json!(2.0));
        assert_eq!(values["terminal.padding"], json!(4));
        registry::validate(&values).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (store, created) = Store::create(dir.path(), binding(), values.clone()).unwrap();
        drop(store);
        let (_, opened) = Store::open(dir.path(), &binding()).unwrap();
        assert_eq!(opened.values, values);
        assert_eq!(opened.content_digest, created.content_digest);
    }
    #[test]
    fn retire_sets_every_profile_document_aside_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, current) = Store::create(dir.path(), binding(), Values::new()).unwrap();
        store.commit(&current, &spacious(&current)).unwrap();
        // A serving writer holds the lock.
        assert!(Store::retire(dir.path(), &binding()).is_err());
        drop(store);
        let other = Binding {
            instance: "other".into(),
            profile: "default".into(),
        };
        assert!(Store::retire(dir.path(), &other).is_err(), "wrong binding");
        // Interrupted after the backup: the primary is still in place.
        std::fs::rename(
            dir.path().join(BACKUP),
            dir.path().join(format!("{BACKUP}.retired-test")),
        )
        .unwrap();
        let retired = Store::retire(dir.path(), &binding()).unwrap();
        assert_eq!(retired, vec![format!("{PRIMARY}.retired-test")]);
        assert!(!dir.path().join(PRIMARY).exists());
        assert!(dir.path().join(LOCK).exists(), "the lock stays");
        assert!(Store::open(dir.path(), &binding()).is_err());
        assert!(Store::seed(dir.path(), binding(), true).is_err());
        // Again: nothing left to retire, and that is not an error.
        assert!(Store::retire(dir.path(), &binding()).unwrap().is_empty());
        // Retire is final: no fresh profile starts beside the history.
        let error = Store::create(dir.path(), binding(), Values::new())
            .err()
            .unwrap();
        assert!(error.to_string().contains("retire is final"), "{error}");
        assert!(!dir.path().join(PRIMARY).exists());
    }
}
