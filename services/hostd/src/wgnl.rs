// SPDX-License-Identifier: MIT OR Apache-2.0
//! Netlink for WireGuard links, with `CAP_NET_ADMIN`: read each peer's
//! last handshake (generic netlink, family `wireguard`), and delete a link
//! so networkd creates it afresh from a changed `.netdev`.
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_MULTI: u16 = 0x2;
const NLM_F_ACK: u16 = 0x4;
const NLM_F_DUMP: u16 = 0x300;
const NLA_TYPE_MASK: u16 = 0x3fff;

const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;

const WG_CMD_GET_DEVICE: u8 = 0;
const WGDEVICE_A_IFNAME: u16 = 2;
const WGDEVICE_A_PEERS: u16 = 8;
const WGPEER_A_PUBLIC_KEY: u16 = 1;
const WGPEER_A_LAST_HANDSHAKE_TIME: u16 = 6;

const RTM_DELLINK: u16 = 17;

/// One peer's handshake state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub public_key: [u8; 32],
    /// Seconds since the epoch of the last handshake; 0 if never.
    pub last_handshake: i64,
}

fn socket(protocol: libc::c_int) -> std::io::Result<OwnedFd> {
    // SAFETY: plain socket(2); the result is checked before use.
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, protocol) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a fresh fd we own.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let timeout = libc::timeval { tv_sec: 3, tv_usec: 0 };
    // SAFETY: setsockopt reads one timeval of the stated size.
    unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&timeout as *const libc::timeval).cast(),
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
    Ok(fd)
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Appends one attribute (padded to four bytes).
fn attr(buf: &mut Vec<u8>, kind: u16, payload: &[u8]) {
    let len = 4 + payload.len();
    buf.extend_from_slice(&(len as u16).to_ne_bytes());
    buf.extend_from_slice(&kind.to_ne_bytes());
    buf.extend_from_slice(payload);
    buf.resize(buf.len() + (align4(len) - len), 0);
}

/// A netlink message: header, then `body`.
fn message(kind: u16, flags: u16, body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(16 + body.len());
    m.extend_from_slice(&((16 + body.len()) as u32).to_ne_bytes());
    m.extend_from_slice(&kind.to_ne_bytes());
    m.extend_from_slice(&flags.to_ne_bytes());
    m.extend_from_slice(&1u32.to_ne_bytes()); // seq
    m.extend_from_slice(&0u32.to_ne_bytes()); // pid
    m.extend_from_slice(body);
    m
}

/// The attributes in `buf`, as (type, payload). Stops at the first that
/// doesn't fit, so truncated or hostile input never panics.
fn attrs(buf: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = vec![];
    let mut at = 0;
    while at + 4 <= buf.len() {
        let len = u16_at(buf, at).unwrap_or(0) as usize;
        let kind = u16_at(buf, at + 2).unwrap_or(0) & NLA_TYPE_MASK;
        if len < 4 || at + len > buf.len() {
            break;
        }
        out.push((kind, &buf[at + 4..at + len]));
        at += align4(len);
    }
    out
}

