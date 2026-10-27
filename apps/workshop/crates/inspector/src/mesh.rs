// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded, renderer-independent port of Mesh Watch's scripts/lib/tally.mix.
//! Event timestamps own the windows; the injected clock owns expiry and age.
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

pub const MAX_LINKS: usize = 64;
pub const MAX_KEYS: usize = 40;
pub const RECENT_MAX: usize = 2000;
pub const PENDING_MAX: usize = 2000;
pub const RECENT_MS: u64 = 15_000;
const PENDING_MS: u64 = 60_000;
const WINDOW_S: u64 = 300;
/// Samples in a link's five-minute per-second history.
pub const HISTORY_S: usize = WINDOW_S as usize;
const TEXT_MAX: usize = 256;
pub const UNKNOWN: &str = "?";
const OTHER: &str = "(other)";

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn text(value: &str) -> String {
    value.chars().take(TEXT_MAX).collect()
}

pub fn node_of(address: &str) -> Option<&str> {
    let prefix = address.strip_suffix(".bus")?;
    let node = prefix.rsplit('.').next()?;
    (!node.is_empty()).then_some(node)
}

pub fn service_of(address: Option<&str>) -> &str {
    let Some(address) = address.filter(|s| !s.is_empty()) else {
        return "anonymous";
    };
    if address.ends_with(".bus") && address.split('.').count() == 2 {
        "noded"
    } else {
        address.split('.').next().unwrap_or("anonymous")
    }
}

/// UTC RFC 3339 broker timestamps, including fractional milliseconds.
pub fn ts_ms(iso: &str) -> Option<u64> {
    let date = iso.get(..10)?;
    let clock = iso.get(11..)?.strip_suffix('Z')?;
    if iso.get(10..11)? != "T" {
        return None;
    }
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let mut c = clock.split(':');
    let hour: u64 = c.next()?.parse().ok()?;
    let minute: u64 = c.next()?.parse().ok()?;
    let mut sec = c.next()?.split('.');
    let second: u64 = sec.next()?.parse().ok()?;
    let fraction = sec.next().unwrap_or("");
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || hour >= 24
        || minute >= 60
        || second >= 60
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || c.next().is_some()
        || d.next().is_some()
        || sec.next().is_some()
    {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day < 1 || day > days[(month - 1) as usize] {
        return None;
    }
    // Gregorian civil date to days since the Unix epoch.
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yo = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let days = era * 146097 + yo * 365 + yo / 4 - yo / 100 + doy - 719468;
    let frac: u64 = format!("{fraction}000").get(..3)?.parse().ok()?;
    u64::try_from(days)
        .ok()?
        .checked_mul(86_400_000)?
        .checked_add(((hour * 60 + minute) * 60 + second) * 1000 + frac)
}

pub fn iso_ms(ms: u64) -> String {
    let z = (ms / 86_400_000) as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(month <= 2);
    let seconds = ms / 1000;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        ms % 1000
    )
}

type Counts = BTreeMap<String, u64>;
fn bump(counts: &mut Counts, key: &str) {
    let key = text(key);
    let key = if counts.contains_key(&key) || counts.len() < MAX_KEYS {
        key
    } else {
        OTHER.into()
    };
    let count = counts.entry(key).or_default();
    *count = count.saturating_add(1);
}
fn top(counts: &Counts, limit: usize) -> Vec<Value> {
    top_counts(counts, limit)
        .into_iter()
        .map(|(name, count)| json!({"name":name,"count":count}))
        .collect()
}
fn top_counts(counts: &Counts, limit: usize) -> Vec<(String, u64)> {
    let mut rows: Vec<_> = counts.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    rows.into_iter()
        .take(limit)
        .map(|(name, count)| (name.clone(), *count))
        .collect()
}

