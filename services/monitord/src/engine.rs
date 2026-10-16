// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! monitord's state: the latest snapshot, the live ring, the history store
//! and the process sampler, and the one tick that moves them all forward.
use crate::history::{self, Live, Retention, Store};
use crate::kernel::{Root, Units};
use crate::process::{self, Process, Raw, Users};
use crate::sampler::{self, Fixed, Reading, Snapshot};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// A `monitord.processes` call keeps the process sampler running this
/// long; after that an idle host stops walking `/proc`.
pub const PROCESS_LEASE: Duration = Duration::from_secs(30);
/// A cold process call samples twice this far apart.
pub const COLD_GAP: Duration = Duration::from_millis(300);
/// `/etc/passwd` is read again after this.
const USERS_TTL: Duration = Duration::from_secs(60);

pub struct Config {
    pub root: Root,
    pub interval: Duration,
    /// `None` keeps history in memory only.
    pub db: Option<PathBuf>,
    pub retention: Retention,
}

#[derive(Default)]
struct Procs {
    /// The last scan, kept to compute the next one's rates.
    scan: Option<(Instant, Vec<Raw>)>,
    table: Option<(Instant, Arc<Vec<Process>>)>,
    lease_until: Option<Instant>,
}

pub struct Engine {
    pub root: Root,
    pub units: Units,
    pub interval: Duration,
    pub retention: Retention,
    pub started: Instant,
    pub started_at: f64,
    fixed: Mutex<Fixed>,
    reading: Mutex<Option<Reading>>,
    latest: RwLock<Option<Arc<Snapshot>>>,
    live: Mutex<Live>,
    store: Option<Mutex<Store>>,
    store_error: Mutex<Option<String>>,
    procs: Mutex<Procs>,
    users: Mutex<(Users, Instant)>,
}

