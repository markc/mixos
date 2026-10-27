// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Named TCP fields from procfs, following the kernel's
//! Documentation/networking/snmp_counter.rst and proc_net(5). Sockstat's
//! TCP mem value is pages, not bytes. No positional SNMP field assumptions.
use super::{Root, rate};
use serde::Serialize;
use std::collections::HashMap;

pub const FIELDS: [(&str, &str); 10] = [
    ("in_segs", "InSegs"),
    ("out_segs", "OutSegs"),
    ("retrans_segs", "RetransSegs"),
    ("in_errs", "InErrs"),
    ("active_opens", "ActiveOpens"),
    ("passive_opens", "PassiveOpens"),
    ("out_rsts", "OutRsts"),
    ("estab_resets", "EstabResets"),
    ("listen_overflows", "ListenOverflows"),
    ("listen_drops", "ListenDrops"),
];

/// Header/value pairs, with unknown (including signed) fields ignored.
pub fn parse(text: &str, section: &str) -> HashMap<String, u64> {
    let mut lines = text.lines();
    while let Some(header) = lines.next() {
        let Some((name, keys)) = header.split_once(':') else {
            continue;
        };
        if name != section {
            continue;
        }
        let Some((name, values)) = lines.next().and_then(|l| l.split_once(':')) else {
            break;
        };
        if name != section {
            break;
        }
        let keys: Vec<_> = keys.split_whitespace().collect();
        let values: Vec<_> = values.split_whitespace().collect();
        if keys.len() != values.len() {
            break;
        }
        return keys
            .into_iter()
            .zip(values)
            .filter_map(|(k, v)| Some((k.to_owned(), v.parse().ok()?)))
            .collect();
    }
    HashMap::new()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Sockstat {
    pub inuse: Option<u64>,
    pub orphan: Option<u64>,
    pub tw: Option<u64>,
    pub alloc: Option<u64>,
    /// Kernel pages allocated to TCP buffers (not socket count).
    pub mem: Option<u64>,
}

pub fn sockstat(text: &str) -> Sockstat {
    sockstat_section(text, "TCP:")
}

fn sockstat_section(text: &str, section: &str) -> Sockstat {
    let fields: Vec<_> = text
        .lines()
        .find_map(|l| l.strip_prefix(section))
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let fields: HashMap<_, _> = fields
        .chunks_exact(2)
        .filter_map(|p| Some((p[0], p[1].parse::<u64>().ok()?)))
        .collect();
    Sockstat {
        inuse: fields.get("inuse").copied(),
        orphan: fields.get("orphan").copied(),
        tw: fields.get("tw").copied(),
        alloc: fields.get("alloc").copied(),
        mem: fields.get("mem").copied(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct Reading {
    pub counters: HashMap<String, u64>,
    pub sockstat: Sockstat,
    pub sockstat6: Sockstat,
}

pub fn read(root: &Root) -> Reading {
    let mut counters = parse(&root.read("proc/net/snmp").unwrap_or_default(), "Tcp");
    counters.extend(parse(
        &root.read("proc/net/netstat").unwrap_or_default(),
        "TcpExt",
    ));
    Reading {
        counters,
        sockstat: sockstat(&root.read("proc/net/sockstat").unwrap_or_default()),
        sockstat6: sockstat_section(
            &root.read("proc/net/sockstat6").unwrap_or_default(),
            "TCP6:",
        ),
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Tcp {
    pub in_segs: Option<f64>,
    pub out_segs: Option<f64>,
    pub retrans_segs: Option<f64>,
    pub in_errs: Option<f64>,
    pub active_opens: Option<f64>,
    pub passive_opens: Option<f64>,
    pub out_rsts: Option<f64>,
    pub estab_resets: Option<f64>,
    pub listen_overflows: Option<f64>,
    pub listen_drops: Option<f64>,
    pub established: Option<u64>,
    pub sockstat: Sockstat,
    /// IPv6 counters, reported separately to preserve sockstat's kernel semantics.
    pub sockstat6: Sockstat,
}

impl Tcp {
    pub fn rates(&self) -> [Option<f64>; 10] {
        [
            self.in_segs,
            self.out_segs,
            self.retrans_segs,
            self.in_errs,
            self.active_opens,
            self.passive_opens,
            self.out_rsts,
            self.estab_resets,
            self.listen_overflows,
            self.listen_drops,
        ]
    }
}

pub fn snapshot(before: &Reading, after: &Reading, dt: f64) -> Tcp {
    let r = |key| rate(*before.counters.get(key)?, *after.counters.get(key)?, dt);
    Tcp {
        in_segs: r("InSegs"),
        out_segs: r("OutSegs"),
        retrans_segs: r("RetransSegs"),
        in_errs: r("InErrs"),
        active_opens: r("ActiveOpens"),
        passive_opens: r("PassiveOpens"),
        out_rsts: r("OutRsts"),
        estab_resets: r("EstabResets"),
        listen_overflows: r("ListenOverflows"),
        listen_drops: r("ListenDrops"),
        established: after.counters.get("CurrEstab").copied(),
        sockstat: after.sockstat.clone(),
        sockstat6: after.sockstat6.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SNMP: &str = "Ip: InReceives\nIp: 3\nTcp: RtoAlgorithm MaxConn InSegs OutSegs RetransSegs CurrEstab InErrs ActiveOpens PassiveOpens OutRsts EstabResets Future\nTcp: 1 -1 1000 2000 5 7 2 40 50 6 8 99\n";
    const EXT: &str = "TcpExt: SyncookiesSent ListenDrops Future ListenOverflows\nTcpExt: 4 3 123 2\nIpExt: InOctets\nIpExt: 9\n";

    #[test]
    fn fixtures_use_names_allow_signed_unrelated_fields_and_future_columns() {
        let mut counters = parse(SNMP, "Tcp");
        counters.extend(parse(EXT, "TcpExt"));
        assert_eq!(counters["InSegs"], 1000);
        assert_eq!(counters["ListenOverflows"], 2);
        assert!(!counters.contains_key("MaxConn"));
        let before = Reading {
            counters,
            ..Reading::default()
        };
        let mut after = before.clone();
        for (_, k) in FIELDS {
            *after.counters.get_mut(k).unwrap() += 10;
        }
        let tcp = snapshot(&before, &after, 2.0);
        assert_eq!(tcp.rates(), [Some(5.0); 10]);
        assert_eq!(tcp.established, Some(7));
        assert_eq!(snapshot(&Reading::default(), &after, 1.0).in_segs, None);
        assert_eq!(snapshot(&after, &before, 1.0).in_segs, None);
        assert_eq!(snapshot(&before, &after, 0.0).in_segs, None);
        assert!(parse("Tcp: InSegs OutSegs\nTcp: 1\n", "Tcp").is_empty());
    }

    #[test]
    fn sockstat_fixture_is_tcp_only_and_unavailable_stays_null() {
        let s = sockstat(
            "sockets: used 22\nTCP: inuse 7 orphan 1 tw 4 alloc 9 mem 12\nUDP: inuse 5 mem 3\n",
        );
        assert_eq!(
            (s.inuse, s.orphan, s.tw, s.alloc, s.mem),
            (Some(7), Some(1), Some(4), Some(9), Some(12))
        );
        assert_eq!(sockstat("TCP: inuse 3 future 9\n").mem, None);
        assert_eq!(sockstat(""), Sockstat::default());
    }

    #[test]
    fn ipv6_sockstat_fixture_is_read_separately_and_missing_fields_stay_null() {
        let dir = tempfile::tempdir().unwrap();
        let net = dir.path().join("proc/net");
        std::fs::create_dir_all(&net).unwrap();
        std::fs::write(net.join("sockstat"), "TCP: inuse 0 orphan 1 tw 4 alloc 90 mem 12\n").unwrap();
        let fixture = "TCP6: inuse 87\nUDP6: inuse 5\nRAW6: inuse 2\nFRAG6: inuse 0 memory 0\n";
        std::fs::write(net.join("sockstat6"), fixture).unwrap();
        let root = Root::new(dir.path());
        let reading = read(&root);
        let tcp = snapshot(&Reading::default(), &reading, 1.0);
        assert_eq!(tcp.sockstat.inuse, Some(0));
        assert_eq!(tcp.sockstat.alloc, Some(90));
        assert_eq!(tcp.sockstat6.inuse, Some(87));
        assert_eq!((tcp.sockstat6.orphan, tcp.sockstat6.tw, tcp.sockstat6.alloc, tcp.sockstat6.mem), (None, None, None, None));
        let extended = sockstat_section("TCP6: mem 4 alloc 8 tw 3 orphan 2 inuse 7\n", "TCP6:");
        assert_eq!((extended.inuse, extended.orphan, extended.tw, extended.alloc, extended.mem), (Some(7), Some(2), Some(3), Some(8), Some(4)));
        std::fs::remove_file(net.join("sockstat6")).unwrap();
        assert_eq!(read(&root).sockstat6, Sockstat::default());
    }
}
