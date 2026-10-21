// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! History: every sample for the last ten minutes in memory, and minute
//! and hour rollups of the durable series in SQLite.
//!
//! The database is a cache. If it can't be opened, monitord keeps serving
//! live history and says so in `monitord.status`.
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};

/// How long the in-memory ring holds every sample.
pub const LIVE_S: u64 = 600;
pub const MINUTE: u64 = 60;
pub const HOUR: u64 = 3600;

/// One point: `[unix seconds, average, minimum, maximum]`.
pub type Point = [f64; 4];

/// The live ring: per series, the samples of the last `LIVE_S` seconds.
#[derive(Debug, Default)]
pub struct Live {
    series: HashMap<String, VecDeque<(f64, f64)>>,
}

impl Live {
    pub fn push(&mut self, ts: f64, values: &[(String, f64, bool)]) {
        for (name, value, _) in values {
            self.series
                .entry(name.clone())
                .or_default()
                .push_back((ts, *value));
        }
        let oldest = ts - LIVE_S as f64;
        self.series.retain(|_, ring| {
            while ring.front().is_some_and(|(t, _)| *t < oldest) {
                ring.pop_front();
            }
            !ring.is_empty()
        });
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.series.keys().cloned().collect();
        names.sort();
        names
    }

    /// Points in `[from, to]`, averaged into buckets of `step` seconds.
    pub fn query(&self, name: &str, from: f64, to: f64, step: u64) -> Vec<Point> {
        let Some(ring) = self.series.get(name) else {
            return Vec::new();
        };
        bucket(
            ring.iter()
                .filter(|(t, _)| *t >= from && *t <= to)
                .map(|&(t, v)| [t, v, v, v]),
            step,
        )
    }
}

/// Merge points into buckets of `step` seconds, each stamped with its
/// bucket's start. A step of 0 or 1 leaves live points as they are.
fn bucket(points: impl Iterator<Item = Point>, step: u64) -> Vec<Point> {
    if step <= 1 {
        return points.collect();
    }
    let mut out: BTreeMap<u64, (f64, usize, f64, f64)> = BTreeMap::new();
    for [t, avg, min, max] in points {
        let key = (t as u64 / step) * step;
        let e = out
            .entry(key)
            .or_insert((0.0, 0, f64::INFINITY, f64::NEG_INFINITY));
        e.0 += avg;
        e.1 += 1;
        e.2 = e.2.min(min);
        e.3 = e.3.max(max);
    }
    out.into_iter()
        .map(|(t, (sum, n, min, max))| [t as f64, sum / n as f64, min, max])
        .collect()
}

/// How long each SQLite tier is kept.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Retention {
    pub minute_days: u32,
    pub hour_days: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            minute_days: 7,
            hour_days: 365,
        }
    }
}

/// A minute being gathered for one series.
#[derive(Clone, Copy, Debug)]
struct Acc {
    sum: f64,
    n: u32,
    min: f64,
    max: f64,
}

