// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mix::stats::{
    BUILTIN_NAMES, CANONICAL_KEYWORDS, ExecutionMode, HOF_NAMES, SessionRecord, StatsBucket,
    StatsCounters, UsageStats, current_iso_week, date_for_epoch_day, datetime_string,
    iso_week_for_date, today_string,
};
use rusqlite::{Connection, params};

const MAX_PERSISTED_TIMESTAMP: u64 = 253_402_300_799; // 9999-12-31 23:59:59 UTC
const STALE_TEMP_AGE: Duration = Duration::from_secs(3600);
static TEMP_SUFFIX: AtomicU64 = AtomicU64::new(0);
static STATS_ENABLED: OnceLock<bool> = OnceLock::new();

pub fn stats_setting_enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(str::trim),
        Some(v) if v.eq_ignore_ascii_case("off")
            || v.eq_ignore_ascii_case("false")
            || v == "0"
    )
}

/// Read MIX_STATS once for the process. Front ends consult this before
/// allocating a collector, which makes the kill switch skip all event work.
pub fn stats_enabled() -> bool {
    *STATS_ENABLED.get_or_init(|| stats_setting_enabled(std::env::var("MIX_STATS").ok().as_deref()))
}

fn debug_note(message: impl std::fmt::Display) {
    if std::env::var("MIX_DEBUG").is_ok() {
        eprintln!("mix-stats: {message}");
    }
}

fn is_canonical_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
        && iso_week_for_date(value).is_some()
}

fn is_canonical_week(value: &str) -> bool {
    value.len() == 8
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[5] == b'W'
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 5) || byte.is_ascii_digit())
        && value[6..]
            .parse::<u8>()
            .is_ok_and(|week| (1..=53).contains(&week))
}

pub fn stats_dir() -> Option<PathBuf> {
    if !stats_enabled() {
        return None;
    }
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME")
        && !xdg.is_empty()
    {
        let path = PathBuf::from(&xdg);
        if path.is_absolute() {
            return Some(path.join("mix"));
        }
        debug_note(format!(
            "XDG_STATE_HOME={xdg:?} is not absolute; using HOME fallback"
        ));
    }
    let home = std::env::var("HOME").ok().filter(|s| !s.is_empty())?;
    Some(PathBuf::from(home).join(".local/state/mix"))
}

fn ensure_stats_dir() -> Option<PathBuf> {
    let dir = stats_dir()?;
    if let Err(error) = fs::create_dir_all(&dir) {
        debug_note(format!("cannot create {}: {error}", dir.display()));
        return None;
    }
    Some(dir)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn legacy_data_file_count(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return false;
            };
            path.is_file()
                && !name.starts_with('.')
                && (name == "mix.db" || name.ends_with(".json"))
        })
        .count()
}

fn legacy_advisory() {
    let Some(new_dir) = ensure_stats_dir() else {
        return;
    };
    let marker = new_dir.join(".legacy-acknowledged");
    if marker.exists() {
        return;
    }
    let legacy_dir = crate::paths::mixos_src().join("_stats");
    if legacy_dir == new_dir {
        return;
    }
    let count = legacy_data_file_count(&legacy_dir);
    if count == 0 {
        return;
    }
    let legacy = shell_quote(&legacy_dir.display().to_string());
    let new = shell_quote(&new_dir.display().to_string());
    let new_slash = shell_quote(&format!("{}/", new_dir.display()));
    let marker = shell_quote(&marker.display().to_string());
    eprintln!(
        "mix-stats: legacy stats found at {} ({count} stats files).",
        legacy_dir.display()
    );
    eprintln!("           They are not migrated automatically. To migrate:");
    eprintln!("             mkdir -p {new} && \\");
    eprintln!(
        "               (cd {legacy} && mv -n -- *.json mix.db {new_slash} 2>/dev/null || true)"
    );
    eprintln!("           To keep them there and suppress this notice:");
    eprintln!("             touch {marker}");
}

struct StatsLock(File);

impl StatsLock {
    fn acquire(
        dir: &Path,
        name: &str,
        exclusive: bool,
        deadline: Duration,
    ) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(name))?;
        let operation = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        } | libc::LOCK_NB;
        let started = Instant::now();
        loop {
            let result = unsafe { libc::flock(file.as_raw_fd(), operation) };
            if result == 0 {
                return Ok(Self(file));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::WouldBlock || started.elapsed() >= deadline {
                return Err(error);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for StatsLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Run records, `runs/YYYY-MM-DD.jsonl`, named for the UTC day they were
/// appended on. Immutable history: never rewritten, truncated or deleted. The
/// name is only where a record was written; its buckets carry their own dates.
const RUNS_DIR: &str = "runs";
/// Advisory per-day aggregates, `cache/YYYY-MM-DD.json`, and legacy document
/// bounds, `cache/legacy/<name>.json`. Disposable: each names the exact file
/// it summarises, and anything else is recomputed.
const CACHE_DIR: &str = "cache";
/// `clear` and `reset`, appended and applied at read time.
const OPS_FILE: &str = "ops.jsonl";
/// The new store's own lock, held only while refreshing the cache. Binaries
/// from before the JSONL store take `stats.lock` for every write and drop
/// the write if it is busy for 200 ms, so this store never takes that one.
const CACHE_LOCK: &str = "cache.lock";
/// Serialises `clear` and `reset`: reading positions and appending the op
/// happen as one step. Runs and older binaries never take it.
const OPS_LOCK: &str = "ops.lock";
/// How long an op waits for another op before refusing.
const OPS_LOCK_WAIT: Duration = Duration::from_secs(2);
/// Attempts for a mid-session flush.
const PENDING_FLUSH_ATTEMPTS: usize = 3;
/// Attempts to read the legacy documents as one consistent snapshot.
const LEGACY_READ_ATTEMPTS: u32 = 5;

/// A writer for one framed record: production makes exactly one `write(2)`.
/// Tests substitute short, failing or repeating writers.
type WriteOnce<'a> = &'a mut dyn FnMut(&mut File, &[u8]) -> std::io::Result<usize>;

/// One `write(2)`. `File::write` is a single syscall; `write_all` would loop
/// and could split a record across writes that interleave with other runs.
fn write_once(file: &mut File, bytes: &[u8]) -> std::io::Result<usize> {
    file.write(bytes)
}

/// How one append went.
#[derive(Debug)]
enum Append {
    /// The whole record landed.
    Written,
    /// Only this many bytes landed: a torn record, which readers skip (or, if
    /// only the final newline is missing, read once). Never retried.
    Short(usize),
    /// Known to have written nothing: the open failed, or `write(2)` failed
    /// with an error the kernel raises before writing any byte.
    Clean(String),
    /// `write(2)` failed with an error that does not prove nothing landed (an
    /// EIO from a failed commit can follow data the file already holds).
    Uncertain(String),
}

/// Errors `write(2)` returns before transferring any byte.
fn wrote_nothing(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(
            libc::ENOSPC
                | libc::EDQUOT
                | libc::EAGAIN
                | libc::EINTR
                | libc::EFBIG
                | libc::EBADF
                | libc::EINVAL
                | libc::EROFS
                | libc::EACCES
                | libc::EPERM
        )
    )
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// 128 random bits as hex, from `getrandom(2)`. If the kernel cannot supply
/// them, time, pid and a counter still make the id unique to this process.
fn random_id() -> String {
    let mut bytes = [0_u8; 16];
    let filled = unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if usize::try_from(filled) != Ok(bytes.len()) {
        let count = TEMP_SUFFIX.fetch_add(1, Ordering::Relaxed);
        return format!("{:x}-{:x}-{count:x}", now_nanos(), std::process::id());
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The stored form of a delta: its dated buckets and run records. The
/// top-level totals derive from the buckets, so they are not stored twice.
fn record_json(stats: &UsageStats) -> serde_json::Value {
    let mut value = stats.to_json();
    if let Some(object) = value.as_object_mut() {
        object.retain(|key, _| key == "buckets" || key == "sessions");
    }
    value
}

/// Frame one JSON value as a record: a newline, the value, a newline. The
/// leading newline ends any torn record before it, so a crash cannot cost the
/// next run its line.
fn frame(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    let mut bytes = vec![b'\n'];
    serde_json::to_writer(&mut bytes, value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// A delta as one framed record carrying a random `batch` id (readers keep
/// one copy of each batch per day file) and its write time `at` in Unix
/// nanoseconds (metadata only; nothing is ordered by it).
fn encode_record(delta: &UsageStats) -> Result<Vec<u8>, String> {
    let mut value = record_json(delta);
    if let Some(object) = value.as_object_mut() {
        object.insert("batch".into(), random_id().into());
        object.insert("at".into(), now_nanos().into());
    }
    frame(&value)
}

/// Open `path` for append (creating it and its directory) and hand the
/// record to `write` once.
fn append_to(path: &Path, bytes: &[u8], write: WriteOnce<'_>) -> Append {
    let open = || OpenOptions::new().create(true).append(true).open(path);
    let mut file = match open() {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent()
                && let Err(error) = fs::create_dir_all(parent)
            {
                return Append::Clean(format!("create {}: {error}", parent.display()));
            }
            match open() {
                Ok(file) => file,
                Err(error) => return Append::Clean(format!("open {}: {error}", path.display())),
            }
        }
        Err(error) => return Append::Clean(format!("open {}: {error}", path.display())),
    };
    match write(&mut file, bytes) {
        Ok(written) if written == bytes.len() => Append::Written,
        Ok(written) => Append::Short(written),
        Err(error) if wrote_nothing(&error) => {
            Append::Clean(format!("write {}: {error}", path.display()))
        }
        Err(error) => Append::Uncertain(format!("write {}: {error}", path.display())),
    }
}

fn run_path(dir: &Path, day: &str) -> PathBuf {
    dir.join(RUNS_DIR).join(format!("{day}.jsonl"))
}

pub fn flush_batch(mut delta: UsageStats) {
    if !stats_enabled() {
        return;
    }
    delta.finalize_session();
    let Some(dir) = stats_dir() else {
        return;
    };
    flush_run(&dir, &delta, &today_string(), &mut write_once);
}

/// The exit path of a run, and of a `--serve` generation retiring at RELOAD:
/// encode the finished delta and append it to `day`'s file, once. Nothing
/// else: no listing, no lock, no maintenance, no retry. A short or failed
/// write drops the run; collection is best-effort.
fn flush_run(dir: &Path, delta: &UsageStats, day: &str, write: WriteOnce<'_>) {
    let bytes = match encode_record(delta) {
        Ok(bytes) => bytes,
        Err(error) => return debug_note(format!("run not recorded: {error}")),
    };
    match append_to(&run_path(dir, day), &bytes, write) {
        Append::Written => {}
        Append::Short(written) => debug_note(format!(
            "run dropped: short write ({written} of {} bytes)",
            bytes.len()
        )),
        Append::Clean(error) | Append::Uncertain(error) => {
            debug_note(format!("run dropped: {error}"))
        }
    }
}

/// Persist a mid-session delta from `UsageStats::drain_pending_buckets` (a
/// REPL `mix stats` pipeline drains the live collector before the child
/// reads): buckets only, no session record, nothing finalized.
pub fn flush_pending_delta(delta: &UsageStats) -> Result<(), Box<UsageStats>> {
    if !stats_enabled() || delta.buckets.is_empty() {
        return Ok(());
    }
    let Some(dir) = stats_dir() else {
        return Err(Box::new(delta.clone()));
    };
    flush_pending_to(&dir, delta, &mut write_once)
}

/// The record, its batch id and its file are fixed before the first attempt,
/// so every retry repeats the same bytes into the same file and readers keep
/// one copy. A short write is final. `Err` hands the delta back for the
/// caller to merge into the live collector, and only when every attempt is
/// known to have written nothing: after an uncertain attempt the bytes may
/// already be on disk, and a merged-back delta would be flushed again under a
/// new batch id and counted twice, so it is dropped instead.
fn flush_pending_to(
    dir: &Path,
    delta: &UsageStats,
    write: WriteOnce<'_>,
) -> Result<(), Box<UsageStats>> {
    let bytes = match encode_record(delta) {
        Ok(bytes) => bytes,
        Err(error) => {
            debug_note(format!("pending flush not encoded: {error}"));
            return Err(Box::new(delta.clone()));
        }
    };
    let path = run_path(dir, &today_string());
    let mut uncertain = false;
    for _ in 0..PENDING_FLUSH_ATTEMPTS {
        match append_to(&path, &bytes, write) {
            Append::Written => return Ok(()),
            Append::Short(written) => {
                debug_note(format!(
                    "pending flush dropped: short write ({written} of {} bytes)",
                    bytes.len()
                ));
                return Ok(());
            }
            Append::Clean(error) => debug_note(format!("pending flush attempt failed: {error}")),
            Append::Uncertain(error) => {
                uncertain = true;
                debug_note(format!("pending flush attempt uncertain: {error}"));
            }
        }
    }
    if uncertain {
        debug_note("pending flush dropped: an attempt may have landed");
        return Ok(());
    }
    Err(Box::new(delta.clone()))
}

/// Compatibility entry point used by older callers.
pub fn save_stats(stats: &mut UsageStats) {
    stats.finalize_session();
    flush_batch(stats.clone());
}

/// Parse and validate one stored line; `None` for a blank, torn or invalid
/// one. A complete record that only lost its final newline still parses.
/// Returns the record's batch id with its contents.
fn parse_record(line: &[u8]) -> Option<(Option<String>, UsageStats)> {
    let line = line.trim_ascii();
    if line.is_empty() {
        return None;
    }
    let value = match serde_json::from_slice::<serde_json::Value>(line) {
        Ok(value) => value,
        Err(error) => {
            debug_note(format!("skipped an unreadable stats record: {error}"));
            return None;
        }
    };
    let batch = value
        .get("batch")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let stats = UsageStats::from_json(value);
    if let Err(error) = validate_persisted_doc(&stats) {
        debug_note(format!("skipped a stats record: {error}"));
        return None;
    }
    Some((batch, stats))
}

/// Stream `reader` line by line into `f` with each line's end offset;
/// returns the bytes consumed.
fn for_each_line(
    reader: impl std::io::Read,
    mut f: impl FnMut(&[u8], u64),
) -> std::io::Result<u64> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let mut consumed = 0_u64;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(consumed);
        }
        consumed = consumed.saturating_add(read as u64);
        f(&line, consumed);
    }
}

/// Raw day files on disk, oldest first.
fn raw_days(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let entries = match fs::read_dir(dir.join(RUNS_DIR)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut days: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let day = name.to_str()?.strip_suffix(".jsonl")?;
            is_canonical_date(day).then(|| (day.to_string(), entry.path()))
        })
        .collect();
    days.sort();
    Ok(days)
}

/// Accumulates records into one: buckets that share date, mode and script
/// are summed, and each run is kept once.
#[derive(Default)]
struct Merger {
    buckets: HashMap<(String, ExecutionMode, Option<String>), StatsCounters>,
    sessions: Vec<SessionRecord>,
    seen: HashSet<String>,
}

impl Merger {
    fn add(&mut self, stats: UsageStats) {
        for bucket in stats.buckets {
            self.buckets
                .entry((bucket.date, bucket.mode, bucket.script))
                .or_default()
                .merge(&bucket.counters);
        }
        for session in stats.sessions {
            if self.seen.insert(session.id.clone()) {
                self.sessions.push(session);
            }
        }
    }

    fn finish(self) -> UsageStats {
        let mut merged = empty_doc("");
        merged.buckets = self
            .buckets
            .into_iter()
            .map(|((date, mode, script), counters)| StatsBucket {
                date,
                mode,
                script,
                counters,
            })
            .collect();
        merged.buckets.sort_by(|a, b| {
            (&a.date, a.mode.as_str(), &a.script).cmp(&(&b.date, b.mode.as_str(), &b.script))
        });
        merged.sessions = self.sessions;
        merged.rebuild_aggregates();
        merged
    }
}

/// Which file, exactly: taken from the `fstat` of the descriptor that was
/// read, so a same-length replacement or a reused inode does not match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileId {
    dev: u64,
    ino: u64,
    len: u64,
    mtime_ns: i64,
    ctime_ns: i64,
}

impl FileId {
    fn of(meta: &fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            mtime_ns: meta
                .mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.mtime_nsec()),
            ctime_ns: meta
                .ctime()
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.ctime_nsec()),
        }
    }

    fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "dev": self.dev, "ino": self.ino, "len": self.len,
            "mtime_ns": self.mtime_ns, "ctime_ns": self.ctime_ns,
        })
    }

    fn from_json(value: &serde_json::Value) -> Option<Self> {
        Some(Self {
            dev: value.get("dev")?.as_u64()?,
            ino: value.get("ino")?.as_u64()?,
            len: value.get("len")?.as_u64()?,
            mtime_ns: value.get("mtime_ns")?.as_i64()?,
            ctime_ns: value.get("ctime_ns")?.as_i64()?,
        })
    }
}

