// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Memory and swap from `/proc/meminfo`, in bytes.
use super::field_u64;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Memory {
    pub total: u64,
    /// What the kernel could give a new program without swapping
    /// (`MemAvailable`).
    pub available: u64,
    /// `total - available`: what Task Manager calls "in use".
    pub used: u64,
    pub free: u64,
    /// Page cache and reclaimable slab.
    pub cached: u64,
    pub buffers: u64,
    pub shared: u64,
    pub dirty: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Swap {
    pub total: u64,
    pub used: u64,
    pub free: u64,
}

pub fn parse(meminfo: &str) -> (Memory, Swap) {
    let kb = |key: &str| field_u64(meminfo, key).map(|v| v * 1024);
    let total = kb("MemTotal").unwrap_or(0);
    let free = kb("MemFree").unwrap_or(0);
    let buffers = kb("Buffers").unwrap_or(0);
    let cached = kb("Cached").unwrap_or(0) + kb("SReclaimable").unwrap_or(0);
    // Kernels before 3.14 have no MemAvailable; free plus cache is the
    // estimate they used.
    let available = kb("MemAvailable")
        .unwrap_or(free + buffers + cached)
        .min(total);
    let swap_total = kb("SwapTotal").unwrap_or(0);
    let swap_free = kb("SwapFree").unwrap_or(0).min(swap_total);
    (
        Memory {
            total,
            available,
            used: total - available,
            free,
            cached,
            buffers,
            shared: kb("Shmem").unwrap_or(0),
            dirty: kb("Dirty").unwrap_or(0),
        },
        Swap {
            total: swap_total,
            used: swap_total - swap_free,
            free: swap_free,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_is_total_less_available() {
        let (mem, swap) = parse(
            "MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 600 kB\nBuffers: 10 kB\n\
             Cached: 300 kB\nSReclaimable: 20 kB\nShmem: 5 kB\nSwapTotal: 200 kB\nSwapFree: 150 kB\n",
        );
        assert_eq!(mem.total, 1_024_000);
        assert_eq!(mem.used, 400 * 1024);
        assert_eq!(mem.cached, 320 * 1024);
        assert_eq!(swap.used, 50 * 1024);
    }

    #[test]
    fn an_old_kernel_estimates_available() {
        let (mem, _) =
            parse("MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 150 kB\n");
        assert_eq!(mem.available, 300 * 1024);
    }
}