pub struct Store {
    path: PathBuf,
    conn: Connection,
    retention: Retention,
    ids: HashMap<String, i64>,
    minute: Option<u64>,
    acc: HashMap<String, Acc>,
    last_hour_rollup: Option<u64>,
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS series (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS minute (
    series INTEGER NOT NULL,
    ts     INTEGER NOT NULL,
    avg    REAL NOT NULL,
    min    REAL NOT NULL,
    max    REAL NOT NULL,
    PRIMARY KEY (series, ts)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS hour (
    series INTEGER NOT NULL,
    ts     INTEGER NOT NULL,
    avg    REAL NOT NULL,
    min    REAL NOT NULL,
    max    REAL NOT NULL,
    PRIMARY KEY (series, ts)
) WITHOUT ROWID;
";

/// The schema's version, in SQLite's `user_version`. A database written by
/// a newer monitord is left alone rather than rewritten.
const SCHEMA_VERSION: i64 = 1;

impl Store {
    pub fn open(path: &Path, retention: Retention) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        anyhow::ensure!(
            version <= SCHEMA_VERSION,
            "{} has schema {version}, newer than this monitord's {SCHEMA_VERSION}",
            path.display()
        );
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self {
            path: path.to_owned(),
            conn,
            retention,
            ids: HashMap::new(),
            minute: None,
            acc: HashMap::new(),
            last_hour_rollup: None,
        })
    }

    /// Read-only, for the command line reading a running daemon's history.
    pub fn open_read_only(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self {
            path: path.to_owned(),
            conn,
            retention: Retention::default(),
            ids: HashMap::new(),
            minute: None,
            acc: HashMap::new(),
            last_hour_rollup: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Add one sample's durable series. When the minute turns, the minute
    /// just finished is written; when the hour turns, it is rolled up and
    /// old rows are pruned.
    pub fn add(&mut self, ts: f64, values: &[(String, f64, bool)]) -> anyhow::Result<()> {
        let minute = (ts as u64 / MINUTE) * MINUTE;
        if self.minute.is_some_and(|m| m != minute) {
            self.flush()?;
        }
        self.minute = Some(minute);
        for (name, value, durable) in values {
            if !durable {
                continue;
            }
            let acc = self.acc.entry(name.clone()).or_insert(Acc {
                sum: 0.0,
                n: 0,
                min: f64::INFINITY,
                max: f64::NEG_INFINITY,
            });
            acc.sum += value;
            acc.n += 1;
            acc.min = acc.min.min(*value);
            acc.max = acc.max.max(*value);
        }
        let hour = (ts as u64 / HOUR) * HOUR;
        match self.last_hour_rollup {
            None => self.last_hour_rollup = Some(hour),
            Some(last) if last != hour => {
                self.rollup(last, hour)?;
                self.prune(ts as u64)?;
                self.last_hour_rollup = Some(hour);
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// Write the minute being gathered.
    pub fn flush(&mut self) -> anyhow::Result<()> {
        let Some(minute) = self.minute else {
            return Ok(());
        };
        let acc = std::mem::take(&mut self.acc);
        if acc.is_empty() {
            return Ok(());
        }
        let ids: Vec<(i64, Acc)> = acc
            .into_iter()
            .map(|(name, a)| Ok((self.id(&name)?, a)))
            .collect::<anyhow::Result<_>>()?;
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO minute (series, ts, avg, min, max) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (id, a) in ids {
                insert.execute(params![id, minute as i64, a.sum / a.n as f64, a.min, a.max])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn id(&mut self, name: &str) -> anyhow::Result<i64> {
        if let Some(id) = self.ids.get(name) {
            return Ok(*id);
        }
        self.conn
            .execute("INSERT OR IGNORE INTO series (name) VALUES (?1)", [name])?;
        let id = self
            .conn
            .query_row("SELECT id FROM series WHERE name = ?1", [name], |r| {
                r.get(0)
            })?;
        self.ids.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Roll the minutes in `[from, to)` up into hours.
    fn rollup(&self, from: u64, to: u64) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO hour (series, ts, avg, min, max)
             SELECT series, (ts / 3600) * 3600, avg(avg), min(min), max(max)
             FROM minute WHERE ts >= ?1 AND ts < ?2
             GROUP BY series, ts / 3600",
            params![from as i64, to as i64],
        )?;
        Ok(())
    }

    fn prune(&self, now: u64) -> anyhow::Result<()> {
        let day = 86_400u64;
        let r = self.retention;
        self.conn.execute(
            "DELETE FROM minute WHERE ts < ?1",
            [now.saturating_sub(r.minute_days as u64 * day) as i64],
        )?;
        self.conn.execute(
            "DELETE FROM hour WHERE ts < ?1",
            [now.saturating_sub(r.hour_days as u64 * day) as i64],
        )?;
        Ok(())
    }

    pub fn names(&self) -> anyhow::Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT name FROM series ORDER BY name")?;
        let names = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(names)
    }

    /// Points of one tier (`minute` or `hour`) in `[from, to]`, in buckets
    /// of `step` seconds.
    pub fn query(
        &self,
        tier: Tier,
        name: &str,
        from: u64,
        to: u64,
        step: u64,
    ) -> anyhow::Result<Vec<Point>> {
        let Some(id) = self
            .conn
            .query_row("SELECT id FROM series WHERE name = ?1", [name], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
        else {
            return Ok(Vec::new());
        };
        let step = step.max(tier.seconds()) as i64;
        let sql = format!(
            "SELECT (ts / ?4) * ?4 AS t, avg(avg), min(min), max(max) FROM {}
             WHERE series = ?1 AND ts >= ?2 AND ts <= ?3 GROUP BY t ORDER BY t",
            tier.table()
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let points = stmt
            .query_map(params![id, from as i64, to as i64, step], |r| {
                Ok([r.get::<_, i64>(0)? as f64, r.get(1)?, r.get(2)?, r.get(3)?])
            })?
            .collect::<Result<_, _>>()?;
        Ok(points)
    }

    pub fn stats(&self) -> serde_json::Value {
        let count = |table: &str| -> Option<i64> {
            self.conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .ok()
        };
        let size = std::fs::metadata(&self.path).map(|m| m.len()).ok();
        serde_json::json!({
            "path": self.path,
            "bytes": size,
            "series": count("series"),
            "minute_rows": count("minute"),
            "hour_rows": count("hour"),
            "retention": self.retention,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Live,
    Minute,
    Hour,
}

impl Tier {
    pub fn seconds(self) -> u64 {
        match self {
            Tier::Live => 1,
            Tier::Minute => MINUTE,
            Tier::Hour => HOUR,
        }
    }

    fn table(self) -> &'static str {
        match self {
            Tier::Minute => "minute",
            // Live is never a table; a caller asking for it gets hours.
            Tier::Live | Tier::Hour => "hour",
        }
    }

    /// The finest tier that still holds `from`, `now` seconds being now.
    pub fn for_range(from: f64, now: f64, retention: Retention) -> Tier {
        let age = now - from;
        if age <= LIVE_S as f64 {
            Tier::Live
        } else if age <= retention.minute_days as f64 * 86_400.0 {
            Tier::Minute
        } else {
            Tier::Hour
        }
    }
}

/// The bucket size that keeps a range within `max_points`, a whole
/// multiple of the tier's own resolution.
pub fn step(from: f64, to: f64, tier: Tier, max_points: usize) -> u64 {
    let span = (to - from).max(1.0) as u64;
    let base = tier.seconds();
    let wanted = span.div_ceil(max_points.max(1) as u64).max(base);
    wanted.div_ceil(base) * base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(name: &str, value: f64) -> (String, f64, bool) {
        (name.to_owned(), value, true)
    }

    #[test]
    fn the_live_ring_forgets_after_ten_minutes() {
        let mut live = Live::default();
        live.push(1000.0, &[v("cpu.busy", 10.0)]);
        live.push(1001.0, &[v("cpu.busy", 30.0)]);
        assert_eq!(live.query("cpu.busy", 0.0, 2000.0, 1).len(), 2);
        let b = live.query("cpu.busy", 0.0, 2000.0, 10);
        assert_eq!(b, vec![[1000.0, 20.0, 10.0, 30.0]]);
        live.push(1000.0 + LIVE_S as f64 + 0.5, &[v("cpu.busy", 50.0)]);
        assert_eq!(live.query("cpu.busy", 0.0, 9999.0, 1).len(), 2);
    }

    #[test]
    fn minutes_are_written_when_the_minute_turns_and_roll_up_to_hours() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("h.db"), Retention::default()).unwrap();
        let t0 = 7200.0; // an hour boundary
        store.add(t0, &[v("cpu.busy", 10.0)]).unwrap();
        store
            .add(
                t0 + 1.0,
                &[v("cpu.busy", 30.0), ("cpu.core.0.busy".into(), 1.0, false)],
            )
            .unwrap();
        assert!(
            store
                .query(Tier::Minute, "cpu.busy", 0, 99_999, 60)
                .unwrap()
                .is_empty()
        );
        store.add(t0 + 60.0, &[v("cpu.busy", 50.0)]).unwrap();
        let minutes = store
            .query(Tier::Minute, "cpu.busy", 0, 99_999, 60)
            .unwrap();
        assert_eq!(minutes, vec![[7200.0, 20.0, 10.0, 30.0]]);
        assert_eq!(
            store.names().unwrap(),
            vec!["cpu.busy".to_owned()],
            "per-core stays in memory"
        );
        // The next hour starts: the finished one is rolled up.
        store.add(t0 + 3600.0, &[v("cpu.busy", 0.0)]).unwrap();
        let hours = store
            .query(Tier::Hour, "cpu.busy", 0, 99_999, 3600)
            .unwrap();
        assert_eq!(hours, vec![[7200.0, 35.0, 10.0, 50.0]]);
    }

    #[test]
    fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(Store::open(&path, Retention::default()).is_err());
    }

    #[test]
    fn steps_keep_a_range_within_its_points() {
        assert_eq!(step(0.0, 600.0, Tier::Live, 1000), 1);
        assert_eq!(step(0.0, 600.0, Tier::Live, 100), 6);
        assert_eq!(step(0.0, 7.0 * 86_400.0, Tier::Minute, 1000), 660);
        assert_eq!(step(0.0, 3600.0, Tier::Hour, 1000), 3600);
    }

    #[test]
    fn the_tier_follows_the_range_start() {
        let r = Retention::default();
        assert_eq!(Tier::for_range(9_500.0, 10_000.0, r), Tier::Live);
        assert_eq!(Tier::for_range(0.0, 86_400.0, r), Tier::Minute);
        assert_eq!(Tier::for_range(0.0, 30.0 * 86_400.0, r), Tier::Hour);
    }
}
