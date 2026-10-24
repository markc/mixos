// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The command table and dispatch. Every command reads; none changes the host
//! (docs/docs/decisions/2026-10-16-monitord-and-monitor.md).
use crate::engine::Engine;
use crate::history;
use crate::process::{self, Query};
use crate::sampler;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct Command {
    pub name: &'static str,
    pub args: &'static [&'static str],
    pub summary: &'static str,
}

pub const COMMANDS: &[Command] = &[
    Command {
        name: "monitord.status",
        args: &[],
        summary: "monitord itself: version, uptime, sample interval, the process sampler and the history database",
    },
    Command {
        name: "monitord.snapshot",
        args: &[],
        summary: "The system now: CPU and each core, memory, swap, load, pressure, disks, filesystems, network interfaces",
    },
    Command {
        name: "monitord.processes",
        args: &[
            "sort",
            "reverse",
            "limit",
            "filter",
            "user",
            "tree",
            "kernel_threads",
        ],
        summary: "The process table with CPU share, memory and I/O; sorted, filtered, limited, or as a tree",
    },
    Command {
        name: "monitord.process",
        args: &["pid"],
        summary: "One process in full: command line, executable, memory, I/O, open files, unit, children",
    },
    Command {
        name: "monitord.series",
        args: &[],
        summary: "The history series monitord holds, live (last ten minutes) and stored",
    },
    Command {
        name: "monitord.history",
        args: &["series", "from", "since", "to", "max_points"],
        summary: "History of named series as [time, avg, min, max] points, from memory or the database",
    },
];

pub fn manifest() -> Vec<bus::CommandDescriptor> {
    std::iter::once(bus::CommandDescriptor::new(
        "HELP",
        &[],
        "List served commands",
        true,
    ))
    .chain(
        COMMANDS
            .iter()
            .map(|v| bus::CommandDescriptor::new(v.name, v.args, v.summary, true)),
    )
    .collect()
}

fn bad(error: impl std::fmt::Display) -> Value {
    json!({"status": "bad_request", "error": error.to_string()})
}

fn failed(error: impl std::fmt::Display) -> Value {
    json!({"status": "failed", "error": error.to_string()})
}

fn args<T: for<'de> Deserialize<'de> + Default>(body: &Value) -> Result<T, Value> {
    match body {
        Value::Null => Ok(T::default()),
        Value::Object(map) if map.is_empty() => Ok(T::default()),
        _ => serde_json::from_value(body.clone()).map_err(bad),
    }
}

/// Answer one command. `Err` carries the refusal body (rc 10 on the wire).
pub async fn dispatch(engine: &Arc<Engine>, command_name: &str, body: &Value) -> Result<Value, Value> {
    match command_name {
        "HELP" => Ok(json!({
            "commands": COMMANDS.iter().map(|v| json!({"name": v.name, "args": v.args, "summary": v.summary})).collect::<Vec<_>>()
        })),
        "monitord.status" => Ok(status(engine)),
        "monitord.snapshot" => {
            let s = engine.snapshot().await.map_err(failed)?;
            serde_json::to_value(&*s).map_err(failed)
        }
        "monitord.processes" => processes(engine, args(body)?).await,
        "monitord.process" => process(engine, args(body)?).await,
        "monitord.series" => Ok(json!({
            "live": engine.live_names(),
            "stored": engine.stored_names(),
        })),
        "monitord.history" => {
            let e = engine.clone();
            let query: HistoryQuery = args(body)?;
            tokio::task::spawn_blocking(move || history(&e, query))
                .await
                .map_err(failed)?
        }
        other => Err(json!({"status": "UNKNOWN_COMMAND", "command": other})),
    }
}

pub fn status(engine: &Engine) -> Value {
    json!({
        "component": "monitord",
        "version": crate::COMPONENT_VERSION,
        "host": engine.latest().map(|s| s.host.clone()),
        "started_at": engine.started_at,
        "uptime_s": engine.uptime().as_secs_f64(),
        "interval_s": engine.interval.as_secs_f64(),
        "root": engine.root.path(),
        "process_sampler": engine.process_sampler(),
        "history": engine.history_status(),
    })
}

