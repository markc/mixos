// SPDX-License-Identifier: MIT OR Apache-2.0
//! Executed commands (`docs/decisions/2026-10-11-executed-commands.md`):
//! every command the registry runs is recorded with how it was reached, and
//! the application hands the records to its Bus connection, which publishes
//! them on `<app>.command.executed`.
//!
//! The [`Journal`] numbers each run as it happens (`seq`, gapless per
//! process) and keeps a bounded queue that drops its oldest run when full.
//! It reports how many it dropped with each take; the connection's
//! publisher, which sends one run at a time, writes the running loss into
//! the `dropped` of the next run it actually sends.

use serde::Serialize;
use std::collections::VecDeque;

/// How a command was reached: the mechanism that dispatched it, not an
/// earlier cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A menu row, including one chosen through synthetic input.
    Menu,
    /// A key binding, including a synthetic key press.
    Shortcut,
    /// The command palette or Help search.
    Palette,
    /// Any other control in the window, such as a toolbar button.
    Ui,
    /// `<app>.execute` or another verb documented as running a command.
    Bus,
    /// The shared macro runner (reserved until apps have a Macros menu).
    Macro,
}

/// One accepted command run, as `<app>.command.executed` carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Executed {
    /// The body's version.
    pub v: u8,
    /// This process: a UUID drawn once.
    pub run: String,
    /// Per run, from 1, in execution order.
    pub seq: u64,
    /// Milliseconds since the Unix epoch when the handler returned.
    pub at_ms: u64,
    /// The command's stable id.
    pub id: String,
    pub origin: Origin,
    /// Events this run lost since the last one published: written by the
    /// publisher just before it sends the run, 0 until then.
    pub dropped: u64,
}

impl Executed {
    /// The JSON body subscribers receive (built field by field, so it
    /// cannot fail and no run goes missing uncounted).
    pub fn body(&self) -> serde_json::Value {
        serde_json::json!({
            "v": self.v,
            "run": self.run,
            "seq": self.seq,
            "at_ms": self.at_ms,
            "id": self.id,
            "origin": self.origin,
            "dropped": self.dropped,
        })
    }
}

/// How many runs wait to be handed to the connection before the oldest is
/// dropped.
pub const QUEUE: usize = 256;

/// The topic an application with Bus service name `service` publishes on.
pub fn topic(service: &str) -> String {
    format!("{service}.command.executed")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The runs recorded and not yet taken.
#[derive(Debug)]
pub struct Journal {
    run: String,
    next: u64,
    queue: VecDeque<Executed>,
    capacity: usize,
    lost: u64,
}

impl Default for Journal {
    fn default() -> Self {
        Self::new(QUEUE)
    }
}

impl Journal {
    pub fn new(capacity: usize) -> Self {
        Self {
            run: uuid::Uuid::now_v7().to_string(),
            next: 1,
            queue: VecDeque::new(),
            capacity: capacity.max(1),
            lost: 0,
        }
    }

    /// This process's run id.
    pub fn run(&self) -> &str {
        &self.run
    }

    /// Record a run of `id` that the handler has just returned from.
    pub fn record(&mut self, id: &str, origin: Origin) {
        let seq = self.next;
        self.next += 1;
        if self.queue.len() >= self.capacity {
            self.queue.pop_front();
            self.lost += 1;
        }
        self.queue.push_back(Executed {
            v: 1,
            run: self.run.clone(),
            seq,
            at_ms: now_ms(),
            id: id.to_owned(),
            origin,
            dropped: 0,
        });
    }

    /// Everything waiting, oldest first, and how many runs were dropped
    /// before them since the last take.
    pub fn take(&mut self) -> (Vec<Executed>, u64) {
        (
            self.queue.drain(..).collect(),
            std::mem::take(&mut self.lost),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_is_assigned_in_order_and_a_full_queue_drops_its_oldest() {
        let mut j = Journal::new(2);
        j.record("a", Origin::Menu);
        j.record("b", Origin::Bus);
        j.record("c", Origin::Shortcut); // pushes "a" out
        let (taken, lost) = j.take();
        assert_eq!(
            taken
                .iter()
                .map(|e| (e.id.as_str(), e.seq))
                .collect::<Vec<_>>(),
            vec![("b", 2), ("c", 3)]
        );
        assert_eq!(lost, 1);
        assert_eq!(j.take(), (Vec::new(), 0));
    }

    #[test]
    fn the_body_is_the_contract() {
        let mut j = Journal::new(8);
        j.record("file.open", Origin::Palette);
        let event = j.take().0.remove(0);
        let body = event.body();
        assert_eq!(body, serde_json::to_value(&event).unwrap());
        assert_eq!(body["v"], 1);
        assert_eq!(body["id"], "file.open");
        assert_eq!(body["origin"], "palette");
        assert_eq!(body["run"], j.run());
        assert_eq!(body["dropped"], 0);
        assert!(body["at_ms"].as_u64().unwrap() > 1_700_000_000_000);
    }
}