/// The dates a record set touches, by bucket and by run: the only bounds a
/// window may skip on. A file's name says nothing about its contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DateBounds(Option<(String, String)>);

impl DateBounds {
    fn include(&mut self, date: &str) {
        match &mut self.0 {
            None => self.0 = Some((date.to_string(), date.to_string())),
            Some((min, max)) => {
                if date < min.as_str() {
                    *min = date.to_string();
                }
                if date > max.as_str() {
                    *max = date.to_string();
                }
            }
        }
    }

    fn of(stats: &UsageStats) -> Self {
        let mut bounds = Self::default();
        for bucket in &stats.buckets {
            bounds.include(&bucket.date);
        }
        for session in &stats.sessions {
            bounds.include(&date_for_epoch_day(session.started / 86_400));
        }
        bounds
    }

    fn to_json(&self) -> serde_json::Value {
        match &self.0 {
            None => serde_json::Value::Null,
            Some((min, max)) => serde_json::json!([min, max]),
        }
    }

    fn from_json(value: &serde_json::Value) -> Option<Self> {
        if value.is_null() {
            return Some(Self(None));
        }
        let pair = value.as_array()?;
        let (min, max) = (pair.first()?.as_str()?, pair.get(1)?.as_str()?);
        (is_canonical_date(min) && is_canonical_date(max) && min <= max)
            .then(|| Self(Some((min.to_string(), max.to_string()))))
    }
}

/// A `clear` or `reset`, anchored to storage positions instead of a clock.
/// It covers, in each raw day file that existed when it was issued, exactly
/// the records whose JSON ends within the length that file had then;
/// anything appended later, to any file, is after it. Ops apply only to the
/// JSONL store: while legacy weekly documents exist, `clear` and `reset`
/// refuse (see `append_op`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum OpKind {
    Clear(String),
    Reset(String),
}

#[derive(Debug, Clone)]
struct Op {
    kind: OpKind,
    /// Raw day file -> (dev, ino, length) when the op was issued.
    positions: HashMap<String, (u64, u64, u64)>,
}

impl Op {
    /// How many leading bytes of this raw file the op covers.
    fn cutoff(&self, day: &str, id: &FileId) -> u64 {
        match self.positions.get(day) {
            Some((dev, ino, len)) if *dev == id.dev && *ino == id.ino => *len,
            _ => 0,
        }
    }
}

fn parse_op(line: &[u8]) -> Option<Op> {
    let value: serde_json::Value = serde_json::from_slice(line.trim_ascii()).ok()?;
    let args = value.get("args")?;
    let kind = match value.get("op")?.as_str()? {
        "clear" => OpKind::Clear(args.get("name")?.as_str()?.to_string()),
        "reset" => {
            let week = args.get("week")?.as_str()?;
            is_canonical_week(week).then(|| OpKind::Reset(week.to_string()))?
        }
        _ => return None,
    };
    let mut positions = HashMap::new();
    for (day, position) in value.get("positions")?.as_object()? {
        let position = position.as_array()?;
        positions.insert(
            day.clone(),
            (
                position.first()?.as_u64()?,
                position.get(1)?.as_u64()?,
                position.get(2)?.as_u64()?,
            ),
        );
    }
    Some(Op { kind, positions })
}

/// Every recorded op, collapsed: ops of the same kind on the same target
/// (repeated clears of one name, repeated resets of one week) become one,
/// covering the union of their prefixes (the largest length per file).
/// Collapsing bounds the number of ops a report applies; every report still
/// reads the whole of ops.jsonl to get there.
fn load_ops(dir: &Path) -> Vec<Op> {
    let Ok(file) = File::open(dir.join(OPS_FILE)) else {
        return Vec::new();
    };
    let mut collapsed: Vec<Op> = Vec::new();
    let mut index: HashMap<OpKind, usize> = HashMap::new();
    let result = for_each_line(file, |line, _| {
        let Some(op) = parse_op(line) else {
            return;
        };
        match index.get(&op.kind) {
            Some(&at) => {
                let into = &mut collapsed[at];
                for (day, (dev, ino, len)) in op.positions {
                    let slot = into.positions.entry(day).or_insert((dev, ino, len));
                    if (slot.0, slot.1) == (dev, ino) {
                        slot.2 = slot.2.max(len);
                    } else {
                        *slot = (dev, ino, len);
                    }
                }
            }
            None => {
                index.insert(op.kind.clone(), collapsed.len());
                collapsed.push(op);
            }
        }
    });
    if let Err(error) = result {
        debug_note(format!("reading {OPS_FILE}: {error}"));
    }
    collapsed
}

fn apply_kind(stats: &mut UsageStats, kind: &OpKind) -> bool {
    match kind {
        OpKind::Clear(name) => clear_name_from_stats(stats, name),
        OpKind::Reset(week) => {
            let in_week = |date: &str| iso_week_for_date(date).as_deref() == Some(week.as_str());
            let before = (stats.buckets.len(), stats.sessions.len());
            stats.buckets.retain(|bucket| !in_week(&bucket.date));
            stats
                .sessions
                .retain(|session| !in_week(&date_for_epoch_day(session.started / 86_400)));
            stats.rebuild_aggregates();
            before != (stats.buckets.len(), stats.sessions.len())
        }
    }
}

/// What one raw day file holds, read from one descriptor: its records before
/// any op (each batch once), the file's identity from that descriptor's
/// `fstat` with the bytes actually consumed, the dates it touches, and the
/// smallest and largest payload end among its records.
struct RawDay {
    stats: UsageStats,
    id: FileId,
    bounds: DateBounds,
    payload: Option<(u64, u64)>,
    /// Whether the file stayed the same while it was read.
    stable: bool,
}

/// Where a record's own bytes end: just after its closing JSON byte. The
/// newline after it may have been written by the next record (a record whose
/// write came up short by its final newline), so it is not part of the
/// record, and coverage never depends on what a later append supplied.
fn payload_end(line: &[u8], line_end: u64) -> u64 {
    let trailing = line.len() - line.trim_ascii_end().len();
    line_end.saturating_sub(trailing as u64)
}

/// Whether an op with `cutoff` covers all, none or part of records whose
/// payload ends span `payload`.
#[derive(Debug, PartialEq, Eq)]
enum Coverage {
    All,
    None,
    Part,
}

fn coverage(cutoff: u64, payload: Option<(u64, u64)>) -> Coverage {
    match payload {
        None => Coverage::None,
        Some((_, last)) if cutoff >= last => Coverage::All,
        Some((first, _)) if cutoff < first => Coverage::None,
        Some(_) => Coverage::Part,
    }
}

/// Read a raw day. With `ops`, apply each op to exactly the records whose
/// payload ends within the bytes it covers.
fn read_raw_day(day: &str, path: &Path, ops: Option<&[Op]>) -> std::io::Result<RawDay> {
    let file = File::open(path)?;
    let before = FileId::of(&file.metadata()?);
    let mut merger = Merger::default();
    let mut batches = HashSet::new();
    let mut payload: Option<(u64, u64)> = None;
    let cutoffs: Vec<(u64, &OpKind)> = ops
        .unwrap_or_default()
        .iter()
        .map(|op| (op.cutoff(day, &before), &op.kind))
        .filter(|(cutoff, _)| *cutoff > 0)
        .collect();
    let consumed = for_each_line(&file, |line, end| {
        let Some((batch, mut stats)) = parse_record(line) else {
            return;
        };
        let own_end = payload_end(line, end);
        payload = Some(match payload {
            None => (own_end, own_end),
            Some((first, last)) => (first.min(own_end), last.max(own_end)),
        });
        if let Some(batch) = batch
            && !batches.insert(batch)
        {
            return;
        }
        for (cutoff, kind) in &cutoffs {
            if own_end <= *cutoff {
                apply_kind(&mut stats, kind);
            }
        }
        merger.add(stats);
    })?;
    let after = FileId::of(&file.metadata()?);
    let stats = merger.finish();
    let bounds = DateBounds::of(&stats);
    let id = FileId {
        len: consumed,
        ..before
    };
    Ok(RawDay {
        stats,
        stable: after == before && consumed == before.len,
        id,
        bounds,
        payload,
    })
}

fn cache_path(dir: &Path, day: &str) -> PathBuf {
    dir.join(CACHE_DIR).join(format!("{day}.json"))
}

/// The first line of a cache file: which raw file it summarises, the dates
/// its records touch, and their payload-end range, readable without the
/// aggregate on line two.
struct CacheHeader {
    id: FileId,
    bounds: DateBounds,
    payload: Option<(u64, u64)>,
}

/// Which kind of cache file a header belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheKind {
    /// `cache/DAY.json`: a raw day's aggregate, which ops are judged against,
    /// so it must say where its records' payloads end.
    RawAggregate,
    /// `cache/legacy/NAME.json`: only a legacy document's date bounds.
    LegacyBounds,
}