/// Sends one request and collects the reply messages' bodies (after the
/// netlink header) until the reply ends. An error reply is an error.
fn transact(protocol: libc::c_int, request: &[u8]) -> std::io::Result<Vec<Vec<u8>>> {
    let fd = socket(protocol)?;
    // SAFETY: sends `request.len()` initialised bytes on a socket we own.
    if unsafe { libc::send(fd.as_raw_fd(), request.as_ptr().cast(), request.len(), 0) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut buf = vec![0u8; 256 * 1024];
    let mut bodies = vec![];
    loop {
        // SAFETY: receives into a buffer of exactly `buf.len()` bytes.
        let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let data = &buf[..n as usize];
        let mut at = 0;
        let mut more = false;
        while at + 16 <= data.len() {
            let len = u32_at(data, at).unwrap_or(0) as usize;
            let kind = u16_at(data, at + 4).unwrap_or(0);
            let flags = u16_at(data, at + 6).unwrap_or(0);
            if len < 16 || at + len > data.len() {
                break;
            }
            let body = &data[at + 16..at + len];
            match kind {
                NLMSG_DONE => return Ok(bodies),
                NLMSG_ERROR => {
                    let errno = body.get(..4).map(|e| i32::from_ne_bytes(e.try_into().unwrap_or_default())).unwrap_or(0);
                    if errno == 0 {
                        return Ok(bodies); // an ack
                    }
                    return Err(std::io::Error::from_raw_os_error(-errno));
                }
                _ => {
                    bodies.push(body.to_vec());
                    more |= flags & NLM_F_MULTI != 0;
                }
            }
            at += align4(len);
        }
        if !more {
            return Ok(bodies);
        }
    }
}

/// The generic netlink family id for `wireguard`.
fn wireguard_family() -> std::io::Result<u16> {
    let mut body = vec![CTRL_CMD_GETFAMILY, 1, 0, 0];
    attr(&mut body, CTRL_ATTR_FAMILY_NAME, b"wireguard\0");
    let replies = transact(libc::NETLINK_GENERIC, &message(GENL_ID_CTRL, NLM_F_REQUEST, &body))?;
    replies
        .iter()
        .filter_map(|r| r.get(4..))
        .flat_map(attrs)
        .find(|(kind, _)| *kind == CTRL_ATTR_FAMILY_ID)
        .and_then(|(_, v)| u16_at(v, 0))
        .ok_or_else(|| std::io::Error::other("no wireguard netlink family"))
}

/// Every peer of WireGuard link `name`, with its last handshake.
pub fn peers(name: &str) -> std::io::Result<Vec<Peer>> {
    let family = wireguard_family()?;
    let mut body = vec![WG_CMD_GET_DEVICE, 1, 0, 0];
    let mut ifname = name.as_bytes().to_vec();
    ifname.push(0);
    attr(&mut body, WGDEVICE_A_IFNAME, &ifname);
    let replies = transact(
        libc::NETLINK_GENERIC,
        &message(family, NLM_F_REQUEST | NLM_F_DUMP, &body),
    )?;
    Ok(parse_peers(&replies))
}

/// Peers from a device dump's reply bodies (each a genl header, then
/// device attributes). A large device spans several replies.
pub fn parse_peers(replies: &[Vec<u8>]) -> Vec<Peer> {
    let mut out = vec![];
    for reply in replies {
        let Some(device) = reply.get(4..) else { continue };
        for (kind, list) in attrs(device) {
            if kind != WGDEVICE_A_PEERS {
                continue;
            }
            for (_, peer) in attrs(list) {
                let mut key = None;
                let mut last = 0i64;
                for (k, v) in attrs(peer) {
                    match k {
                        WGPEER_A_PUBLIC_KEY => key = <[u8; 32]>::try_from(v).ok(),
                        WGPEER_A_LAST_HANDSHAKE_TIME => {
                            last = v
                                .get(..8)
                                .map(|s| i64::from_ne_bytes(s.try_into().unwrap_or_default()))
                                .unwrap_or(0)
                        }
                        _ => {}
                    }
                }
                if let Some(public_key) = key {
                    out.push(Peer { public_key, last_handshake: last });
                }
            }
        }
    }
    out
}

/// Deletes a link by index. A link already gone is not an error.
pub fn delete_link(ifindex: i32) -> std::io::Result<()> {
    let mut body = vec![libc::AF_UNSPEC as u8, 0, 0, 0];
    body.extend_from_slice(&ifindex.to_ne_bytes());
    body.extend_from_slice(&[0u8; 8]); // flags, change
    match transact(libc::NETLINK_ROUTE, &message(RTM_DELLINK, NLM_F_REQUEST | NLM_F_ACK, &body)) {
        Ok(_) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::ENODEV) => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer_attrs(key: [u8; 32], last: i64) -> Vec<u8> {
        let mut p = vec![];
        attr(&mut p, WGPEER_A_PUBLIC_KEY, &key);
        let mut ts = last.to_ne_bytes().to_vec();
        ts.extend_from_slice(&0i64.to_ne_bytes());
        attr(&mut p, WGPEER_A_LAST_HANDSHAKE_TIME, &ts);
        p
    }

    #[test]
    fn parses_peers_across_replies_and_survives_truncation() {
        let mut list = vec![];
        attr(&mut list, 0x8000, &peer_attrs([1; 32], 1700));
        attr(&mut list, 0x8001, &peer_attrs([2; 32], 0));
        let mut device = vec![WG_CMD_GET_DEVICE, 1, 0, 0];
        attr(&mut device, WGDEVICE_A_IFNAME, b"wg0\0");
        attr(&mut device, WGDEVICE_A_PEERS | 0x8000, &list);
        let mut second = vec![WG_CMD_GET_DEVICE, 1, 0, 0];
        let mut list2 = vec![];
        attr(&mut list2, 0x8000, &peer_attrs([3; 32], 42));
        attr(&mut second, WGDEVICE_A_PEERS | 0x8000, &list2);
        let peers = parse_peers(&[device.clone(), second]);
        assert_eq!(peers.len(), 3);
        assert_eq!(peers[0], Peer { public_key: [1; 32], last_handshake: 1700 });
        assert_eq!(peers[1].last_handshake, 0);
        assert_eq!(peers[2].public_key, [3; 32]);
        for cut in 0..device.len() {
            let _ = parse_peers(&[device[..cut].to_vec()]);
        }
    }

    #[test]
    fn deleting_a_missing_link_is_not_an_error_or_needs_privilege() {
        // Unprivileged test runs get EPERM; root gets success for a gone
        // link. Either way it must not panic, and ENODEV is success.
        match delete_link(999_999) {
            Ok(()) => {}
            Err(e) => assert_eq!(e.raw_os_error(), Some(libc::EPERM)),
        }
    }
}
