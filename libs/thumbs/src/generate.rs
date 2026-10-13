// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Making one thumbnail: cache first, else the file's thumbnailer, run as a
//! plain argv in its own process group under a timeout, its output checked
//! and stored in the shared cache. A failure the file itself caused is
//! recorded there too, so the same file is not retried until it changes.
//!
//! Everything that touches the file before its thumbnailer runs (the mount
//! walk, `stat`, `statfs`, `open`, the sniff) runs on a disposable probe
//! thread under a deadline. A filesystem that never answers strands that
//! thread, never the worker: the request is skipped, and while
//! [`MAX_PROBES`] probe threads are alive no new one starts.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::cache::{self, Cache, Lookup, Size};
use crate::mounts::Mounts;
use crate::png;
use crate::registry::{Fill, Registry};

/// What came of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The thumbnail's path in the cache.
    Ready(PathBuf),
    /// No thumbnail: why (shown nowhere; for logs and the Bus).
    Failed(String),
    /// Not attempted: not a regular file, inside the cache, too large, or
    /// no thumbnailer takes its type. The MIME icon stays.
    Skipped(&'static str),
}

/// Limits on a run.
#[derive(Clone, Debug)]
pub struct Limits {
    /// The longest a thumbnailer may run.
    pub timeout: Duration,
    /// Files larger than this are skipped.
    pub max_file: u64,
    /// The largest output read back.
    pub max_output: u64,
    /// The longest the probe of a file may take before it is abandoned.
    pub probe: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            timeout: Duration::from_secs(10),
            max_file: 50 * 1024 * 1024,
            max_output: 16 * 1024 * 1024,
            probe: Duration::from_secs(2),
        }
    }
}

/// Probe threads alive at once, waited on or abandoned: the most threads a
/// hung filesystem can strand. With none free, a request is refused
/// without a probe until one ends.
pub const MAX_PROBES: usize = 4;

/// Set once the stranded-probe warning has been logged.
static STUCK_WARNED: AtomicBool = AtomicBool::new(false);

/// What the probe learned about a file, all from one descriptor.
struct Facts {
    mtime: u64,
    /// The cache's answer for the file at the size asked.
    lookup: Lookup,
    /// Its type, sniffed only on a miss; `None` on a hit or failure record,
    /// or when it is past [`Limits::max_file`].
    mime: Option<String>,
    len: u64,
}