/// A raw-aggregate header must carry `payload`: a range with
/// 0 < first <= last <= the summarised length, or null for a file with no
/// records (checked against the body). A header without it (written before
/// payload coverage existed) or with an impossible range is rejected, so the
/// day is read from the raw file instead of letting every op miss it.
fn read_cache_header(path: &Path, kind: CacheKind) -> Option<(CacheHeader, BufReader<File>)> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).ok()?;
    let value: serde_json::Value = serde_json::from_slice(line.trim_ascii()).ok()?;
    let id = FileId::from_json(value.get("id")?)?;
    let bounds = DateBounds::from_json(value.get("dates")?)?;
    let payload = match (kind, value.get("payload")) {
        (CacheKind::LegacyBounds, _) => None,
        (CacheKind::RawAggregate, Some(serde_json::Value::Array(pair))) => {
            let (first, last) = (pair.first()?.as_u64()?, pair.get(1)?.as_u64()?);
            (0 < first && first <= last && last <= id.len).then_some(Some((first, last)))?
        }
        (CacheKind::RawAggregate, Some(serde_json::Value::Null)) => None,
        (CacheKind::RawAggregate, _) => return None,
    };
    Some((
        CacheHeader {
            id,
            bounds,
            payload,
        },
        reader,
    ))
}

/// The aggregate on a cache file's second line, fully validated.
fn read_cache_body(mut reader: BufReader<File>) -> Option<UsageStats> {
    let mut body = Vec::new();
    reader.read_to_end(&mut body).ok()?;
    let value: serde_json::Value = serde_json::from_slice(body.trim_ascii()).ok()?;
    let stats = UsageStats::from_json(value);
    validate_persisted_doc(&stats).ok()?;
    Some(stats)
}

fn write_cache(dir: &Path, day: &str, raw: &RawDay) -> Result<(), String> {
    let payload = raw
        .payload
        .map_or(serde_json::Value::Null, |(first, last)| {
            serde_json::json!([first, last])
        });
    let header = serde_json::json!({
        "day": day,
        "id": raw.id.to_json(),
        "dates": raw.bounds.to_json(),
        "payload": payload,
    });
    let mut bytes = serde_json::to_vec(&header).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    serde_json::to_writer(&mut bytes, &record_json(&raw.stats)).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    fs::create_dir_all(dir.join(CACHE_DIR)).map_err(|e| e.to_string())?;
    atomic_write_bytes(&cache_path(dir, day), &bytes)
}

/// A raw day's contribution to a window, with every op applied. The cache
/// is used only when its header names exactly the file as it is now (device,
/// inode, length, mtime and ctime). Its date bounds then decide whether the
/// day can touch the window at all, and its aggregate is used unless an op
/// covers some of its records but not all (judged by payload ends, as for a
/// single record); then the raw file is read record by record. Without a
/// usable cache the raw file is read; with `may_cache`, a stable read of a
/// day before today refreshes the cache. A day that cannot be read counts
/// nothing.
fn day_records(
    dir: &Path,
    day: &str,
    path: &Path,
    ops: &[Op],
    window: &StatsWindow,
    may_cache: bool,
) -> UsageStats {
    let current = match fs::metadata(path) {
        Ok(meta) if meta.is_file() => FileId::of(&meta),
        Ok(_) | Err(_) => {
            debug_note(format!("skipped {}: not a readable file", path.display()));
            return empty_doc("");
        }
    };
    if let Some((header, reader)) =
        read_cache_header(&cache_path(dir, day), CacheKind::RawAggregate)
        && header.id == current
    {
        if !window.overlaps(&header.bounds) {
            return empty_doc("");
        }
        let covered: Vec<Coverage> = ops
            .iter()
            .map(|op| coverage(op.cutoff(day, &header.id), header.payload))
            .collect();
        if !covered.contains(&Coverage::Part)
            && let Some(mut stats) = read_cache_body(reader)
            // A null payload range only describes a file with no records.
            && (header.payload.is_some() || (stats.buckets.is_empty() && stats.sessions.is_empty()))
        {
            for (op, covered) in ops.iter().zip(covered) {
                if covered == Coverage::All {
                    apply_kind(&mut stats, &op.kind);
                }
            }
            return stats;
        }
    }
    let raw = match read_raw_day(day, path, None) {
        Ok(raw) => raw,
        Err(error) => {
            debug_note(format!("skipped {}: {error}", path.display()));
            return empty_doc("");
        }
    };
    if may_cache
        && raw.stable
        && let Err(error) = write_cache(dir, day, &raw)
    {
        debug_note(format!("cache for {day} not written: {error}"));
    }
    if ops
        .iter()
        .all(|op| coverage(op.cutoff(day, &raw.id), raw.payload) == Coverage::None)
    {
        return raw.stats;
    }
    match read_raw_day(day, path, Some(ops)) {
        Ok(raw) => raw.stats,
        Err(error) => {
            debug_note(format!("skipped {}: {error}", path.display()));
            empty_doc("")
        }
    }
}

/// Where a legacy read is, for the test seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyPhase {
    /// Every document is read; the snapshot is about to be validated.
    Read,
    /// The snapshot validated; the merge is next.
    Validated,
}

/// Seam for tests: called at each phase of a legacy read, so a test can
/// model an old writer racing it.
type BetweenReads<'a> = &'a mut dyn FnMut(LegacyPhase);

/// The weekly documents of the pre-JSONL store, read in place: binaries from
/// before the JSONL store may still be running and writing them, and they
/// write only these files while new binaries write only JSONL, so the two
/// sources never overlap. Read with the old store's rules: a `current.json`
/// from an earlier week whose fingerprint that week's archive already lists
/// was merged by a rotation that died before removing it, so it is skipped;
/// each run counts once across documents. Nothing here is written, renamed
/// or quarantined, and no lock is taken (an old writer waits at most 200 ms
/// for `stats.lock` and then drops its write). Instead the read is a
/// validated snapshot: every document is identified from the descriptor it
/// was read through, including the archive the rotation rule consults, the
/// set is listed and stat'ed again afterwards, and any change (an old writer
/// replacing, rotating or creating a document mid-read) means a retry. The
/// merge then uses only what the snapshot captured. Returns the merged
/// contribution and whether it stayed unstable after every attempt.
/// Documents whose cached bounds miss `window` are identified but not parsed.
/// `mix.db` held only what the documents carried and is ignored.
fn legacy_contribution(
    dir: &Path,
    window: &StatsWindow,
    may_cache: bool,
    between: BetweenReads<'_>,
) -> (UsageStats, bool) {
    let now_week = current_iso_week();
    let mut documents = Vec::new();
    for attempt in 0..LEGACY_READ_ATTEMPTS {
        documents.clear();
        let listed = legacy_sources(dir);
        let mut seen = Vec::new();
        for path in &listed {
            match read_legacy_source(dir, path, window, may_cache, false) {
                Some((id, document)) => {
                    seen.push((path.clone(), Some(id)));
                    documents.extend(document.map(|document| (path.clone(), document)));
                }
                None => seen.push((path.clone(), None)),
            }
        }
        // The rotation rule for an earlier week's current.json needs that
        // week's archive: capture it in this snapshot even if its bounds
        // would have skipped it.
        let rotation_archive = documents
            .iter()
            .find(|(path, _)| path.file_name().is_some_and(|name| name == "current.json"))
            .map(|(_, current)| document_week(current, &now_week))
            .filter(|week| *week != now_week)
            .map(|week| dir.join(format!("{week}.json")))
            .filter(|archive| {
                listed.contains(archive) && !documents.iter().any(|(path, _)| path == archive)
            });
        if let Some(archive) = rotation_archive
            && let Some((id, document)) = read_legacy_source(dir, &archive, window, may_cache, true)
        {
            if let Some(entry) = seen.iter_mut().find(|(path, _)| *path == archive) {
                entry.1 = Some(id);
            }
            documents.extend(document.map(|document| (archive.clone(), document)));
        }
        between(LegacyPhase::Read);
        let unchanged = legacy_sources(dir) == listed
            && seen
                .iter()
                .all(|(path, id)| fs::metadata(path).ok().map(|meta| FileId::of(&meta)) == *id);
        if unchanged {
            between(LegacyPhase::Validated);
            return (merge_legacy(&documents), false);
        }
        std::thread::sleep(Duration::from_millis(10 << attempt));
    }
    (merge_legacy(&documents), true)
}

/// `current.json` and every `YYYY-WNN.json`, sorted.
fn legacy_sources(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name == "current.json"
                        || name.strip_suffix(".json").is_some_and(is_canonical_week)
                })
        })
        .collect();
    paths.sort();
    paths
}

fn legacy_bounds_path(dir: &Path, source: &Path) -> PathBuf {
    let name = source.file_name().unwrap_or_default().to_string_lossy();
    dir.join(CACHE_DIR).join("legacy").join(name.as_ref())
}

/// Identify one legacy document through the descriptor it is read from and,
/// unless `force` is off and its cached bounds (for exactly this file) miss
/// the window, parse it. `None` if it cannot be opened; `Some((id, None))` if
/// it was skipped or does not parse.
fn read_legacy_source(
    dir: &Path,
    path: &Path,
    window: &StatsWindow,
    may_cache: bool,
    force: bool,
) -> Option<(FileId, Option<UsageStats>)> {
    let mut file = File::open(path).ok()?;
    let id = FileId::of(&file.metadata().ok()?);
    let bounds_path = legacy_bounds_path(dir, path);
    if !force
        && let Some((header, _)) = read_cache_header(&bounds_path, CacheKind::LegacyBounds)
        && header.id == id
        && !window.overlaps(&header.bounds)
    {
        return Some((id, None));
    }
    let mut content = String::new();
    if let Err(error) = file.read_to_string(&mut content) {
        debug_note(format!("skipped legacy {}: {error}", path.display()));
        return Some((id, None));
    }
    let document = match serde_json::from_str(&content) {
        Ok(value) => UsageStats::from_json(value),
        Err(error) => {
            debug_note(format!("skipped legacy {}: {error}", path.display()));
            return Some((id, None));
        }
    };
    if let Err(error) = validate_persisted_doc(&document) {
        debug_note(format!("skipped legacy {}: {error}", path.display()));
        return Some((id, None));
    }
    if may_cache {
        let header =
            serde_json::json!({"id": id.to_json(), "dates": DateBounds::of(&document).to_json()});
        let mut bytes = header.to_string().into_bytes();
        bytes.push(b'\n');
        let written = bounds_path
            .parent()
            .map(fs::create_dir_all)
            .transpose()
            .map_err(|e| e.to_string())
            .and_then(|_| atomic_write_bytes(&bounds_path, &bytes));
        if let Err(error) = written {
            debug_note(format!("legacy bounds not cached: {error}"));
        }
    }
    Some((id, Some(document)))
}

/// Merge the captured legacy documents with the old store's rotation rule
/// and keep each run once. Reads nothing from disk: every document the rule
/// consults was captured, and validated, with the rest.
fn merge_legacy(documents: &[(PathBuf, UsageStats)]) -> UsageStats {
    let now_week = current_iso_week();
    let archives: HashMap<&str, &UsageStats> = documents
        .iter()
        .filter_map(|(path, document)| {
            let week = path.file_name()?.to_str()?.strip_suffix(".json")?;
            is_canonical_week(week).then_some((week, document))
        })
        .collect();
    let mut merger = Merger::default();
    for (path, document) in documents {
        if path.file_name().is_some_and(|name| name == "current.json") {
            let week = document_week(document, &now_week);
            if week != now_week {
                let mut rotated_source = document.clone();
                rotated_source.week.clone_from(&week);
                let rotated = archives
                    .get(week.as_str())
                    .zip(rotation_fingerprint(&rotated_source).ok())
                    .is_some_and(|(archive, source)| archive.rotation_sources.contains(&source));
                if rotated {
                    continue;
                }
            }
        }
        merger.add(document.clone());
    }
    merger.finish()
}

/// The week a legacy document belongs to (the old store's rule).
fn document_week(stats: &UsageStats, fallback: &str) -> String {
    if is_canonical_week(&stats.week) {
        return stats.week.clone();
    }
    let from_buckets = stats
        .buckets
        .iter()
        .filter(|bucket| is_canonical_date(&bucket.date))
        .max_by_key(|bucket| &bucket.date)
        .and_then(|bucket| iso_week_for_date(&bucket.date));
    let from_sessions = stats
        .sessions
        .iter()
        .max_by_key(|session| session.started)
        .map(|session| date_for_epoch_day(session.started / 86_400))
        .and_then(|date| iso_week_for_date(&date));
    let from_last_date = is_canonical_date(&stats.last_date)
        .then(|| iso_week_for_date(&stats.last_date))
        .flatten();
    from_buckets
        .or(from_sessions)
        .or(from_last_date)
        .unwrap_or_else(|| fallback.to_string())
}

