// SPDX-License-Identifier: MIT OR Apache-2.0
//! File thumbnails, as Dolphin shows them: cached in the freedesktop shared
//! cache ([`cache`]), made by the system's thumbnailers ([`registry`],
//! [`generate`]), which reach every format the system knows. Requests run
//! on a small pool ([`Pool`]) off the caller's thread and come back through
//! its reply callback.
//!
//! Headless: no toolkit, no pixels. Showing a cached thumbnail is the
//! toolkit's part (`toolkit::thumbnail`).

pub mod cache;
pub mod generate;
pub mod mounts;
pub mod png;
pub mod registry;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub use cache::Size;
pub use generate::Outcome;

/// How many thumbnailers a pool runs at once.
pub const WORKERS: usize = 2;

/// Whether a request is still wanted: the caller's live generation (of a
/// listing, a folder view…) and the one the request was made at. A request
/// whose generation has moved on is dropped unrun, and its running
/// thumbnailer is killed.
#[derive(Clone, Debug)]
pub struct Ticket {
    live: Arc<AtomicU64>,
    at: u64,
}

impl Ticket {
    /// A ticket at generation `at` of `live`. Share one counter per thing
    /// that can move on (a pane's listing, a folder view) and bump it when
    /// it does; never replace it. A counter made fresh per request never
    /// goes stale, and superseded requests then run to the end.
    pub fn new(live: Arc<AtomicU64>, at: u64) -> Ticket {
        Ticket { live, at }
    }

    /// The generation the request was made at.
    pub fn at(&self) -> u64 {
        self.at
    }

    pub fn is_stale(&self) -> bool {
        self.live.load(Ordering::Acquire) != self.at
    }
}

/// One thumbnail to make. A request replaces the waiting one in the same
/// `lane` (dopus: one per pane, the selection moved on); a caller that
/// wants every request kept gives each its own lane (the file's path).
#[derive(Clone, Debug)]
pub struct Request<K> {
    pub lane: K,
    pub ticket: Ticket,
    pub path: PathBuf,
    pub size: Size,
}

/// What came of a request, handed to the pool's reply callback.
#[derive(Clone, Debug)]
pub struct Reply<K> {
    pub lane: K,
    /// The generation the request was made at ([`Ticket::at`]).
    pub generation: u64,
    pub path: PathBuf,
    pub outcome: Outcome,
}

/// The reply callback: `false` when its receiver is gone (the worker stops).
type Respond<K> = dyn Fn(Reply<K>) -> bool + Send + Sync;