impl Engine {
    pub fn new(config: Config) -> Arc<Self> {
        let (store, store_error) = match &config.db {
            None => (None, None),
            Some(path) => match Store::open(path, config.retention) {
                Ok(store) => (Some(Mutex::new(store)), None),
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "history database unavailable; live history only");
                    (None, Some(error.to_string()))
                }
            },
        };
        let root = config.root;
        Arc::new(Self {
            units: Units::system(),
            interval: config.interval,
            retention: config.retention,
            started: Instant::now(),
            started_at: sampler::now_unix(),
            fixed: Mutex::new(Fixed::read(&root)),
            reading: Mutex::new(None),
            latest: RwLock::new(None),
            live: Mutex::new(Live::default()),
            store,
            store_error: Mutex::new(store_error),
            procs: Mutex::new(Procs::default()),
            users: Mutex::new((Users::read(&root), Instant::now())),
            root,
        })
    }

    /// One sample: read the counters, and when there is an earlier reading,
    /// publish a snapshot and record its series. Blocking file reads; run it
    /// off the async runtime.
    pub fn tick(&self) {
        let now = sampler::read(&self.root);
        let before = self.reading.lock().unwrap().replace(now.clone());
        if let Some(before) = before {
            let snapshot = {
                let mut fixed = self.fixed.lock().unwrap();
                sampler::snapshot(&self.root, &mut fixed, &before, &now)
            };
            self.record(&snapshot);
            *self.latest.write().unwrap() = Some(Arc::new(snapshot));
        }
        if self.lease_active() {
            self.process_tick();
        }
    }

    fn record(&self, snapshot: &Snapshot) {
        let values = sampler::series(snapshot);
        self.live.lock().unwrap().push(snapshot.ts, &values);
        if let Some(store) = &self.store {
            let result = store.lock().unwrap().add(snapshot.ts, &values);
            let mut error = self.store_error.lock().unwrap();
            match result {
                Ok(()) => *error = None,
                Err(e) => {
                    if error.is_none() {
                        tracing::warn!(error = %e, "history write failed");
                    }
                    *error = Some(e.to_string());
                }
            }
        }
    }

    /// Write the minute being gathered, on the way out.
    pub fn flush(&self) {
        if let Some(store) = &self.store
            && let Err(error) = store.lock().unwrap().flush()
        {
            tracing::warn!(%error, "history flush failed");
        }
    }

    pub fn latest(&self) -> Option<Arc<Snapshot>> {
        self.latest.read().unwrap().clone()
    }

    /// The latest snapshot, or, in the first moments after start, one made
    /// now from two readings a short gap apart.
    pub async fn snapshot(self: &Arc<Self>) -> anyhow::Result<Arc<Snapshot>> {
        if let Some(s) = self.latest() {
            return Ok(s);
        }
        let me = self.clone();
        let first = tokio::task::spawn_blocking(move || sampler::read(&me.root)).await?;
        tokio::time::sleep(COLD_GAP).await;
        let me = self.clone();
        let snapshot = tokio::task::spawn_blocking(move || {
            let second = sampler::read(&me.root);
            let mut fixed = me.fixed.lock().unwrap();
            sampler::snapshot(&me.root, &mut fixed, &first, &second)
        })
        .await?;
        Ok(Arc::new(snapshot))
    }

    fn lease_active(&self) -> bool {
        self.procs
            .lock()
            .unwrap()
            .lease_until
            .is_some_and(|until| Instant::now() < until)
    }

    pub fn users(&self) -> Users {
        let mut users = self.users.lock().unwrap();
        if users.1.elapsed() > USERS_TTL {
            *users = (Users::read(&self.root), Instant::now());
        }
        users.0.clone()
    }

    /// Online CPUs and the boot time, from the latest snapshot or, before
    /// there is one, from `/proc/stat`.
    fn cores_and_boot(&self) -> (usize, u64) {
        if let Some(s) = self.latest() {
            return (s.cpu.cores, s.boot_time);
        }
        let stat = self
            .root
            .read("proc/stat")
            .map(|t| crate::kernel::cpu::parse_stat(&t))
            .unwrap_or_default();
        (stat.cores.iter().flatten().count(), stat.boot_time)
    }

    /// Scan the processes and, against the previous scan, make the table.
    fn process_tick(&self) {
        let scan = process::scan(&self.root);
        let at = Instant::now();
        let (cores, boot) = self.cores_and_boot();
        let users = self.users();
        let mut procs = self.procs.lock().unwrap();
        if let Some((then, before)) = &procs.scan {
            let dt = at.duration_since(*then).as_secs_f64();
            let rows = process::table(before, &scan, dt, cores, boot, self.units, &users);
            procs.table = Some((at, Arc::new(rows)));
        }
        procs.scan = Some((at, scan));
    }

    /// The process table, fresh. Holds the process sampler on for
    /// [`PROCESS_LEASE`]; a cold call samples twice, [`COLD_GAP`] apart.
    pub async fn processes(self: &Arc<Self>) -> anyhow::Result<Arc<Vec<Process>>> {
        let fresh = self.interval * 2 + Duration::from_millis(500);
        {
            let mut procs = self.procs.lock().unwrap();
            procs.lease_until = Some(Instant::now() + PROCESS_LEASE);
            if let Some((at, table)) = &procs.table
                && at.elapsed() < fresh
            {
                return Ok(table.clone());
            }
            // A scan from before the sampler went idle would average CPU
            // over minutes: start again.
            if procs.scan.as_ref().is_some_and(|(at, _)| at.elapsed() >= fresh) {
                procs.scan = None;
            }
        }
        let me = self.clone();
        tokio::task::spawn_blocking(move || me.process_tick()).await?;
        // No recent scan to compare with: take a second one.
        let needs_second = self
            .procs
            .lock()
            .unwrap()
            .table
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= COLD_GAP);
        if needs_second {
            tokio::time::sleep(COLD_GAP).await;
            let me = self.clone();
            tokio::task::spawn_blocking(move || me.process_tick()).await?;
        }
        self.procs
            .lock()
            .unwrap()
            .table
            .as_ref()
            .map(|(_, t)| t.clone())
            .ok_or_else(|| anyhow::anyhow!("no process table"))
    }

    pub fn process_sampler(&self) -> serde_json::Value {
        let procs = self.procs.lock().unwrap();
        let until = procs.lease_until.filter(|u| Instant::now() < *u);
        serde_json::json!({
            "active": until.is_some(),
            "lease_left_s": until.map(|u| u.duration_since(Instant::now()).as_secs_f64()),
            "rows": procs.table.as_ref().map(|(_, t)| t.len()),
        })
    }

    pub fn live_names(&self) -> Vec<String> {
        self.live.lock().unwrap().names()
    }

    pub fn stored_names(&self) -> Vec<String> {
        self.store
            .as_ref()
            .and_then(|s| s.lock().unwrap().names().ok())
            .unwrap_or_default()
    }

    pub fn history_status(&self) -> serde_json::Value {
        let error = self.store_error.lock().unwrap().clone();
        match &self.store {
            Some(store) => {
                let mut stats = store.lock().unwrap().stats();
                stats["error"] = serde_json::json!(error);
                stats
            }
            None => serde_json::json!({"path": null, "error": error, "retention": self.retention}),
        }
    }

    /// Points for one series. The tier is the finest that holds `from`.
    pub fn history(
        &self,
        name: &str,
        from: f64,
        to: f64,
        max_points: usize,
    ) -> anyhow::Result<(history::Tier, u64, Vec<history::Point>)> {
        let now = sampler::now_unix();
        let mut tier = history::Tier::for_range(from, now, self.retention);
        if tier != history::Tier::Live && self.store.is_none() {
            // Without the database, memory is all there is.
            tier = history::Tier::Live;
        }
        let step = history::step(from, to, tier, max_points);
        let points = match (tier, &self.store) {
            (history::Tier::Live, _) | (_, None) => self.live.lock().unwrap().query(name, from, to, step),
            (tier, Some(store)) => {
                store
                    .lock()
                    .unwrap()
                    .query(tier, name, from.max(0.0) as u64, to.max(0.0) as u64, step)?
            }
        };
        Ok((tier, step, points))
    }

    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }
}

/// Sample every `interval` until the task is dropped.
pub async fn run(engine: Arc<Engine>) {
    let mut ticker = tokio::time::interval(engine.interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let e = engine.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || e.tick()).await {
            tracing::error!(%error, "sample failed");
        }
    }
}