/// The old store's rotation fingerprint of a document.
fn rotation_fingerprint(stats: &UsageStats) -> Result<String, String> {
    let bytes = serde_json::to_vec(&stats.to_json()).map_err(|error| error.to_string())?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

/// The positions of every raw day file now, for a new op.
fn current_positions(dir: &Path) -> Result<HashMap<String, (u64, u64, u64)>, String> {
    let mut positions = HashMap::new();
    for (day, path) in raw_days(dir)? {
        if let Ok(meta) = fs::metadata(&path)
            && meta.is_file()
        {
            positions.insert(day, (meta.dev(), meta.ino(), meta.len()));
        }
    }
    Ok(positions)
}

/// Why `clear` and `reset` refuse while the pre-JSONL weekly documents exist.
const LEGACY_REFUSAL: &str = "the old weekly stats (current.json, YYYY-WNN.json) are still \
     live on this host, and an older mix binary may still be writing them, so they cannot be \
     cleared exactly; nothing was recorded. They age out of week windows; once they are gone, \
     clear and reset work.";

/// Record an op: refuse while legacy documents exist (ops cover only the
/// JSONL store), then, under `ops.lock` so concurrent ops each see the
/// positions the other recorded, read every raw file's position and append
/// the op with one write. Runs and older binaries never take `ops.lock`.
fn append_op(dir: &Path, kind: OpKind) -> Result<(), String> {
    if !legacy_sources(dir).is_empty() {
        return Err(LEGACY_REFUSAL.to_string());
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let _lock = StatsLock::acquire(dir, OPS_LOCK, true, OPS_LOCK_WAIT).map_err(|error| {
        format!("{OPS_LOCK} busy ({error}); another clear or reset is running, try again")
    })?;
    // Again under the lock: a legacy document may have appeared while this
    // op waited. Still only a point-in-time check, not proof that no older
    // binary will write one later.
    if !legacy_sources(dir).is_empty() {
        return Err(LEGACY_REFUSAL.to_string());
    }
    let positions = current_positions(dir)?;
    let (op, args) = match &kind {
        OpKind::Clear(name) => ("clear", serde_json::json!({"name": name})),
        OpKind::Reset(week) => ("reset", serde_json::json!({"week": week})),
    };
    let positions: serde_json::Map<String, serde_json::Value> = positions
        .into_iter()
        .map(|(day, (dev, ino, len))| (day, serde_json::json!([dev, ino, len])))
        .collect();
    let bytes = frame(&serde_json::json!({
        "op": op,
        "args": args,
        "at": now_nanos(),
        "id": random_id(),
        "positions": positions,
    }))?;
    match append_to(&dir.join(OPS_FILE), &bytes, &mut write_once) {
        Append::Written => Ok(()),
        Append::Short(written) => Err(format!(
            "{OPS_FILE}: short write ({written} of {} bytes); run it again",
            bytes.len()
        )),
        Append::Clean(error) | Append::Uncertain(error) => Err(error),
    }
}

/// An atomic write that dies between create and rename leaves
/// `.<name>.<pid>.<n>.tmp` behind, and nothing ever reads it. Remove those
/// older than an hour; a younger one may belong to a write in progress.
fn remove_stale_temps(dir: &Path) {
    for dir in [
        dir.to_path_buf(),
        dir.join(CACHE_DIR),
        dir.join(CACHE_DIR).join("legacy"),
    ] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !(name.starts_with('.') && name.ends_with(".tmp")) {
                continue;
            }
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= STALE_TEMP_AGE);
            if stale && let Err(error) = fs::remove_file(entry.path()) {
                debug_note(format!("cannot remove stale {name}: {error}"));
            }
        }
    }
}

fn validate_persisted_doc(stats: &UsageStats) -> Result<(), String> {
    if !stats.last_date.is_empty() && !is_canonical_date(&stats.last_date) {
        return Err(format!("invalid persisted last_date {:?}", stats.last_date));
    }
    if let Some(bucket) = stats
        .buckets
        .iter()
        .find(|bucket| !is_canonical_date(&bucket.date))
    {
        return Err(format!("invalid persisted bucket date {:?}", bucket.date));
    }
    if let Some(session) = stats
        .sessions
        .iter()
        .find(|session| session.started > MAX_PERSISTED_TIMESTAMP)
    {
        return Err(format!(
            "persisted session timestamp {} exceeds supported range",
            session.started
        ));
    }
    Ok(())
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let suffix = TEMP_SUFFIX.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("stats.json");
    let temp = path.with_file_name(format!(".{name}.{}.{}.tmp", std::process::id(), suffix));
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| format!("create {}: {e}", temp.display()))?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temp, path).map_err(|e| format!("rename {}: {e}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn empty_doc(week: &str) -> UsageStats {
    let mut stats = UsageStats::new();
    stats.builtins.clear();
    stats.functions.clear();
    stats.aliases.clear();
    stats.commands.clear();
    stats.keywords.clear();
    stats.meta.clear();
    stats.errors.clear();
    stats.buckets.clear();
    stats.sessions.clear();
    stats.week = week.to_string();
    stats.last_date.clear();
    stats
}

/// Load records into a fresh SQLite database with the tables the on-disk
/// mix.db had, so `mix stats query` keeps answering the same SQL.
fn load_sqlite(connection: &mut Connection, stats: &UsageStats) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE usage (
             date TEXT, category TEXT, name TEXT, count INTEGER,
             PRIMARY KEY (date, category, name));
         CREATE TABLE sessions (
             started INTEGER, duration_secs INTEGER, commands INTEGER, peak_memory_kb INTEGER);
         CREATE TABLE usage_context (
             date TEXT, mode TEXT, script TEXT DEFAULT '', category TEXT, name TEXT, count INTEGER,
             PRIMARY KEY(date, mode, script, category, name));
         CREATE TABLE runs (
             id TEXT PRIMARY KEY, started INTEGER, duration_secs INTEGER, commands INTEGER,
             peak_memory_kb INTEGER, mode TEXT, script TEXT DEFAULT '');
         CREATE TABLE stats_meta (key TEXT PRIMARY KEY, value TEXT);",
    )?;
    let mut by_date: BTreeMap<&str, StatsCounters> = BTreeMap::new();
    for bucket in &stats.buckets {
        by_date
            .entry(bucket.date.as_str())
            .or_default()
            .merge(&bucket.counters);
        insert_counter_maps(
            &transaction,
            &bucket.date,
            Some((bucket.mode.as_str(), bucket.script.as_deref().unwrap_or(""))),
            &bucket.counters,
        )?;
    }
    for (date, counters) in &by_date {
        insert_counter_maps(&transaction, date, None, counters)?;
    }
    for run in &stats.sessions {
        transaction.execute(
            "INSERT OR IGNORE INTO runs
             (id, started, duration_secs, commands, peak_memory_kb, mode, script)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run.id,
                sqlite_i64(run.started),
                sqlite_i64(run.duration_secs),
                sqlite_i64(run.commands),
                sqlite_i64(run.peak_memory_kb),
                run.mode.as_str(),
                run.script.as_deref().unwrap_or("")
            ],
        )?;
        transaction.execute(
            "INSERT INTO sessions (started, duration_secs, commands, peak_memory_kb)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                sqlite_i64(run.started),
                sqlite_i64(run.duration_secs),
                sqlite_i64(run.commands),
                sqlite_i64(run.peak_memory_kb)
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO stats_meta(key, value) VALUES ('schema_version', '2')",
        [],
    )?;
    transaction.commit()
}

fn insert_counter_maps(
    tx: &rusqlite::Transaction<'_>,
    date: &str,
    context: Option<(&str, &str)>,
    counters: &StatsCounters,
) -> rusqlite::Result<()> {
    for (category, map) in counter_maps(counters) {
        for (name, count) in map {
            if let Some((mode, script)) = context {
                tx.execute(
                    "INSERT OR REPLACE INTO usage_context
                     (date, mode, script, category, name, count) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![date, mode, script, category, name, sqlite_i64(*count)],
                )?;
            } else {
                tx.execute(
                    "INSERT OR REPLACE INTO usage(date, category, name, count)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![date, category, name, sqlite_i64(*count)],
                )?;
            }
        }
    }
    Ok(())
}

fn sqlite_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn counter_maps(c: &StatsCounters) -> [(&'static str, &HashMap<String, u64>); 7] {
    [
        ("builtin", &c.builtins),
        ("function", &c.functions),
        ("alias", &c.aliases),
        ("command", &c.commands),
        ("keyword", &c.keywords),
        ("meta", &c.meta),
        ("error", &c.errors),
    ]
}

fn saturating_sum<'a>(values: impl IntoIterator<Item = &'a u64>) -> u64 {
    values
        .into_iter()
        .fold(0_u64, |total, value| total.saturating_add(*value))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatsWindow {
    CurrentWeek,
    Week(String),
    Since(String),
    AllTime,
    LastDays(u32),
}

impl StatsWindow {
    fn header(&self) -> String {
        match self {
            Self::CurrentWeek => format!("week {}", current_iso_week()),
            Self::Week(week) => format!("week {week}"),
            Self::Since(date) => format!("since {date}"),
            Self::AllTime => "all time".to_string(),
            Self::LastDays(days) => format!("last {days} recorded days"),
        }
    }

    /// Whether content touching exactly the dates in `bounds` can contribute
    /// to this window. Bounds come from the content itself (a validated
    /// cache header), never from a file name.
    fn overlaps(&self, bounds: &DateBounds) -> bool {
        let Some((min, max)) = &bounds.0 else {
            return false;
        };
        let spans_week = |week: &str| {
            iso_week_for_date(min).is_none_or(|w| w.as_str() <= week)
                && iso_week_for_date(max).is_none_or(|w| w.as_str() >= week)
        };
        match self {
            Self::CurrentWeek => spans_week(&current_iso_week()),
            Self::Week(week) => spans_week(week),
            Self::Since(start) => max.as_str() >= start.as_str(),
            Self::AllTime | Self::LastDays(_) => true,
        }
    }
}

/// Runs counted by (mode, script).
type RunCounts = HashMap<(ExecutionMode, Option<String>), u64>;

#[derive(Debug, Clone)]
pub struct StatsSnapshot {
    pub window: StatsWindow,
    pub counters: StatsCounters,
    pub buckets: Vec<StatsBucket>,
    /// Run records, kept only for reports that list them (`sessions`, `raw`).
    pub sessions: Vec<SessionRecord>,
    pub runs: RunCounts,
}

impl StatsSnapshot {
    fn usage(&self) -> UsageStats {
        let mut result = empty_doc(&self.window.header());
        result.buckets.clone_from(&self.buckets);
        result.sessions.clone_from(&self.sessions);
        result.rebuild_aggregates();
        result.last_date = result
            .buckets
            .iter()
            .map(|bucket| bucket.date.as_str())
            .max()
            .unwrap_or("")
            .to_string();
        result.week = match &self.window {
            StatsWindow::CurrentWeek => current_iso_week(),
            StatsWindow::Week(week) => week.clone(),
            _ => String::new(),
        };
        result
    }
}

/// Aggregates records into a window as they stream past: buckets summed by
/// date, mode and script, runs counted, and run records kept only on request.
struct Accumulator {
    window: StatsWindow,
    current_week: String,
    keep_sessions: bool,
    buckets: HashMap<(String, ExecutionMode, Option<String>), StatsCounters>,
    sessions: Vec<SessionRecord>,
    runs: RunCounts,
}

impl Accumulator {
    fn new(window: StatsWindow, keep_sessions: bool) -> Self {
        Self {
            window,
            current_week: current_iso_week(),
            keep_sessions,
            buckets: HashMap::new(),
            sessions: Vec::new(),
            runs: HashMap::new(),
        }
    }

    fn matches(&self, date: &str) -> bool {
        match &self.window {
            StatsWindow::CurrentWeek => {
                iso_week_for_date(date).as_deref() == Some(self.current_week.as_str())
            }
            StatsWindow::Week(week) => iso_week_for_date(date).as_deref() == Some(week.as_str()),
            StatsWindow::Since(start) => date >= start.as_str(),
            StatsWindow::AllTime | StatsWindow::LastDays(_) => true,
        }
    }

    fn add(&mut self, stats: UsageStats) {
        for bucket in stats.buckets {
            if self.matches(&bucket.date) {
                self.buckets
                    .entry((bucket.date, bucket.mode, bucket.script))
                    .or_default()
                    .merge(&bucket.counters);
            }
        }
        for session in stats.sessions {
            if self.matches(&date_for_epoch_day(session.started / 86_400)) {
                *self
                    .runs
                    .entry((session.mode, session.script.clone()))
                    .or_default() += 1;
                if self.keep_sessions {
                    self.sessions.push(session);
                }
            }
        }
    }

    fn finish(self) -> StatsSnapshot {
        let mut buckets: Vec<StatsBucket> = self
            .buckets
            .into_iter()
            .map(|((date, mode, script), counters)| StatsBucket {
                date,
                mode,
                script,
                counters,
            })
            .collect();
        if let StatsWindow::LastDays(days) = &self.window {
            let mut dates: Vec<&str> = buckets.iter().map(|b| b.date.as_str()).collect();
            dates.sort_unstable();
            dates.dedup();
            let keep: HashSet<String> = dates
                .into_iter()
                .rev()
                .take(*days as usize)
                .map(str::to_string)
                .collect();
            buckets.retain(|bucket| keep.contains(&bucket.date));
        }
        buckets.sort_by(|a, b| {
            (&a.date, a.mode.as_str(), &a.script).cmp(&(&b.date, b.mode.as_str(), &b.script))
        });
        for bucket in &mut buckets {
            bucket
                .counters
                .keywords
                .retain(|name, _| CANONICAL_KEYWORDS.contains(&name.as_str()));
        }
        let mut counters = StatsCounters::default();
        for bucket in &buckets {
            counters.merge(&bucket.counters);
        }
        StatsSnapshot {
            window: self.window,
            counters,
            buckets,
            sessions: self.sessions,
            runs: self.runs,
        }
    }
}

pub fn load_snapshot(
    window: StatsWindow,
    pending: Option<&UsageStats>,
    keep_sessions: bool,
) -> Result<StatsSnapshot, String> {
    let dir = ensure_stats_dir().ok_or_else(|| "stats state directory unavailable".to_string())?;
    // Reading needs no lock: runs and ops only ever append, the cache is
    // replaced atomically, and legacy documents are read as a validated
    // snapshot. The lock (`cache.lock`, never the old binaries' `stats.lock`)
    // only decides who may refresh the cache and sweep stale temp files; if
    // another report holds it, this one just reads.
    let lock = StatsLock::acquire(&dir, CACHE_LOCK, true, Duration::ZERO).ok();
    if lock.is_some() {
        remove_stale_temps(&dir);
    }
    load_snapshot_from(&dir, window, pending, keep_sessions, lock.is_some())
}

/// Everything recorded that falls in `window`: the legacy weekly documents
/// (which ops never touch), then every raw day with every `clear` and
/// `reset` applied (a day is skipped only when a validated cache shows its
/// dates miss the window), then the live `pending` collector. Buckets and
/// runs are filtered by their own dates. `may_cache` lets fresh reads of
/// days before today refresh the cache. Today is never cached.
fn load_snapshot_from(
    dir: &Path,
    window: StatsWindow,
    pending: Option<&UsageStats>,
    keep_sessions: bool,
    may_cache: bool,
) -> Result<StatsSnapshot, String> {
    let ops = load_ops(dir);
    let mut accumulator = Accumulator::new(window.clone(), keep_sessions);
    let (legacy, unstable) = legacy_contribution(dir, &window, may_cache, &mut |_| {});
    if unstable {
        eprintln!(
            "mix stats: legacy store busy: an older mix kept changing it during the report, so its share may be incomplete"
        );
    }
    accumulator.add(legacy);
    let today = today_string();
    for (day, path) in raw_days(dir)? {
        let cacheable = may_cache && day < today;
        accumulator.add(day_records(dir, &day, &path, &ops, &window, cacheable));
    }
    if let Some(pending) = pending {
        accumulator.add(pending.clone());
    }
    Ok(accumulator.finish())
}

pub fn cmd_stats_dispatch(args: &[&str], pending: Option<&mut UsageStats>) -> i32 {
    if !stats_enabled() {
        println!("Usage statistics — disabled (MIX_STATS)");
        return 0;
    }
    if args.first().copied() == Some("coverage") {
        let Some(dir) = args.get(1) else {
            eprintln!("mix stats coverage: requires a directory");
            return 2;
        };
        return crate::stats_coverage::run(Path::new(dir));
    }
    legacy_advisory();
    match dispatch_report(args, pending) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("mix stats: {error}");
            1
        }
    }
}

