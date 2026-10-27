// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Unprivileged TCP socket diagnostics. Wire layouts are Linux UAPI:
//! linux/{netlink,inet_diag,sock_diag,tcp}.h, not a C struct cast. Integers
//! are native endian except addresses/ports. tcp_info grows by appending:
//! read each optional field only when its bytes are present.
use crate::{
    kernel,
    process::{Users, UsersCache},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const SOCK_DIAG_BY_FAMILY: u16 = 20;
const INET_DIAG_INFO: u16 = 2;
const HEADER: usize = 16;
const DIAG: usize = 72;
/// Refuse an oversized dump rather than silently misreport its top talkers.
pub const MAX_SOCKETS: usize = 65_536;
pub const HOLD: Duration = Duration::from_secs(30);
const DUMP_TIMEOUT: Duration = Duration::from_secs(2);

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_ne_bytes(b.get(at..at + 8)?.try_into().ok()?))
}
fn aligned(n: usize) -> usize {
    (n + 3) & !3
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "malformed inet_diag dump")
}

/// Both directions share the dump deadline, including backpressure and EINTR.
fn retry_io<T>(
    deadline: Instant,
    mut operation: impl FnMut() -> io::Result<T>,
    mut wait: impl FnMut(Duration) -> io::Result<()>,
) -> io::Result<T> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "inet_diag timeout"));
        }
        match operation() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => match wait(left) {
                Ok(()) => continue,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            },
            result => return result,
        }
    }
}

