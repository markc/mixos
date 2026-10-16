// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Network interface counters from `/proc/net/dev`, and each interface's
//! state from `/sys/class/net`.
use super::Root;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub rx_errors: u64,
    pub rx_dropped: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    pub tx_errors: u64,
    pub tx_dropped: u64,
}

/// Every interface but loopback, by name.
pub fn parse(text: &str) -> Vec<(String, Counters)> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            if name == "lo" {
                return None;
            }
            let f: Vec<u64> = rest
                .split_whitespace()
                .map(|w| w.parse().ok())
                .collect::<Option<_>>()?;
            if f.len() < 12 {
                return None;
            }
            Some((
                name.to_owned(),
                Counters {
                    rx_bytes: f[0],
                    rx_packets: f[1],
                    rx_errors: f[2],
                    rx_dropped: f[3],
                    tx_bytes: f[8],
                    tx_packets: f[9],
                    tx_errors: f[10],
                    tx_dropped: f[11],
                },
            ))
        })
        .collect()
}

pub fn interfaces(root: &Root) -> Vec<(String, Counters)> {
    root.read("proc/net/dev")
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// `up`, `down`, `dormant`... as the kernel reports it. Virtual links
/// often say `unknown` while passing traffic.
pub fn operstate(root: &Root, name: &str) -> Option<String> {
    root.read_trimmed(&format!("sys/class/net/{name}/operstate"))
}

/// The link speed in Mbit/s, when the driver knows it (not for Wi-Fi or
/// most virtual links, which report -1 or fail the read).
pub fn speed_mbps(root: &Root, name: &str) -> Option<u64> {
    let speed: i64 = root
        .read_trimmed(&format!("sys/class/net/{name}/speed"))?
        .parse()
        .ok()?;
    u64::try_from(speed).ok().filter(|s| *s > 0)
}

/// A physical link has a `device` link in sysfs; bridges, tunnels, veths
/// and WireGuard do not.
pub fn physical(root: &Root, name: &str) -> bool {
    root.join(&format!("sys/class/net/{name}/device")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_dev_skips_headers_and_loopback() {
        let found = parse(
            "Inter-|   Receive                                                |  Transmit\n \
             face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    \
             lo: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0\n  \
             eth0: 5000 50 1 2 0 0 0 0 7000 70 3 4 0 0 0 0\n",
        );
        assert_eq!(found.len(), 1);
        let (name, c) = &found[0];
        assert_eq!(name, "eth0");
        assert_eq!((c.rx_bytes, c.tx_bytes), (5000, 7000));
        assert_eq!((c.rx_errors, c.tx_dropped), (1, 4));
    }
}