struct Shared<K> {
    /// Waiting requests, at most one per lane, taken oldest first so no lane
    /// starves.
    queue: Mutex<VecDeque<Request<K>>>,
    ready: Condvar,
    /// Set (under `queue`'s lock, so no idle worker misses it) when the
    /// owner is gone: idle workers wake and exit, and a running thumbnailer
    /// is killed.
    closed: AtomicBool,
    /// Thumbnail files on network and FUSE filesystems too (off by default).
    remote: AtomicBool,
    /// The application the cache is written as ([`cache::Cache::new`]).
    app: String,
    /// Built once, on the first worker (loading the MIME database and the
    /// thumbnailers costs a few milliseconds). `None` without a cache root.
    generator: OnceLock<Option<generate::Generator>>,
    /// The workers, joined when the pool closes.
    handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

/// How long closing the pool waits for its workers. A worker running a
/// thumbnailer notices within a poll (15 ms) and kills it; one stuck in the
/// kernel (a hung network read) is left behind rather than hanging quit.
const SHUTDOWN: Duration = Duration::from_secs(1);

/// Closes the pool when the last owner-side handle drops. Workers never
/// hold one, so they cannot keep the pool alive.
struct Lifetime<K>(Arc<Shared<K>>);

impl<K> Drop for Lifetime<K> {
    fn drop(&mut self) {
        {
            let _queue = self.0.queue.lock().unwrap_or_else(|e| e.into_inner());
            self.0.closed.store(true, Ordering::Release);
        }
        self.0.ready.notify_all();
        let handles =
            std::mem::take(&mut *self.0.handles.lock().unwrap_or_else(|e| e.into_inner()));
        let deadline = Instant::now() + SHUTDOWN;
        for handle in handles {
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

/// A fixed pool of thumbnail workers, started on first use, stopped when
/// the owner drops its last handle.
pub struct Pool<K> {
    shared: Arc<Shared<K>>,
    /// Held for its drop only.
    _lifetime: Arc<Lifetime<K>>,
    started: Arc<OnceLock<()>>,
    respond: Arc<Respond<K>>,
}

impl<K> Clone for Pool<K> {
    fn clone(&self) -> Self {
        Pool {
            shared: self.shared.clone(),
            _lifetime: self._lifetime.clone(),
            started: self.started.clone(),
            respond: self.respond.clone(),
        }
    }
}

/// What a worker holds: the queue and the reply callback, not the pool.
struct Worker<K> {
    shared: Arc<Shared<K>>,
    respond: Arc<Respond<K>>,
}

impl<K: PartialEq + Send + 'static> Pool<K> {
    /// A pool writing the cache as `app` (see [`cache::Cache::new`]), whose
    /// workers hand each result to `respond`. `respond` runs on a worker
    /// thread; it returns `false` once its receiver is gone. Panics on an
    /// `app` that is not a plain name, here rather than on a worker.
    pub fn new(app: &str, respond: impl Fn(Reply<K>) -> bool + Send + Sync + 'static) -> Pool<K> {
        assert!(
            cache::valid_app(app),
            "thumbnail cache app name {app:?} is not a plain name"
        );
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            closed: AtomicBool::new(false),
            remote: AtomicBool::new(false),
            app: app.to_owned(),
            generator: OnceLock::new(),
            handles: Mutex::new(Vec::new()),
        });
        Pool {
            _lifetime: Arc::new(Lifetime(shared.clone())),
            shared,
            started: Arc::new(OnceLock::new()),
            respond: Arc::new(respond),
        }
    }

    /// Ask for a thumbnail, replacing the request still waiting in its lane.
    pub fn submit(&self, request: Request<K>) {
        self.start();
        self.enqueue(request);
        self.shared.ready.notify_one();
    }

    /// [`Pool::submit`]'s queueing, without starting or waking a worker. A
    /// request already stale is dropped here, under the lock, so a late
    /// submission never displaces a current one in its lane.
    fn enqueue(&self, request: Request<K>) {
        let mut queue = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        if request.ticket.is_stale() {
            return;
        }
        queue.retain(|waiting| waiting.lane != request.lane);
        queue.push_back(request);
    }

    /// Thumbnail files on network and FUSE filesystems too.
    pub fn set_remote(&self, remote: bool) {
        self.shared.remote.store(remote, Ordering::Release);
    }

    pub fn remote(&self) -> bool {
        self.shared.remote.load(Ordering::Acquire)
    }

