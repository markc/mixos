// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! What the kernel says, read from `/proc` and `/sys` under a root
//! directory: `/` in production, a fixture tree in tests. Each parser takes
//! the file's text, so it is tested without a filesystem at all.
pub mod cpu;
pub mod disk;
pub mod load;
pub mod memory;
pub mod net;
pub mod pressure;
pub mod tcp;

use std::path::{Path, PathBuf};

/// Where the kernel's files are read from.
#[derive(Clone, Debug)]
pub struct Root(PathBuf);

impl Root {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn system() -> Self {
        Self::new("/")
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// `rel` has no leading slash: `proc/stat`.
    pub fn join(&self, rel: &str) -> PathBuf {
        self.0.join(rel.trim_start_matches('/'))
    }

    pub fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.join(rel)).ok()
    }

    pub fn read_trimmed(&self, rel: &str) -> Option<String> {
        self.read(rel).map(|s| s.trim().to_owned())
    }

    pub fn read_u64(&self, rel: &str) -> Option<u64> {
        self.read_trimmed(rel)?.parse().ok()
    }
}

/// The scheduler's clock tick and the page size, from the running kernel.
/// Tests pass their own.
#[derive(Clone, Copy, Debug)]
pub struct Units {
    pub ticks_per_s: u64,
    pub page_bytes: u64,
}

impl Units {
    pub fn system() -> Self {
        // SAFETY: sysconf reads a constant; it has no preconditions.
        let (ticks, page) = unsafe {
            (
                libc::sysconf(libc::_SC_CLK_TCK),
                libc::sysconf(libc::_SC_PAGESIZE),
            )
        };
        Self {
            ticks_per_s: if ticks > 0 { ticks as u64 } else { 100 },
            page_bytes: if page > 0 { page as u64 } else { 4096 },
        }
    }
}

/// The value after `key:` in a `key: value unit` file (meminfo, status).
pub fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k == key).then(|| v.trim())
    })
}

/// The first number in a field's value: `"  1234 kB"` is 1234.
pub fn field_u64(text: &str, key: &str) -> Option<u64> {
    field(text, key)?.split_whitespace().next()?.parse().ok()
}

/// A per-second rate from two counter readings `dt` seconds apart. A
/// counter that went backwards (a reset, a device that came back) gives
/// nothing rather than a huge number.
pub fn rate(before: u64, after: u64, dt: f64) -> Option<f64> {
    (after >= before && dt > 0.0).then(|| (after - before) as f64 / dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_found_by_exact_key() {
        let text = "MemTotal:       16000 kB\nMemFree:  10 kB\nMem: 3\n";
        assert_eq!(field_u64(text, "MemTotal"), Some(16000));
        assert_eq!(field_u64(text, "Mem"), Some(3));
        assert_eq!(field_u64(text, "Swap"), None);
    }

    #[test]
    fn a_counter_that_goes_back_has_no_rate() {
        assert_eq!(rate(100, 300, 2.0), Some(100.0));
        assert_eq!(rate(300, 100, 2.0), None);
        assert_eq!(rate(1, 2, 0.0), None);
    }
}