/// Small typed renderer input. Recent animation headers stay in Mesh; the GUI
/// caches this summary by revision and wall-clock second, never serialising it.
/// Directional messages in [since start, last minute, last five minutes] order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkCounts {
    pub out: [u64; 3],
    pub input: [u64; 3],
}
impl LinkCounts {
    fn add(&mut self, other: Self) {
        for i in 0..3 {
            self.out[i] = self.out[i].saturating_add(other.out[i]);
            self.input[i] = self.input[i].saturating_add(other.input[i]);
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct ViewStats {
    pub services: Vec<(String, u64)>,
    pub commands: Vec<(String, u64)>,
}
#[derive(Clone, Debug, Default)]
pub struct ViewLink {
    pub counts: LinkCounts,
    pub stats: ViewStats,
    pub history: LinkHistory,
}

/// Oldest first, ending with the current (possibly incomplete) second.
/// Missing seconds are zero, including time before observation started.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkHistory {
    pub out: Vec<f64>,
    pub input: Vec<f64>,
    /// Cached shared scale, at least one message/s.
    pub max: f64,
}
impl Default for LinkHistory {
    fn default() -> Self {
        Self {
            out: vec![0.0; HISTORY_S],
            input: vec![0.0; HISTORY_S],
            max: 1.0,
        }
    }
}
#[derive(Clone, Debug)]
pub struct ViewPing {
    pub ok: bool,
    pub ms: Option<u64>,
    pub age_ms: u64,
    pub failed: bool,
}
#[derive(Clone, Debug, Default)]
pub struct MeshView {
    pub links: BTreeMap<String, ViewLink>,
    /// Zero histories for known peers without traffic; never synthetic links.
    pub quiet_histories: BTreeMap<String, LinkHistory>,
    pub total: LinkCounts,
    /// Same window order as LinkCounts.
    pub local_counts: [u64; 3],
    pub local_stats: ViewStats,
    pub pings: BTreeMap<String, ViewPing>,
    pub observing: bool,
    pub observe_error: Option<String>,
    pub last_event_age_ms: Option<u64>,
    pub ring_dropped: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Link {
    pub out_msgs: u64,
    pub out_bytes: u64,
    pub in_msgs: u64,
    pub in_bytes: u64,
    pub rejected: u64,
    pub dropped: u64,
    pub last_ms: Option<u64>,
    /// Only inbound traffic clears a failed ping; sending to a dead peer cannot.
    pub last_in_ms: Option<u64>,
    commands: Counts,
    services: Counts,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Local {
    pub msgs: u64,
    pub bytes: u64,
    pub rejected: u64,
    pub dropped: u64,
    pub last_ms: Option<u64>,
    services: Counts,
    commands: Counts,
}
#[derive(Clone, Debug, Serialize)]
pub struct Node {
    pub name: String,
    pub wg_ip: Option<String>,
    #[serde(rename = "self")]
    pub local: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct Ping {
    pub ok: bool,
    pub rc: Option<i64>,
    pub ms: Option<u64>,
    pub at_ms: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Recent {
    pub seq: u64,
    pub t: u64,
    pub dir: String,
    pub from: String,
    pub to: String,
    pub size: u64,
    pub command: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub outcome: String,
}
#[derive(Clone, Debug, Serialize)]
struct Pending {
    node: String,
    t: u64,
}

#[derive(Serialize)]
pub struct Mesh {
    pub self_name: String,
    pub observer: String,
    pub started_ms: u64,
    /// Arrival time of any accepted observation, including our own traffic.
    /// Broker timestamps still own traffic windows; they can be reused on a
    /// delayed reply, so they cannot measure the stream's liveness.
    pub last_observe_ms: Option<u64>,
    pub observing: bool,
    pub observe_error: Option<String>,
    pub nodes_error: Option<String>,
    pub nodes: Vec<Node>,
    pub links: BTreeMap<String, Link>,
    pub local: Local,
    pub recent: VecDeque<Recent>,
    pub pings: BTreeMap<String, Ping>,
    pub probe_at_ms: Option<u64>,
    pub seen: u64,
    pub ring_dropped: u64,
    pub recent_capped: u64,
    seq: u64,
    buckets: BTreeMap<u64, Counts>,
    pending: BTreeMap<String, Pending>,
    pub(crate) ping_queue: VecDeque<String>,
    pub(crate) ping_ticket: Option<u64>,
    pub(crate) nodes_ticket: Option<u64>,
    pub observe_ticket: Option<u64>,
    pub(crate) resume_observe: bool,
    pub(crate) topology_needed: bool,
    revision: u64,
    #[serde(skip)]
    pub clock: fn() -> u64,
}
impl Default for Mesh {
    fn default() -> Self {
        Self::new("local", "workshop", now_ms)
    }
}
impl Mesh {
    pub fn new(node: &str, observer: &str, clock: fn() -> u64) -> Self {
        Self {
            self_name: text(node),
            observer: text(observer),
            started_ms: clock(),
            last_observe_ms: None,
            observing: false,
            observe_error: None,
            nodes_error: None,
            nodes: Vec::new(),
            links: BTreeMap::new(),
            local: Local::default(),
            recent: VecDeque::new(),
            pings: BTreeMap::new(),
            probe_at_ms: None,
            seen: 0,
            ring_dropped: 0,
            recent_capped: 0,
            seq: 0,
            buckets: BTreeMap::new(),
            pending: BTreeMap::new(),
            ping_queue: VecDeque::new(),
            ping_ticket: None,
            nodes_ticket: None,
            observe_ticket: None,
            resume_observe: false,
            topology_needed: false,
            revision: 0,
            clock,
        }
    }
    pub fn now(&self) -> u64 {
        (self.clock)()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn set_observing(&mut self, on: bool, error: Option<&str>) {
        self.revision = self.revision.wrapping_add(1);
        self.observing = on;
        self.observe_error = if on { None } else { error.map(text) };
    }
    pub fn set_nodes(&mut self, info: &Value, peers: &Value) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(name) = info["name"].as_str().or_else(|| info["node"].as_str()) {
            self.self_name = text(name);
        }
        let mut nodes = BTreeMap::new();
        let entries = peers
            .pointer("/authority/routing_view")
            .or_else(|| peers.get("peers"));
        for peer in entries
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(4096)
        {
            let Some(name) = peer["name"].as_str() else {
                continue;
            };
            if name == self.self_name || peer["class"].as_str().is_some_and(|c| c != "active-bus") {
                continue;
            }
            if nodes.len() >= MAX_LINKS {
                break;
            }
            nodes.insert(
                text(name),
                Node {
                    name: text(name),
                    wg_ip: peer["wg_ip"]
                        .as_str()
                        .or_else(|| peer["mesh_ip"].as_str())
                        .map(text),
                    local: false,
                },
            );
        }
        self.nodes = vec![Node {
            name: self.self_name.clone(),
            wg_ip: info["wg_ip"]
                .as_str()
                .or_else(|| peers["wg_ip"].as_str())
                .map(text),
            local: true,
        }];
        self.nodes.extend(nodes.into_values());
        self.pings
            .retain(|name, _| self.nodes.iter().any(|n| !n.local && n.name == *name));
        self.nodes_error = None;
    }
    /// Retain only known peers and the fields shown by the map. Missing results
    /// preserve the previous ping, with its own completion time.
    pub fn set_probe(&mut self, results: &Value, now: u64) {
        self.revision = self.revision.wrapping_add(1);
        self.probe_at_ms = Some(now);
        for node in self.nodes.iter().filter(|n| !n.local) {
            if let Some(result) = results.get(&node.name).filter(|r| r.is_object()) {
                let ping = Ping {
                    ok: result["ok"] == true,
                    rc: result["rc"].as_i64(),
                    ms: result["ms"].as_u64(),
                    at_ms: result["at"].as_u64().unwrap_or(now),
                };
                if self
                    .pings
                    .get(&node.name)
                    .is_none_or(|old| old.at_ms <= ping.at_ms)
                {
                    self.pings.insert(node.name.clone(), ping);
                }
            }
        }
    }
    pub fn ping_failed(&self, node: &str) -> bool {
        self.pings.get(node).is_some_and(|ping| {
            !ping.ok
                && self
                    .links
                    .get(node)
                    .and_then(|link| link.last_in_ms)
                    .is_none_or(|last| last <= ping.at_ms)
        })
    }
    pub fn add_event(&mut self, event: &Value) {
        let now = self.now();
        self.prune(now);
        self.add_event_at(event, now);
    }
    /// Oracle add-then-prune surface. Production ingestion prunes once before
    /// this step, so expired correlations and clock-step buckets cannot survive.
    pub fn add_event_at(&mut self, event: &Value, now: u64) {
        self.revision = self.revision.wrapping_add(1);
        self.last_observe_ms = Some(now);
        self.seen = self.seen.saturating_add(1);
        self.ring_dropped = self
            .ring_dropped
            .saturating_add(event["dropped_count"].as_u64().unwrap_or(0));
        if ["from", "to"]
            .iter()
            .any(|key| event[*key].as_str() == Some(self.observer.as_str()))
        {
            return;
        }
        let dir = event["direction"].as_str().unwrap_or("");
        if !matches!(dir, "local" | "mesh_out" | "mesh_in") {
            return;
        }
        let t = event["ts"].as_str().and_then(ts_ms).unwrap_or(now).min(now);
        let size = event["size"].as_u64().unwrap_or(0);
        let command = text(event["command"].as_str().unwrap_or(UNKNOWN));
        let outcome = event["outcome"].as_str().unwrap_or("");
        let message_type = event["message_type"].as_str().unwrap_or("");
        let (from, to, bucket) = if dir == "local" {
            let local = &mut self.local;
            local.msgs = local.msgs.saturating_add(1);
            local.bytes = local.bytes.saturating_add(size);
            local.rejected = local
                .rejected
                .saturating_add(u64::from(outcome == "rejected"));
            local.dropped = local
                .dropped
                .saturating_add(u64::from(outcome == "dropped"));
            local.last_ms = Some(local.last_ms.unwrap_or(0).max(t));
            bump(&mut local.services, service_of(event["to"].as_str()));
            bump(&mut local.commands, &command);
            (
                self.self_name.clone(),
                self.self_name.clone(),
                "local".into(),
            )
        } else {
            let out = dir == "mesh_out";
            let corr = event["correlation_id"]
                .as_str()
                .filter(|s| s.len() <= TEXT_MAX);
            let pending = corr.and_then(|c| self.pending.get(c));
            let named = event["peer"].as_str().filter(|s| !s.is_empty());
            let address = event[if out { "to" } else { "from" }]
                .as_str()
                .and_then(node_of);
            let mut peer = text(
                named
                    .or(address)
                    // A reused correlation on an unrelated incoming request
                    // must not place it on the outgoing request's link.
                    .or_else(|| {
                        pending
                            .filter(|p| {
                                message_type == "response" && now.saturating_sub(p.t) <= PENDING_MS
                            })
                            .map(|p| p.node.as_str())
                    })
                    .unwrap_or(UNKNOWN),
            );
            if peer != UNKNOWN && !self.nodes.iter().any(|n| n.name == peer) {
                self.topology_needed = true;
            }
            if !self.links.contains_key(&peer) && self.links.len() >= MAX_LINKS {
                peer = UNKNOWN.into();
            }
            if let Some(corr) = corr {
                if out && message_type == "request" {
                    self.pending.insert(
                        corr.into(),
                        Pending {
                            node: peer.clone(),
                            t,
                        },
                    );
                } else if message_type == "response" {
                    self.pending.remove(corr);
                }
            }
            let link = self.links.entry(peer.clone()).or_default();
            if out {
                link.out_msgs = link.out_msgs.saturating_add(1);
                link.out_bytes = link.out_bytes.saturating_add(size);
            } else {
                link.in_msgs = link.in_msgs.saturating_add(1);
                link.in_bytes = link.in_bytes.saturating_add(size);
                link.last_in_ms = Some(link.last_in_ms.unwrap_or(0).max(t));
            }
            link.rejected = link
                .rejected
                .saturating_add(u64::from(outcome == "rejected"));
            link.dropped = link.dropped.saturating_add(u64::from(outcome == "dropped"));
            link.last_ms = Some(link.last_ms.unwrap_or(0).max(t));
            bump(&mut link.commands, &command);
            bump(
                &mut link.services,
                service_of(event[if out { "to" } else { "from" }].as_str()),
            );
            let bucket = format!("{}:{peer}", if out { "out" } else { "in" });
            if out {
                (self.self_name.clone(), peer, bucket)
            } else {
                (peer, self.self_name.clone(), bucket)
            }
        };
        let count = self
            .buckets
            .entry(t / 1000)
            .or_default()
            .entry(bucket)
            .or_default();
        *count = count.saturating_add(1);
        self.seq = self.seq.saturating_add(1);
        self.recent.push_back(Recent {
            seq: self.seq,
            t,
            dir: dir.into(),
            from,
            to,
            size,
            command,
            message_type: text(message_type),
            outcome: text(outcome),
        });
        self.bound();
    }
    pub fn prune(&mut self, now: u64) {
        self.revision = self.revision.wrapping_add(1);
        self.recent.retain(|r| now.saturating_sub(r.t) <= RECENT_MS);
        self.buckets.retain(|second, _| {
            *second >= (now / 1000).saturating_sub(WINDOW_S)
                && *second <= (now / 1000).saturating_add(5)
        });
        self.pending
            .retain(|_, p| now.saturating_sub(p.t) <= PENDING_MS);
        self.bound();
    }
    fn bound(&mut self) {
        while self.recent.len() > RECENT_MAX {
            self.recent.pop_front();
            self.recent_capped = self.recent_capped.saturating_add(1);
        }
        // Inclusive lower boundary matches tally.mix; at most 301 seconds,
        // even with out-of-order or future-dated broker timestamps.
        while self.buckets.len() > WINDOW_S as usize + 1 {
            self.buckets.pop_first();
        }
        while self.pending.len() > PENDING_MAX {
            let oldest = self
                .pending
                .iter()
                .min_by_key(|(_, p)| p.t)
                .map(|(k, _)| k.clone())
                .unwrap();
            self.pending.remove(&oldest);
        }
    }
    pub fn window(&self, key: &str, from_ms: u64) -> u64 {
        self.buckets
            .range(from_ms / 1000..)
            .fold(0_u64, |sum, (_, counts)| {
                sum.saturating_add(*counts.get(key).unwrap_or(&0))
            })
    }
    fn window_at(&self, key: &str, from_ms: u64, now: u64) -> u64 {
        self.buckets
            .range(from_ms / 1000..=(now / 1000).saturating_add(5))
            .fold(0_u64, |sum, (_, counts)| {
                sum.saturating_add(*counts.get(key).unwrap_or(&0))
            })
    }
    /// Exactly 300 one-second buckets, [now-299 s, now], from existing
    /// bounded storage. Unlike aggregate inclusive windows this has exactly
    /// 300 cells. Future buckets after a clock step back are ignored.
    pub fn link_history_at(&self, peer: &str, now: u64) -> LinkHistory {
        let mut history = LinkHistory::default();
        let out = format!("out:{peer}");
        let input = format!("in:{peer}");
        for i in 0..HISTORY_S {
            let Some(second) = (now / 1000).checked_sub((HISTORY_S - 1 - i) as u64) else {
                continue;
            };
            if let Some(bucket) = self.buckets.get(&second) {
                history.out[i] = bucket.get(&out).copied().unwrap_or(0) as f64;
                history.input[i] = bucket.get(&input).copied().unwrap_or(0) as f64;
                history.max = history.max.max(history.out[i]).max(history.input[i]);
            }
        }
        history
    }

    pub fn view_at(&self, now: u64) -> MeshView {
        let m1 = now.saturating_sub(60_000);
        let m5 = now.saturating_sub(WINDOW_S * 1000);
        let mut view = MeshView {
            local_counts: [
                self.local.msgs,
                self.window_at("local", m1, now),
                self.window_at("local", m5, now),
            ],
            local_stats: ViewStats {
                services: top_counts(&self.local.services, 6),
                commands: top_counts(&self.local.commands, 5),
            },
            observing: self.observing,
            observe_error: self.observe_error.clone(),
            last_event_age_ms: self.last_observe_ms.map(|last| now.saturating_sub(last)),
            ring_dropped: self.ring_dropped,
            ..Default::default()
        };
        for (peer, link) in &self.links {
            let out = format!("out:{peer}");
            let input = format!("in:{peer}");
            let counts = LinkCounts {
                out: [
                    link.out_msgs,
                    self.window_at(&out, m1, now),
                    self.window_at(&out, m5, now),
                ],
                input: [
                    link.in_msgs,
                    self.window_at(&input, m1, now),
                    self.window_at(&input, m5, now),
                ],
            };
            view.total.add(counts);
            view.links.insert(
                peer.clone(),
                ViewLink {
                    counts,
                    history: self.link_history_at(peer, now),
                    stats: ViewStats {
                        services: top_counts(&link.services, 6),
                        commands: top_counts(&link.commands, 5),
                    },
                },
            );
        }
        // Keep quiet peers' zero histories separate from observed links.
        for node in self.nodes.iter().filter(|node| !node.local) {
            if !view.links.contains_key(&node.name) {
                view.quiet_histories
                    .insert(node.name.clone(), LinkHistory::default());
            }
        }
        view.pings = self
            .pings
            .iter()
            .map(|(name, ping)| {
                (
                    name.clone(),
                    ViewPing {
                        ok: ping.ok,
                        ms: ping.ms,
                        age_ms: now.saturating_sub(ping.at_ms),
                        failed: self.ping_failed(name),
                    },
                )
            })
            .collect();
        view
    }
    pub fn snapshot(&self) -> Value {
        self.snapshot_at(self.now())
    }
    pub fn snapshot_at(&self, now: u64) -> Value {
        let m1 = now.saturating_sub(60_000);
        let m5 = now.saturating_sub(WINDOW_S * 1000);
        let links: Vec<_> = self.links.iter().map(|(peer, link)| json!({
            "a":self.self_name,"b":peer,"out_msgs":link.out_msgs,"out_bytes":link.out_bytes,
            "in_msgs":link.in_msgs,"in_bytes":link.in_bytes,
            "out_1m":self.window_at(&format!("out:{peer}"),m1,now),"in_1m":self.window_at(&format!("in:{peer}"),m1,now),
            "out_5m":self.window_at(&format!("out:{peer}"),m5,now),"in_5m":self.window_at(&format!("in:{peer}"),m5,now),
            "rejected":link.rejected,"dropped":link.dropped,"top_commands":top(&link.commands,5),
            "top_services":top(&link.services,6),"last_at":link.last_ms.map(iso_ms),
        })).collect();
        let results: BTreeMap<_, _> = self
            .pings
            .iter()
            .map(|(name, ping)| {
                (
                    name,
                    json!({
                        "ok":ping.ok,"rc":ping.rc,"ms":ping.ms,"at":iso_ms(ping.at_ms),
                        "age_ms":now.saturating_sub(ping.at_ms),"failed":self.ping_failed(name)
                    }),
                )
            })
            .collect();
        let recent: Vec<_> = self
            .recent
            .iter()
            .filter(|r| now.saturating_sub(r.t) <= RECENT_MS)
            .map(|r| {
                let mut value = serde_json::to_value(r).unwrap();
                value["t"] = json!(iso_ms(r.t));
                value
            })
            .collect();
        json!({"written_at":iso_ms(now),"this_node":self.nodes.first(),
            "peers":self.nodes.iter().filter(|n| !n.local).collect::<Vec<_>>(),"nodes":self.nodes,
            "observer":{"name":self.observer,"node":self.self_name,"started_at":iso_ms(self.started_ms),
                "observing":self.observing,"observe_error":self.observe_error,"events_seen":self.seen,
                "last_event_at":self.last_observe_ms.map(iso_ms),
                "last_event_age_ms":self.last_observe_ms.map(|last| now.saturating_sub(last)),
                "ring_dropped":self.ring_dropped,"recent_capped":self.recent_capped},
            "nodes_error":self.nodes_error,"links":links,
            "local":{"msgs":self.local.msgs,"bytes":self.local.bytes,"msgs_1m":self.window_at("local",m1,now),
                "msgs_5m":self.window_at("local",m5,now),"rejected":self.local.rejected,"dropped":self.local.dropped,
                "top_services":top(&self.local.services,6),"top_commands":top(&self.local.commands,5),
                "last_at":self.local.last_ms.map(iso_ms)},
            "probe":{"at":self.probe_at_ms.map(iso_ms),"results":results,"running":self.ping_ticket.is_some()},"recent":recent})
    }
}

impl crate::Engine {
    /// Selection and reconnect use the same observe operation as Traffic.
    /// A refusal stays visible until another explicit selection/reconnect.
    pub fn ensure_observing(&mut self) {
        if self.connected
            && !self.quitting
            && self.workshop.subscription.is_none()
            && self.workshop.mesh.observe_ticket.is_none()
        {
            self.queue_workshop("observe", json!({"on":true}), None);
        }
    }
    pub fn refresh_mesh(&mut self) {
        if self.connected && !self.quitting && self.workshop.mesh.nodes_ticket.is_none() {
            self.queue_workshop("mesh.nodes", json!({}), None);
        }
    }
    pub fn can_ping_mesh(&self) -> bool {
        self.connected
            && !self.quitting
            && self.ui.dialog.is_none()
            && self.workshop.mesh.ping_ticket.is_none()
            && self.workshop.mesh.nodes.iter().any(|node| !node.local)
    }
    pub fn mesh_ping(&mut self) -> Result<Value, (&'static str, String)> {
        if self.quitting {
            return Err(("BUSY", "Workshop is closing".into()));
        }
        if !self.connected {
            return Err(("DISCONNECTED", self.text("disconnected")));
        }
        if self.workshop.mesh.ping_ticket.is_some() {
            return Err(("BUSY", self.text("mesh-ping-busy")));
        }
        let peers: Vec<_> = self
            .workshop
            .mesh
            .nodes
            .iter()
            .filter(|n| !n.local)
            .map(|n| n.name.clone())
            .collect();
        if peers.is_empty() {
            return Err(("UNAVAILABLE", self.text("no-peers")));
        }
        self.workshop.mesh.ping_queue = peers.iter().cloned().collect();
        self.next_mesh_ping();
        Ok(json!({"pinging":peers}))
    }
    fn next_mesh_ping(&mut self) {
        if let Some(node) = self.workshop.mesh.ping_queue.pop_front() {
            self.queue_workshop(
                "mesh.ping",
                json!({"node":node,"service":format!("noded.{node}.bus"),
                "command":"noded.ping","timeout_ms":2000}),
                None,
            );
        }
    }
    /// Apply effect results in the engine, including tickets invalidated by a
    /// disconnect. Stale connection work must never resurrect its observer.
    pub fn mesh_completed(
        &mut self,
        ticket: u64,
        operation: &str,
        args: &Value,
        body: &Value,
    ) -> bool {
        match operation {
            "mesh.nodes" => {
                if self.workshop.mesh.nodes_ticket != Some(ticket) {
                    return false;
                }
                self.workshop.mesh.nodes_ticket = None;
                if body.get("error_code").is_some() {
                    self.workshop.mesh.nodes_error =
                        Some(text(body["message"].as_str().unwrap_or("unavailable")));
                } else {
                    self.workshop.mesh.set_nodes(&body["info"], &body["peers"]);
                }
            }
            "mesh.ping" => {
                if self.workshop.mesh.ping_ticket != Some(ticket) {
                    return false;
                }
                self.workshop.mesh.ping_ticket = None;
                if let Some(node) = args["node"].as_str() {
                    self.workshop
                        .mesh
                        .set_probe(&json!({node:body}), self.workshop.mesh.now());
                }
                if self.connected {
                    self.next_mesh_ping();
                }
            }
            "observe" => {
                if self.workshop.mesh.observe_ticket != Some(ticket) {
                    return false;
                }
                self.workshop.mesh.observe_ticket = None;
                let refused = body.get("error_code").is_some();
                if !refused {
                    self.workshop.subscription =
                        body["subscription_id"].as_str().map(str::to_owned);
                }
                let on = self.workshop.subscription.is_some();
                self.workshop
                    .mesh
                    .set_observing(on, body["message"].as_str());
            }
            _ => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clock() -> u64 {
        ts_ms("2026-10-25T13:10:00.000Z").unwrap()
    }
    fn base() -> u64 {
        ts_ms("2026-10-25T14:00:00.000Z").unwrap()
    }
    fn state() -> Mesh {
        Mesh::new("alpha", "observer", clock)
    }
    #[test]
    fn typed_view_matches_bus_windows_and_ages_without_recent_json() {
        let mut mesh = state();
        nodes(&mut mesh);
        let before = mesh.revision();
        mesh.add_event_at(
            &json!({"direction":"mesh_in","peer":"beta","command":"noded.info"}),
            base(),
        );
        mesh.add_event_at(
            &json!({"direction":"local","to":"webd","command":"webd.info"}),
            base(),
        );
        mesh.set_probe(&json!({"beta":{"ok":true,"ms":3}}), base());
        assert!(mesh.revision() > before);
        for age in [0, 61_000, 301_000] {
            let view = mesh.view_at(base() + age);
            let json = mesh.snapshot_at(base() + age);
            let link = json["links"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["b"] == "beta")
                .unwrap();
            for (i, field) in ["in_msgs", "in_1m", "in_5m"].iter().enumerate() {
                assert_eq!(
                    view.links["beta"].counts.input[i],
                    link[*field].as_u64().unwrap()
                );
            }
            assert_eq!(
                view.local_counts[1],
                json["local"]["msgs_1m"].as_u64().unwrap()
            );
            assert_eq!(view.last_event_age_ms, Some(age));
            assert_eq!(view.pings["beta"].age_ms, age);
            assert_eq!(view.total.input, view.links["beta"].counts.input);
        }
    }

    #[test]
    fn future_events_clamp_and_clock_step_back_prunes_future_buckets() {
        let mut mesh = state();
        mesh.add_event_at(
            &json!({"direction":"mesh_out","peer":"beta","ts":iso_ms(base() + 3_600_000)}),
            base(),
        );
        assert_eq!(
            mesh.buckets.keys().copied().collect::<Vec<_>>(),
            [base() / 1000]
        );
        assert_eq!(mesh.recent.back().unwrap().t, base());
        mesh.add_event_at(
            &json!({"direction":"mesh_in","peer":"beta"}),
            base() + 10_000,
        );
        // A read also ignores far-future buckets before the next ingestion.
        assert_eq!(mesh.snapshot_at(base())["links"][0]["in_5m"], 0);
        mesh.clock = base;
        mesh.add_event(&json!({"direction":"local","from":"observer"}));
        assert_eq!(mesh.buckets.len(), 1);
        assert_eq!(mesh.view_at(base()).links["beta"].counts.input[2], 0);
        assert_eq!(
            mesh.links["beta"].in_msgs, 1,
            "totals survive clock changes"
        );
        // The tolerated boundary is inclusive; seconds beyond it are removed.
        mesh.buckets.insert(base() / 1000 + 5, Counts::new());
        mesh.buckets.insert(base() / 1000 + 6, Counts::new());
        mesh.prune(base());
        assert!(mesh.buckets.contains_key(&(base() / 1000 + 5)));
        assert!(!mesh.buckets.contains_key(&(base() / 1000 + 6)));
    }

    #[test]
    fn joining_peer_refreshes_once_in_flight_and_map_kind_is_distinct() {
        let mut engine = crate::Engine::new(str::to_owned);
        engine.take_effects();
        nodes(&mut engine.workshop.mesh);
        engine.workshop.subscription = Some("live".into());
        for _ in 0..3 {
            engine.workshop_command(
                0,
                "noded.observe.event",
                &json!({"subscription_id":"live","direction":"mesh_in","peer":"delta"}),
            );
        }
        let effects = engine.take_effects();
        assert_eq!(effects.iter().filter(|e| matches!(e, crate::Effect::Workshop { operation, .. } if operation == "mesh.nodes")).count(), 1);
        let ticket = engine.workshop.mesh.nodes_ticket.unwrap();
        engine.mesh_completed(
            ticket,
            "mesh.nodes",
            &json!({}),
            &json!({"info":{"name":"alpha"},"peers":{"peers":[{"name":"delta"}]}}),
        );
        engine.workshop_command(
            0,
            "noded.observe.event",
            &json!({"subscription_id":"live","direction":"mesh_in","peer":"delta"}),
        );
        assert!(engine.take_effects().is_empty());
        assert_eq!(engine.workshop.mesh.links["delta"].in_msgs, 4);
        engine.ui.expanded.insert("nodes".into());
        let rows = engine.rows();
        assert_eq!(
            rows.iter().find(|r| r["key"] == "nodes:mesh").unwrap()["kind"],
            "mesh_map"
        );
        assert_eq!(
            rows.iter().find(|r| r["key"] == "mesh").unwrap()["kind"],
            "peers"
        );
    }

    #[test]
    fn traffic_beyond_topology_and_link_caps_survives_as_unplaced_counts() {
        let mut mesh = state();
        let peers: Vec<_> = (0..MAX_LINKS + 2)
            .map(|i| json!({"name":format!("peer-{i:02}")}))
            .collect();
        mesh.set_nodes(&json!({"name":"alpha"}), &json!({"peers":peers}));
        assert_eq!(mesh.nodes.len(), MAX_LINKS + 1);
        let missing = format!("peer-{:02}", MAX_LINKS);
        mesh.add_event_at(&json!({"direction":"mesh_in","peer":missing}), base());
        assert_eq!(mesh.view_at(base()).links[&missing].counts.input[0], 1);
        assert!(!mesh.nodes.iter().any(|n| n.name == missing));
        for i in 0..MAX_LINKS + 2 {
            mesh.add_event_at(
                &json!({"direction":"mesh_in","peer":format!("peer-{i:02}")}),
                base(),
            );
        }
        let view = mesh.view_at(base());
        assert_eq!(view.total.input[0], MAX_LINKS as u64 + 3);
        assert!(view.links[UNKNOWN].counts.input[0] > 0);
        let unplaced: u64 = view
            .links
            .iter()
            .filter(|(name, _)| !mesh.nodes.iter().any(|n| n.name == **name))
            .map(|(_, l)| l.counts.input[0])
            .sum();
        assert!(unplaced >= 3);
    }

    #[test]
    fn reconnect_resumes_only_previously_enabled_observation_and_closing_ping_refuses() {
        for on in [false, true] {
            let mut engine = crate::Engine::new(str::to_owned);
            engine.take_effects();
            if on {
                engine.workshop.subscription = Some("live".into());
                engine.workshop.mesh.set_observing(true, None);
            }
            engine.delivery(crate::bus::Delivery::Disconnected);
            engine.delivery(crate::bus::Delivery::Disconnected);
            engine.delivery(crate::bus::Delivery::Connected);
            let effects = engine.take_effects();
            assert_eq!(effects.iter().any(|e| matches!(e, crate::Effect::Workshop { operation, .. } if operation == "observe")), on);
            if !on {
                assert!(engine.workshop.mesh.observe_error.is_none());
            }
        }
        let mut engine = crate::Engine::new(str::to_owned);
        engine.take_effects();
        engine.quitting = true;
        engine.workshop.mesh.ping_ticket = Some(1);
        assert_eq!(
            engine.mesh_ping().unwrap_err(),
            ("BUSY", "Workshop is closing".into())
        );
    }

    #[test]
    fn concurrent_observe_commands_return_busy_and_explicit_stop_survives_reconnect() {
        let mut engine = crate::Engine::new(str::to_owned);
        engine.take_effects();
        engine.command(1, "workshop.observe", r#"{"on":true}"#);
        engine.take_effects();
        engine.command(2, "workshop.observe", r#"{"on":false}"#);
        assert!(
            matches!(&engine.take_effects()[..], [crate::Effect::Reply { id:2, rc:10, body }] if body["error_code"] == "BUSY")
        );
        let ticket = engine.workshop.mesh.observe_ticket.unwrap();
        engine.mesh_completed(
            ticket,
            "observe",
            &json!({"on":true}),
            &json!({"subscription_id":"live"}),
        );
        engine.command(3, "workshop.observe", r#"{"on":false}"#);
        engine.take_effects();
        // Disconnect while the stop is in flight also respects that intent.
        engine.delivery(crate::bus::Delivery::Disconnected);
        engine.delivery(crate::bus::Delivery::Connected);
        assert!(!engine.take_effects().iter().any(
            |e| matches!(e, crate::Effect::Workshop { operation, .. } if operation == "observe")
        ));
    }
    #[test]
    fn custom_observer_excludes_from_and_to_in_every_direction_but_counts_stream_health() {
        let mut st = Mesh::new("alpha", "workshop-example", base);
        let mut seen = 0;
        for direction in ["local", "mesh_out", "mesh_in"] {
            for own_field in ["from", "to"] {
                seen += 1;
                let mut event = json!({"direction":direction,"peer":"beta","from":"other","to":"webd",
                    "message_type":"request","correlation_id":"self","dropped_count":seen});
                event[own_field] = json!("workshop-example");
                st.add_event_at(&event, base());
            }
        }
        assert_eq!(st.seen, 6);
        assert_eq!(st.ring_dropped, 21);
        assert_eq!(st.last_observe_ms, Some(base()));
        assert_eq!(st.local.msgs, 0);
        assert!(
            st.links.is_empty()
                && st.recent.is_empty()
                && st.pending.is_empty()
                && st.buckets.is_empty()
        );
        st.add_event_at(
            &json!({"direction":"local","from":"workshop","to":"webd"}),
            base(),
        );
        assert_eq!(
            st.local.msgs, 1,
            "only this instance's Bus name is excluded"
        );
    }

    #[test]
    fn placement_prefers_peer_then_directional_address_then_response_correlation() {
        let mut st = state();
        st.add_event_at(
            &json!({"direction":"mesh_out","peer":"gamma","to":"noded.beta.bus",
            "message_type":"request","correlation_id":"named"}),
            base(),
        );
        assert_eq!(st.links["gamma"].out_msgs, 1);
        assert_eq!(st.pending["named"].node, "gamma");
        st.add_event_at(
            &json!({"direction":"mesh_in","peer":"delta","from":"noded.beta.bus",
            "message_type":"response","correlation_id":"named"}),
            base(),
        );
        assert_eq!(st.links["delta"].in_msgs, 1);
        assert!(!st.pending.contains_key("named"));
        for peer in [json!(""), json!(null), json!(17)] {
            st.add_event_at(
                &json!({"direction":"mesh_out","peer":peer,"to":"noded.beta.bus"}),
                base(),
            );
            st.add_event_at(
                &json!({"direction":"mesh_in","peer":peer,"from":"noded.beta.bus"}),
                base(),
            );
        }
        assert_eq!(
            (st.links["beta"].out_msgs, st.links["beta"].in_msgs),
            (3, 3)
        );
        st.add_event_at(&json!({"direction":"mesh_out","to":"beta.bus","message_type":"request","correlation_id":"c1"}),base());
        st.add_event_at(&json!({"direction":"mesh_in","from":"noded","message_type":"response","correlation_id":"c1"}),base());
        assert_eq!(st.links["beta"].in_msgs, 4);
        assert!(st.pending.is_empty());
        st.add_event_at(&json!({"direction":"mesh_out","peer":"beta","message_type":"request","correlation_id":"address"}),base());
        st.add_event_at(&json!({"direction":"mesh_in","from":"noded.gamma.bus","message_type":"response","correlation_id":"address"}),base());
        assert_eq!(
            st.links["gamma"].in_msgs, 1,
            "a sender address wins over pending correlation"
        );
        assert_eq!(st.links["beta"].in_msgs, 4);
        assert!(st.pending.is_empty());
    }

    #[test]
    fn only_requests_create_correlations_and_only_responses_can_use_them() {
        let mut st = state();
        st.add_event_at(&json!({"direction":"mesh_in","peer":"beta","message_type":"request","correlation_id":"inbound"}),base());
        st.add_event_at(&json!({"direction":"mesh_out","peer":"beta","message_type":"event","correlation_id":"event"}),base());
        assert!(st.pending.is_empty());
        st.add_event_at(&json!({"direction":"mesh_out","peer":"beta","message_type":"request","correlation_id":"c1"}),base());
        for message_type in ["request", "event"] {
            st.add_event_at(&json!({"direction":"mesh_in","from":"noded","message_type":message_type,"correlation_id":"c1"}),base());
        }
        assert_eq!(st.links[UNKNOWN].in_msgs, 2);
        assert_eq!(st.pending.len(), 1);
        st.add_event_at(&json!({"direction":"mesh_in","from":"noded","message_type":"response","correlation_id":"c1"}),base());
        assert_eq!(st.links["beta"].in_msgs, 2);
        assert!(st.pending.is_empty());
        st.add_event_at(&json!({"direction":"mesh_out","peer":"beta","message_type":"request","correlation_id":"c2"}),base());
        st.add_event_at(
            &json!({"direction":"mesh_out","message_type":"response","correlation_id":"c2"}),
            base(),
        );
        assert!(
            st.pending.is_empty(),
            "a response consumes its pending correlation"
        );
    }

    #[test]
    fn expired_correlation_cannot_place_a_reply_even_before_explicit_prune() {
        let mut st = state();
        st.add_event_at(&json!({"direction":"mesh_out","peer":"beta","message_type":"request","correlation_id":"c1"}),base());
        st.add_event_at(
            &json!({"direction":"mesh_in","message_type":"response","correlation_id":"c1"}),
            base() + PENDING_MS + 1,
        );
        assert_eq!(st.links[UNKNOWN].in_msgs, 1);
        assert_eq!(st.links["beta"].in_msgs, 0);
        assert!(st.pending.is_empty());
        let later = base() + PENDING_MS + 1;
        st.add_event_at(&json!({"direction":"mesh_out","peer":"beta","message_type":"request","correlation_id":"c2"}),later);
        st.prune(later + PENDING_MS + 1);
        assert!(
            st.pending.is_empty(),
            "unanswered requests expire after 60 seconds"
        );
    }

    #[test]
    fn link_commands_and_services_overflow_into_other_without_losing_counts() {
        let mut st = state();
        for i in 0..70 {
            st.add_event_at(&json!({"direction":"mesh_out","to":format!("service-{i}.beta.bus"),"command":format!("command-{i}")}),base());
        }
        let link = &st.links["beta"];
        assert_eq!(
            (link.out_msgs, link.commands.len(), link.services.len()),
            (70, MAX_KEYS + 1, MAX_KEYS + 1)
        );
        assert_eq!((link.commands[OTHER], link.services[OTHER]), (30, 30));
        assert_eq!(st.snapshot_at(base())["links"][0]["out_1m"], 70);
    }

    #[test]
    fn stream_health_counts_loss_before_own_traffic_exclusion_and_saturates() {
        let mut engine = crate::Engine::new(str::to_owned);
        engine.take_effects();
        engine.workshop.mesh = Mesh::new("alpha", "workshop-example", base);
        engine.workshop.subscription = Some("live".into());
        engine.workshop.mesh.set_observing(true, None);
        engine.workshop.paused = true;
        for dropped in [u64::MAX, 1] {
            engine.workshop_command(
                0,
                "noded.observe.event",
                &json!({"subscription_id":"live",
                "direction":"local","from":"workshop-example","dropped_count":dropped}),
            );
        }
        engine.command(42, "workshop.mesh", "{}");
        let crate::Effect::Reply { rc, body, .. } = engine.take_effects().pop().unwrap() else {
            panic!("mesh reply")
        };
        assert_eq!(rc, 0);
        assert_eq!(body["observer"]["ring_dropped"], u64::MAX);
        assert_eq!(body["observer"]["events_seen"], 2);
        assert_eq!(body["observer"]["last_event_age_ms"], 0);
        assert_eq!(body["local"]["msgs"], 0);
        assert!(engine.workshop.traffic.is_empty());
    }

    #[test]
    fn last_observe_age_uses_arrival_clock_including_own_events_and_survives_disconnect() {
        let mut st = state();
        assert!(st.snapshot_at(base())["observer"]["last_event_at"].is_null());
        assert!(st.snapshot_at(base())["observer"]["last_event_age_ms"].is_null());
        st.set_observing(true, None);
        st.add_event_at(
            &json!({"direction":"local","from":"observer","ts":iso_ms(base()-90_000)}),
            base(),
        );
        let snap = st.snapshot_at(base() + 45_000);
        assert_eq!(snap["observer"]["last_event_at"], iso_ms(base()));
        assert_eq!(snap["observer"]["last_event_age_ms"], 45_000);
        assert_eq!(
            snap["observer"]["observing"], true,
            "silence does not stop a live subscription"
        );
        assert_eq!(st.local.msgs, 0);
        st.set_observing(false, Some("broker disconnected"));
        let snap = st.snapshot_at(base() + 50_000);
        assert_eq!(snap["observer"]["last_event_age_ms"], 50_000);
        assert_eq!(snap["observer"]["observing"], false);
        assert_eq!(snap["observer"]["observe_error"], "broker disconnected");
    }

    #[test]
    fn observing_and_reading_a_quiet_mesh_never_queue_a_ping_or_topology_poll() {
        let mut engine = crate::Engine::new(str::to_owned);
        engine.take_effects();
        nodes(&mut engine.workshop.mesh);
        engine.workshop.mesh.clock = base;
        engine.workshop.mesh.set_observing(true, None);
        engine.workshop.subscription = Some("live".into());
        for _ in 0..3 {
            engine.workshop_command(
                0,
                "noded.observe.event",
                &json!({"subscription_id":"live","direction":"mesh_in","peer":"beta"}),
            );
            assert!(engine.take_effects().is_empty());
            engine.command(42, "workshop.mesh", "{}");
            assert!(matches!(
                &engine.take_effects()[..],
                [crate::Effect::Reply { rc: 0, .. }]
            ));
        }
        engine.delivery(crate::bus::Delivery::Disconnected);
        assert!(engine.take_effects().is_empty());
        assert!(engine.workshop.subscription.is_none());
        assert!(!engine.workshop.mesh.observing);
    }

    #[test]
    fn mesh_retains_only_headers_and_does_not_infer_rtt_from_broker_timestamps() {
        let mut st = state();
        for (direction, message_type) in [("mesh_out", "request"), ("mesh_in", "response")] {
            st.add_event_at(
                &json!({"direction":direction,"message_type":message_type,"peer":"beta",
                "command":"noded.info","correlation_id":"c1","ts":iso_ms(base()),
                "body":"BODY_ONLY_MARKER","payload":{"data":"PAYLOAD_ONLY_MARKER"}}),
                base(),
            );
        }
        let snap = st.snapshot_at(base());
        assert_eq!(
            (st.links["beta"].out_msgs, st.links["beta"].in_msgs),
            (1, 1)
        );
        assert!(snap["probe"]["results"].as_object().unwrap().is_empty());
        for value in snap["links"]
            .as_array()
            .unwrap()
            .iter()
            .chain(snap["recent"].as_array().unwrap())
        {
            for key in ["ms", "rtt", "body", "payload"] {
                assert!(value.get(key).is_none(), "{key}");
            }
        }
        for retained in [snap.to_string(), serde_json::to_string(&st).unwrap()] {
            assert!(!retained.contains("BODY_ONLY_MARKER"));
            assert!(!retained.contains("PAYLOAD_ONLY_MARKER"));
        }
    }
    // Keep the reference oracle's event argument order visible in the port.
    #[allow(clippy::too_many_arguments)]
    fn event(
        ts: &str,
        direction: &str,
        from: Option<&str>,
        to: Option<&str>,
        command: &str,
        kind: &str,
        corr: Option<&str>,
        outcome: &str,
        size: u64,
    ) -> Value {
        json!({"ts":ts,"direction":direction,"from":from,"to":to,"command":command,
            "message_type":kind,"correlation_id":corr,"outcome":outcome,"size":size,"dropped_count":0})
    }
    fn nodes(st: &mut Mesh) {
        st.set_nodes(
            &json!({"name":"alpha","wg_ip":"192.0.2.1"}),
            &json!({"peers":[
            {"name":"gamma","wg_ip":"192.0.2.3"},{"name":"beta","wg_ip":"192.0.2.2"},
            {"name":"alpha","wg_ip":"192.0.2.1"}]}),
        );
    }
    #[test]
    fn oracle_addresses_and_times() {
        assert_eq!(node_of("noded.beta.bus"), Some("beta"));
        assert_eq!(
            [node_of("webd"), None::<&str>.and_then(node_of)],
            [None, None]
        );
        assert_eq!(
            [
                service_of(Some("noded.beta.bus")),
                service_of(Some("webd")),
                service_of(None)
            ],
            ["noded", "webd", "anonymous"]
        );
        assert_eq!(ts_ms("2026-10-25T13:09:17.568Z").unwrap() % 1000, 568);
        assert_eq!(ts_ms("2026-10-25T13:09:17Z").unwrap() % 1000, 0);
        assert_eq!(
            iso_ms(ts_ms("2026-10-25T13:09:17.068Z").unwrap()),
            "2026-10-25T13:09:17.068Z"
        );
        assert_eq!(
            (
                node_of("beta.bus"),
                service_of(Some("beta.bus")),
                node_of(".bus")
            ),
            (Some("beta"), "noded", None)
        );
        for ts in ["2026-02-30T00:00:00Z", "garbage", "2026-10-25T25:00:00Z"] {
            assert_eq!(ts_ms(ts), None);
        }
        for ts in [
            "1970-01-01T00:00:00.000Z",
            "2000-02-29T23:59:59.999Z",
            "2026-12-31T23:59:59.001Z",
        ] {
            assert_eq!(iso_ms(ts_ms(ts).unwrap()), ts);
        }
    }
    #[test]
    fn oracle_tally_correlation_local_snapshot_and_expiry() {
        let now = clock();
        let mut st = state();
        st.started_ms = now - 60_000;
        nodes(&mut st);
        assert_eq!(
            st.nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["alpha", "beta", "gamma"]
        );
        st.add_event_at(
            &event(
                "2026-10-25T13:09:40.000Z",
                "mesh_out",
                None,
                Some("noded.beta.bus"),
                "noded.info",
                "request",
                Some("c1"),
                "delivered",
                69,
            ),
            now,
        );
        st.add_event_at(
            &event(
                "2026-10-25T13:09:40.040Z",
                "mesh_in",
                Some("noded"),
                None,
                "noded.info",
                "response",
                Some("c1"),
                "delivered",
                385,
            ),
            now,
        );
        let link = &st.links["beta"];
        assert_eq!(
            [link.out_msgs, link.out_bytes, link.in_msgs, link.in_bytes],
            [1, 69, 1, 385]
        );
        assert!(st.pending.is_empty());
        st.add_event_at(
            &event(
                "2026-10-25T13:09:55.000Z",
                "local",
                None,
                Some("webd"),
                "webd.info",
                "request",
                None,
                "broker_handled",
                50,
            ),
            now,
        );
        st.add_event_at(
            &event(
                "2026-10-25T13:09:56.000Z",
                "local",
                Some("x"),
                Some("maild"),
                "maild.send",
                "request",
                None,
                "rejected",
                10,
            ),
            now,
        );
        st.add_event_at(
            &event(
                "2026-10-25T13:09:57.000Z",
                "local",
                Some("observer"),
                Some("noded"),
                "noded.peers",
                "request",
                None,
                "broker_handled",
                61,
            ),
            now,
        );
        st.add_event_at(
            &event(
                "2026-10-25T13:09:58.000Z",
                "mesh_in",
                Some("noded"),
                None,
                "x.y",
                "response",
                Some("zz"),
                "delivered",
                5,
            ),
            now,
        );
        assert_eq!(
            [st.local.msgs, st.local.bytes, st.local.rejected],
            [2, 60, 1]
        );
        assert_eq!(
            st.local.services,
            BTreeMap::from([("webd".into(), 1), ("maild".into(), 1)])
        );
        assert_eq!(st.links[UNKNOWN].in_msgs, 1);
        assert_eq!((st.seen, st.recent.len()), (6, 5));
        let snap = st.snapshot_at(now);
        let link = snap["links"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["b"] == "beta")
            .unwrap();
        assert_eq!(
            [
                &link["out_1m"],
                &link["in_1m"],
                &link["out_5m"],
                &link["in_5m"]
            ],
            [&json!(1), &json!(1), &json!(1), &json!(1)]
        );
        assert_eq!(
            snap["recent"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["seq"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [3, 4, 5]
        );
        assert_eq!(
            [
                st.recent[0].from.as_str(),
                st.recent[0].to.as_str(),
                st.recent[1].from.as_str(),
                st.recent[1].to.as_str()
            ],
            ["alpha", "beta", "beta", "alpha"]
        );
        assert_eq!(snap["local"]["msgs_1m"], 2);
        assert_eq!(snap["local"]["top_services"][0]["count"], 1);
        st.add_event_at(
            &event(
                "2026-10-25T13:09:59.000Z",
                "mesh_out",
                None,
                Some("noded.gamma.bus"),
                "noded.ping",
                "request",
                Some("c9"),
                "delivered",
                9,
            ),
            now,
        );
        st.prune(now + 400_000);
        assert!(st.recent.is_empty() && st.pending.is_empty());
        assert_eq!(st.links["beta"].out_msgs, 1);
        assert_eq!(st.snapshot_at(now + 400_000)["local"]["msgs_5m"], 0);
    }
    #[test]
    fn oracle_bounds_windows_observing_and_peer_field() {
        let mut counts = Counts::new();
        for i in 1..=45 {
            bump(&mut counts, &format!("cmd.{i}"));
        }
        assert_eq!((counts.len(), counts[OTHER]), (41, 5));
        let mut big = state();
        for i in 1..=2500 {
            let at = base() + i;
            big.add_event_at(&json!({"ts":iso_ms(at),"direction":"local","to":"webd","command":"webd.info","outcome":"delivered","size":1}),at);
        }
        let at = base() + 2501;
        big.add_event_at(&json!({"ts":iso_ms(at),"direction":"mesh_out","to":"noded.beta.bus","command":"noded.info","message_type":"request","correlation_id":"k","size":1}),at);
        big.prune(at);
        assert_eq!(
            (
                big.snapshot_at(at)["local"]["msgs_1m"].as_u64().unwrap(),
                big.recent.len(),
                big.recent_capped
            ),
            (2500, 2000, 501)
        );
        assert_eq!(big.snapshot_at(at)["links"][0]["out_1m"], 1);
        let mut many = state();
        for i in 1..=70 {
            many.add_event_at(&json!({"ts":"2026-10-25T14:00:01.000Z","direction":"mesh_out","to":format!("noded.n{i}.bus"),"command":"x","size":1}),base());
        }
        assert_eq!((many.links.len(), many.links[UNKNOWN].out_msgs), (65, 6));
        many.set_observing(false, Some("observe_unauthorised"));
        assert_eq!(many.snapshot_at(base())["observer"]["observing"], false);
        assert_eq!(
            many.snapshot_at(base())["observer"]["observe_error"],
            "observe_unauthorised"
        );
        many.set_observing(true, None);
        assert_eq!(many.snapshot_at(base())["observer"]["observing"], true);
        assert!(many.snapshot_at(base())["observer"]["observe_error"].is_null());
        let mut st = state();
        nodes(&mut st);
        let mut e = event(
            "2026-10-25T14:00:02.000Z",
            "mesh_in",
            None,
            Some("noded"),
            "noded.info",
            "request",
            Some("p1"),
            "delivered",
            9,
        );
        e["peer"] = json!("gamma");
        st.add_event_at(&e, base());
        assert!(st.links.contains_key("gamma") && !st.links.contains_key(UNKNOWN));
        assert_eq!(st.recent[0].from, "gamma");
        st.add_event_at(
            &event(
                "2026-10-25T14:00:01.000Z",
                "mesh_in",
                Some("noded"),
                None,
                "noded.ping",
                "response",
                None,
                "delivered",
                5,
            ),
            base(),
        );
        assert_eq!(st.recent[1].message_type, "response");
    }
    #[test]
    fn oracle_probe_keeps_only_map_peers_and_each_results_time() {
        let mut st = state();
        nodes(&mut st);
        st.set_probe(&json!({"beta":{"ok":true,"rc":0,"ms":3,"extra":"x"},"gamma":{"ok":false,"rc":-2,"ms":"slow"},"alpha":{"ok":true},"stranger":{"ok":true}}),base());
        let p = st.snapshot_at(base())["probe"]["results"].clone();
        assert_eq!(st.snapshot_at(base())["probe"]["at"], iso_ms(base()));
        assert_eq!(
            p,
            json!({"beta":{"ok":true,"rc":0,"ms":3,"at":iso_ms(base()),"age_ms":0,"failed":false},
            "gamma":{"ok":false,"rc":-2,"ms":null,"at":iso_ms(base()),"age_ms":0,"failed":true}})
        );
        st.set_probe(
            &json!({"beta":{"ok":true,"rc":0,"ms":5,"at":base()+9000}}),
            base() + 12000,
        );
        assert!(!st.pings["gamma"].ok);
        assert_eq!(st.pings["gamma"].at_ms, base());
        assert_eq!(st.pings["beta"].at_ms, base() + 9000);
        st.set_probe(&json!("garbage"), base());
        assert_eq!(
            st.pings.keys().map(String::as_str).collect::<Vec<_>>(),
            ["beta", "gamma"]
        );
        assert_eq!(
            st.snapshot_at(base() + 12000)["probe"]["results"]["beta"]["age_ms"],
            3000
        );
    }
    #[test]
    fn bounds_expiry_missing_timestamp_and_failed_ping_recovery() {
        let mut st = state();
        nodes(&mut st);
        for i in 0..PENDING_MAX + 5 {
            st.add_event_at(&json!({"direction":"mesh_out","to":"noded.beta.bus","message_type":"request","correlation_id":format!("c{i}"),"command":"x".repeat(5000)}),base());
        }
        assert_eq!(st.pending.len(), PENDING_MAX);
        assert_eq!(st.recent.len(), RECENT_MAX);
        assert!(st.recent.iter().all(|r| r.command.len() <= TEXT_MAX));
        assert_eq!(st.recent.back().unwrap().t, base());
        st.prune(base() + PENDING_MS + 1);
        assert!(st.pending.is_empty() && st.recent.is_empty());
        for i in 0..1000 {
            st.add_event_at(
                &json!({"direction":"local","ts":iso_ms(base()+i*1000)}),
                base(),
            );
        }
        assert_eq!(
            st.buckets.len(),
            1,
            "future timestamps share the current second"
        );
        st.set_probe(&json!({"beta":{"ok":false,"rc":10}}), base() + 1_000_000);
        st.add_event_at(
            &json!({"direction":"mesh_out","peer":"beta"}),
            base() + 1_000_001,
        );
        assert!(st.ping_failed("beta"));
        st.add_event_at(
            &json!({"direction":"mesh_in","peer":"beta"}),
            base() + 1_000_002,
        );
        assert!(!st.ping_failed("beta"));
        st.set_probe(&json!({"beta":{"ok":false,"rc":10}}), base() + 1_000_003);
        assert!(st.ping_failed("beta"));
        st.set_probe(
            &json!({"beta":{"ok":true,"rc":0,"ms":4}}),
            base() + 1_000_004,
        );
        assert!(!st.ping_failed("beta"));
    }
    #[test]
    fn windows_expire_without_more_events_and_match_inclusive_seconds() {
        let mut st = state();
        st.add_event_at(
            &json!({"direction":"mesh_out","to":"beta.bus","ts":iso_ms(base())}),
            base(),
        );
        assert_eq!(st.snapshot_at(base() + 60_999)["links"][0]["out_1m"], 1);
        assert_eq!(st.snapshot_at(base() + 61_000)["links"][0]["out_1m"], 0);
        assert_eq!(st.snapshot_at(base() + 300_999)["links"][0]["out_5m"], 1);
        assert_eq!(st.snapshot_at(base() + 301_000)["links"][0]["out_5m"], 0);
        assert_eq!(st.snapshot_at(base() + 301_000)["links"][0]["out_msgs"], 1);
    }
    #[test]
    fn link_history_is_bounded_zero_filled_directional_and_cached_in_typed_view() {
        let mut mesh = state();
        nodes(&mut mesh);
        let now = base() + 400_000;
        for (age, direction, peer) in [
            (300, "mesh_out", "beta"), // Outside the 300-cell graph.
            (299, "mesh_out", "beta"),
            (12, "mesh_in", "beta"),
            (12, "mesh_in", "beta"),
            (0, "mesh_out", "beta"),
            (0, "mesh_out", "gamma"),
        ] {
            mesh.add_event_at(
                &json!({"direction":direction,"peer":peer,
                "ts":iso_ms(now - age * 1000)}),
                now,
            );
        }
        let history = mesh.link_history_at("beta", now);
        assert_eq!((history.out.len(), history.input.len()), (300, 300));
        assert_eq!(
            (history.out[0], history.input[287], history.out[299]),
            (1.0, 2.0, 1.0)
        );
        assert_eq!(history.out.iter().sum::<f64>(), 2.0);
        assert_eq!(history.input.iter().sum::<f64>(), 2.0);
        assert_eq!(history.max, 2.0);
        assert_eq!(mesh.view_at(now).links["beta"].history, history);
        assert_eq!(mesh.link_history_at("missing", now), LinkHistory::default());
        assert_eq!(
            mesh.link_history_at("beta", now + 300_000),
            LinkHistory::default()
        );
        let stepped_back = mesh.link_history_at("beta", now - 1000);
        assert_eq!(stepped_back.out.iter().sum::<f64>(), 2.0); // Both old boundary samples now fit.
        let early = mesh.link_history_at("beta", 0);
        assert_eq!(early, LinkHistory::default());
        let mut quiet = state();
        nodes(&mut quiet);
        let view = quiet.view_at(base());
        assert!(
            view.links.is_empty(),
            "quiet topology must not create traffic links"
        );
        assert_eq!(view.quiet_histories["beta"], LinkHistory::default());
        assert_eq!(view.quiet_histories["gamma"], LinkHistory::default());
        assert!(!mesh.view_at(now).quiet_histories.contains_key("beta"));
    }
    #[test]
    fn topology_uses_active_authority_addresses_and_caps_nodes_pings_and_labels() {
        let mut st = state();
        st.set_nodes(
            &json!({"name":"alpha","wg_ip":"192.0.2.1"}),
            &json!({"authority":{"routing_view":[
            {"name":"gamma","class":"transport-only","mesh_ip":"192.0.2.3"},
            {"name":"beta","class":"active-bus","mesh_ip":"192.0.2.2"}
        ]},"peers":[{"name":"stale"}]}),
        );
        assert_eq!(st.nodes.len(), 2);
        assert_eq!(st.nodes[1].name, "beta");
        assert_eq!(st.nodes[1].wg_ip.as_deref(), Some("192.0.2.2"));
        let peers: Vec<_> = (0..MAX_LINKS + 10)
            .map(|i| json!({"name":format!("peer-{i}"),"wg_ip":"192.0.2.2"}))
            .collect();
        st.set_nodes(&json!({"name":"alpha"}), &json!({"peers":peers}));
        assert_eq!(st.nodes.len(), MAX_LINKS + 1);
        let results: serde_json::Map<_, _> = peers
            .iter()
            .map(|p| {
                (
                    p["name"].as_str().unwrap().to_owned(),
                    json!({"ok":true,"ms":1}),
                )
            })
            .collect();
        st.set_probe(&Value::Object(results), base());
        assert_eq!(st.pings.len(), MAX_LINKS);
        st.set_nodes(&json!({"name":"alpha"}), &json!({"peers":[]}));
        assert!(st.pings.is_empty());
        for i in 0..100 {
            st.add_event_at(&json!({"direction":"local","to":format!("service-{i}"),"command":format!("command-{i}")}),base());
        }
        assert_eq!(
            (st.local.services.len(), st.local.commands.len()),
            (MAX_KEYS + 1, MAX_KEYS + 1)
        );
        assert_eq!(st.local.services[OTHER], 60);
        assert!(
            st.buckets
                .values()
                .all(|counts| counts.len() <= MAX_LINKS * 2 + 3)
        );
    }
    #[test]
    fn paused_dock_still_feeds_tally_before_retention() {
        let mut workshop = crate::workshop::Workshop::default();
        workshop.mesh.clock = base;
        workshop.paused = true;
        workshop.observe(json!({"direction":"mesh_in","peer":"beta","command":"noded.info"}));
        assert!(workshop.traffic.is_empty());
        assert_eq!(workshop.mesh.links["beta"].in_msgs, 1);
    }
    #[test]
    fn mesh_selection_starts_observe_once_and_commands_are_open() {
        let mut engine = crate::Engine::new(str::to_owned);
        engine.take_effects();
        let row = crate::engine::find(&engine.tree(), "nodes:mesh")
            .unwrap()
            .clone();
        engine.select_row(&row);
        let effects = engine.take_effects();
        assert_eq!(
            effects
                .iter()
                .filter(
                    |e| matches!(e,crate::Effect::Workshop {operation,..} if operation=="observe")
                )
                .count(),
            1
        );
        engine.select_row(&row);
        assert!(engine.take_effects().is_empty());
        let ticket = engine.workshop.mesh.observe_ticket.unwrap();
        engine.mesh_completed(
            ticket,
            "observe",
            &json!({"on":true}),
            &json!({"error_code":"UNAVAILABLE","message":"observe refused"}),
        );
        engine.select_row(&row);
        assert!(
            engine.take_effects().iter().any(
                |e| matches!(e,crate::Effect::Workshop {operation,..} if operation=="observe")
            )
        );
        engine.command(42, "workshop.mesh", "{}");
        let effect = engine.take_effects().pop().unwrap();
        let crate::Effect::Reply { id, rc, body } = effect else {
            panic!("reply")
        };
        assert_eq!((id, rc), (42, 0));
        for key in [
            "this_node",
            "peers",
            "links",
            "local",
            "probe",
            "observer",
            "recent",
        ] {
            assert!(body.get(key).is_some(), "{key}");
        }
        assert_eq!(body["observer"]["observe_error"], "observe refused");
    }
    #[test]
    fn ping_command_issues_one_call_per_peer_and_reconnect_fences_completions() {
        let mut engine = crate::Engine::new(str::to_owned);
        engine.take_effects();
        nodes(&mut engine.workshop.mesh);
        engine.command(7, "workshop.mesh.ping", "{}");
        let effects = engine.take_effects();
        assert!(effects.iter().any(|e| matches!(e,crate::Effect::Reply {id:7,rc:0,body} if body==&json!({"pinging":["beta","gamma"]}))));
        engine.command(8, "workshop.mesh.ping", "{}");
        assert!(
            matches!(&engine.take_effects()[..],[crate::Effect::Reply {rc:10,body,..}] if body["error_code"]=="BUSY")
        );
        let mut calls = effects
            .into_iter()
            .filter_map(|effect| {
                if let crate::Effect::Workshop {
                    ticket,
                    operation,
                    args,
                    ..
                } = effect
                {
                    Some((ticket, operation, args))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for node in ["beta", "gamma"] {
            assert_eq!(calls.len(), 1);
            let (ticket, operation, args) = calls.pop().unwrap();
            assert_eq!(operation, "mesh.ping");
            assert_eq!(
                args,
                json!({"node":node,"service":format!("noded.{node}.bus"),"command":"noded.ping","timeout_ms":2000})
            );
            assert!(engine.workshop_completion(ticket, &operation));
            engine.mesh_completed(ticket, &operation, &args, &json!({"ok":true,"rc":0,"ms":3}));
            calls = engine
                .take_effects()
                .into_iter()
                .filter_map(|e| {
                    if let crate::Effect::Workshop {
                        ticket,
                        operation,
                        args,
                        ..
                    } = e
                    {
                        Some((ticket, operation, args))
                    } else {
                        None
                    }
                })
                .collect();
        }
        assert!(calls.is_empty());
        assert_eq!(engine.workshop.mesh.pings.len(), 2);
        engine.workshop.subscription = Some("live".into());
        engine.workshop.mesh.set_observing(true, None);
        engine.delivery(crate::bus::Delivery::Disconnected);
        engine.delivery(crate::bus::Delivery::Connected);
        let ticket = engine.workshop.mesh.observe_ticket.unwrap();
        let effects = engine.take_effects();
        assert!(effects.iter().any(
            |e| matches!(e,crate::Effect::Workshop {operation,..} if operation=="mesh.nodes")
        ));
        engine.delivery(crate::bus::Delivery::Disconnected);
        assert!(!engine.mesh_completed(
            ticket,
            "observe",
            &json!({"on":true}),
            &json!({"subscription_id":"stale"})
        ));
        assert!(engine.workshop.subscription.is_none());
        assert!(!engine.workshop.mesh.observing);
    }
}
