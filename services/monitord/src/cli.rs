// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The command line. `serve` is the daemon; the rest work with no daemon
//! running, reading the kernel in process (or the history database), so a
//! person on a node's console or an agent over ssh gets an answer either
//! way. `--json` prints what the matching Bus verb would return.
use crate::engine::{Config, Engine};
use crate::history::{self, Retention, Store, Tier};
use crate::kernel::Root;
use crate::process::Query;
use crate::{action, sampler, service};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "monitord",
    version,
    about = "Samples the system and its processes, keeps history, and answers on the Bus"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Sample, record history, and serve the Bus when a broker is reachable.
    Serve {
        /// Serve no Bus: sample and record only.
        #[arg(long)]
        no_bus: bool,
        /// Milliseconds between samples.
        #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u64).range(100..=60_000))]
        interval_ms: u64,
        /// The history database. Default: $STATE_DIRECTORY/history.db,
        /// else /var/lib/monitord/history.db.
        #[arg(long, conflicts_with = "no_db")]
        db: Option<PathBuf>,
        /// Keep history in memory only.
        #[arg(long)]
        no_db: bool,
        /// Days of minute history to keep.
        #[arg(long, default_value_t = 7)]
        minute_days: u32,
        /// Days of hour history to keep.
        #[arg(long, default_value_t = 365)]
        hour_days: u32,
        /// Read the kernel's files under this directory (tests).
        #[arg(long, hide = true, default_value = "/")]
        root: PathBuf,
    },
    /// The system now, sampled over half a second.
    Snapshot {
        #[arg(long)]
        json: bool,
    },
    /// The process table, sampled over half a second.
    Ps {
        /// cpu, memory, io, pid, name or user.
        #[arg(long, default_value = "cpu")]
        sort: String,
        #[arg(long)]
        reverse: bool,
        /// Rows to show; 0 is all.
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Only rows whose name or command line contains this.
        #[arg(long)]
        filter: Option<String>,
        /// Only this user's processes (name or uid).
        #[arg(long)]
        user: Option<String>,
        /// Parents before children.
        #[arg(long)]
        tree: bool,
        /// Include the kernel's own threads.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Stored history from the database (minute and hour rollups).
    History {
        /// Series names; a trailing `*` matches a prefix (`net.*`).
        #[arg(required = true)]
        series: Vec<String>,
        /// Seconds back from now.
        #[arg(long, default_value_t = 3600.0)]
        since: f64,
        #[arg(long, default_value_t = 60)]
        max_points: usize,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Send a process a signal, as you (TERM unless named).
    Signal {
        pid: u32,
        #[arg(default_value = "TERM")]
        signal: String,
    },
    /// Change a process's nice value, as you (-20 to 19).
    Renice {
        pid: u32,
        #[arg(allow_hyphen_values = true)]
        nice: i32,
    },
}

pub fn default_db() -> PathBuf {
    std::env::var_os("STATE_DIRECTORY")
        // systemd may pass several, colon-separated; the first is ours.
        .and_then(|dirs| {
            dirs.to_str()
                .and_then(|d| d.split(':').next())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("/var/lib/monitord"))
        .join("history.db")
}

/// An engine for one command: no database, sampling only when asked.
fn local_engine() -> std::sync::Arc<Engine> {
    Engine::new(Config {
        root: Root::system(),
        interval: Duration::from_millis(500),
        db: None,
        retention: Retention::default(),
    })
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

pub fn snapshot(json_out: bool) -> anyhow::Result<()> {
    let engine = local_engine();
    let s = runtime()?.block_on(async {
        engine.tick();
        tokio::time::sleep(Duration::from_millis(500)).await;
        engine.tick();
        engine.snapshot().await
    })?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&*s)?);
        return Ok(());
    }
    print!("{}", render_snapshot(&s));
    Ok(())
}

pub fn ps(query: Query, json_out: bool) -> anyhow::Result<()> {
    let engine = local_engine();
    let reply = runtime()?.block_on(async {
        service::dispatch(&engine, "monitord.processes", &serde_json::to_value(QueryBody::from(&query))?)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e["error"].as_str().unwrap_or("failed")))
    })?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&reply)?);
        return Ok(());
    }
    print!("{}", render_ps(&reply));
    Ok(())
}