    fn start(&self) {
        self.started.get_or_init(|| {
            let mut handles = self
                .shared
                .handles
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for _ in 0..WORKERS {
                let worker = Worker {
                    shared: self.shared.clone(),
                    respond: self.respond.clone(),
                };
                if let Ok(handle) = std::thread::Builder::new()
                    .name("thumbs".into())
                    .spawn(move || worker.work())
                {
                    handles.push(handle);
                }
            }
            if handles.is_empty() {
                tracing::warn!("no thumbnail worker could be started: thumbnails stay pending");
            }
        });
    }
}

impl<K> Worker<K> {
    fn work(&self) {
        loop {
            let job = {
                let mut queue = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if self.shared.closed.load(Ordering::Acquire) {
                        return;
                    }
                    if let Some(job) = queue.pop_front() {
                        break job;
                    }
                    queue = self
                        .shared
                        .ready
                        .wait(queue)
                        .unwrap_or_else(|e| e.into_inner());
                }
            };
            if job.ticket.is_stale() {
                continue;
            }
            let generator = self.shared.generator.get_or_init(|| {
                let generator = generate::Generator::from_env(&self.shared.app);
                if let Some(generator) = &generator {
                    generator.sweep();
                }
                generator
            });
            // Stopped when the owner closes the pool, or when the request
            // goes stale (navigation): nobody wants this thumbnail any more.
            let cancelled = || self.shared.closed.load(Ordering::Acquire) || job.ticket.is_stale();
            let outcome = match generator {
                Some(generator) => generator.thumbnail_with(
                    &job.path,
                    job.size,
                    self.shared.remote.load(Ordering::Acquire),
                    &cancelled,
                ),
                None => Outcome::Skipped("no thumbnail cache directory"),
            };
            let reply = Reply {
                lane: job.lane,
                generation: job.ticket.at,
                path: job.path,
                outcome,
            };
            if !(self.respond)(reply) {
                return; // the owner is gone
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn ticket(at: u64) -> Ticket {
        Ticket::new(Arc::new(AtomicU64::new(at)), at)
    }

    fn request(lane: &'static str, path: &str) -> Request<&'static str> {
        Request {
            lane,
            ticket: ticket(0),
            path: PathBuf::from(path),
            size: Size::Normal,
        }
    }

    fn channel_pool() -> (Pool<&'static str>, mpsc::Receiver<Reply<&'static str>>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let pool = Pool::new("dopus", move |reply| {
            tx.lock()
                .unwrap_or_else(|e| e.into_inner())
                .send(reply)
                .is_ok()
        });
        (pool, rx)
    }

    impl<K: Clone> Pool<K> {
        fn waiting(&self) -> Vec<(K, PathBuf)> {
            let queue = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            queue
                .iter()
                .map(|j| (j.lane.clone(), j.path.clone()))
                .collect()
        }
    }

    #[test]
    fn each_lane_keeps_one_waiting_request_and_none_starves() {
        let (pool, _rx) = channel_pool();
        // No workers started: the queue alone.
        pool.enqueue(request("right", "r1"));
        pool.enqueue(request("left", "l1"));
        pool.enqueue(request("left", "l2"));
        pool.enqueue(request("left", "l3"));
        // The right lane's request is first: the left lane's stream of
        // newer requests replaced its own, never jumping the queue.
        assert_eq!(
            pool.waiting(),
            vec![
                ("right", PathBuf::from("r1")),
                ("left", PathBuf::from("l3"))
            ]
        );
    }

    /// A late submission for an older generation never displaces the
    /// current request waiting in its lane.
    #[test]
    fn a_stale_submission_leaves_the_current_one_waiting() {
        let (pool, _rx) = channel_pool();
        let live = Arc::new(AtomicU64::new(2));
        let at = |generation, path: &str| Request {
            lane: "left",
            ticket: Ticket::new(live.clone(), generation),
            path: PathBuf::from(path),
            size: Size::Normal,
        };
        pool.enqueue(at(2, "current"));
        pool.enqueue(at(1, "late"));
        assert_eq!(pool.waiting(), vec![("left", PathBuf::from("current"))]);
    }

    #[test]
    fn a_ticket_goes_stale_when_its_generation_moves_on() {
        let live = Arc::new(AtomicU64::new(3));
        let ticket = Ticket::new(live.clone(), 3);
        assert!(!ticket.is_stale());
        live.store(4, Ordering::Release);
        assert!(ticket.is_stale());
        assert_eq!(ticket.at(), 3);
    }

    #[test]
    fn idle_workers_stop_when_the_owner_drops_the_pool() {
        let (pool, _rx) = channel_pool();
        pool.start();
        let shared = Arc::downgrade(&pool.shared);
        drop(pool);
        // The workers hold the shared state until they exit.
        let deadline = Instant::now() + Duration::from_secs(5);
        while shared.upgrade().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(shared.upgrade().is_none(), "workers outlived the pool");
    }

    /// A pool whose one thumbnailer hangs (`tail -f`, 60 s timeout), with
    /// a PNG submitted in lane "left" at generation 0 and its run started.
    /// `None` without shared-mime-info to type the PNG.
    struct Hanging {
        pool: Pool<&'static str>,
        rx: mpsc::Receiver<Reply<&'static str>>,
        live: Arc<AtomicU64>,
        scratch: PathBuf,
        file: PathBuf,
        cache: cache::Cache,
    }

    fn hanging(dir: &std::path::Path) -> Option<Hanging> {
        let data = dir.join("share");
        std::fs::create_dir_all(data.join("thumbnailers")).unwrap();
        std::fs::write(
            data.join("thumbnailers/hang.thumbnailer"),
            "[Thumbnailer Entry]\nExec=tail -f %i %o\nMimeType=image/png;\n",
        )
        .unwrap();
        let mut generator = generate::Generator::new(
            cache::Cache::new(dir.join("cache/thumbnails"), "dopus"),
            registry::Registry::load_from(&[data], &|_| true),
            mimedb::Database::load(),
            dir.join("run"),
        );
        generator.limits.timeout = Duration::from_secs(60);
        generator.mounts = Arc::new(mounts::Mounts::fixed("1 0 0:1 / / rw - ext4 src rw\n"));
        if generator
            .mime
            .for_path(std::path::Path::new("a.png"), false)
            .guess
            .mime
            != "image/png"
        {
            return None; // no shared-mime-info here
        }
        let scratch = generator.scratch.clone();
        let cache = generator.cache.clone();
        let file = dir.join("a.png");
        std::fs::write(&file, png::blank()).unwrap();
        let (pool, rx) = channel_pool();
        assert!(pool.shared.generator.set(Some(generator)).is_ok());
        let live = Arc::new(AtomicU64::new(0));
        pool.submit(Request {
            lane: "left",
            ticket: Ticket::new(live.clone(), 0),
            path: file.clone(),
            size: Size::Normal,
        });
        // Wait for the run to start (its job directory appears).
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read_dir(&scratch).map_or(0, |d| d.count()) == 0 && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        Some(Hanging {
            pool,
            rx,
            live,
            scratch,
            file,
            cache,
        })
    }

    /// A newer generation kills the older request's running thumbnailer at
    /// once, recording nothing: the worker is free for the new request.
    #[test]
    fn a_stale_ticket_cancels_a_running_thumbnailer() {
        let dir = tempfile::tempdir().unwrap();
        let Some(h) = hanging(dir.path()) else {
            return;
        };
        let started = Instant::now();
        h.live.store(1, Ordering::Release);
        let arrived = h.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(arrived.lane, "left");
        assert_eq!(arrived.generation, 0);
        assert_eq!(arrived.outcome, Outcome::Skipped("cancelled"));
        let uri = cache::uri(&h.file).unwrap();
        assert!(!h.cache.fail_path(&uri).exists());
        assert_eq!(std::fs::read_dir(&h.scratch).unwrap().count(), 0);
        drop(h.pool);
    }

    /// Dropping the pool kills a running thumbnailer and joins its worker:
    /// nothing the pool started outlives it.
    #[test]
    fn a_running_thumbnailer_dies_with_the_pool() {
        let dir = tempfile::tempdir().unwrap();
        let Some(Hanging { pool, scratch, .. }) = hanging(dir.path()) else {
            return;
        };
        let shared = Arc::downgrade(&pool.shared);
        let started = Instant::now();
        drop(pool);
        assert!(started.elapsed() < SHUTDOWN, "the worker was not cancelled");
        assert!(shared.upgrade().is_none(), "workers outlived the pool");
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
    }

    #[test]
    #[should_panic(expected = "not a plain name")]
    fn a_cache_app_name_must_be_plain() {
        cache::Cache::new(PathBuf::from("/c"), "../x");
    }

    /// Refused when the pool is made, not later on a worker thread.
    #[test]
    #[should_panic(expected = "not a plain name")]
    fn a_pool_app_name_must_be_plain() {
        let _pool: Pool<u8> = Pool::new("Dopus", |_| true);
    }
}