fn wait_ready(fd: &OwnedFd, events: i16, left: Duration) -> io::Result<()> {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events,
        revents: 0,
    };
    // SAFETY: poll points at one valid pollfd.
    if unsafe { libc::poll(&mut poll, 1, left.as_millis().max(1) as i32) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// nlmsghdr + inet_diag_req_v2; all states, no address/port filter.
fn request(family: u8, seq: u32) -> Vec<u8> {
    let mut b = vec![0; HEADER + 56];
    let len = b.len() as u32;
    b[0..4].copy_from_slice(&len.to_ne_bytes());
    b[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    b[6..8].copy_from_slice(&0x301u16.to_ne_bytes()); // REQUEST | DUMP
    b[8..12].copy_from_slice(&seq.to_ne_bytes());
    b[16] = family;
    b[17] = libc::IPPROTO_TCP as u8;
    b[18] = 1 << (INET_DIAG_INFO - 1);
    b[20..24].copy_from_slice(&u32::MAX.to_ne_bytes());
    b[64..72].fill(0xff); // INET_DIAG_NOCOOKIE
    b
}

#[derive(Clone, Debug, PartialEq)]
pub struct Raw {
    pub cookie: [u32; 2],
    pub local: IpAddr,
    pub remote: IpAddr,
    pub local_port: u16,
    pub remote_port: u16,
    pub ifindex: u32,
    pub state: u8,
    pub uid: u32,
    pub bytes_received: Option<u64>,
    pub bytes_acked: Option<u64>,
    pub rtt_us: Option<u32>,
    pub retransmits: Option<u32>,
}

fn diag(b: &[u8]) -> io::Result<Raw> {
    if b.len() < DIAG {
        return Err(invalid());
    }
    let address = |at| -> io::Result<IpAddr> {
        match i32::from(b[0]) {
            libc::AF_INET => Ok(Ipv4Addr::from(
                <[u8; 4]>::try_from(&b[at..at + 4]).map_err(|_| invalid())?,
            )
            .into()),
            libc::AF_INET6 => Ok(Ipv6Addr::from(
                <[u8; 16]>::try_from(&b[at..at + 16]).map_err(|_| invalid())?,
            )
            .into()),
            _ => Err(invalid()),
        }
    };
    let mut r = Raw {
        cookie: [
            u32_at(b, 44).ok_or_else(invalid)?,
            u32_at(b, 48).ok_or_else(invalid)?,
        ],
        local: address(8)?,
        remote: address(24)?,
        local_port: u16::from_be_bytes([b[4], b[5]]),
        remote_port: u16::from_be_bytes([b[6], b[7]]),
        ifindex: u32_at(b, 40).ok_or_else(invalid)?,
        state: b[1],
        uid: u32_at(b, 64).ok_or_else(invalid)?,
        bytes_received: None,
        bytes_acked: None,
        rtt_us: None,
        retransmits: None,
    };
    let mut at = DIAG;
    while at < b.len() {
        let len = usize::from(u16_at(b, at).ok_or_else(invalid)?);
        let kind = u16_at(b, at + 2).ok_or_else(invalid)? & 0x3fff;
        if len < 4 || at + len > b.len() {
            return Err(invalid());
        }
        if kind == INET_DIAG_INFO {
            let info = &b[at + 4..at + len];
            r.rtt_us = u32_at(info, 68).filter(|v| *v > 0);
            r.retransmits = u32_at(info, 100); // tcpi_total_retrans, not retry backoff
            r.bytes_acked = u64_at(info, 120);
            r.bytes_received = u64_at(info, 128);
        }
        at += aligned(len);
    }
    Ok(r)
}

/// Consume a multipart datagram, returning whether NLMSG_DONE was seen.
/// An interrupted, truncated or error dump never becomes a valid table.
fn decode(b: &[u8], seq: u32, rows: &mut Vec<Raw>) -> io::Result<bool> {
    let mut at = 0;
    while at < b.len() {
        let h = b.get(at..at + HEADER).ok_or_else(invalid)?;
        let len = u32_at(h, 0).ok_or_else(invalid)? as usize;
        if len < HEADER {
            return Err(invalid());
        }
        let msg = b.get(at + HEADER..at + len).ok_or_else(invalid)?;
        if u32_at(h, 8) != Some(seq) {
            return Err(invalid());
        }
        if u16_at(h, 6).ok_or_else(invalid)? & 0x10 != 0 {
            return Err(io::Error::other("inet_diag dump interrupted; retry"));
        }
        match u16_at(h, 4).ok_or_else(invalid)? {
            1 => {} // NLMSG_NOOP
            2 | 3 => {
                if msg.len() >= 4 {
                    let error = i32::from_ne_bytes(msg[..4].try_into().map_err(|_| invalid())?);
                    if error != 0 {
                        return Err(io::Error::from_raw_os_error(error.saturating_neg()));
                    }
                } else if u16_at(h, 4) == Some(2) {
                    return Err(invalid());
                }
                if u16_at(h, 4) == Some(3) {
                    return Ok(true);
                }
            }
            4 => return Err(io::Error::other("inet_diag overrun")),
            SOCK_DIAG_BY_FAMILY => {
                if rows.len() >= MAX_SOCKETS {
                    return Err(io::Error::other("TCP socket limit exceeded"));
                }
                rows.push(diag(msg)?);
            }
            _ => return Err(invalid()),
        }
        at += aligned(len);
    }
    Ok(false)
}

/// One bounded IPv4 + IPv6 dump. Connected netlink accepts only the kernel;
/// recvfrom checks the sender too. OwnedFd closes on every exit path.
pub fn dump() -> io::Result<Vec<Raw>> {
    // SAFETY: socket has constant domain/type/protocol and returns a new fd.
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            libc::NETLINK_SOCK_DIAG,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socket returned a fresh fd, now exclusively owned here.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: sockaddr_nl's all-zero representation is valid.
    let mut kernel: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    kernel.nl_family = libc::AF_NETLINK as u16;
    // SAFETY: pointer and length describe the valid address for this call.
    if unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&kernel as *const libc::sockaddr_nl).cast(),
            std::mem::size_of_val(&kernel) as _,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut rows = Vec::new();
    let mut buffer = vec![0; 65_536];
    let deadline = Instant::now() + DUMP_TIMEOUT;
    for (seq, family) in [(1, libc::AF_INET), (2, libc::AF_INET6)] {
        let req = request(family as u8, seq);
        retry_io(
            deadline,
            || {
                // SAFETY: req is readable; the connected peer is the kernel.
                let n = unsafe { libc::send(fd.as_raw_fd(), req.as_ptr().cast(), req.len(), 0) };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else if n as usize != req.len() {
                    Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "short inet_diag request",
                    ))
                } else {
                    Ok(())
                }
            },
            |left| wait_ready(&fd, libc::POLLOUT, left),
        )?;
        loop {
            let (n, sender) = retry_io(
                deadline,
                || {
                    let mut sender = kernel;
                    let mut addrlen = std::mem::size_of_val(&sender) as libc::socklen_t;
                    // SAFETY: buffer and sender are writable for the given lengths.
                    let n = unsafe {
                        libc::recvfrom(
                            fd.as_raw_fd(),
                            buffer.as_mut_ptr().cast(),
                            buffer.len(),
                            libc::MSG_TRUNC,
                            (&mut sender as *mut libc::sockaddr_nl).cast(),
                            &mut addrlen,
                        )
                    };
                    if n < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok((n, sender))
                    }
                },
                |left| wait_ready(&fd, libc::POLLIN, left),
            )?;
            if n == 0 || n as usize > buffer.len() || sender.nl_pid != 0 {
                return Err(invalid());
            }
            if decode(&buffer[..n as usize], seq, &mut rows)? {
                break;
            }
        }
    }
    Ok(rows)
}

