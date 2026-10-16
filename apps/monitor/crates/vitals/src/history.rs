// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The graphs' history: the last [`WINDOW`] readings of each series,
//! oldest first. It fills from each snapshot as it arrives, and is seeded
//! from `monitord.history` when Monitor starts watching a node, so the
//! graphs are full at once instead of creeping in from the right.
use crate::model::Snapshot;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

/// Readings each graph shows: a minute at monitord's default interval.
pub const WINDOW: usize = 60;

/// The series Monitor graphs, as monitord names them (a trailing `*` is a
/// prefix), for the seeding `monitord.history` call.
pub const SEEDED: [&str; 5] = ["cpu.busy", "cpu.core.*", "mem.used", "disk.*", "net.*"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rings {
    series: BTreeMap<String, VecDeque<f64>>,
}

impl Rings {
    pub fn push(&mut self, name: &str, value: f64) {
        if !value.is_finite() {
            return;
        }
        let ring = self.series.entry(name.to_owned()).or_default();
        ring.push_back(value);
        while ring.len() > WINDOW {
            ring.pop_front();
        }
    }

    /// The series' readings, oldest first; empty when there are none.
    pub fn get(&self, name: &str) -> Vec<f64> {
        self.series
            .get(name)
            .map(|r| r.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn last(&self, name: &str) -> Option<f64> {
        self.series.get(name)?.back().copied()
    }

    pub fn clear(&mut self) {
        self.series.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.series.is_empty()
    }

    /// Add one snapshot's readings, named as monitord names them.
    pub fn record(&mut self, s: &Snapshot) {
        self.push("cpu.busy", s.cpu.busy);
        for core in &s.cpu.per_core {
            self.push(&format!("cpu.core.{}.busy", core.cpu), core.busy);
        }
        self.push("mem.used", s.memory.used as f64);
        for d in &s.disks {
            if let Some(v) = d.read_bps {
                self.push(&format!("disk.{}.read", d.name), v);
            }
            if let Some(v) = d.write_bps {
                self.push(&format!("disk.{}.write", d.name), v);
            }
            if let Some(v) = d.busy {
                self.push(&format!("disk.{}.busy", d.name), v);
            }
        }
        for i in &s.net {
            if let Some(v) = i.rx_bps {
                self.push(&format!("net.{}.rx", i.name), v);
            }
            if let Some(v) = i.tx_bps {
                self.push(&format!("net.{}.tx", i.name), v);
            }
        }
    }

    /// Seed from a `monitord.history` reply: each series' points are
    /// `[time, avg, min, max]`; the averages, oldest first, go in before
    /// anything already held.
    pub fn seed(&mut self, reply: &Value) {
        let Some(series) = reply.get("series").and_then(Value::as_object) else {
            return;
        };
        for (name, points) in series {
            let seeded: Vec<f64> = points
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.get(1)?.as_f64())
                .filter(|v| v.is_finite())
                .collect();
            let ring = self.series.entry(name.clone()).or_default();
            let held: Vec<f64> = ring.drain(..).collect();
            ring.extend(seeded.into_iter().chain(held));
            while ring.len() > WINDOW {
                ring.pop_front();
            }
        }
    }
}

/// The scale for a graph of `values`: at least `floor`, rounded up to a
/// 1, 2 or 5 step so the axis reads cleanly.
pub fn scale(values: &[f64], floor: f64) -> f64 {
    let peak = values.iter().copied().fold(floor, f64::max);
    if peak <= 0.0 {
        return 1.0;
    }
    let magnitude = 10f64.powf(peak.log10().floor());
    for step in [1.0, 2.0, 5.0, 10.0] {
        if peak <= step * magnitude {
            return step * magnitude;
        }
    }
    10.0 * magnitude
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rings_keep_the_window() {
        let mut r = Rings::default();
        for i in 0..(WINDOW + 5) {
            r.push("cpu.busy", i as f64);
        }
        let got = r.get("cpu.busy");
        assert_eq!(got.len(), WINDOW);
        assert_eq!(got[0], 5.0);
        assert_eq!(r.last("cpu.busy"), Some((WINDOW + 4) as f64));
        r.push("cpu.busy", f64::NAN);
        assert_eq!(r.get("cpu.busy").len(), WINDOW);
    }

    #[test]
    fn seeding_goes_before_what_is_held() {
        let mut r = Rings::default();
        r.push("mem.used", 9.0);
        r.seed(&json!({"series": {"mem.used": [[1, 1.0, 0, 2], [2, 2.0, 1, 3]], "cpu.busy": []}}));
        assert_eq!(r.get("mem.used"), [1.0, 2.0, 9.0]);
        assert!(r.get("cpu.busy").is_empty());
    }

    #[test]
    fn scales_round_up_to_clean_steps() {
        assert_eq!(scale(&[0.0, 3.0], 1.0), 5.0);
        assert_eq!(scale(&[130.0], 1.0), 200.0);
        assert_eq!(scale(&[], 100.0), 100.0);
        assert_eq!(scale(&[0.0], 0.0), 1.0);
    }
}
