// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Sliding-window write limits, per caller and command, plus a host-wide cap.
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

pub const COUNT: usize = 30;
const HOST_COUNT: usize = COUNT * 4;
const WINDOW: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct RateLimiter {
    calls: HashMap<(String, String), VecDeque<Instant>>,
    host: VecDeque<Instant>,
}

impl RateLimiter {
    pub fn admit(&mut self, command: &str, caller: &str, now: Instant) -> bool {
        let recent = |at: &Instant| now.saturating_duration_since(*at) < WINDOW;
        self.host.retain(recent);
        self.calls.retain(|_, calls| {
            calls.retain(recent);
            !calls.is_empty()
        });
        let key = (command.to_owned(), caller.to_owned());
        if self.host.len() >= HOST_COUNT
            || self
                .calls
                .get(&key)
                .is_some_and(|calls| calls.len() >= COUNT)
        {
            return false;
        }
        self.calls.entry(key).or_default().push_back(now);
        self.host.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_windows_are_independent_and_expire() {
        let mut limiter = RateLimiter::default();
        let now = Instant::now();
        for _ in 0..COUNT {
            assert!(limiter.admit("net.link.apply", "a", now));
        }
        assert!(!limiter.admit("net.link.apply", "a", now));
        assert!(limiter.admit("net.link.apply", "b", now));
        assert!(limiter.admit("net.wg.apply", "a", now));
        assert!(limiter.admit("net.link.apply", "a", now + WINDOW));
    }

    #[test]
    fn many_callers_share_a_host_cap() {
        let mut limiter = RateLimiter::default();
        let now = Instant::now();
        for i in 0..HOST_COUNT {
            assert!(limiter.admit("net.link.apply", &i.to_string(), now));
        }
        assert!(!limiter.admit("net.link.apply", "next", now));
        assert!(limiter.admit("net.link.apply", "next", now + WINDOW));
    }
}
