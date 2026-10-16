// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Load average from `/proc/loadavg`, uptime from `/proc/uptime`.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Load {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
    /// Runnable tasks now, and every task (threads included).
    pub runnable: u64,
    pub tasks: u64,
}

pub fn parse(loadavg: &str) -> Option<Load> {
    let mut words = loadavg.split_whitespace();
    let one = words.next()?.parse().ok()?;
    let five = words.next()?.parse().ok()?;
    let fifteen = words.next()?.parse().ok()?;
    let (runnable, tasks) = words.next()?.split_once('/')?;
    Some(Load {
        one,
        five,
        fifteen,
        runnable: runnable.parse().ok()?,
        tasks: tasks.parse().ok()?,
    })
}

pub fn uptime_s(uptime: &str) -> Option<f64> {
    uptime.split_whitespace().next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loadavg_parses() {
        let load = parse("0.52 0.58 1.25 3/1234 98765\n").unwrap();
        assert_eq!((load.one, load.fifteen), (0.52, 1.25));
        assert_eq!((load.runnable, load.tasks), (3, 1234));
        assert!(parse("garbage").is_none());
    }

    #[test]
    fn uptime_is_the_first_number() {
        assert_eq!(uptime_s("12345.67 54321.00\n"), Some(12345.67));
    }
}