pub const STATES: [&str; 13] = [
    "unknown",
    "established",
    "syn-sent",
    "syn-recv",
    "fin-wait-1",
    "fin-wait-2",
    "time-wait",
    "close",
    "close-wait",
    "last-ack",
    "listen",
    "closing",
    "new-syn-recv",
];

#[derive(Clone, Debug, Serialize)]
pub struct Connection {
    pub cookie: [u32; 2],
    pub local: IpAddr,
    pub remote: IpAddr,
    pub local_port: u16,
    pub remote_port: u16,
    pub ifindex: u32,
    pub state: String,
    pub rx_bps: Option<f64>,
    pub tx_bps: Option<f64>,
    pub rtt_us: Option<u32>,
    pub retransmits: Option<u32>,
    pub uid: u32,
    pub user: String,
}

fn table(before: &[Raw], after: &[Raw], dt: f64, users: &Users) -> Vec<Connection> {
    let previous: HashMap<_, _> = before
        .iter()
        .filter(|r| r.cookie != [u32::MAX; 2])
        .map(|r| (r.cookie, r))
        .collect();
    after
        .iter()
        .map(|r| {
            let p = previous.get(&r.cookie);
            Connection {
                cookie: r.cookie,
                local: r.local,
                remote: r.remote,
                local_port: r.local_port,
                remote_port: r.remote_port,
                ifindex: r.ifindex,
                state: STATES
                    .get(r.state as usize)
                    .unwrap_or(&"unknown")
                    .to_string(),
                rx_bps: p.and_then(|p| kernel::rate(p.bytes_received?, r.bytes_received?, dt)),
                tx_bps: p.and_then(|p| kernel::rate(p.bytes_acked?, r.bytes_acked?, dt)),
                rtt_us: r.rtt_us,
                retransmits: r.retransmits,
                uid: r.uid,
                user: users.name(r.uid),
            }
        })
        .collect()
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Query {
    pub limit: Option<usize>,
    pub sort: Option<String>,
    pub state: Option<String>,
    /// Match either local or remote port.
    pub port: Option<u16>,
}

impl Query {
    pub fn validate(&self) -> Result<(), String> {
        if !["throughput", "rx", "tx", "rtt", "retransmits"]
            .contains(&self.sort.as_deref().unwrap_or("throughput"))
        {
            return Err("sort must be throughput, rx, tx, rtt or retransmits".into());
        }
        if self
            .state
            .as_deref()
            .is_some_and(|s| !STATES[1..].contains(&s))
        {
            return Err(format!("state must be one of {}", STATES[1..].join(", ")));
        }
        Ok(())
    }
}

pub fn select(table: &[Connection], query: &Query) -> (Vec<Connection>, usize) {
    let (rows, total) = select_rows(table, query);
    (rows.into_iter().cloned().collect(), total)
}

/// Partition borrowed rows first; only the returned top-N is sorted and cloned.
fn select_rows<'a>(table: &'a [Connection], query: &Query) -> (Vec<&'a Connection>, usize) {
    let mut rows: Vec<_> = table
        .iter()
        .filter(|r| {
            query.state.as_ref().is_none_or(|s| *s == r.state)
                && query
                    .port
                    .is_none_or(|p| p == r.local_port || p == r.remote_port)
        })
        .collect();
    let score = |r: &Connection| match query.sort.as_deref().unwrap_or("throughput") {
        "rx" => r.rx_bps.unwrap_or(-1.0),
        "tx" => r.tx_bps.unwrap_or(-1.0),
        "rtt" => r.rtt_us.map(f64::from).unwrap_or(-1.0),
        "retransmits" => r.retransmits.map(f64::from).unwrap_or(-1.0),
        _ => match (r.rx_bps, r.tx_bps) {
            (None, None) => -1.0,
            (rx, tx) => rx.unwrap_or(0.0) + tx.unwrap_or(0.0),
        },
    };
    let throughput = query.sort.as_deref().unwrap_or("throughput") == "throughput";
    let compare = |a: &&Connection, b: &&Connection| {
        // Listeners only fill spare places after every non-listening
        // socket. Unknown rates sort behind measured rates, including zero.
        (throughput && a.state == "listen")
            .cmp(&(throughput && b.state == "listen"))
            .then_with(|| score(b).total_cmp(&score(a)))
            .then(a.cookie.cmp(&b.cookie))
    };
    let total = rows.len();
    let limit = query.limit.unwrap_or(20).clamp(1, 1000);
    if total > limit {
        rows.select_nth_unstable_by(limit, compare);
        rows.truncate(limit);
    }
    rows.sort_unstable_by(compare);
    (rows, total)
}