/// `Query` as the Bus body it stands for.
#[derive(serde::Serialize)]
struct QueryBody {
    sort: Option<String>,
    reverse: bool,
    limit: Option<usize>,
    filter: Option<String>,
    user: Option<String>,
    tree: bool,
    kernel_threads: bool,
}

impl From<&Query> for QueryBody {
    fn from(q: &Query) -> Self {
        Self {
            sort: q.sort.clone(),
            reverse: q.reverse,
            limit: q.limit,
            filter: q.filter.clone(),
            user: q.user.clone(),
            tree: q.tree,
            kernel_threads: q.kernel_threads,
        }
    }
}

pub fn history(series: Vec<String>, since: f64, max_points: usize, db: Option<PathBuf>, json_out: bool) -> anyhow::Result<()> {
    let path = db.unwrap_or_else(default_db);
    let store = Store::open_read_only(&path)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let now = sampler::now_unix();
    let from = now - since;
    let mut tier = Tier::for_range(from, now, Retention::default());
    if tier == Tier::Live {
        tier = Tier::Minute;
    }
    let step = history::step(from, now, tier, max_points.max(1));
    let known = store.names()?;
    let mut out = serde_json::Map::new();
    for name in series {
        let names: Vec<String> = match name.strip_suffix('*') {
            Some(prefix) => known.iter().filter(|k| k.starts_with(prefix)).cloned().collect(),
            None => vec![name],
        };
        for name in names {
            let points = store.query(tier, &name, from as u64, now as u64, step)?;
            out.insert(name, json!(points));
        }
    }
    let reply = json!({"from": from, "to": now, "tier": tier, "step_s": step, "series": out});
    if json_out {
        println!("{}", serde_json::to_string_pretty(&reply)?);
        return Ok(());
    }
    for (name, points) in &out {
        println!("{name}");
        for p in points.as_array().into_iter().flatten() {
            let ts = p[0].as_f64().unwrap_or(0.0) as i64;
            println!(
                "  {}  avg {:>12}  min {:>12}  max {:>12}",
                clock(ts),
                num(&p[1]),
                num(&p[2]),
                num(&p[3])
            );
        }
    }
    Ok(())
}

pub fn signal(pid: u32, name: &str) -> anyhow::Result<()> {
    let number = action::signal_number(name).ok_or_else(|| anyhow::anyhow!("unknown signal {name:?}"))?;
    action::signal(pid, number).map_err(|e| anyhow::anyhow!("signal {pid}: {e}"))
}

pub fn renice(pid: u32, nice: i32) -> anyhow::Result<()> {
    action::renice(pid, nice).map_err(|e| anyhow::anyhow!("renice {pid}: {e}"))
}

// ── text output ──