async fn processes(engine: &Arc<Engine>, query: Query) -> Result<Value, Value> {
    let table = engine.processes().await.map_err(failed)?;
    let users = engine.users();
    let (rows, total) = process::select((*table).clone(), &query, &users).map_err(bad)?;
    let snapshot = engine.latest();
    Ok(json!({
        "host": snapshot.as_ref().map(|s| s.host.clone()),
        "ts": sampler::now_unix(),
        "total": total,
        "count": rows.len(),
        "rows": rows,
    }))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PidQuery {
    pid: Option<u32>,
}

async fn process(engine: &Arc<Engine>, query: PidQuery) -> Result<Value, Value> {
    let pid = query.pid.ok_or_else(|| bad("pid is required"))?;
    let e = engine.clone();
    let found = tokio::task::spawn_blocking(move || {
        let users = e.users();
        let boot = e.latest().map(|s| s.boot_time).unwrap_or_else(|| {
            e.root
                .read("proc/stat")
                .map(|t| crate::kernel::cpu::parse_stat(&t).boot_time)
                .unwrap_or(0)
        });
        process::detail(&e.root, pid, e.units, &users, boot)
    })
    .await
    .map_err(failed)?;
    found.ok_or_else(|| json!({"status": "not_found", "pid": pid}))
}

/// `series` is one name or a list. A name ending in `.*` matches every
/// series under that prefix (`cpu.core.*`).
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct HistoryQuery {
    series: Option<Value>,
    /// Unix seconds. Or `since`: seconds before now.
    from: Option<f64>,
    since: Option<f64>,
    to: Option<f64>,
    max_points: Option<usize>,
}

const MAX_SERIES: usize = 128;
const DEFAULT_POINTS: usize = 600;
const MAX_POINTS: usize = 10_000;

fn history(engine: &Engine, q: HistoryQuery) -> Result<Value, Value> {
    let wanted: Vec<String> = match q.series {
        Some(Value::String(s)) => vec![s],
        Some(Value::Array(list)) => list
            .into_iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| bad("series must be strings"))
            })
            .collect::<Result<_, _>>()?,
        _ => return Err(bad("series is required: a name or a list of names")),
    };
    let now = sampler::now_unix();
    let to = q.to.unwrap_or(now);
    let from = match (q.from, q.since) {
        (Some(_), Some(_)) => return Err(bad("give from or since, not both")),
        (Some(from), None) => from,
        (None, since) => to - since.unwrap_or(history::LIVE_S as f64),
    };
    if !(from.is_finite() && to.is_finite() && from <= to) {
        return Err(bad("from must be before to"));
    }
    let max_points = q.max_points.unwrap_or(DEFAULT_POINTS).clamp(1, MAX_POINTS);
    let mut known = engine.live_names();
    known.extend(engine.stored_names());
    known.sort();
    known.dedup();
    let mut names = Vec::new();
    for name in wanted {
        match name.strip_suffix('*') {
            Some(prefix) => names.extend(known.iter().filter(|k| k.starts_with(prefix)).cloned()),
            None => names.push(name),
        }
    }
    names.dedup();
    if names.len() > MAX_SERIES {
        return Err(bad(format!(
            "{} series asked for; at most {MAX_SERIES}",
            names.len()
        )));
    }
    let mut series = serde_json::Map::new();
    let mut used = None;
    for name in names {
        let (tier, step, points) = engine
            .history(&name, from, to, max_points)
            .map_err(failed)?;
        used = Some((tier, step));
        series.insert(name, json!(points));
    }
    Ok(json!({
        "from": from,
        "to": to,
        "tier": used.map(|u| u.0),
        "step_s": used.map(|u| u.1),
        "series": series,
    }))
}