#[derive(Default)]
struct State {
    until: Option<Instant>,
    scan: Option<(Instant, Vec<Raw>)>,
    rows: Option<(Instant, Arc<Vec<Connection>>)>,
    error: Option<String>,
}

impl State {
    fn expire(&mut self, now: Instant) -> bool {
        let active = self.until.is_some_and(|u| now < u);
        if !active {
            *self = Self::default();
        }
        active
    }
}

type Source = dyn Fn() -> io::Result<Vec<Raw>> + Send + Sync;

pub struct Sampler {
    state: Mutex<State>,
    /// Coalesce simultaneous cold requests; no blocking guard across await.
    request: tokio::sync::Mutex<()>,
    source: Box<Source>,
    users: Arc<UsersCache>,
    interval: Duration,
}

impl Sampler {
    pub fn new(users: Arc<UsersCache>, interval: Duration) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            request: tokio::sync::Mutex::new(()),
            source: Box::new(dump),
            users,
            interval,
        })
    }

    fn sample(&self, s: &mut State) -> io::Result<()> {
        let raw = (self.source)()?;
        if raw.len() > MAX_SOCKETS {
            return Err(io::Error::other("TCP socket limit exceeded"));
        }
        let at = Instant::now();
        if let Some((then, before)) = &s.scan {
            s.rows = Some((
                at,
                Arc::new(table(
                    before,
                    &raw,
                    at.duration_since(*then).as_secs_f64(),
                    &self.users.get(),
                )),
            ));
        }
        s.scan = Some((at, raw));
        s.error = None;
        Ok(())
    }

    pub fn tick(&self) {
        // A cold get owns its pair across the sleep. Skipping also keeps a
        // tick from replacing its finished table before get returns it.
        let Ok(_request) = self.request.try_lock() else {
            return;
        };
        let mut s = self.state.lock().unwrap();
        if s.expire(Instant::now())
            && let Err(e) = self.sample(&mut s)
        {
            s.error = Some(e.to_string());
            s.scan = None;
            s.rows = None;
        }
    }

    pub async fn get(self: &Arc<Self>) -> anyhow::Result<Arc<Vec<Connection>>> {
        let _request = self.request.lock().await;
        {
            let mut s = self.state.lock().unwrap();
            s.expire(Instant::now());
            s.until = Some(Instant::now() + HOLD);
            if let Some((at, rows)) = &s.rows
                && at.elapsed() < self.interval * 2
                && s.error.is_none()
            {
                return Ok(rows.clone());
            }
            s.scan = None;
            s.rows = None;
        }
        let me = self.clone();
        tokio::task::spawn_blocking(move || me.sample(&mut me.state.lock().unwrap())).await??;
        tokio::time::sleep(self.interval).await;
        let me = self.clone();
        tokio::task::spawn_blocking(move || me.sample(&mut me.state.lock().unwrap())).await??;
        self.state
            .lock()
            .unwrap()
            .rows
            .as_ref()
            .map(|(_, r)| r.clone())
            .ok_or_else(|| anyhow::anyhow!("no TCP connection table"))
    }

    pub fn status(&self) -> serde_json::Value {
        let mut s = self.state.lock().unwrap();
        let active = s.expire(Instant::now());
        serde_json::json!({"active":active,"rows":s.rows.as_ref().map(|(_,r)| r.len()),"error":s.error})
    }
}