fn dispatch_report(args: &[&str], pending: Option<&mut UsageStats>) -> Result<(), String> {
    match args.first().copied() {
        Some("help") => {
            print_help();
            Ok(())
        }
        Some("reset") => reset_current(pending),
        Some("clear") => clear_name(args.get(1).copied(), pending),
        Some("query") => query_db(&args[1..].join(" ")),
        Some("trend") => {
            let name = args.get(1).ok_or("trend requires a name")?;
            let snapshot = load_snapshot(StatsWindow::LastDays(30), pending.as_deref(), false)?;
            print_trend(&snapshot, name);
            Ok(())
        }
        Some("since") => {
            let date = args.get(1).ok_or("since requires a YYYY-MM-DD date")?;
            if !is_canonical_date(date) {
                return Err(format!("invalid date {date:?}"));
            }
            print_overview(&load_snapshot(
                StatsWindow::Since((*date).to_string()),
                pending.as_deref(),
                false,
            )?);
            Ok(())
        }
        Some("week") => {
            let week = args.get(1).ok_or("week requires YYYY-WNN")?;
            if !is_canonical_week(week) {
                return Err(format!("invalid ISO week {week:?}"));
            }
            print_overview(&load_snapshot(
                StatsWindow::Week((*week).to_string()),
                pending.as_deref(),
                false,
            )?);
            Ok(())
        }
        Some("all") => {
            print_overview(&load_snapshot(
                StatsWindow::AllTime,
                pending.as_deref(),
                false,
            )?);
            Ok(())
        }
        command => {
            let keep_sessions = matches!(command, Some("sessions") | Some("raw"));
            let snapshot =
                load_snapshot(StatsWindow::CurrentWeek, pending.as_deref(), keep_sessions)?;
            match command {
                None | Some("overview") => print_overview(&snapshot),
                Some("builtins") => {
                    print_category(&snapshot, "builtins", &snapshot.counters.builtins)
                }
                Some("functions") => {
                    print_category(&snapshot, "functions", &snapshot.counters.functions)
                }
                Some("aliases") => print_category(&snapshot, "aliases", &snapshot.counters.aliases),
                Some("commands") => {
                    print_category(&snapshot, "commands", &snapshot.counters.commands)
                }
                Some("keywords") => {
                    print_category(&snapshot, "keywords", &snapshot.counters.keywords)
                }
                Some("meta") => print_category(&snapshot, "meta", &snapshot.counters.meta),
                Some("errors") => print_category(&snapshot, "errors", &snapshot.counters.errors),
                Some("sessions") => print_sessions(&snapshot),
                Some("never") => print_never(&snapshot),
                Some("raw") => print_raw(&snapshot)?,
                Some("modes") => print_modes(&snapshot),
                Some("scripts") => {
                    let top = args
                        .get(1)
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(10)
                        .clamp(1, 100);
                    print_scripts(&snapshot, top);
                }
                Some(other) => return Err(format!("unknown subcommand {other:?}")),
            }
            Ok(())
        }
    }
}

fn header(snapshot: &StatsSnapshot) {
    println!("Usage statistics — {}", snapshot.window.header());
}

fn print_overview(snapshot: &StatsSnapshot) {
    header(snapshot);
    let mut entries = Vec::new();
    for (category, map) in counter_maps(&snapshot.counters) {
        entries.extend(
            map.iter()
                .map(|(name, count)| (category, name.as_str(), *count)),
        );
    }
    entries.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(b.1)));
    println!("{} total events", snapshot.counters.events());
    for (category, name, count) in entries.into_iter().take(20) {
        println!("{name:<30} {category:<10} {count:>8}");
    }
    if !snapshot.buckets.is_empty() {
        println!();
        print_modes_body(snapshot);
        if snapshot.buckets.iter().any(|b| b.script.is_some()) {
            println!();
            print_scripts_body(snapshot, 5);
        }
    }
}

fn print_category(snapshot: &StatsSnapshot, label: &str, map: &HashMap<String, u64>) {
    header(snapshot);
    println!("{label}:");
    let mut items: Vec<_> = map.iter().collect();
    items.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (name, count) in items {
        println!("{name:<40} {count:>8}");
    }
}

fn print_sessions(snapshot: &StatsSnapshot) {
    header(snapshot);
    for run in &snapshot.sessions {
        println!(
            "{} {:<11} {:>6}s {:>6} commands {}",
            datetime_string(run.started),
            run.mode.as_str(),
            run.duration_secs,
            run.commands,
            run.script.as_deref().unwrap_or("")
        );
    }
}

fn print_never(snapshot: &StatsSnapshot) {
    header(snapshot);
    println!("Never-used builtins:");
    for name in BUILTIN_NAMES.iter().chain(HOF_NAMES.iter()) {
        if snapshot.counters.builtins.get(*name).copied().unwrap_or(0) == 0 {
            println!("  {name}");
        }
    }
    println!("Never-used keywords:");
    for name in CANONICAL_KEYWORDS {
        if snapshot.counters.keywords.get(*name).copied().unwrap_or(0) == 0 {
            println!("  {name}");
        }
    }
}

fn print_modes(snapshot: &StatsSnapshot) {
    header(snapshot);
    print_modes_body(snapshot);
}

fn print_modes_body(snapshot: &StatsSnapshot) {
    #[derive(Default)]
    struct Row {
        events: u64,
        builtins: u64,
        keywords: u64,
        functions: u64,
        commands: u64,
        errors: u64,
        runs: u64,
    }
    let mut rows: BTreeMap<&str, Row> = BTreeMap::new();
    for bucket in &snapshot.buckets {
        let row = rows.entry(bucket.mode.as_str()).or_default();
        row.events = row.events.saturating_add(bucket.counters.events());
        row.builtins = row
            .builtins
            .saturating_add(saturating_sum(bucket.counters.builtins.values()));
        row.keywords = row
            .keywords
            .saturating_add(saturating_sum(bucket.counters.keywords.values()));
        row.functions = row
            .functions
            .saturating_add(saturating_sum(bucket.counters.functions.values()));
        row.commands = row
            .commands
            .saturating_add(saturating_sum(bucket.counters.commands.values()));
        row.errors = row
            .errors
            .saturating_add(saturating_sum(bucket.counters.errors.values()));
    }
    for ((mode, _), runs) in &snapshot.runs {
        let row = rows.entry(mode.as_str()).or_default();
        row.runs = row.runs.saturating_add(*runs);
    }
    println!("Mode          Events Builtins Keywords Functions Commands Errors Runs");
    for (mode, row) in rows {
        println!(
            "{mode:<12} {:>6} {:>8} {:>8} {:>9} {:>8} {:>6} {:>4}",
            row.events,
            row.builtins,
            row.keywords,
            row.functions,
            row.commands,
            row.errors,
            row.runs
        );
    }
}

fn print_scripts(snapshot: &StatsSnapshot, top: usize) {
    header(snapshot);
    print_scripts_body(snapshot, top);
}

fn print_scripts_body(snapshot: &StatsSnapshot, top: usize) {
    let mut rows: HashMap<(String, ExecutionMode), (u64, u64, u64, u64)> = HashMap::new();
    for bucket in &snapshot.buckets {
        let Some(script) = &bucket.script else {
            continue;
        };
        let row = rows.entry((script.clone(), bucket.mode)).or_default();
        row.0 = row.0.saturating_add(bucket.counters.events());
        row.1 = row
            .1
            .saturating_add(saturating_sum(bucket.counters.builtins.values()));
        row.2 = row
            .2
            .saturating_add(saturating_sum(bucket.counters.keywords.values()));
    }
    for ((mode, script), runs) in &snapshot.runs {
        if let Some(script) = script {
            let row = rows.entry((script.clone(), *mode)).or_default();
            row.3 = row.3.saturating_add(*runs);
        }
    }
    let mut rows: Vec<_> = rows.into_iter().collect();
    rows.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.0.cmp(&b.0.0)));
    println!("Script                         Mode        Events Builtins Keywords Runs");
    for ((script, mode), (events, builtins, keywords, runs)) in rows.into_iter().take(top) {
        println!(
            "{script:<30} {:<11} {events:>6} {builtins:>8} {keywords:>8} {runs:>4}",
            mode.as_str()
        );
    }
}

