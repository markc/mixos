// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Numbers as a person reads them. Units are symbols, not words, so they
//! need no translation.

/// Bytes in binary units: `1.5 GiB`.
pub fn bytes(n: f64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n.max(0.0);
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{v:.0} {}", UNITS[0])
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// A transfer rate: `12.3 MiB/s`.
pub fn rate(bps: f64) -> String {
    format!("{}/s", bytes(bps))
}

/// A share: `12.5%`.
pub fn percent(p: f64) -> String {
    format!("{:.1}%", p.clamp(0.0, 100.0))
}

/// A duration as days, hours and minutes: `3d 4h 12m`.
pub fn uptime(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {}s", s % 60)
    }
}

/// A clock speed: `3.42 GHz`, or MHz below one gigahertz.
pub fn mhz(mhz: u64) -> String {
    if mhz >= 1000 {
        format!("{:.2} GHz", mhz as f64 / 1000.0)
    } else {
        format!("{mhz} MHz")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_rates_and_shares() {
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1536.0), "1.5 KiB");
        assert_eq!(bytes(300.0 * 1024.0 * 1024.0), "300 MiB");
        assert_eq!(rate(2048.0), "2.0 KiB/s");
        assert_eq!(percent(130.0), "100.0%");
    }

    #[test]
    fn uptime_and_clock() {
        assert_eq!(uptime(90_061.0), "1d 1h 1m");
        assert_eq!(uptime(3_725.0), "1h 2m");
        assert_eq!(uptime(65.0), "1m 5s");
        assert_eq!(mhz(3420), "3.42 GHz");
        assert_eq!(mhz(800), "800 MHz");
    }
}