/// Bytes as a person reads them: 1.5 GiB.
pub fn bytes(n: f64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n;
    let mut unit = 0;
    while v.abs() >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{v:.0} {}", UNITS[0])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

fn num(v: &Value) -> String {
    v.as_f64().map(|f| format!("{f:.2}")).unwrap_or_else(|| "-".into())
}

/// `HH:MM` in UTC; the command line has no time zone database.
fn clock(ts: i64) -> String {
    let day = ts.rem_euclid(86_400);
    format!("{:02}:{:02}Z", day / 3600, day % 3600 / 60)
}

fn duration(secs: f64) -> String {
    let s = secs as u64;
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else {
        format!("{h}h {m}m")
    }
}

pub fn render_snapshot(s: &sampler::Snapshot) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let u = &s.cpu.usage;
    let _ = writeln!(
        out,
        "{}  up {}",
        s.host,
        s.uptime_s.map(duration).unwrap_or_else(|| "?".into())
    );
    let _ = writeln!(
        out,
        "CPU    {:5.1}%  user {:.1}  system {:.1}  iowait {:.1}  steal {:.1}   {} cores  {}",
        u.busy,
        u.user,
        u.system,
        u.iowait,
        u.steal,
        s.cpu.cores,
        s.cpu.model.as_deref().unwrap_or("")
    );
    if let Some(l) = s.load {
        let _ = writeln!(
            out,
            "Load   {:.2} {:.2} {:.2}   tasks {}  running {}  blocked {}",
            l.one, l.five, l.fifteen, s.tasks.total, s.tasks.running, s.tasks.blocked
        );
    }
    let m = &s.memory;
    let _ = writeln!(
        out,
        "Memory {} of {} ({:.1}%)  cached {}",
        bytes(m.used as f64),
        bytes(m.total as f64),
        if m.total > 0 { m.used as f64 * 100.0 / m.total as f64 } else { 0.0 },
        bytes(m.cached as f64)
    );
    if s.swap.total > 0 {
        let _ = writeln!(out, "Swap   {} of {}", bytes(s.swap.used as f64), bytes(s.swap.total as f64));
    }
    let p = &s.pressure;
    if p.cpu.is_some() || p.memory.is_some() || p.io.is_some() {
        let f = |r: Option<crate::kernel::pressure::Resource>| {
            r.map(|r| format!("{:.1}", r.some.avg10)).unwrap_or_else(|| "-".into())
        };
        let _ = writeln!(out, "Stall  cpu {}%  memory {}%  io {}%  (10 s)", f(p.cpu), f(p.memory), f(p.io));
    }
    for d in &s.disks {
        let _ = writeln!(
            out,
            "Disk   {:<10} read {:>11}/s  write {:>11}/s  busy {:>5}%  {}",
            d.name,
            bytes(d.read_bps.unwrap_or(0.0)),
            bytes(d.write_bps.unwrap_or(0.0)),
            d.busy.map(|b| format!("{b:.1}")).unwrap_or_else(|| "-".into()),
            d.model.as_deref().unwrap_or("")
        );
    }
    for f in &s.filesystems {
        let _ = writeln!(
            out,
            "FS     {:<20} {:>11} of {:>11}  {}",
            f.mount,
            bytes(f.used as f64),
            bytes(f.total as f64),
            f.kind
        );
    }
    for i in &s.net {
        let _ = writeln!(
            out,
            "Net    {:<12} rx {:>11}/s  tx {:>11}/s  {}",
            i.name,
            bytes(i.rx_bps.unwrap_or(0.0)),
            bytes(i.tx_bps.unwrap_or(0.0)),
            i.state.as_deref().unwrap_or("")
        );
    }
    out
}

pub fn render_ps(reply: &Value) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:>8} {:<10} {:>6} {:>10} {:>11} {:>3}  COMMAND",
        "PID", "USER", "CPU%", "MEMORY", "IO/s", "NI"
    );
    for row in reply["rows"].as_array().into_iter().flatten() {
        let depth = row["depth"].as_u64().unwrap_or(0) as usize;
        let io = row["read_bps"].as_f64().unwrap_or(0.0) + row["write_bps"].as_f64().unwrap_or(0.0);
        let io = if row["read_bps"].is_null() && row["write_bps"].is_null() {
            "-".to_owned()
        } else {
            bytes(io)
        };
        let command = row["cmdline"]
            .as_str()
            .filter(|c| !c.is_empty())
            .or(row["name"].as_str())
            .unwrap_or("");
        let mut user = row["user"].as_str().unwrap_or("").to_owned();
        user.truncate(10);
        let _ = writeln!(
            out,
            "{:>8} {:<10} {:>6.1} {:>10} {:>11} {:>3}  {}{}",
            row["pid"],
            user,
            row["cpu"].as_f64().unwrap_or(0.0),
            bytes(row["memory"].as_f64().unwrap_or(0.0)),
            io,
            row["nice"],
            "  ".repeat(depth),
            command
        );
    }
    let _ = writeln!(out, "{} of {} processes", reply["count"], reply["total"]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_read_like_a_person_writes_them() {
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1536.0), "1.5 KiB");
        assert_eq!(bytes(3.0 * 1024.0 * 1024.0 * 1024.0), "3.0 GiB");
    }

    #[test]
    fn durations_and_clocks() {
        assert_eq!(duration(90_061.0), "1d 1h 1m");
        assert_eq!(clock(3_600 * 25 + 60 * 7), "01:07Z");
    }
}