fn print_raw(snapshot: &StatsSnapshot) -> Result<(), String> {
    header(snapshot);
    println!(
        "{}",
        serde_json::to_string_pretty(&snapshot.usage().to_json()).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn print_trend(snapshot: &StatsSnapshot, name: &str) {
    header(snapshot);
    println!("Trend for {name:?}:");
    let mut days: BTreeMap<&str, u64> = BTreeMap::new();
    for bucket in &snapshot.buckets {
        let count = counter_maps(&bucket.counters)
            .iter()
            .fold(0_u64, |total, (_, map)| {
                total.saturating_add(map.get(name).copied().unwrap_or(0))
            });
        let value = days.entry(&bucket.date).or_default();
        *value = value.saturating_add(count);
    }
    for (date, count) in days {
        println!("{date} {count}");
    }
}

fn query_db(sql: &str) -> Result<(), String> {
    if sql.trim().is_empty() {
        return Err("query requires SQL".to_string());
    }
    // The store is JSONL; the query runs over the old mix.db tables, built in
    // memory from everything recorded.
    let snapshot = load_snapshot(StatsWindow::AllTime, None, true)?;
    let mut stats = empty_doc("");
    stats.buckets = snapshot.buckets;
    stats.sessions = snapshot.sessions;
    let mut connection = Connection::open_in_memory().map_err(|e| e.to_string())?;
    load_sqlite(&mut connection, &stats).map_err(|e| e.to_string())?;
    println!("Usage statistics — all-time SQLite query");
    let mut statement = connection.prepare(sql).map_err(|e| e.to_string())?;
    let columns = statement.column_count();
    let names: Vec<String> = statement
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    println!("{}", names.join("\t"));
    let rows = statement
        .query_map([], |row| {
            let mut values = Vec::new();
            for index in 0..columns {
                let value = row.get_ref(index)?;
                values.push(match value {
                    rusqlite::types::ValueRef::Null => "NULL".to_string(),
                    rusqlite::types::ValueRef::Integer(v) => v.to_string(),
                    rusqlite::types::ValueRef::Real(v) => v.to_string(),
                    rusqlite::types::ValueRef::Text(v) => String::from_utf8_lossy(v).into_owned(),
                    rusqlite::types::ValueRef::Blob(v) => format!("<{} bytes>", v.len()),
                });
            }
            Ok(values)
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        println!("{}", row.map_err(|e| e.to_string())?.join("\t"));
    }
    Ok(())
}

/// `reset`: an appended op that drops the current ISO week's buckets and
/// runs from everything stored before it. Nothing stored is rewritten.
fn reset_current(pending: Option<&mut UsageStats>) -> Result<(), String> {
    let dir = ensure_stats_dir().ok_or("stats state directory unavailable")?;
    let week = current_iso_week();
    append_op(&dir, OpKind::Reset(week.clone()))?;
    if let Some(pending) = pending {
        pending.reset_current_week();
    }
    println!("Stats reset for week {week}.");
    Ok(())
}

/// `clear NAME`: an appended op that removes `name` from every counter
/// category of everything stored before it.
fn clear_name(name: Option<&str>, pending: Option<&mut UsageStats>) -> Result<(), String> {
    let name = name.ok_or("clear requires a name")?;
    let dir = ensure_stats_dir().ok_or("stats state directory unavailable")?;
    let recorded = load_snapshot_from(&dir, StatsWindow::AllTime, None, false, false)?;
    let mut changed = counter_maps(&recorded.counters)
        .iter()
        .any(|(_, map)| map.contains_key(name));
    append_op(&dir, OpKind::Clear(name.to_string()))?;
    if let Some(pending) = pending {
        changed |= clear_name_from_stats(pending, name);
    }
    println!(
        "{} {name:?}.",
        if changed { "Cleared" } else { "Not found" }
    );
    Ok(())
}

fn clear_name_from_stats(stats: &mut UsageStats, name: &str) -> bool {
    let mut changed = false;
    for bucket in &mut stats.buckets {
        for (_, map) in counter_maps_mut(&mut bucket.counters) {
            changed |= map.remove(name).is_some();
        }
    }
    stats.rebuild_aggregates();
    changed
}

fn counter_maps_mut(c: &mut StatsCounters) -> [(&'static str, &mut HashMap<String, u64>); 7] {
    [
        ("builtin", &mut c.builtins),
        ("function", &mut c.functions),
        ("alias", &mut c.aliases),
        ("command", &mut c.commands),
        ("keyword", &mut c.keywords),
        ("meta", &mut c.meta),
        ("error", &mut c.errors),
    ]
}

fn print_help() {
    println!("mix stats subcommands:");
    println!("  overview | builtins | functions | aliases | commands | keywords | meta | errors");
    println!("  sessions | never | modes | scripts [N] | raw");
    println!("  week YYYY-WNN | since YYYY-MM-DD | all | trend NAME");
    println!("  coverage DIR | query SQL | clear NAME | reset");
}

#[cfg(test)]
mod tests {
    use super::*;
    use mix::stats::{ExecutionMode, StatsContext, current_epoch_day};

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mix-stats-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn bucket(date: &str, name: &str, count: u64) -> StatsBucket {
        StatsBucket {
            date: date.to_string(),
            mode: ExecutionMode::C,
            script: None,
            counters: StatsCounters {
                builtins: HashMap::from([(name.to_string(), count)]),
                ..StatsCounters::default()
            },
        }
    }

    fn session(id: &str, day: &str) -> SessionRecord {
        let epoch_day = (0..)
            .find(|d| date_for_epoch_day(*d) == day)
            .expect("test dates are after 1970");
        SessionRecord {
            id: id.to_string(),
            started: epoch_day * 86_400 + 3_600,
            duration_secs: 1,
            commands: 1,
            peak_memory_kb: 0,
            mode: ExecutionMode::C,
            script: None,
        }
    }

    /// A finished run dated `day` that used `name` once.
    fn run_on(id: &str, day: &str, name: &str) -> UsageStats {
        let mut run = empty_doc("");
        run.buckets.push(bucket(day, name, 1));
        run.sessions.push(session(id, day));
        run.rebuild_aggregates();
        run
    }

    fn today_run(name: &str) -> UsageStats {
        let mut delta = UsageStats::for_execution(StatsContext::new(ExecutionMode::C, None));
        delta.track_builtin(name);
        delta.finalize_session();
        delta
    }

    fn days_ago(days: u64) -> String {
        date_for_epoch_day(current_epoch_day() - days)
    }

    fn all(dir: &Path) -> StatsSnapshot {
        load_snapshot_from(dir, StatsWindow::AllTime, None, true, false).unwrap()
    }

    /// An AllTime read that may refresh the cache, as a report holding the lock does.
    fn all_caching(dir: &Path) -> StatsSnapshot {
        load_snapshot_from(dir, StatsWindow::AllTime, None, true, true).unwrap()
    }

    fn len_count(snapshot: &StatsSnapshot) -> u64 {
        snapshot.counters.builtins.get("len").copied().unwrap_or(0)
    }

    fn record(dir: &Path, delta: &UsageStats, day: &str) {
        flush_run(dir, delta, day, &mut write_once);
    }

    /// Every file under `dir` with its length and modification time.
    fn tree(dir: &Path) -> BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
        let mut files = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(path) = stack.pop() {
            for entry in fs::read_dir(&path).unwrap().filter_map(Result::ok) {
                let meta = entry.metadata().unwrap();
                if meta.is_dir() {
                    stack.push(entry.path());
                } else {
                    files.insert(entry.path(), (meta.len(), meta.modified().unwrap()));
                }
            }
        }
        files
    }

    #[test]
    fn kill_switch_values_are_case_insensitive() {
        for value in ["off", "OFF", " false ", "FALSE", "0"] {
            assert!(!stats_setting_enabled(Some(value)), "{value}");
        }
        for value in [None, Some(""), Some("on"), Some("1"), Some("no")] {
            assert!(stats_setting_enabled(value));
        }
    }

    #[test]
    fn current_window_keeps_overview_and_never_on_same_counters() {
        let mut pending = UsageStats::for_execution(StatsContext::new(ExecutionMode::C, None));
        pending.track_keyword("if");
        let dir = temp_dir("window");
        let snapshot =
            load_snapshot_from(&dir, StatsWindow::CurrentWeek, Some(&pending), false, false)
                .unwrap();
        assert_eq!(snapshot.counters.keywords.get("if"), Some(&1));
        assert!(
            !CANONICAL_KEYWORDS
                .iter()
                .filter(|name| snapshot.counters.keywords.get(**name).copied().unwrap_or(0) == 0)
                .any(|name| *name == "if")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The exit path appends and does nothing else: with a large history
    /// (many raw days, caches, legacy documents, stale temps) and the lock
    /// held by someone else, it touches no file but today's and is fast.
    #[test]
    fn the_exit_path_only_appends_with_a_large_history_and_a_held_lock() {
        let dir = temp_dir("exit-path");
        for days in 2..120 {
            let day = days_ago(days);
            for index in 0..20 {
                record(&dir, &run_on(&format!("{day}-{index}"), &day, "len"), &day);
            }
        }
        all_caching(&dir);
        let legacy = run_on("legacy", "2026-01-05", "len");
        fs::write(dir.join("current.json"), legacy.to_json().to_string()).unwrap();
        fs::write(dir.join(".current.json.1.0.tmp"), "x").unwrap();
        let today = today_string();
        record(&dir, &today_run("len"), &today);
        let today_file = run_path(&dir, &today);
        let _cache = StatsLock::acquire(&dir, CACHE_LOCK, true, Duration::ZERO).unwrap();
        let _ops = StatsLock::acquire(&dir, OPS_LOCK, true, Duration::ZERO).unwrap();
        let before = tree(&dir);

        let started = Instant::now();
        record(&dir, &today_run("len"), &today);
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(50),
            "exit path took {elapsed:?}"
        );
        let mut after = tree(&dir);
        let grown = after.remove(&today_file).unwrap();
        let mut expected = before;
        let was = expected.remove(&today_file).unwrap();
        assert_eq!(after, expected, "the exit path touched another file");
        assert!(grown.0 > was.0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_short_write_is_dropped_and_never_retried() {
        let dir = temp_dir("short");
        let day = today_string();
        let mut calls = 0;
        let mut short = |file: &mut File, bytes: &[u8]| {
            calls += 1;
            file.write(&bytes[..bytes.len() / 2])
        };
        flush_run(&dir, &today_run("len"), &day, &mut short);
        assert_eq!(calls, 1);
        let mut calls = 0;
        let mut short = |file: &mut File, bytes: &[u8]| {
            calls += 1;
            file.write(&bytes[..bytes.len() / 2])
        };
        let mut pending = empty_doc("");
        pending.buckets.push(bucket(&day, "len", 5));
        assert!(flush_pending_to(&dir, &pending, &mut short).is_ok());
        assert_eq!(calls, 1, "a short mid-session write is final");
        record(&dir, &today_run("len"), &day);
        let snapshot = all(&dir);
        assert_eq!(len_count(&snapshot), 1, "both torn records are skipped");
        assert_eq!(snapshot.sessions.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_complete_record_missing_only_its_newline_counts_once_and_the_next_survives() {
        let dir = temp_dir("no-newline");
        let day = today_string();
        let mut calls = 0;
        let mut all_but_newline = |file: &mut File, bytes: &[u8]| {
            calls += 1;
            file.write(&bytes[..bytes.len() - 1])
        };
        let mut pending = empty_doc("");
        pending.buckets.push(bucket(&day, "len", 5));
        assert!(flush_pending_to(&dir, &pending, &mut all_but_newline).is_ok());
        assert_eq!(calls, 1, "not retried, so it cannot land twice");
        record(&dir, &today_run("len"), &day);
        let snapshot = all(&dir);
        assert_eq!(len_count(&snapshot), 6);
        assert_eq!(snapshot.sessions.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_retried_mid_session_flush_counts_once_by_batch() {
        let dir = temp_dir("retry");
        let day = today_string();
        let mut pending = empty_doc("");
        pending.buckets.push(bucket(&day, "len", 3));
        // The first attempt lands but reports failure; the retry repeats the
        // same bytes, so the batch id is the same and readers keep one copy.
        let mut calls = 0;
        let mut landed_then_failed = |file: &mut File, bytes: &[u8]| {
            calls += 1;
            let written = file.write(bytes)?;
            if calls == 1 {
                return Err(std::io::Error::other("reported failure after landing"));
            }
            Ok(written)
        };
        assert!(flush_pending_to(&dir, &pending, &mut landed_then_failed).is_ok());
        assert_eq!(calls, 2);
        let raw = fs::read_to_string(run_path(&dir, &day)).unwrap();
        assert_eq!(raw.lines().filter(|line| !line.is_empty()).count(), 2);
        assert_eq!(len_count(&all(&dir)), 3);

        // A clean failure every time writes nothing and hands the delta back.
        let mut failing = |_: &mut File, _: &[u8]| -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(libc::ENOSPC))
        };
        assert!(flush_pending_to(&dir, &pending, &mut failing).is_err());
        assert_eq!(len_count(&all(&dir)), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parallel_runs_all_land() {
        let dir = temp_dir("parallel");
        let workers: Vec<_> = (0..64)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || record(&dir, &today_run("len"), &today_string()))
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let snapshot = all(&dir);
        assert_eq!(len_count(&snapshot), 64);
        assert_eq!(snapshot.sessions.len(), 64);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The offset just past each record's closing brace in a raw file, found
    /// by scanning bytes (independent of the production payload code).
    fn closing_braces(path: &Path) -> Vec<u64> {
        let raw = fs::read(path).unwrap();
        let mut ends = Vec::new();
        let mut offset = 0;
        for line in raw.split(|byte| *byte == b'\n') {
            if let Some(last) = line.iter().rposition(|byte| *byte == b'}') {
                ends.push((offset + last + 1) as u64);
            }
            offset += line.len() + 1;
        }
        ends
    }

    /// A raw-day cache in the format 581b28e wrote: no payload range.
    #[test]
    fn a_cache_without_payload_bounds_is_reread_so_ops_apply() {
        let dir = temp_dir("old-cache");
        let day = today_string();
        record(&dir, &today_run("len"), &day);
        record(&dir, &today_run("len"), &day);
        let path = run_path(&dir, &day);
        let id = FileId::of(&fs::metadata(&path).unwrap());
        let mut aggregate = empty_doc("");
        for _ in 0..2 {
            aggregate.buckets.push(bucket(&day, "len", 1));
        }
        aggregate.sessions.push(session("cached-1", &day));
        aggregate.sessions.push(session("cached-2", &day));
        let old_format = format!(
            "{}\n{}\n",
            serde_json::json!({"day": day, "id": id.to_json(), "dates": [day, day]}),
            record_json(&aggregate)
        );
        write_cache_value(&dir, &day, &old_format);

        append_op(&dir, OpKind::Clear("len".into())).unwrap();
        // By construction: both runs were stored before the clear.
        let snapshot = all(&dir);
        assert_eq!(len_count(&snapshot), 0);
        assert_eq!(snapshot.sessions.len(), 2, "a clear keeps the runs");

        append_op(&dir, OpKind::Reset(current_iso_week())).unwrap();
        // By construction: both runs were stored this week, before the reset.
        let snapshot = all(&dir);
        assert!(snapshot.counters.builtins.is_empty());
        assert!(snapshot.sessions.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    fn write_cache_value(dir: &Path, day: &str, value: &str) {
        fs::create_dir_all(dir.join(CACHE_DIR)).unwrap();
        fs::write(cache_path(dir, day), value).unwrap();
    }

    #[test]
    fn the_cache_is_used_only_while_it_matches_the_raw_file() {
        let dir = temp_dir("cache");
        let day = days_ago(5);
        record(&dir, &run_on("a", &day, "len"), &day);
        record(&dir, &run_on("b", &day, "len"), &day);
        assert_eq!(len_count(&all_caching(&dir)), 2);
        assert!(cache_path(&dir, &day).exists());

        // A forged entry that names the raw file exactly is believed, which
        // shows reports really read the cache...
        let id = FileId::of(&fs::metadata(run_path(&dir, &day)).unwrap());
        let ends = closing_braces(&run_path(&dir, &day));
        let header = serde_json::json!({
            "day": day, "id": id.to_json(), "dates": [day, day],
            "payload": [ends[0], ends[ends.len() - 1]],
        });
        let forged = format!(
            "{header}\n{}\n",
            record_json(&run_on("forged", &day, "upper"))
        );
        write_cache_value(&dir, &day, &forged);
        assert_eq!(all(&dir).counters.builtins.get("upper"), Some(&1));

        // ...and corrupt, prefix-only or header-only entries are ignored.
        let header_only = format!("{header}\n");
        for value in ["{broken", &format!("{{\"day\":\"{day}\""), &header_only] {
            write_cache_value(&dir, &day, value);
            let snapshot = all(&dir);
            assert_eq!(len_count(&snapshot), 2, "{value}");
            assert_eq!(snapshot.sessions.len(), 2);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_late_append_after_caching_makes_the_entry_stale() {
        let dir = temp_dir("late");
        let day = days_ago(3);
        record(&dir, &run_on("a", &day, "len"), &day);
        all_caching(&dir);
        record(&dir, &run_on("b", &day, "len"), &day);
        let snapshot = all_caching(&dir);
        assert_eq!(len_count(&snapshot), 2);
        assert_eq!(snapshot.sessions.len(), 2);
        assert_eq!(len_count(&all(&dir)), 2, "the refreshed entry is exact");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clock_rolled_back_onto_a_cached_day_still_counts() {
        let dir = temp_dir("rollback");
        let day = days_ago(9);
        record(&dir, &run_on("a", &day, "len"), &day);
        all_caching(&dir);
        // A run whose clock went back writes today's record into the old day.
        record(&dir, &today_run("len"), &day);
        let snapshot = all(&dir);
        assert_eq!(len_count(&snapshot), 2);
        assert_eq!(snapshot.sessions.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cache_entry_without_a_readable_raw_file_counts_nothing() {
        let dir = temp_dir("orphan");
        let missing = days_ago(4);
        let stats = record_json(&run_on("ghost", &missing, "len"));
        write_cache_value(
            &dir,
            &missing,
            &serde_json::json!({"day": missing, "dev": 0, "ino": 0, "len": 0,
                "min_at": 0, "max_at": 0, "stats": stats})
            .to_string(),
        );
        // Unreadable even as root: the raw path is a directory.
        let unreadable = days_ago(6);
        fs::create_dir_all(run_path(&dir, &unreadable)).unwrap();
        let snapshot = all_caching(&dir);
        assert_eq!(len_count(&snapshot), 0);
        assert!(snapshot.sessions.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_concurrent_with_appenders_loses_nothing_and_applies_only_before_it() {
        let dir = temp_dir("clear-race");
        let day = today_string();
        let workers: Vec<_> = (0..64)
            .map(|index| {
                let dir = dir.clone();
                let day = day.clone();
                std::thread::spawn(move || {
                    if index % 2 == 0 {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    record(&dir, &today_run("len"), &day);
                })
            })
            .collect();
        std::thread::sleep(Duration::from_millis(2));
        // Two op writers race the appenders and each other.
        let clears: Vec<_> = (0..2)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || append_op(&dir, OpKind::Clear("len".into())))
            })
            .collect();
        for clear in clears {
            clear.join().unwrap().unwrap();
        }
        for worker in workers {
            worker.join().unwrap();
        }
        // The expectation, computed without the production cutoff, payload
        // or collapse code: the covered prefix is the largest length either
        // op recorded for today's file, and a record keeps `len` exactly when
        // its closing brace lies beyond it.
        let ops = fs::read_to_string(dir.join(OPS_FILE)).unwrap();
        let recorded: Vec<serde_json::Value> = ops
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(recorded.len(), 2, "both clears recorded whole");
        let covered = recorded
            .iter()
            .filter_map(|op| op["positions"][day.as_str()][2].as_u64())
            .max()
            .unwrap_or(0);
        let raw = fs::read(run_path(&dir, &day)).unwrap();
        let mut kept = 0;
        let mut offset = 0;
        for line in raw.split(|byte| *byte == b'\n') {
            let start = offset;
            offset += line.len() + 1;
            if let Some(last) = line.iter().rposition(|byte| *byte == b'}') {
                let closing = (start + last + 1) as u64;
                if closing > covered {
                    kept += 1;
                }
            }
        }
        let snapshot = all(&dir);
        assert_eq!(snapshot.sessions.len(), 64, "every run survives");
        assert_eq!(
            len_count(&snapshot),
            kept,
            "only runs stored after both clears keep it"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ops_apply_to_cached_days_and_split_a_day_by_position() {
        let dir = temp_dir("ops");
        let old = days_ago(3);
        record(&dir, &run_on("a", &old, "len"), &old);
        all_caching(&dir);
        append_op(&dir, OpKind::Clear("len".into())).unwrap();
        // The op covers the whole cached file, so it applies to the entry.
        assert_eq!(len_count(&all_caching(&dir)), 0);

        // A day whose records straddle an op is read record by record.
        let split = days_ago(4);
        record(&dir, &run_on("b", &split, "upper"), &split);
        append_op(&dir, OpKind::Clear("upper".into())).unwrap();
        record(&dir, &run_on("c", &split, "upper"), &split);
        all_caching(&dir);
        let snapshot = all_caching(&dir);
        assert_eq!(snapshot.counters.builtins.get("upper"), Some(&1));
        assert_eq!(snapshot.sessions.len(), 3, "a clear keeps the runs");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reset_drops_the_current_week_before_it_and_keeps_older_days_and_pending_buckets() {
        let dir = temp_dir("reset");
        let old = "2026-01-05";
        record(&dir, &run_on("old", old, "len"), old);
        record(&dir, &today_run("upper"), &today_string());
        append_op(&dir, OpKind::Reset(current_iso_week())).unwrap();
        record(&dir, &today_run("join"), &today_string());
        let mut pending = UsageStats::new();
        pending.track_builtin("trim");
        pending.buckets.push(bucket(old, "trim", 1));
        pending.rebuild_aggregates();
        pending.reset_current_week();

        let snapshot = all(&dir);
        assert_eq!(len_count(&snapshot), 1);
        assert_eq!(snapshot.counters.builtins.get("upper"), None);
        assert_eq!(snapshot.counters.builtins.get("join"), Some(&1));
        assert_eq!(pending.buckets.len(), 1);
        assert_eq!(pending.buckets[0].date, old);
        let _ = fs::remove_dir_all(&dir);
    }

    fn weekly_doc(week: &str, runs: &[UsageStats]) -> UsageStats {
        let mut merger = Merger::default();
        for run in runs {
            merger.add(run.clone());
        }
        let mut doc = merger.finish();
        doc.week = week.to_string();
        doc.last_date = doc
            .buckets
            .iter()
            .map(|b| b.date.clone())
            .max()
            .unwrap_or_default();
        doc
    }

    #[test]
    fn an_interrupted_old_rotation_is_counted_once() {
        let dir = temp_dir("rotation");
        // The old store rotated a 2026-W02 current.json into 2026-W02.json,
        // recording its fingerprint, then died before removing current.json.
        let current_text = weekly_doc("2026-W02", &[run_on("a", "2026-01-06", "len")])
            .to_json()
            .to_string();
        let mut archive = weekly_doc(
            "2026-W02",
            &[
                run_on("a", "2026-01-06", "len"),
                run_on("z", "2026-01-07", "len"),
            ],
        );
        // The old store fingerprinted the document as it read it back.
        let reread = UsageStats::from_json(serde_json::from_str(&current_text).unwrap());
        archive
            .rotation_sources
            .push(rotation_fingerprint(&reread).unwrap());
        fs::write(dir.join("current.json"), &current_text).unwrap();
        fs::write(dir.join("2026-W02.json"), archive.to_json().to_string()).unwrap();
        let before = tree(&dir);

        let snapshot = all(&dir);
        assert_eq!(len_count(&snapshot), 2);
        assert_eq!(snapshot.sessions.len(), 2);
        assert_eq!(tree(&dir), before, "legacy documents are read in place");

        // Without the fingerprint the rotation never happened: both count.
        archive.rotation_sources.clear();
        fs::write(dir.join("2026-W02.json"), archive.to_json().to_string()).unwrap();
        assert_eq!(len_count(&all(&dir)), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_binary_writing_after_the_new_store_ran_stays_visible() {
        let dir = temp_dir("mixed-fleet");
        record(&dir, &today_run("len"), &today_string());
        let week = current_iso_week();
        let today = today_string();
        let first = weekly_doc(&week, &[run_on("old-1", &today, "upper")]);
        fs::write(dir.join("current.json"), first.to_json().to_string()).unwrap();
        assert_eq!(all_caching(&dir).counters.builtins.get("upper"), Some(&1));
        // The old binary keeps recording into its own document.
        let second = weekly_doc(
            &week,
            &[
                run_on("old-1", &today, "upper"),
                run_on("old-2", &today, "upper"),
            ],
        );
        fs::write(dir.join("current.json"), second.to_json().to_string()).unwrap();
        let snapshot = all(&dir);
        assert_eq!(snapshot.counters.builtins.get("upper"), Some(&2));
        assert_eq!(len_count(&snapshot), 1);
        assert_eq!(snapshot.sessions.len(), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn query_runs_sql_over_the_old_schema_loaded_in_memory() {
        let dir = temp_dir("query");
        let old = days_ago(9);
        record(&dir, &run_on("a", &old, "len"), &old);
        record(&dir, &run_on("b", &old, "len"), &old);
        record(&dir, &today_run("len"), &today_string());
        let mut saturated = empty_doc("");
        saturated.buckets.push(bucket(&old, "upper", u64::MAX));
        let mut run = session("saturated", &old);
        run.duration_secs = u64::MAX;
        saturated.sessions.push(run);
        record(&dir, &saturated, &old);

        let snapshot = all(&dir);
        let mut stats = empty_doc("");
        stats.buckets = snapshot.buckets;
        stats.sessions = snapshot.sessions;
        let mut connection = Connection::open_in_memory().unwrap();
        load_sqlite(&mut connection, &stats).unwrap();
        let row: (i64, i64, i64, i64) = connection
            .query_row(
                "SELECT (SELECT SUM(count) FROM usage WHERE name = 'len'),
                        (SELECT COUNT(*) FROM runs),
                        (SELECT count FROM usage WHERE name = 'upper'),
                        (SELECT duration_secs FROM runs WHERE id = 'saturated')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (3, 4, i64::MAX, i64::MAX));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_temps_go_only_when_a_report_sweeps() {
        let dir = temp_dir("temps");
        let stale = dir.join(".current.json.1.0.tmp");
        let fresh = dir.join(".current.json.2.0.tmp");
        for path in [&stale, &fresh] {
            fs::write(path, "x").unwrap();
        }
        File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - STALE_TEMP_AGE * 2)
            .unwrap();
        record(&dir, &today_run("len"), &today_string());
        assert!(stale.exists(), "the exit path never sweeps");
        remove_stale_temps(&dir);
        assert!(!stale.exists());
        assert!(fresh.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// N1: attempts that land but report an error (an EIO after a failed
    /// commit) must never be handed back for a merge, or the next flush would
    /// write them again under a new batch id.
    #[test]
    fn uncertain_failures_after_landing_are_dropped_not_handed_back() {
        let dir = temp_dir("uncertain");
        let day = today_string();
        let mut pending = empty_doc("");
        pending.buckets.push(bucket(&day, "len", 4));
        let mut calls = 0;
        let mut landed_then_eio = |file: &mut File, bytes: &[u8]| -> std::io::Result<usize> {
            calls += 1;
            file.write_all(bytes)?;
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        };
        assert!(
            flush_pending_to(&dir, &pending, &mut landed_then_eio).is_ok(),
            "an uncertain outcome is not mergeable"
        );
        assert_eq!(calls, PENDING_FLUSH_ATTEMPTS);
        assert_eq!(len_count(&all(&dir)), 4, "every landed copy is one batch");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A record with a chosen write time, framed and appended to `day`.
    fn append_record_at(dir: &Path, day: &str, stats: &UsageStats, at: u64) {
        let mut value = record_json(stats);
        let object = value.as_object_mut().unwrap();
        object.insert("batch".into(), random_id().into());
        object.insert("at".into(), at.into());
        fs::create_dir_all(dir.join(RUNS_DIR)).unwrap();
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(run_path(dir, day))
            .unwrap()
            .write_all(&frame(&value).unwrap())
            .unwrap();
    }

    /// N2: an op covers what was stored before it, whatever the clocks said.
    #[test]
    fn ops_follow_storage_position_not_the_clock() {
        let dir = temp_dir("position");
        let day = today_string();
        // Written before the clear by a process whose clock ran ahead...
        append_record_at(&dir, &day, &run_on("ahead", &day, "len"), u64::MAX / 2);
        append_op(&dir, OpKind::Clear("len".into())).unwrap();
        // ...and after it by one whose clock was rolled back.
        append_record_at(&dir, &day, &run_on("behind", &day, "len"), 1);
        let snapshot = all(&dir);
        assert_eq!(
            len_count(&snapshot),
            1,
            "only the record stored after the clear keeps len"
        );
        assert_eq!(snapshot.sessions.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Ops cover only the JSONL store; while a legacy document exists they
    /// refuse and record nothing.
    #[test]
    fn clear_and_reset_refuse_while_legacy_documents_exist() {
        let dir = temp_dir("refuse");
        record(&dir, &today_run("len"), &today_string());
        let legacy = weekly_doc("2026-W02", &[run_on("a", "2026-01-06", "len")]);
        for name in ["current.json", "2026-W02.json"] {
            let path = dir.join(name);
            fs::write(&path, legacy.to_json().to_string()).unwrap();
            for kind in [
                OpKind::Clear("len".into()),
                OpKind::Reset(current_iso_week()),
            ] {
                let error = append_op(&dir, kind).unwrap_err();
                assert!(error.contains("old weekly stats"), "{error}");
            }
            assert!(!dir.join(OPS_FILE).exists(), "nothing recorded ({name})");
            fs::remove_file(&path).unwrap();
        }
        append_op(&dir, OpKind::Clear("len".into())).unwrap();
        assert_eq!(len_count(&all(&dir)), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Finding 2: a record that lost only its final newline ends at its
    /// closing brace, so a clear issued then covers it, and the newline the
    /// next run supplies does not uncover it, raw or cached.
    #[test]
    fn a_record_cleared_after_a_short_write_stays_cleared() {
        let dir = temp_dir("short-then-clear");
        let mut all_but_newline =
            |file: &mut File, bytes: &[u8]| file.write(&bytes[..bytes.len() - 1]);
        let today = today_string();
        flush_run(
            &dir,
            &run_on("a", &today, "len"),
            &today,
            &mut all_but_newline,
        );
        append_op(&dir, OpKind::Clear("len".into())).unwrap();
        record(&dir, &run_on("b", &today, "len"), &today);
        // By construction: run a was stored before the clear, run b after.
        let snapshot = all(&dir);
        assert_eq!(len_count(&snapshot), 1);
        assert_eq!(snapshot.sessions.len(), 2);

        let past = days_ago(2);
        flush_run(
            &dir,
            &run_on("c", &past, "upper"),
            &past,
            &mut all_but_newline,
        );
        append_op(&dir, OpKind::Clear("upper".into())).unwrap();
        record(&dir, &run_on("d", &past, "upper"), &past);
        for _ in 0..2 {
            let snapshot = all_caching(&dir);
            assert_eq!(
                snapshot.counters.builtins.get("upper"),
                Some(&1),
                "run d only"
            );
        }
        assert!(cache_path(&dir, &past).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Finding 3: an archive an old binary creates after the snapshot
    /// validated was never captured, so it cannot hide current.json.
    #[test]
    fn an_archive_created_after_the_snapshot_cannot_hide_current() {
        let dir = temp_dir("late-archive");
        let current_text = weekly_doc("2026-W02", &[run_on("a", "2026-01-06", "len")])
            .to_json()
            .to_string();
        fs::write(dir.join("current.json"), &current_text).unwrap();
        let reread = UsageStats::from_json(serde_json::from_str(&current_text).unwrap());
        let mut archive = weekly_doc("2026-W02", &[run_on("a", "2026-01-06", "len")]);
        archive
            .rotation_sources
            .push(rotation_fingerprint(&reread).unwrap());
        let archive_text = archive.to_json().to_string();
        let archive_path = dir.join("2026-W02.json");
        let mut late_rotation = |phase: LegacyPhase| {
            if phase == LegacyPhase::Validated && !archive_path.exists() {
                fs::write(&archive_path, &archive_text).unwrap();
            }
        };
        let (legacy, unstable) =
            legacy_contribution(&dir, &StatsWindow::AllTime, false, &mut late_rotation);
        assert!(!unstable);
        // By construction: the snapshot held current.json alone, one run of len.
        assert_eq!(legacy.builtins.get("len"), Some(&1));
        assert_eq!(legacy.sessions.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Concurrent clears and resets take turns under ops.lock, and refuse
    /// once the wait runs out.
    #[test]
    fn ops_wait_for_ops_lock_and_refuse_when_it_stays_busy() {
        let dir = temp_dir("ops-lock");
        record(&dir, &today_run("len"), &today_string());
        let held = StatsLock::acquire(&dir, OPS_LOCK, true, Duration::ZERO).unwrap();
        let waiting = {
            let dir = dir.clone();
            std::thread::spawn(move || append_op(&dir, OpKind::Clear("len".into())))
        };
        std::thread::sleep(Duration::from_millis(300));
        assert!(!dir.join(OPS_FILE).exists(), "the clear waits for the lock");
        drop(held);
        waiting.join().unwrap().unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(OPS_FILE))
                .unwrap()
                .lines()
                .filter(|l| !l.is_empty())
                .count(),
            1
        );

        let _held = StatsLock::acquire(&dir, OPS_LOCK, true, Duration::ZERO).unwrap();
        let error = append_op(&dir, OpKind::Clear("len".into())).unwrap_err();
        assert!(error.contains("ops.lock busy"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// N4: a raw file can hold buckets dated another day (the clock moved
    /// back mid-process); windows filter by each bucket's own date.
    #[test]
    fn a_raw_file_holding_another_weeks_buckets_is_filtered_by_bucket_date() {
        let dir = temp_dir("bucket-dates");
        // Written into 2026-01-05 (week 2) but dated 2026-01-12 (week 3).
        record(&dir, &run_on("moved", "2026-01-12", "len"), "2026-01-05");
        for caching in [true, true, false] {
            let week3 = load_snapshot_from(
                &dir,
                StatsWindow::Week("2026-W03".into()),
                None,
                true,
                caching,
            )
            .unwrap();
            assert_eq!(len_count(&week3), 1);
            assert_eq!(week3.sessions.len(), 1);
            let week2 = load_snapshot_from(
                &dir,
                StatsWindow::Week("2026-W02".into()),
                None,
                true,
                caching,
            )
            .unwrap();
            assert_eq!(len_count(&week2), 0);
        }
        assert!(cache_path(&dir, "2026-01-05").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// N4: a legacy document can hold a day from the neighbouring week.
    #[test]
    fn a_legacy_document_holding_a_neighbouring_weeks_day_is_counted_there() {
        let dir = temp_dir("legacy-dates");
        // 2026-08-30 is the Sunday of week 35; current.json is week 36.
        let archive = weekly_doc("2026-W35", &[run_on("a", "2026-08-29", "upper")]);
        let current = weekly_doc(
            "2026-W36",
            &[
                run_on("b", "2026-08-30", "upper"),
                run_on("c", "2026-08-31", "upper"),
            ],
        );
        fs::write(dir.join("2026-W35.json"), archive.to_json().to_string()).unwrap();
        fs::write(dir.join("current.json"), current.to_json().to_string()).unwrap();
        for caching in [true, true, false] {
            let week35 = load_snapshot_from(
                &dir,
                StatsWindow::Week("2026-W35".into()),
                None,
                true,
                caching,
            )
            .unwrap();
            assert_eq!(week35.counters.builtins.get("upper"), Some(&2));
            assert_eq!(week35.sessions.len(), 2);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// N5: a same-length rewrite of a cached day, even with its mtime put
    /// back, is not served from the cache.
    #[test]
    fn a_same_length_rewrite_is_not_served_from_the_cache() {
        let dir = temp_dir("same-length");
        let day = days_ago(7);
        record(&dir, &run_on("a", &day, "len"), &day);
        assert_eq!(len_count(&all_caching(&dir)), 1);
        let path = run_path(&dir, &day);
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        let rewritten = fs::read_to_string(&path)
            .unwrap()
            .replace("\"len\"", "\"abs\"");
        fs::write(&path, rewritten).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        let snapshot = all(&dir);
        assert_eq!(len_count(&snapshot), 0);
        assert_eq!(snapshot.counters.builtins.get("abs"), Some(&1));
        let _ = fs::remove_dir_all(&dir);
    }

    /// N6: a legacy document changing mid-read is read again; one that never
    /// settles is reported as unstable rather than silently mixed.
    #[test]
    fn a_legacy_store_changing_mid_read_is_read_again() {
        let dir = temp_dir("legacy-race");
        let week = current_iso_week();
        let today = today_string();
        let doc = |count: u64| {
            let mut run = run_on("old-1", &today, "upper");
            run.buckets[0]
                .counters
                .builtins
                .insert("upper".into(), count);
            weekly_doc(&week, &[run]).to_json().to_string()
        };
        let current = dir.join("current.json");
        fs::write(&current, doc(1)).unwrap();
        let mut calls = 0;
        let mut old_writer_once = |phase: LegacyPhase| {
            if phase != LegacyPhase::Read {
                return;
            }
            if calls == 0 {
                fs::write(&current, doc(2)).unwrap();
            }
            calls += 1;
        };
        let (legacy, unstable) =
            legacy_contribution(&dir, &StatsWindow::AllTime, false, &mut old_writer_once);
        assert!(!unstable);
        assert_eq!(calls, 2, "the changed read was retried");
        assert_eq!(legacy.builtins.get("upper"), Some(&2));

        let mut count = 2;
        let mut never_settles = |phase: LegacyPhase| {
            if phase != LegacyPhase::Read {
                return;
            }
            count += 1;
            fs::write(&current, doc(count)).unwrap();
        };
        let (_, unstable) =
            legacy_contribution(&dir, &StatsWindow::AllTime, false, &mut never_settles);
        assert!(unstable);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_thousand_clears_of_one_name_collapse_to_one_op() {
        let dir = temp_dir("collapse");
        let day = today_string();
        for index in 0..1000 {
            if index % 100 == 0 {
                record(&dir, &today_run("len"), &day);
            }
            append_op(&dir, OpKind::Clear("len".into())).unwrap();
        }
        record(&dir, &today_run("len"), &day);
        assert_eq!(load_ops(&dir).len(), 1);
        let snapshot = all(&dir);
        assert_eq!(len_count(&snapshot), 1, "only the run after the last clear");
        assert_eq!(snapshot.sessions.len(), 11);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn report_windows_require_canonical_values() {
        assert!(is_canonical_date("2026-08-01"));
        assert!(!is_canonical_date("2026-8-1"));
        assert!(is_canonical_week("2026-W34"));
        assert!(!is_canonical_week("definitely-not-a-week"));
        assert!(!is_canonical_week("2026-W00"));
    }

    #[test]
    fn every_window_has_an_explicit_header() {
        assert!(StatsWindow::CurrentWeek.header().starts_with("week "));
        assert_eq!(
            StatsWindow::Week("2026-W34".into()).header(),
            "week 2026-W34"
        );
        assert_eq!(
            StatsWindow::Since("2026-08-01".into()).header(),
            "since 2026-08-01"
        );
        assert_eq!(StatsWindow::AllTime.header(), "all time");
        assert_eq!(StatsWindow::LastDays(30).header(), "last 30 recorded days");
    }
}
