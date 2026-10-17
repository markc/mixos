// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Bounded LRU cache of compiled regexes, shared by every builtin that
//! compiles a user pattern through `builtins::compile_regex` (re_match,
//! re_find, re_replace, re_replace_must, re_split, grep_lines).
//!
//! Before 0.117.0 each call recompiled its pattern, so a per-line
//! `re_match` loop spent nearly all its time in the regex compiler (65 s
//! for a 216 MiB tree search, `_doc` search evaluation 2026-10-17).
//!
//! The key is the exact pattern string: compile_regex takes no flags, and
//! inline flags such as `(?i)` are part of the pattern. Only successful
//! compiles are stored, so an invalid pattern recompiles and raises the
//! same error every time. `regex::Regex` is `Send + Sync` and its clone is
//! a reference-count bump, so one process-wide cache behind a mutex serves
//! the evaluator's threads; the lock is never held across a compile.
//!
//! Bounds: at most `MAX_ENTRIES` patterns and `MAX_TOTAL_BYTES` of pattern
//! text, least recently used evicted first. A pattern longer than
//! `MAX_PATTERN_BYTES` is compiled but never stored. A script generating
//! unique patterns therefore churns the cache without growing it.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

pub(crate) const MAX_ENTRIES: usize = 64;
pub(crate) const MAX_TOTAL_BYTES: usize = 64 * 1024;
pub(crate) const MAX_PATTERN_BYTES: usize = 4 * 1024;

struct Cache {
    // pattern -> (compiled, last-use tick). Eviction scans for the lowest
    // tick: O(MAX_ENTRIES) and only on insert into a full cache.
    map: HashMap<String, (regex::Regex, u64)>,
    bytes: usize,
    tick: u64,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| {
    Mutex::new(Cache {
        map: HashMap::new(),
        bytes: 0,
        tick: 0,
    })
});

fn lock() -> std::sync::MutexGuard<'static, Cache> {
    // A panic elsewhere while holding the lock leaves the map consistent
    // (every mutation is a single insert/remove), so recover the guard.
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The cached compile of `pattern`, if any; marks it most recently used.
pub(crate) fn get(pattern: &str) -> Option<regex::Regex> {
    let mut c = lock();
    c.tick += 1;
    let tick = c.tick;
    let (re, used) = c.map.get_mut(pattern)?;
    *used = tick;
    Some(re.clone())
}

/// Store a successful compile of `pattern`, evicting least recently used
/// entries until both bounds hold.
pub(crate) fn put(pattern: &str, re: &regex::Regex) {
    if pattern.len() > MAX_PATTERN_BYTES {
        return;
    }
    let mut c = lock();
    if c.map.contains_key(pattern) {
        // Another thread compiled the same pattern meanwhile.
        return;
    }
    while !c.map.is_empty()
        && (c.map.len() >= MAX_ENTRIES || c.bytes + pattern.len() > MAX_TOTAL_BYTES)
    {
        let oldest = c
            .map
            .iter()
            .min_by_key(|(_, (_, used))| *used)
            .map(|(k, _)| k.clone())
            .expect("non-empty map has a minimum");
        c.map.remove(&oldest);
        c.bytes -= oldest.len();
    }
    c.tick += 1;
    let tick = c.tick;
    c.bytes += pattern.len();
    c.map.insert(pattern.to_string(), (re.clone(), tick));
}

#[cfg(test)]
fn stats() -> (usize, usize) {
    let c = lock();
    (c.map.len(), c.bytes)
}

#[cfg(test)]
fn contains(pattern: &str) -> bool {
    lock().map.contains_key(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The cache is process-wide and cargo runs tests on parallel threads:
    // these tests take TEST_LOCK so one test's churn cannot evict another's
    // entries, and each uses patterns no other test touches.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn compile(p: &str) -> regex::Regex {
        let re = regex::Regex::new(p).unwrap();
        put(p, &re);
        re
    }

    #[test]
    fn hit_returns_an_equivalent_regex() {
        let _g = serial();
        let p = "lru-hit-(\\d+)";
        compile(p);
        let re = get(p).expect("just stored");
        assert_eq!(re.as_str(), p);
        assert_eq!(&re.captures("lru-hit-42").unwrap()[1], "42");
    }

    #[test]
    fn bounds_hold_under_many_unique_patterns() {
        let _g = serial();
        for i in 0..(MAX_ENTRIES * 20) {
            compile(&format!("lru-unique-{i}-[a-z]+"));
            let (n, bytes) = stats();
            assert!(n <= MAX_ENTRIES, "{n} entries");
            assert!(bytes <= MAX_TOTAL_BYTES, "{bytes} bytes");
        }
    }

    #[test]
    fn total_bytes_bound_holds_with_large_patterns() {
        let _g = serial();
        // Each pattern is just under the per-pattern cap, so the byte
        // bound bites long before the entry bound.
        let pad = "x".repeat(MAX_PATTERN_BYTES - 32);
        for i in 0..64 {
            compile(&format!("lru-big-{i}-{pad}"));
            let (n, bytes) = stats();
            assert!(n <= MAX_ENTRIES);
            assert!(bytes <= MAX_TOTAL_BYTES, "{bytes} bytes");
        }
    }

    #[test]
    fn oversized_pattern_is_not_stored() {
        let _g = serial();
        let p = format!("lru-oversized-{}", "y".repeat(MAX_PATTERN_BYTES));
        compile(&p);
        assert!(!contains(&p));
        assert!(get(&p).is_none());
    }

    #[test]
    fn recently_used_entry_survives_eviction() {
        let _g = serial();
        // Touch the keeper between every insert: it stays the most
        // recently used entry, so the scan never picks it.
        let keep = "lru-keeper-\\w";
        compile(keep);
        for i in 0..(MAX_ENTRIES * 3) {
            assert!(get(keep).is_some(), "keeper evicted after {i} inserts");
            compile(&format!("lru-churn-{i}"));
        }
    }
}
