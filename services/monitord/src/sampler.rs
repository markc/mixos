// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! System-wide sampling: two readings of the kernel's counters become one
//! snapshot of rates and percentages, and a flat list of named series for
//! history.
use crate::kernel::{self, Root, cpu, disk, load, memory, net, pressure, tcp};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Instant;

/// The counters one reading takes.
#[derive(Clone, Debug)]
pub struct Reading {
    pub at: Instant,
    pub stat: cpu::Stat,
    pub disks: Vec<(String, disk::Counters)>,
    pub net: Vec<(String, net::Counters)>,
    pub tcp: tcp::Reading,
}

pub fn read(root: &Root) -> Reading {
    Reading {
        at: Instant::now(),
        stat: root
            .read("proc/stat")
            .map(|t| cpu::parse_stat(&t))
            .unwrap_or_default(),
        disks: disk::disks(root),
        net: net::interfaces(root),
        tcp: tcp::read(root),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Core {
    pub cpu: usize,
    #[serde(flatten)]
    pub usage: cpu::Usage,
    pub mhz: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Cpu {
    pub model: Option<String>,
    /// Logical CPUs online.
    pub cores: usize,
    #[serde(flatten)]
    pub usage: cpu::Usage,
    pub per_core: Vec<Core>,
    pub context_switches_ps: Option<f64>,
    pub forks_ps: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Disk {
    pub name: String,
    pub model: Option<String>,
    pub rotational: Option<bool>,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub reads_ps: Option<f64>,
    pub writes_ps: Option<f64>,
    /// Percent of the interval the disk had I/O in flight.
    pub busy: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Interface {
    pub name: String,
    pub state: Option<String>,
    pub physical: bool,
    pub speed_mbps: Option<u64>,
    pub rx_bps: Option<f64>,
    pub tx_bps: Option<f64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Tasks {
    /// Tasks (threads included) in the load average's count.
    pub total: u64,
    pub running: u64,
    pub blocked: u64,
}

/// Everything `monitord.snapshot` returns.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub host: String,
    /// Unix seconds, with the fraction.
    pub ts: f64,
    pub interval_s: f64,
    pub uptime_s: Option<f64>,
    pub boot_time: u64,
    pub cpu: Cpu,
    pub memory: memory::Memory,
    pub swap: memory::Swap,
    pub load: Option<load::Load>,
    pub pressure: pressure::Pressure,
    pub tasks: Tasks,
    pub disks: Vec<Disk>,
    pub filesystems: Vec<disk::Filesystem>,
    pub net: Vec<Interface>,
    pub tcp: tcp::Tcp,
}

pub fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn hostname(root: &Root) -> String {
    root.read_trimmed("proc/sys/kernel/hostname")
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

/// Things that don't change while monitord runs, read once.
#[derive(Clone, Debug)]
pub struct Fixed {
    pub host: String,
    pub model: Option<String>,
    pub disk_models: HashMap<String, (Option<String>, Option<bool>)>,
}

impl Fixed {
    pub fn read(root: &Root) -> Self {
        Self {
            host: hostname(root),
            model: root.read("proc/cpuinfo").and_then(|t| cpu::model(&t)),
            disk_models: HashMap::new(),
        }
    }

    fn disk(&mut self, root: &Root, name: &str) -> (Option<String>, Option<bool>) {
        self.disk_models
            .entry(name.to_owned())
            .or_insert_with(|| (disk::model(root, name), disk::rotational(root, name)))
            .clone()
    }
}

/// Turn two readings into a snapshot. The instantaneous files (memory,
/// load, pressure, filesystems) are read now.
pub fn snapshot(root: &Root, fixed: &mut Fixed, before: &Reading, after: &Reading) -> Snapshot {
    let dt = after.at.duration_since(before.at).as_secs_f64();
    let rate = |a: u64, b: u64| kernel::rate(a, b, dt);
    let per_core = after
        .stat
        .cores
        .iter()
        .enumerate()
        .filter_map(|(i, now)| {
            let now = now.as_ref()?;
            let then = before.stat.cores.get(i)?.as_ref()?;
            Some(Core {
                cpu: i,
                usage: cpu::usage(then, now).unwrap_or_default(),
                mhz: cpu::mhz(root, i),
            })
        })
        .collect::<Vec<_>>();
    let (memory, swap) = root
        .read("proc/meminfo")
        .map(|t| memory::parse(&t))
        .unwrap_or_default();
    let load = root.read("proc/loadavg").and_then(|t| load::parse(&t));
    let previous_disks: HashMap<&str, &disk::Counters> =
        before.disks.iter().map(|(n, c)| (n.as_str(), c)).collect();
    let disks = after
        .disks
        .iter()
        .map(|(name, now)| {
            let then = previous_disks.get(name.as_str());
            let (model, rotational) = fixed.disk(root, name);
            let r = |f: fn(&disk::Counters) -> u64| then.and_then(|t| rate(f(t), f(now)));
            Disk {
                name: name.clone(),
                model,
                rotational,
                read_bps: r(|c| c.sectors_read).map(|v| v * disk::SECTOR as f64),
                write_bps: r(|c| c.sectors_written).map(|v| v * disk::SECTOR as f64),
                reads_ps: r(|c| c.reads),
                writes_ps: r(|c| c.writes),
                busy: r(|c| c.busy_ms).map(|ms| (ms / 10.0).clamp(0.0, 100.0)),
            }
        })
        .collect();
    let previous_net: HashMap<&str, &net::Counters> =
        before.net.iter().map(|(n, c)| (n.as_str(), c)).collect();
    let net = after
        .net
        .iter()
        .map(|(name, now)| {
            let then = previous_net.get(name.as_str());
            Interface {
                name: name.clone(),
                state: net::operstate(root, name),
                physical: net::physical(root, name),
                speed_mbps: net::speed_mbps(root, name),
                rx_bps: then.and_then(|t| rate(t.rx_bytes, now.rx_bytes)),
                tx_bps: then.and_then(|t| rate(t.tx_bytes, now.tx_bytes)),
                rx_bytes: now.rx_bytes,
                tx_bytes: now.tx_bytes,
                rx_errors: now.rx_errors,
                tx_errors: now.tx_errors,
            }
        })
        .collect();
    Snapshot {
        host: fixed.host.clone(),
        ts: now_unix(),
        interval_s: dt,
        uptime_s: root.read("proc/uptime").and_then(|t| load::uptime_s(&t)),
        boot_time: after.stat.boot_time,
        cpu: Cpu {
            model: fixed.model.clone(),
            cores: per_core.len(),
            usage: cpu::usage(&before.stat.total, &after.stat.total).unwrap_or_default(),
            per_core,
            context_switches_ps: rate(before.stat.context_switches, after.stat.context_switches),
            forks_ps: rate(before.stat.forks, after.stat.forks),
        },
        memory,
        swap,
        tasks: Tasks {
            total: load.map(|l| l.tasks).unwrap_or(0),
            running: after.stat.running,
            blocked: after.stat.blocked,
        },
        load,
        pressure: pressure::read(root),
        disks,
        filesystems: disk::filesystems(root),
        net,
        tcp: tcp::snapshot(&before.tcp, &after.tcp, dt),
    }
}

/// The snapshot as named numbers. `durable` series go to SQLite; the rest
/// (per core) are kept in memory only.
pub fn series(s: &Snapshot) -> Vec<(String, f64, bool)> {
    let mut out: Vec<(String, f64, bool)> = Vec::new();
    let mut put = |name: String, value: Option<f64>, durable: bool| {
        if let Some(v) = value.filter(|v| v.is_finite()) {
            out.push((name, v, durable));
        }
    };
    let u = &s.cpu.usage;
    put("cpu.busy".into(), Some(u.busy), true);
    put("cpu.user".into(), Some(u.user), true);
    put("cpu.system".into(), Some(u.system), true);
    put("cpu.iowait".into(), Some(u.iowait), true);
    put("cpu.steal".into(), Some(u.steal), true);
    for core in &s.cpu.per_core {
        put(
            format!("cpu.core.{}.busy", core.cpu),
            Some(core.usage.busy),
            false,
        );
        put(
            format!("cpu.core.{}.mhz", core.cpu),
            core.mhz.map(|m| m as f64),
            false,
        );
    }
    put("mem.used".into(), Some(s.memory.used as f64), true);
    put(
        "mem.available".into(),
        Some(s.memory.available as f64),
        true,
    );
    put("mem.cached".into(), Some(s.memory.cached as f64), true);
    put("swap.used".into(), Some(s.swap.used as f64), true);
    if let Some(load) = s.load {
        put("load.1".into(), Some(load.one), true);
        put("load.5".into(), Some(load.five), true);
        put("load.15".into(), Some(load.fifteen), true);
    }
    let p = &s.pressure;
    for (name, resource) in [("cpu", p.cpu), ("memory", p.memory), ("io", p.io)] {
        let Some(r) = resource else { continue };
        put(format!("psi.{name}.some"), Some(r.some.avg10), true);
        put(format!("psi.{name}.full"), r.full.map(|f| f.avg10), true);
    }
    for d in &s.disks {
        put(format!("disk.{}.read", d.name), d.read_bps, true);
        put(format!("disk.{}.write", d.name), d.write_bps, true);
        put(format!("disk.{}.busy", d.name), d.busy, true);
    }
    for i in &s.net {
        put(format!("net.{}.rx", i.name), i.rx_bps, true);
        put(format!("net.{}.tx", i.name), i.tx_bps, true);
    }
    for ((name, _), value) in tcp::FIELDS.iter().zip(s.tcp.rates()) {
        put(format!("tcp.{name}"), value, true);
    }
    put(
        "tcp.established".into(),
        s.tcp.established.map(|v| v as f64),
        true,
    );
    out
}

/// The small summary on the `monitord.summary` topic.
pub fn summary(s: &Snapshot) -> serde_json::Value {
    let sum = |f: &dyn Fn(&Interface) -> Option<f64>| s.net.iter().filter_map(f).sum::<f64>();
    let dsum = |f: &dyn Fn(&Disk) -> Option<f64>| s.disks.iter().filter_map(f).sum::<f64>();
    serde_json::json!({
        "host": s.host,
        "ts": s.ts,
        "uptime_s": s.uptime_s,
        "cpu": {"busy": s.cpu.usage.busy, "cores": s.cpu.cores},
        "memory": {"used": s.memory.used, "total": s.memory.total},
        "swap": {"used": s.swap.used, "total": s.swap.total},
        "load": s.load.map(|l| [l.one, l.five, l.fifteen]),
        "net": {"rx_bps": sum(&|i| i.rx_bps), "tx_bps": sum(&|i| i.tx_bps)},
        "disk": {"read_bps": dsum(&|d| d.read_bps), "write_bps": dsum(&|d| d.write_bps)},
        "pressure": {
            "cpu": s.pressure.cpu.map(|r| r.some.avg10),
            "memory": s.pressure.memory.map(|r| r.some.avg10),
            "io": s.pressure.io.map(|r| r.some.avg10),
        },
        "tasks": s.tasks.total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Live, Retention, Store, Tier};

    #[test]
    fn tcp_exactly_eleven_durable_series_roll_up_with_extrema_and_no_sockstat() {
        let dir = tempfile::tempdir().unwrap();
        let root = Root::new(dir.path());
        let mut fixed = Fixed::read(&root);
        let mut before = read(&root);
        for (_, key) in tcp::FIELDS {
            before.tcp.counters.insert(key.into(), 100);
        }
        let mut after = before.clone();
        after.at += std::time::Duration::from_secs(2);
        for (_, key) in tcp::FIELDS {
            after.tcp.counters.insert(key.into(), 120);
        }
        after.tcp.counters.insert("CurrEstab".into(), 7);
        after.tcp.sockstat = tcp::sockstat("TCP: inuse 9 orphan 1 tw 2 alloc 10 mem 20");
        let s = snapshot(&root, &mut fixed, &before, &after);
        assert_eq!(s.tcp.rates(), [Some(10.0); 10]);
        let values: Vec<_> = series(&s)
            .into_iter()
            .filter(|(n, _, _)| n.starts_with("tcp."))
            .collect();
        assert_eq!(values.len(), 11);
        assert!(values.iter().all(|(_, _, durable)| *durable));
        assert!(values.iter().all(|(n, _, _)| !n.contains("sockstat")));
        let mut live = Live::default();
        let mut store = Store::open(&dir.path().join("history.db"), Retention::default()).unwrap();
        for (t, multiplier) in [(7200.0, 1.0), (7201.0, 3.0), (10800.0, 1.0)] {
            let values: Vec<_> = values
                .iter()
                .map(|(n, v, d)| (n.clone(), v * multiplier, *d))
                .collect();
            live.push(t, &values);
            store.add(t, &values).unwrap();
        }
        for (name, value, _) in &values {
            for tier in [Tier::Minute, Tier::Hour] {
                let points = store
                    .query(tier, name, 7200, 10799, tier.seconds())
                    .unwrap();
                assert_eq!(points, vec![[7200.0, value * 2.0, *value, value * 3.0]]);
            }
            assert_eq!(live.query(name, 10800.0, 10800.0, 1)[0][1], *value);
        }
        assert_eq!(
            snapshot(&root, &mut fixed, &read(&root), &after)
                .tcp
                .in_segs,
            None
        );
    }
}