/// Holds one of the [`MAX_PROBES`] slots; released when the probe thread
/// ends, however it ends.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(probes: &Arc<AtomicUsize>) -> Option<Slot> {
        probes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_PROBES).then_some(n + 1)
            })
            .ok()
            .map(|_| Slot(probes.clone()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// How the source file is opened (a seam: tests refuse it).
pub type Opener = fn(&Path) -> std::io::Result<std::fs::File>;

/// Why a run gave no thumbnail.
enum Fault {
    /// The file's own doing (its thumbnailer failed, timed out or wrote no
    /// usable PNG): recorded, so it is not retried until the file changes.
    File(String),
    /// The environment's (no scratch directory, the thumbnailer would not
    /// start, the cache could not be written): not recorded.
    Env(String),
    /// Stopped by the caller (quit, or the request went stale): not recorded.
    Cancelled,
}

/// Everything a worker needs to make thumbnails.
pub struct Generator {
    pub cache: Cache,
    pub registry: Registry,
    pub mime: Arc<mimedb::Database>,
    /// A private directory for thumbnailer output before it is checked.
    pub scratch: PathBuf,
    pub limits: Limits,
    /// The mount table: remote files are refused from it, untouched.
    pub mounts: Arc<Mounts>,
    /// Opens the source ([`open_source`]).
    pub opener: Opener,
    /// Probe threads alive ([`MAX_PROBES`]).
    probes: Arc<AtomicUsize>,
}

impl Generator {
    /// A generator over `cache` and `registry`, with this process's mount
    /// table and the default limits and opener.
    pub fn new(
        cache: Cache,
        registry: Registry,
        mime: mimedb::Database,
        scratch: PathBuf,
    ) -> Generator {
        Generator {
            cache,
            registry,
            mime: Arc::new(mime),
            scratch,
            limits: Limits::default(),
            mounts: Arc::new(Mounts::proc()),
            opener: open_source,
            probes: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// From the environment: the shared cache, the installed thumbnailers,
    /// the MIME database and a scratch directory in `$XDG_RUNTIME_DIR`
    /// (else the cache's own `.<app>-scratch`), all written as `app` (see
    /// [`Cache::new`]). `None` without a cache root.
    pub fn from_env(app: &str) -> Option<Generator> {
        let root = cache::root()?;
        let scratch = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .map(|p| p.join(format!("{app}-thumbnails")))
            .unwrap_or_else(|| root.join(format!(".{app}-scratch")));
        Some(Generator::new(
            Cache::new(root, app),
            Registry::load(),
            mimedb::Database::load(),
            scratch,
        ))
    }

    /// The thumbnail of `file` at `size`, local files only, never cancelled.
    pub fn thumbnail(&self, file: &Path, size: Size) -> Outcome {
        self.thumbnail_with(file, size, false, &|| false)
    }

    /// The thumbnail of `file` at `size`. A file on a network or FUSE
    /// filesystem is skipped unless `remote`. When `cancelled` turns true
    /// while a thumbnailer runs, it is killed and nothing is recorded.
    pub fn thumbnail_with(
        &self,
        file: &Path,
        size: Size,
        remote: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Outcome {
        if self.cache.holds(file) {
            return Outcome::Skipped("inside the thumbnail cache");
        }
        if cancelled() {
            return Outcome::Skipped("cancelled");
        }
        let facts = match self.probe(file, size, remote, cancelled) {
            Ok(facts) => facts,
            Err(why) => return Outcome::Skipped(why),
        };
        let (mtime, len) = (facts.mtime, facts.len);
        match facts.lookup {
            Lookup::Hit(path) => return Outcome::Ready(path),
            Lookup::Failed => return Outcome::Failed("failed before".into()),
            Lookup::Miss => {}
        }
        let Some(mime) = facts.mime else {
            return Outcome::Skipped("too large");
        };
        let Some(thumbnailer) = self.thumbnailer_for(&mime) else {
            return Outcome::Skipped("no thumbnailer for its type");
        };
        let Some(uri) = cache::uri(file) else {
            return Outcome::Skipped("not an absolute path");
        };
        // A directory of this job's own, made exclusively: nothing another
        // run (or a crashed earlier one) left can be mistaken for output.
        let job = match self.job_dir() {
            Ok(dir) => dir,
            Err(e) => return Outcome::Failed(format!("scratch: {e}")),
        };
        let output = job.join("thumbnail.png");
        let result = (|| -> Result<PathBuf, Fault> {
            let argv = thumbnailer
                .argv(&Fill {
                    input: file,
                    uri: &uri,
                    output: &output,
                    size: size.px(),
                })
                .ok_or_else(|| Fault::Env("the thumbnailer's Exec cannot be filled".into()))?;
            run(&argv, self.limits.timeout, cancelled)?;
            // Never through a symlink, never blocking on a FIFO the
            // thumbnailer left there: a regular file, capped.
            let bytes = cache::read_regular(&output, self.limits.max_output)
                .map_err(|e| Fault::File(format!("the thumbnailer's output: {e}")))?;
            // Whole, structurally (no pixel is decoded here), and no
            // larger than asked.
            if !png::is_whole(&bytes) {
                return Err(Fault::File("the thumbnailer wrote no valid PNG".into()));
            }
            let (w, h) = png::size(&bytes)
                .ok_or_else(|| Fault::File("the thumbnailer wrote no valid PNG".into()))?;
            if w > size.px() || h > size.px() {
                return Err(Fault::File(format!("{w}x{h} is larger than {}", size.px())));
            }
            // Too large once tagged is the output's fault; any other write
            // failure is the environment's.
            self.cache
                .store(file, mtime, len, Some(&mime), size, &bytes)
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::FileTooLarge => Fault::File(format!("cache: {e}")),
                    _ => Fault::Env(format!("cache: {e}")),
                })
        })();
        let _ = std::fs::remove_dir_all(&job);
        match result {
            Ok(path) => Outcome::Ready(path),
            Err(Fault::Cancelled) => Outcome::Skipped("cancelled"),
            Err(Fault::Env(reason)) => Outcome::Failed(reason),
            // Stopped meanwhile: the failure may be the stopping's, so it
            // is not held against the file.
            Err(Fault::File(_)) if cancelled() => Outcome::Skipped("cancelled"),
            Err(Fault::File(reason)) => {
                let _ = self.cache.store_failure(file, mtime);
                Outcome::Failed(reason)
            }
        }
    }

    /// Run [`examine`] on a probe thread, waiting at most
    /// [`Limits::probe`], or until `cancelled`. A probe that does not come
    /// back in time is left running (never joined); it holds its slot until
    /// it ends, so at most [`MAX_PROBES`] are ever alive.
    fn probe(
        &self,
        file: &Path,
        size: Size,
        remote: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Facts, &'static str> {
        let Some(slot) = Slot::take(&self.probes) else {
            return Err("a filesystem is not responding");
        };
        let (mounts, mime, opener) = (self.mounts.clone(), self.mime.clone(), self.opener);
        let (cache, max_file, path) = (self.cache.clone(), self.limits.max_file, file.to_owned());
        let (tx, rx) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("thumbs-probe".into())
            .spawn(move || {
                let _slot = slot;
                let probe = Probe {
                    mounts: &mounts,
                    mime: &mime,
                    cache: &cache,
                    opener,
                    max_file,
                };
                let _ = tx.send(probe.examine(&path, size, remote));
            });
        if spawned.is_err() {
            // The closure, and the slot with it, was dropped.
            return Err("no probe thread");
        }
        let deadline = Instant::now() + self.limits.probe;
        loop {
            match rx.recv_timeout(Duration::from_millis(15)) {
                Ok(facts) => return facts,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("unreadable"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let wanted = !cancelled();
            if wanted && Instant::now() < deadline {
                continue;
            }
            // An answer that raced the deadline still counts.
            if let Ok(facts) = rx.try_recv() {
                return facts;
            }
            if !wanted {
                return Err("cancelled");
            }
            // Only a probe that ran out of time says anything about its
            // filesystem; a cancelled one was merely not wanted.
            if !STUCK_WARNED.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    file = %file.display(),
                    "a thumbnail probe did not answer; its filesystem may be hung"
                );
            }
            return Err("the filesystem did not answer");
        }
    }

    /// Clear what an earlier run of this app left behind: its temporary
    /// cache files and its job directories, once a day old (a younger one
    /// may belong to another run still going). Run once, when the pool
    /// starts.
    pub fn sweep(&self) {
        self.sweep_older_than(STALE);
    }

    fn sweep_older_than(&self, age: Duration) {
        self.cache.sweep_temporaries(age);
        cache::sweep_dir(&self.scratch, &[], age, &is_job_dir, true);
    }

    /// The thumbnailer for `mime`, else for the nearest type it is a
    /// subclass of (`image/x-foo` falls back to what takes its parent).
    fn thumbnailer_for(&self, mime: &str) -> Option<&crate::registry::Thumbnailer> {
        let mime = self.mime.unalias(mime);
        self.registry.for_mime(mime).or_else(|| {
            self.mime
                .parents(mime)
                .iter()
                .find_map(|parent| self.registry.for_mime(parent))
        })
    }

    /// A fresh private directory for one run, under the scratch root.
    fn job_dir(&self) -> std::io::Result<PathBuf> {
        use std::os::unix::fs::DirBuilderExt;
        create_private_dir(&self.scratch)?;
        for _ in 0..8 {
            let dir = self.scratch.join(cache::unique("job"));
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => return Ok(dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other("no free job directory"))
    }
}

/// What a probe does: everything that touches `file` before its
/// thumbnailer runs. The mount table first, before anything touches the
/// path (asking a hung mount anything, even `stat`, may never return);
/// `statfs` after it catches what the table cannot name. Checked by path
/// first, so a FIFO or device is never opened; then opened once, never
/// blocking (a FIFO swapped in since the stat opens at once and its fstat
/// refuses it), and sniffed through that descriptor. A file this user may
/// not read is skipped with no failure record, and its cached thumbnail is
/// not shown (the standard's rules).
/// What a probe thread works with.
struct Probe<'a> {
    mounts: &'a Mounts,
    mime: &'a mimedb::Database,
    cache: &'a Cache,
    opener: Opener,
    max_file: u64,
}

impl Probe<'_> {
    /// The cache is consulted once the descriptor's metadata is in, before
    /// the sniff: a valid thumbnail is served without reading the file.
    fn examine(&self, file: &Path, size: Size, remote: bool) -> Result<Facts, &'static str> {
        if !remote && self.mounts.is_remote(file) {
            return Err("on a remote filesystem");
        }
        let meta = std::fs::metadata(file).map_err(|_| "unreadable")?;
        if !meta.is_file() {
            return Err("not a file");
        }
        if !remote && !is_local(file) {
            return Err("on a remote filesystem");
        }
        let source = (self.opener)(file).map_err(|_| "unreadable")?;
        let meta = source.metadata().map_err(|_| "unreadable")?;
        if !meta.is_file() {
            return Err("not a file");
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(cache::mtime_secs)
            .ok_or("no modification time")?;
        let lookup = self.cache.lookup(file, mtime, meta.len(), size);
        let mime = (lookup == Lookup::Miss && meta.len() <= self.max_file)
            .then(|| sniff(self.mime, file, &source));
        Ok(Facts {
            mtime,
            lookup,
            mime,
            len: meta.len(),
        })
    }
}

/// `file`'s type from its name and its leading bytes, read through the
/// already-open `source` (an unreadable start types it by name alone).
fn sniff(mime: &mimedb::Database, file: &Path, source: &std::fs::File) -> String {
    use std::io::Read;
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned());
    let mut data = Vec::new();
    let read = source.take(mime.sniff_len() as u64).read_to_end(&mut data);
    let data = read.is_ok().then_some(data.as_slice());
    mime.guess(name.as_deref(), data).mime
}

/// A job directory's exact name, as [`cache::unique`] makes it:
/// `job.<pid>.<n>.<nanos>`.
fn is_job_dir(name: &str) -> bool {
    name.strip_prefix("job.").is_some_and(|rest| {
        let parts: Vec<&str> = rest.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// How old a leftover temporary file or job directory is before
/// [`Generator::sweep`] removes it.
const STALE: Duration = Duration::from_secs(24 * 60 * 60);

/// Filesystem magics (`statfs` `f_type`) refused after the mount table
/// passes a file, for what the table could not name. Each from Linux's
/// `include/uapi/linux/magic.h` unless noted. `FUSE_SUPER_MAGIC` covers
/// `fuseblk` too, which the table refuses as well: one policy for both.
const REMOTE_FS: &[u32] = &[
    0x0000_6969, // NFS_SUPER_MAGIC
    0x0000_517B, // SMB_SUPER_MAGIC
    0xFF53_4D42, // CIFS_SUPER_MAGIC
    0xFE53_4D42, // SMB2_SUPER_MAGIC
    0x6573_5546, // FUSE_SUPER_MAGIC
    0x0102_1997, // V9FS_MAGIC
    0x5346_414F, // AFS_SUPER_MAGIC
    0x6B41_4653, // AFS_FS_MAGIC (kAFS)
    0x00C3_6400, // CEPH_SUPER_MAGIC
    0x7375_7245, // CODA_SUPER_MAGIC
    0x0000_564C, // NCP_SUPER_MAGIC
    0x2003_0528, // ORANGEFS_SUPER_MAGIC (fs/orangefs/orangefs-kernel.h)
    0x0BD0_0BD0, // LL_SUPER_MAGIC (Lustre, out of tree)
    0x4750_4653, // GPFS ("GPFS", out of tree)
];

/// Whether `f_type` names a network or FUSE filesystem.
pub fn remote_type(f_type: u32) -> bool {
    REMOTE_FS.contains(&f_type)
}

/// Whether `file` is on a local filesystem by `statfs`. A filesystem that
/// cannot be asked counts as remote: no preview rather than a stall.
pub fn is_local(file: &Path) -> bool {
    nix::sys::statfs::statfs(file).is_ok_and(|fs| {
        // f_type is a 32-bit magic in a wider (signed) field.
        let magic = fs.filesystem_type().0 as u32;
        !remote_type(magic)
    })
}

/// Open `file` for reading without blocking: a FIFO or device opens at once
/// (and is then refused by its fstat) instead of waiting for a writer.
pub fn open_source(file: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOCTTY)
        .open(file)
}

/// Run `argv` with no stdin, no output, in its own process group, for at
/// most `timeout`, or until `cancelled` turns true. However it ends,
/// nothing it started outlives it: the group is killed while the exited
/// child is still unreaped (a zombie leader keeps the group id from being
/// reused), then the child is reaped.
fn run(
    argv: &[std::ffi::OsString],
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), Fault> {
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
    use std::os::unix::process::CommandExt;
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| Fault::Env("empty argv".into()))?;
    if cancelled() {
        return Err(Fault::Cancelled);
    }
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| {
            Fault::Env(format!(
                "could not start {}: {e}",
                program.to_string_lossy()
            ))
        })?;
    let Ok(raw) = i32::try_from(child.id()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Fault::Env("child pid out of range".into()));
    };
    let pid = nix::unistd::Pid::from_raw(raw);
    let started = Instant::now();
    let stopped = loop {
        let exited = waitid(
            Id::Pid(pid),
            WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
        );
        match exited {
            Ok(WaitStatus::StillAlive) if cancelled() => break Some(Fault::Cancelled),
            Ok(WaitStatus::StillAlive) if started.elapsed() >= timeout => {
                break Some(Fault::File(format!(
                    "timed out after {}s",
                    timeout.as_secs()
                )));
            }
            Ok(WaitStatus::StillAlive) => std::thread::sleep(Duration::from_millis(15)),
            // Exited, but stopped meanwhile: the exit may be the stopping's.
            Ok(_) if cancelled() => break Some(Fault::Cancelled),
            Ok(_) => break None,
            Err(nix::errno::Errno::EINTR) => {}
            Err(e) => break Some(Fault::Env(format!("wait: {e}"))),
        }
    };
    let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
    let status = child.wait().map_err(|e| Fault::Env(format!("wait: {e}")))?;
    match stopped {
        Some(fault) => Err(fault),
        None if status.success() => Ok(()),
        None => Err(Fault::File(format!("exited with {status}"))),
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A generator over a scratch tree with one thumbnailer, `exec`. The
    /// usual fake is `cp %i %o`: from this side, all a thumbnailer does
    /// is turn an input into a PNG at the output path.
    fn fixture(exec: &str) -> (tempfile::TempDir, Generator) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("share");
        std::fs::create_dir_all(data.join("thumbnailers")).unwrap();
        std::fs::write(
            data.join("thumbnailers/fake.thumbnailer"),
            format!("[Thumbnailer Entry]\nExec={exec}\nMimeType=image/png;\n"),
        )
        .unwrap();
        let mut generator = Generator::new(
            Cache::new(dir.path().join("cache/thumbnails"), "dopus"),
            Registry::load_from(&[data], &|_| true),
            mimedb::Database::load(),
            dir.path().join("run"),
        );
        generator.limits.timeout = Duration::from_millis(500);
        // Everything local but `<dir>/remote`, an NFS mount.
        generator.mounts = Arc::new(Mounts::fixed(&format!(
            "1 0 0:1 / / rw - ext4 src rw\n2 1 0:2 / {} rw - nfs4 src rw\n",
            dir.path().join("remote").display()
        )));
        (dir, generator)
    }

    fn png_typed(generator: &Generator) -> bool {
        generator
            .mime
            .for_path(Path::new("a.png"), false)
            .guess
            .mime
            == "image/png"
    }

    fn png_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, png::blank()).unwrap();
        path
    }

    #[test]
    fn a_thumbnailer_output_is_cached_then_served() {
        let (dir, generator) = fixture("cp %i %o");
        if generator
            .mime
            .for_path(Path::new("a.png"), false)
            .guess
            .mime
            != "image/png"
        {
            return; // no shared-mime-info here: nothing to type the fixture
        }
        let file = png_file(dir.path(), "a picture.png");
        let Outcome::Ready(path) = generator.thumbnail(&file, Size::Normal) else {
            panic!("expected a thumbnail");
        };
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(png::text(&bytes, "Thumb::URI"), cache::uri(&file));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Served from the cache: the scratch dir holds nothing.
        assert_eq!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Ready(path)
        );
        assert_eq!(std::fs::read_dir(&generator.scratch).unwrap().count(), 0);
    }

    #[test]
    fn failures_are_recorded_and_not_retried() {
        let (dir, generator) = fixture("false %i %o");
        if generator
            .mime
            .for_path(Path::new("a.png"), false)
            .guess
            .mime
            != "image/png"
        {
            return;
        }
        let file = png_file(dir.path(), "b.png");
        assert!(matches!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Failed(reason) if reason.starts_with("exited")
        ));
        assert_eq!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Failed("failed before".into())
        );
    }

    #[test]
    fn a_hung_thumbnailer_is_killed() {
        let (dir, generator) = fixture("tail -f %i %o");
        if generator
            .mime
            .for_path(Path::new("a.png"), false)
            .guess
            .mime
            != "image/png"
        {
            return;
        }
        let file = png_file(dir.path(), "c.png");
        let started = Instant::now();
        assert!(matches!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Failed(reason) if reason.starts_with("timed out")
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Output that is not a regular file (a symlink to the input, here) is
    /// refused, never followed.
    #[test]
    fn a_symlinked_output_is_refused() {
        let (dir, generator) = fixture("ln -s %i %o");
        if generator
            .mime
            .for_path(Path::new("a.png"), false)
            .guess
            .mime
            != "image/png"
        {
            return;
        }
        let file = png_file(dir.path(), "d.png");
        assert!(matches!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Failed(reason) if reason.starts_with("the thumbnailer's output")
        ));
        assert_eq!(std::fs::read_dir(&generator.scratch).unwrap().count(), 0);
    }

    /// A FIFO is refused at once, not waited on: as output, and as a source
    /// (opened without blocking, then refused by its fstat).
    #[test]
    fn a_fifo_is_never_read() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("out.png");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).unwrap();
        let started = Instant::now();
        assert!(cache::read_regular(&fifo, 1024).is_err());
        let source = open_source(&fifo).unwrap();
        assert!(!source.metadata().unwrap().is_file());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// Cancelling kills the run and records no failure: the file is tried
    /// again next time.
    #[test]
    fn a_cancelled_run_records_nothing() {
        let (dir, generator) = fixture("tail -f %i %o");
        if generator
            .mime
            .for_path(Path::new("a.png"), false)
            .guess
            .mime
            != "image/png"
        {
            return;
        }
        let file = png_file(dir.path(), "e.png");
        let started = Instant::now();
        assert_eq!(
            generator.thumbnail_with(&file, Size::Normal, false, &|| true),
            Outcome::Skipped("cancelled")
        );
        assert!(started.elapsed() < Duration::from_millis(400));
        let mtime = cache::mtime_secs(std::fs::metadata(&file).unwrap().modified().unwrap());
        assert_eq!(
            generator.cache.lookup(
                &file,
                mtime.unwrap(),
                png::blank().len() as u64,
                Size::Normal
            ),
            Lookup::Miss
        );
    }

    /// A source that cannot be opened (EACCES, through the seam: tests run
    /// as root on the build workers) is skipped: its cached thumbnail is not
    /// served, no thumbnailer runs and no failure is recorded.
    #[test]
    fn a_refused_open_is_skipped_unrecorded_and_uncached() {
        let marker = tempfile::tempdir().unwrap();
        let ran = marker.path().join("ran");
        let (dir, mut generator) = fixture(&format!("touch {} %i %o", ran.display()));
        generator.opener = |_| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        let file = png_file(dir.path(), "g.png");
        let meta = std::fs::metadata(&file).unwrap();
        let mtime = cache::mtime_secs(meta.modified().unwrap()).unwrap();
        generator
            .cache
            .store(&file, mtime, meta.len(), None, Size::Normal, &png::blank())
            .unwrap();
        assert_eq!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Skipped("unreadable")
        );
        assert!(!ran.exists(), "no thumbnailer ran");
        let uri = cache::uri(&file).unwrap();
        assert!(!generator.cache.fail_path(&uri).exists());
    }

    /// A file on a remote mount is refused from the mount table alone: this
    /// one does not even exist, and is not reported unreadable.
    #[test]
    fn a_remote_file_is_refused_untouched() {
        let (dir, generator) = fixture("cp %i %o");
        let file = dir.path().join("remote/never-looked-at.png");
        assert_eq!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Skipped("on a remote filesystem")
        );
        assert_eq!(
            generator.thumbnail_with(&file, Size::Normal, true, &|| false),
            Outcome::Skipped("unreadable"),
            "allowed, it is looked at"
        );
    }

    /// Output that is only a PNG header (no pixel data) is the file's
    /// failure, recorded.
    #[test]
    fn a_header_only_output_is_refused() {
        let (dir, generator) = fixture("cp %i %o");
        if !png_typed(&generator) {
            return;
        }
        // The input is itself a header-only PNG, copied out as the output.
        let mut header_only = png::blank();
        header_only.drain(33..61); // the IDAT chunk
        assert!(png::size(&header_only).is_some());
        let file = dir.path().join("h.png");
        std::fs::write(&file, &header_only).unwrap();
        assert_eq!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Failed("the thumbnailer wrote no valid PNG".into())
        );
        assert!(
            generator
                .cache
                .fail_path(&cache::uri(&file).unwrap())
                .exists()
        );
    }

    /// A thumbnailer that will not start is the environment's failure, not
    /// the file's: nothing is recorded.
    #[test]
    fn an_environment_failure_is_not_recorded() {
        let (dir, generator) = fixture("/nonexistent/thumbnailer %i %o");
        if !png_typed(&generator) {
            return;
        }
        let file = png_file(dir.path(), "i.png");
        assert!(matches!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Failed(reason) if reason.starts_with("could not start")
        ));
        assert!(
            !generator
                .cache
                .fail_path(&cache::uri(&file).unwrap())
                .exists()
        );
    }

    /// A file this user may not read is skipped, with no failure record
    /// (for real; a no-op as root, where the seam test above covers it).
    #[test]
    fn an_unreadable_file_is_skipped_unrecorded() {
        let (dir, generator) = fixture("cp %i %o");
        let file = png_file(dir.path(), "f.png");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&file).is_ok() {
            return; // running as root: nothing is unreadable
        }
        assert_eq!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Skipped("unreadable")
        );
        let uri = cache::uri(&file).unwrap();
        assert!(!generator.cache.fail_path(&uri).exists());
    }

    #[test]
    fn network_and_fuse_filesystems_are_remote() {
        for magic in [
            0x6969,
            0x517B,
            0xFF53_4D42,
            0xFE53_4D42,
            0x6573_5546,
            0x0102_1997,
            0x5346_414F,
            0x6B41_4653,
            0x00C3_6400,
            0x7375_7245,
            0x564C,
            0x2003_0528,
            0x0BD0_0BD0,
            0x4750_4653,
        ] {
            assert!(remote_type(magic), "{magic:#x}");
        }
        assert_eq!(REMOTE_FS.len(), 14, "every listed magic is covered here");
        // BALLOON_KVM_MAGIC, once listed as OrangeFS by mistake.
        assert!(!remote_type(0x1366_1366));
        // ext4, btrfs, xfs, tmpfs, zfs.
        for magic in [0xEF53, 0x9123_683E, 0x5846_5342, 0x0102_1994, 0x2FC1_2FC1] {
            assert!(!remote_type(magic), "{magic:#x}");
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(is_local(dir.path()));
    }

    #[test]
    fn stale_job_directories_are_swept() {
        let (dir, generator) = fixture("cp %i %o");
        let old = generator.scratch.join("job.1.2.3");
        let young = generator.scratch.join("job.1.2.4");
        let other = generator.scratch.join("keep");
        let odd = generator.scratch.join("job.x.2.3");
        for d in [&old, &young, &other, &odd] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::create_dir_all(old.join("nested")).unwrap();
        std::fs::write(old.join("nested/deep.png"), b"x").unwrap();
        std::fs::write(old.join("thumbnail.png"), b"x").unwrap();
        // A link out of a job directory is removed, its target never.
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep.png"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, old.join("link")).unwrap();
        let day_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 86_400);
        for d in [&old, &other, &odd] {
            std::fs::File::open(d)
                .unwrap()
                .set_modified(day_ago)
                .unwrap();
        }
        generator.sweep();
        assert!(!old.exists());
        assert!(young.exists());
        assert!(other.exists(), "not a job directory");
        assert!(odd.exists(), "not a job directory's exact name");
        assert!(outside.join("keep.png").exists());

        // A scratch root that is a symlink is not traversed.
        let (dir, mut generator) = fixture("cp %i %o");
        let real = dir.path().join("real");
        let stale = real.join("job.1.2.3");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::File::open(&stale)
            .unwrap()
            .set_modified(day_ago)
            .unwrap();
        generator.scratch = dir.path().join("linked");
        std::os::unix::fs::symlink(&real, &generator.scratch).unwrap();
        generator.sweep();
        assert!(stale.exists());
    }

    /// A symlink at the scratch top named exactly like a job directory is
    /// neither entered nor removed: only real directories are swept.
    #[test]
    fn a_job_named_symlink_is_not_followed() {
        let (dir, generator) = fixture("cp %i %o");
        std::fs::create_dir_all(&generator.scratch).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep.png"), b"x").unwrap();
        let link = generator.scratch.join("job.1.2.3");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let day_ago = nix::sys::time::TimeSpec::from_duration(
            (std::time::SystemTime::now() - Duration::from_secs(2 * 86_400))
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap(),
        );
        nix::sys::stat::utimensat(
            None,
            &link,
            &day_ago,
            &day_ago,
            nix::sys::stat::UtimensatFlags::NoFollowSymlink,
        )
        .unwrap();
        generator.sweep();
        assert!(link.symlink_metadata().is_ok(), "not a directory: left");
        assert!(outside.join("keep.png").exists());
    }

    /// A filesystem that never answers strands a probe, not the worker: the
    /// request is skipped unrecorded within the deadline, and once
    /// [`MAX_PROBES`] probes are stranded no new one starts until they come
    /// back.
    #[test]
    fn a_hung_probe_is_abandoned_and_capped() {
        let (dir, mut generator) = fixture("cp %i %o");
        generator.limits.probe = Duration::from_millis(50);
        generator.opener = |_| {
            std::thread::sleep(Duration::from_millis(1500));
            Err(std::io::Error::from(std::io::ErrorKind::TimedOut))
        };
        let file = png_file(dir.path(), "slow.png");
        for _ in 0..MAX_PROBES {
            let started = Instant::now();
            assert_eq!(
                generator.thumbnail(&file, Size::Normal),
                Outcome::Skipped("the filesystem did not answer")
            );
            assert!(started.elapsed() < Duration::from_millis(1000));
        }
        let started = Instant::now();
        assert_eq!(
            generator.thumbnail(&file, Size::Normal),
            Outcome::Skipped("a filesystem is not responding")
        );
        assert!(started.elapsed() < Duration::from_millis(50), "no probe");
        let deadline = Instant::now() + Duration::from_secs(5);
        while generator.probes.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            generator.probes.load(Ordering::Acquire),
            0,
            "they came back"
        );
        let uri = cache::uri(&file).unwrap();
        assert!(!generator.cache.fail_path(&uri).exists());
    }

    /// Stopped before its thumbnailer starts (the job directory is made):
    /// it never starts, and nothing is recorded.
    #[test]
    fn a_request_stopped_before_its_run_never_spawns() {
        let marker = tempfile::tempdir().unwrap();
        let ran = marker.path().join("ran");
        let (dir, generator) = fixture(&format!("touch {} %i %o", ran.display()));
        if !png_typed(&generator) {
            return;
        }
        let file = png_file(dir.path(), "k.png");
        let scratch = generator.scratch.clone();
        let stopped = move || std::fs::read_dir(&scratch).is_ok_and(|mut d| d.next().is_some());
        assert_eq!(
            generator.thumbnail_with(&file, Size::Normal, false, &stopped),
            Outcome::Skipped("cancelled")
        );
        assert!(!ran.exists(), "no thumbnailer ran");
    }

    /// Stopped while its thumbnailer runs, and it exits before the stop is
    /// seen (failing, or leaving unusable output): the failure may be the
    /// stopping's, so none is recorded.
    #[test]
    fn a_run_stopped_as_it_fails_records_nothing() {
        let marker = tempfile::tempdir().unwrap();
        let ran = marker.path().join("ran");
        for exec in [
            // Marks, then exits non-zero (no such directory). `-r %i` names
            // the input, as every template must, without changing it.
            format!("touch -r %i {} /nonexistent/x %o", ran.display()),
            // Marks, exits 0, leaves an empty (invalid) output.
            format!("touch -r %i {} %o", ran.display()),
        ] {
            let _ = std::fs::remove_file(&ran);
            let (dir, generator) = fixture(&exec);
            if !png_typed(&generator) {
                return;
            }
            let file = png_file(dir.path(), "l.png");
            let marked = ran.clone();
            let stopped = move || marked.exists();
            assert_eq!(
                generator.thumbnail_with(&file, Size::Normal, false, &stopped),
                Outcome::Skipped("cancelled"),
                "{exec}"
            );
            assert!(ran.exists(), "it ran: {exec}");
            let uri = cache::uri(&file).unwrap();
            assert!(!generator.cache.fail_path(&uri).exists(), "{exec}");
        }
    }

    #[test]
    fn some_files_are_reported_unreadable() {
        let (_dir, generator) = fixture("cp %i %o");
        assert_eq!(
            generator.thumbnail(Path::new("/nonexistent/e.png"), Size::Normal),
            Outcome::Skipped("unreadable")
        );
    }

    #[test]
    fn some_files_are_skipped() {
        let (dir, generator) = fixture("cp %i %o");
        assert_eq!(
            generator.thumbnail(dir.path(), Size::Normal),
            Outcome::Skipped("not a file")
        );
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "hello").unwrap();
        assert_eq!(
            generator.thumbnail(&text, Size::Normal),
            Outcome::Skipped("no thumbnailer for its type")
        );
        let inside = generator.cache.root().join("normal/x.png");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::write(&inside, png::blank()).unwrap();
        assert_eq!(
            generator.thumbnail(&inside, Size::Normal),
            Outcome::Skipped("inside the thumbnail cache")
        );
    }
}
