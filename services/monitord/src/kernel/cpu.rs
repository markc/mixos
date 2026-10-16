// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! CPU time from `/proc/stat`, the model from `/proc/cpuinfo` and each
//! core's clock from cpufreq.
use super::Root;
use serde::Serialize;

/// Cumulative ticks in each state, as `/proc/stat` counts them. Guest time
/// is already inside user and nice, so it is not added again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ticks {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl Ticks {
    pub fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }
}

/// One reading of `/proc/stat`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    pub total: Ticks,
    /// Indexed by the kernel's CPU number; a CPU that is offline is absent
    /// from the file and `None` here.
    pub cores: Vec<Option<Ticks>>,
    pub context_switches: u64,
    pub forks: u64,
    pub running: u64,
    pub blocked: u64,
    pub boot_time: u64,
}

fn ticks(fields: &[&str]) -> Option<Ticks> {
    let n = |i: usize| fields.get(i).and_then(|f| f.parse::<u64>().ok());
    Some(Ticks {
        user: n(0)?,
        nice: n(1)?,
        system: n(2)?,
        idle: n(3)?,
        iowait: n(4).unwrap_or(0),
        irq: n(5).unwrap_or(0),
        softirq: n(6).unwrap_or(0),
        steal: n(7).unwrap_or(0),
    })
}

pub fn parse_stat(text: &str) -> Stat {
    let mut stat = Stat::default();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let Some(key) = words.next() else { continue };
        let rest: Vec<&str> = words.collect();
        let first = || rest.first().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "cpu" => stat.total = ticks(&rest).unwrap_or_default(),
            "ctxt" => stat.context_switches = first(),
            "processes" => stat.forks = first(),
            "procs_running" => stat.running = first(),
            "procs_blocked" => stat.blocked = first(),
            "btime" => stat.boot_time = first(),
            _ => {
                let Some(index) = key.strip_prefix("cpu").and_then(|n| n.parse::<usize>().ok())
                else {
                    continue;
                };
                // A sanity bound: no machine has a million CPUs.
                if index >= 1 << 20 {
                    continue;
                }
                if stat.cores.len() <= index {
                    stat.cores.resize(index + 1, None);
                }
                stat.cores[index] = ticks(&rest);
            }
        }
    }
    stat
}

/// How a CPU spent an interval, in percent of that interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Usage {
    /// Everything but idle and iowait.
    pub busy: f64,
    pub user: f64,
    /// System, interrupts and soft interrupts.
    pub system: f64,
    pub iowait: f64,
    pub steal: f64,
}

pub fn usage(before: &Ticks, after: &Ticks) -> Option<Usage> {
    let total = after.total().checked_sub(before.total())?;
    if total == 0 {
        return None;
    }
    let pct = |a: u64, b: u64| a.saturating_sub(b) as f64 * 100.0 / total as f64;
    let idle = pct(after.idle, before.idle) + pct(after.iowait, before.iowait);
    Some(Usage {
        busy: (100.0 - idle).clamp(0.0, 100.0),
        user: pct(after.user, before.user) + pct(after.nice, before.nice),
        system: pct(after.system, before.system)
            + pct(after.irq, before.irq)
            + pct(after.softirq, before.softirq),
        iowait: pct(after.iowait, before.iowait),
        steal: pct(after.steal, before.steal),
    })
}

/// The first `model name` in `/proc/cpuinfo` (x86), else `Model` (Arm
/// boards), else nothing.
pub fn model(cpuinfo: &str) -> Option<String> {
    ["model name", "Model", "Hardware"].iter().find_map(|key| {
        cpuinfo.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            (k.trim() == *key && !v.trim().is_empty()).then(|| v.trim().to_owned())
        })
    })
}

/// A core's current clock in MHz, when cpufreq reports one.
pub fn mhz(root: &Root, core: usize) -> Option<u64> {
    let khz = root.read_u64(&format!(
        "sys/devices/system/cpu/cpu{core}/cpufreq/scaling_cur_freq"
    ))?;
    Some(khz / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  100 10 50 800 20 5 5 10 0 0
cpu0 50 5 25 400 10 2 3 5 0 0
cpu2 50 5 25 400 10 3 2 5 0 0
intr 12345 1 2 3
ctxt 999
btime 1700000000
processes 4242
procs_running 3
procs_blocked 1
";

    #[test]
    fn stat_reads_totals_cores_and_counters() {
        let stat = parse_stat(STAT);
        assert_eq!(stat.total.user, 100);
        assert_eq!(stat.total.total(), 1000);
        assert_eq!(stat.cores.len(), 3);
        assert!(stat.cores[1].is_none(), "cpu1 is offline");
        assert_eq!(stat.cores[2].unwrap().irq, 3);
        assert_eq!(stat.context_switches, 999);
        assert_eq!(stat.boot_time, 1_700_000_000);
        assert_eq!(stat.forks, 4242);
        assert_eq!((stat.running, stat.blocked), (3, 1));
    }

    #[test]
    fn usage_is_percent_of_the_interval() {
        let before = Ticks {
            user: 100,
            idle: 900,
            ..Ticks::default()
        };
        let after = Ticks {
            user: 150,
            system: 25,
            idle: 1100,
            iowait: 25,
            ..Ticks::default()
        };
        // 300 ticks passed: 50 user, 25 system, 200 idle, 25 iowait.
        let u = usage(&before, &after).unwrap();
        assert!((u.busy - 25.0).abs() < 1e-9, "{u:?}");
        assert!((u.user - 50.0 / 3.0).abs() < 1e-9);
        assert!((u.iowait - 25.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn no_time_passing_has_no_usage() {
        let t = Ticks::default();
        assert_eq!(usage(&t, &t), None);
    }

    #[test]
    fn the_model_comes_from_x86_or_arm_fields() {
        assert_eq!(
            model("processor\t: 0\nmodel name\t: AMD Ryzen 9 7950X\n").as_deref(),
            Some("AMD Ryzen 9 7950X")
        );
        assert_eq!(
            model("Model\t\t: Raspberry Pi 5\n").as_deref(),
            Some("Raspberry Pi 5")
        );
        assert_eq!(model("flags: fpu\n"), None);
    }
}