/// Independent of system sampling: a slow diagnostic dump cannot delay /proc.
pub async fn run(sampler: Arc<Sampler>) {
    let mut timer = tokio::time::interval(sampler.interval);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        timer.tick().await;
        let sampler = sampler.clone();
        let _ = tokio::task::spawn_blocking(move || sampler.tick()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::Root;

    #[test]
    fn nonblocking_io_retries_backpressure_and_interrupts_with_one_deadline() {
        let deadline = Instant::now() + DUMP_TIMEOUT;
        let mut attempts = 0;
        let mut waits = 0;
        let result = retry_io(
            deadline,
            || {
                attempts += 1;
                match attempts {
                    1 | 2 => Err(io::Error::from_raw_os_error(libc::EAGAIN)),
                    3 => Err(io::Error::from_raw_os_error(libc::EINTR)),
                    _ => Ok(72),
                }
            },
            |left| {
                assert!(left > Duration::ZERO && left <= DUMP_TIMEOUT);
                waits += 1;
                if waits == 1 {
                    Err(io::Error::from_raw_os_error(libc::EINTR))
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result.unwrap(), 72);
        assert_eq!((attempts, waits), (4, 2));

        let mut attempts = 0;
        let error = retry_io::<()>(
            Instant::now() + Duration::from_millis(10),
            || {
                attempts += 1;
                Err(io::Error::from_raw_os_error(libc::EAGAIN))
            },
            |left| {
                std::thread::sleep(left);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(attempts <= 1, "deadline must not restart after waiting");
        let error = retry_io::<()>(
            deadline,
            || Err(io::Error::from_raw_os_error(libc::EPERM)),
            |_| panic!("permanent errors must not wait"),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    }

    fn hex(text: &str) -> Vec<u8> {
        let text = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<String>();
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn requests_encode_uapi_all_states_and_tcp_info_without_privileged_extensions() {
        // nlmsghdr + req_v2, also accepted by the kernel in the live test.
        // Native words are little endian in these x86-64 fixtures.
        if cfg!(target_endian = "little") {
            let expected = hex(
                &("4800000014000103010000000000000002060200ffffffff".to_owned()
                    + &"00".repeat(40)
                    + &"ff".repeat(8)),
            );
            assert_eq!(request(libc::AF_INET as u8, 1), expected);
            let mut ipv6 = expected;
            ipv6[16] = libc::AF_INET6 as u8;
            assert_eq!(request(libc::AF_INET6 as u8, 1), ipv6);
        }
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn captured_wire_fixtures_decode_addresses_uid_tcp_info_and_multipart_done() {
        for (fixture, local, remote) in [
            (
                include_str!("../tests/fixtures/inet_diag_v4.hex"),
                "192.0.2.1",
                "198.51.100.2",
            ),
            (
                include_str!("../tests/fixtures/inet_diag_v6.hex"),
                "2001:db8::1",
                "2001:db8::2",
            ),
        ] {
            let mut wire = hex(fixture);
            let mut rows = Vec::new();
            assert!(!decode(&wire, 1, &mut rows).unwrap());
            assert_eq!(rows.len(), 1);
            let row = &rows[0];
            assert_eq!(row.local.to_string(), local);
            assert_eq!(row.remote.to_string(), remote);
            assert_eq!(
                (row.local_port, row.remote_port, row.uid, row.cookie),
                (443, 45000, 1000, [1, 0])
            );
            assert_eq!(row.bytes_received, Some(128));
            assert_eq!(row.bytes_acked, Some(0));
            assert_eq!(row.retransmits, Some(0));
            assert!(row.rtt_us.unwrap() > 0);
            wire.extend(hex("1400000003000200010000000000000000000000"));
            let mut rows = Vec::new();
            assert!(decode(&wire, 1, &mut rows).unwrap());
            assert_eq!(rows.len(), 1);
        }
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn short_tcp_info_omits_new_fields_unknown_attributes_skip_bad_lengths_refuse() {
        let wire = hex(include_str!("../tests/fixtures/inet_diag_v4.hex"));
        let mut b = wire[HEADER..].to_vec();
        let mut at = DIAG;
        while u16_at(&b, at + 2) != Some(INET_DIAG_INFO) {
            at += aligned(usize::from(u16_at(&b, at).unwrap()));
        }
        b[at..at + 2].copy_from_slice(&108u16.to_ne_bytes());
        b.truncate(at + 108); // old 104-byte tcp_info
        let row = diag(&b).unwrap();
        assert!(row.rtt_us.is_some() && row.retransmits.is_some());
        assert!(row.bytes_acked.is_none() && row.bytes_received.is_none());
        let old = b.clone();
        b.extend([5, 0, 0xe7, 3, 42, 0, 0, 0]); // future attribute, including padding
        assert_eq!(diag(&b).unwrap(), row);
        b[at..at + 2].copy_from_slice(&3u16.to_ne_bytes());
        assert!(diag(&b).is_err());
        let mut b = old;
        b[at..at + 2].copy_from_slice(&u16::MAX.to_ne_bytes());
        assert!(diag(&b).is_err());
        let mut full = vec![raw(1, 0); MAX_SOCKETS];
        assert!(decode(&wire, 1, &mut full).is_err());
    }

    fn raw(cookie: u32, bytes: u64) -> Raw {
        Raw {
            cookie: [cookie, 0],
            local: "192.0.2.1".parse().unwrap(),
            remote: "198.51.100.2".parse().unwrap(),
            local_port: 443,
            remote_port: 45000,
            ifindex: 0,
            state: 1,
            uid: 1000,
            bytes_received: Some(bytes),
            bytes_acked: Some(bytes * 2),
            rtt_us: Some(1500),
            retransmits: Some(2),
        }
    }

    #[test]
    fn throughput_ranking_backfills_listeners_and_puts_unknown_after_measured_zero() {
        let users = Users::default();
        let mut rows = table(&[raw(1, 0)], &[raw(1, 0)], 1.0, &users);
        let zero = rows[0].clone();
        let mut unknown = zero.clone();
        unknown.cookie = [0, 0];
        unknown.rx_bps = None;
        unknown.tx_bps = None;
        let mut listener = unknown.clone();
        listener.state = "listen".into();
        listener.cookie = [2, 0];
        rows = vec![listener.clone(), unknown, zero];
        for i in 3..23 {
            let mut traffic = rows[2].clone();
            traffic.cookie = [i, 0];
            traffic.rx_bps = Some(f64::from(i) * 1024.0);
            rows.push(traffic);
        }
        let (top, total) = select(&rows, &Query::default());
        assert_eq!(total, 23);
        assert_eq!(top.len(), 20);
        assert!(
            top.iter()
                .all(|r| r.state != "listen" && r.rx_bps.unwrap() > 0.0)
        );
        let all = select(
            &rows,
            &Query {
                limit: Some(100),
                ..Query::default()
            },
        )
        .0;
        assert_eq!(all[20].cookie, [1, 0], "measured zero before unavailable");
        assert_eq!(all[21].cookie, [0, 0]);
        assert_eq!(
            all[22].state, "listen",
            "listeners only backfill spare rows"
        );
        let filtered = select(
            &rows,
            &Query {
                state: Some("listen".into()),
                ..Query::default()
            },
        )
        .0;
        assert_eq!(filtered.len(), 1, "explicit listener queries still work");
    }

    #[test]
    fn borrowed_top_n_matches_full_ranking_for_large_filtered_tables() {
        use std::cmp::Reverse;
        let raw: Vec<_> = (0..4096).map(|i| raw((i * 37) % 4096, 0)).collect();
        let mut rows = table(&raw, &raw, 1.0, &Users::default());
        for row in &mut rows {
            let id = row.cookie[0];
            row.local_port = if id.is_multiple_of(3) { 80 } else { 443 };
            row.state = if id.is_multiple_of(7) { "listen" } else { "established" }.into();
            row.rx_bps = (!id.is_multiple_of(11)).then_some(f64::from(id));
            row.tx_bps = row.rx_bps.map(|_| f64::from(4096 - id));
            row.rtt_us = row.rx_bps.map(|_| id % 101);
            row.retransmits = row.rx_bps.map(|_| id % 17);
        }
        for sort in ["throughput", "rx", "tx", "rtt", "retransmits"] {
            for limit in [0, 1, 20, 1000, usize::MAX] {
                let query = Query {
                    sort: Some(sort.into()),
                    limit: Some(limit),
                    port: Some(80),
                    ..Query::default()
                };
                let mut expected: Vec<_> = rows.iter().filter(|r| r.local_port == 80).collect();
                expected.sort_by_key(|r| {
                    let id = r.cookie[0];
                    let score = (!id.is_multiple_of(11)).then_some(match sort {
                        "rx" => id,
                        "tx" => 4096 - id,
                        "rtt" => id % 101,
                        "retransmits" => id % 17,
                        _ => 4096,
                    });
                    (
                        sort == "throughput" && id.is_multiple_of(7),
                        Reverse(score),
                        r.cookie,
                    )
                });
                let (selected, total) = select_rows(&rows, &query);
                assert_eq!(total, expected.len());
                expected.truncate(limit.clamp(1, 1000));
                assert_eq!(selected.len(), expected.len());
                for (actual, expected) in selected.iter().zip(&expected) {
                    assert!(
                        std::ptr::eq(*actual, *expected),
                        "selection must borrow source rows"
                    );
                }
                let (owned, _) = select(&rows, &query);
                assert_eq!(
                    owned.iter().map(|r| r.cookie).collect::<Vec<_>>(),
                    expected.iter().map(|r| r.cookie).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn cookies_rates_resets_new_sockets_sort_filter_and_limits() {
        let users = Users::parse("alice:x:1000:1000::/home/user:/bin/mix\n");
        let before = [raw(1, 100), raw(2, 500)];
        let after = [raw(1, 500), raw(2, 100), raw(3, 10)];
        let rows = table(&before, &after, 2.0, &users);
        assert_eq!((rows[0].rx_bps, rows[0].tx_bps), (Some(200.0), Some(400.0)));
        assert_eq!(rows[0].user, "alice");
        assert_eq!(rows[1].rx_bps, None);
        assert_eq!(rows[2].tx_bps, None);
        assert_eq!(table(&before, &after, 0.0, &users)[0].rx_bps, None);
        let (top, total) = select(
            &rows,
            &Query {
                limit: Some(1),
                ..Query::default()
            },
        );
        assert_eq!(total, 3);
        assert_eq!(top[0].cookie, [1, 0]);
        assert_eq!(
            select(
                &rows,
                &Query {
                    state: Some("listen".into()),
                    ..Query::default()
                }
            )
            .1,
            0
        );
        assert_eq!(
            select(
                &rows,
                &Query {
                    port: Some(45000),
                    ..Query::default()
                }
            )
            .1,
            3
        );
        assert_eq!(
            select(
                &rows,
                &Query {
                    port: Some(80),
                    ..Query::default()
                }
            )
            .1,
            0
        );
        assert!(
            Query {
                sort: Some("oops".into()),
                ..Query::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Query {
                state: Some("oops".into()),
                ..Query::default()
            }
            .validate()
            .is_err()
        );
        let unknown = raw(u32::MAX, 1);
        let unknown = Raw {
            cookie: [u32::MAX; 2],
            ..unknown
        };
        assert_eq!(
            table(
                std::slice::from_ref(&unknown),
                std::slice::from_ref(&unknown),
                1.0,
                &users
            )[0]
            .rx_bps,
            None
        );
    }

    #[tokio::test]
    async fn cold_calls_coalesce_hold_and_expiry_discards_every_row() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let scans = Arc::new(AtomicU64::new(0));
        let count = scans.clone();
        let sampler = Arc::new(Sampler {
            state: Mutex::new(State::default()),
            request: tokio::sync::Mutex::new(()),
            source: Box::new(move || Ok(vec![raw(1, count.fetch_add(100, Ordering::SeqCst))])),
            users: UsersCache::new(Root::new("/nonexistent")),
            interval: Duration::from_millis(20),
        });
        sampler.tick();
        assert_eq!(scans.load(Ordering::SeqCst), 0, "idle must not dump");
        let (a, b) = tokio::join!(sampler.get(), sampler.get());
        assert!(a.unwrap()[0].rx_bps.unwrap() > 0.0);
        assert!(Arc::ptr_eq(&b.unwrap(), &sampler.get().await.unwrap()));
        assert_eq!(
            scans.load(Ordering::SeqCst),
            200,
            "one cold pair for concurrent requests"
        );
        assert_eq!(sampler.status()["active"], true);
        sampler.tick();
        assert_eq!(scans.load(Ordering::SeqCst), 300);
        {
            let mut s = sampler.state.lock().unwrap();
            let until = s.until.unwrap();
            assert!(s.expire(until - Duration::from_nanos(1)));
            assert!(!s.expire(until));
            assert!(s.scan.is_none() && s.rows.is_none());
        }
        sampler.tick();
        assert_eq!(scans.load(Ordering::SeqCst), 300);
        sampler.get().await.unwrap();
        assert_eq!(
            scans.load(Ordering::SeqCst),
            500,
            "expired call needs another pair"
        );
    }

    #[tokio::test]
    async fn tick_during_cold_call_cannot_replace_its_baseline() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let scans = Arc::new(AtomicU64::new(0));
        let first = Arc::new(tokio::sync::Notify::new());
        let count = scans.clone();
        let sampled = first.clone();
        let sampler = Arc::new(Sampler {
            state: Mutex::new(State::default()),
            request: tokio::sync::Mutex::new(()),
            source: Box::new(move || {
                let bytes = count.fetch_add(100, Ordering::SeqCst);
                sampled.notify_one();
                Ok(vec![raw(1, bytes)])
            }),
            users: UsersCache::new(Root::new("/nonexistent")),
            interval: Duration::from_millis(40),
        });
        let (rows, ()) = tokio::join!(sampler.get(), async {
            first.notified().await;
            sampler.tick();
            assert_eq!(
                scans.load(Ordering::SeqCst),
                100,
                "tick must skip the cold pair"
            );
        });
        assert_eq!(scans.load(Ordering::SeqCst), 200);
        let rate = rows.unwrap()[0].rx_bps.unwrap();
        assert!(rate > 0.0 && rate <= 100.0 / sampler.interval.as_secs_f64());
        sampler.tick();
        assert_eq!(scans.load(Ordering::SeqCst), 300, "ticks resume after get");
    }

    #[tokio::test]
    async fn connection_samples_reuse_the_shared_users_cache() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("etc")).unwrap();
        let passwd = dir.path().join("etc/passwd");
        std::fs::write(&passwd, "alice:x:1000:1000::/:/bin/mix\n").unwrap();
        let users = UsersCache::new(Root::new(dir.path()));
        std::fs::write(&passwd, "bob:x:1000:1000::/:/bin/mix\n").unwrap();
        let sampler = Arc::new(Sampler {
            state: Mutex::new(State::default()),
            request: tokio::sync::Mutex::new(()),
            source: Box::new(|| Ok(vec![raw(1, 100)])),
            users: users.clone(),
            interval: Duration::from_millis(1),
        });
        assert_eq!(sampler.get().await.unwrap()[0].user, "alice");
        sampler.tick();
        assert_eq!(sampler.get().await.unwrap()[0].user, "alice");
        assert_eq!(users.get().name(1000), "alice");
    }

    #[test]
    fn malformed_messages_errors_and_interruptions_are_refused() {
        let mut rows = Vec::new();
        for bytes in [vec![], vec![0; 15], vec![0; 16]] {
            if !bytes.is_empty() {
                assert!(decode(&bytes, 1, &mut rows).is_err());
            }
        }
        let mut done = vec![0; 20];
        done[..4].copy_from_slice(&20u32.to_ne_bytes());
        done[4..6].copy_from_slice(&3u16.to_ne_bytes());
        done[8..12].copy_from_slice(&1u32.to_ne_bytes());
        assert!(decode(&done, 1, &mut rows).unwrap());
        assert!(decode(&done, 2, &mut rows).is_err());
        done[6] = 0x10;
        assert!(decode(&done, 1, &mut rows).is_err());
        done[6] = 0;
        done[4..6].copy_from_slice(&2u16.to_ne_bytes());
        done[16..20].copy_from_slice(&(-libc::EPERM).to_ne_bytes());
        assert_eq!(
            decode(&done, 1, &mut rows).unwrap_err().raw_os_error(),
            Some(libc::EPERM)
        );
        assert!(diag(&[0; 71]).is_err());
    }

    /// Exercises the worker's own sockets, without a broker or privilege.
    #[test]
    fn live_unprivileged_ipv4_and_ipv6_dumps() {
        use std::{
            io::{Read, Write},
            net::{TcpListener, TcpStream},
        };
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let listener = TcpListener::bind(bind).unwrap();
            let address = listener.local_addr().unwrap();
            let mut client = TcpStream::connect(address).unwrap();
            let (mut server, _) = listener.accept().unwrap();
            client.write_all(&[42; 128]).unwrap();
            server.read_exact(&mut [0; 128]).unwrap();
            let rows = dump().unwrap();
            let socket = rows
                .iter()
                .find(|r| r.local_port == address.port() && r.state == 1)
                .unwrap();
            assert!(socket.bytes_received.unwrap() >= 128);
            assert!(socket.rtt_us.is_some());
            assert!(
                rows.iter()
                    .any(|r| r.local_port == address.port() && r.state == 10)
            );
            assert!(dump().unwrap().iter().any(|r| r.cookie == socket.cookie));
        }
    }
}
