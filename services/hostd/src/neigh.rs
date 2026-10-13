// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Is a gateway really there? hostd sends it one UDP datagram (which makes
//! the kernel resolve its link-layer address), then reads the kernel's
//! neighbour entry for it over rtnetlink. Neither needs a privilege, unlike
//! ICMP echo.
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    time::Duration,
};

const RTM_NEWNEIGH: u16 = 28;
const RTM_GETNEIGH: u16 = 30;
const NLMSG_DONE: u16 = 3;
const NLMSG_ERROR: u16 = 2;
const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_DUMP: u16 = 0x300;
const NDA_DST: u16 = 1;

/// Neighbour states (`NUD_*`).
pub const NUD_INCOMPLETE: u16 = 0x01;
pub const NUD_REACHABLE: u16 = 0x02;
pub const NUD_STALE: u16 = 0x04;
pub const NUD_FAILED: u16 = 0x20;
pub const NUD_NOARP: u16 = 0x40;
pub const NUD_PERMANENT: u16 = 0x80;

/// Whether a neighbour state proves the gateway answered (or needs no
/// answer: a link without ARP, or a static entry).
pub fn confirmed(state: u16) -> bool {
    state & (NUD_REACHABLE | NUD_STALE | NUD_PERMANENT | NUD_NOARP) != 0
}

/// Sends one datagram to the discard port, so the kernel resolves the
/// gateway's link-layer address. The socket is bound to the link
/// (`SO_BINDTODEVICE`, unprivileged since Linux 5.7) and an IPv6 destination
/// carries the link as its scope, so the datagram leaves through the link
/// being proven even when another link has an overlapping route. Errors are
/// ignored: the neighbour table is the answer.
pub fn nudge(link: &str, ifindex: i32, gateway: IpAddr) {
    let (bind, to): (SocketAddr, SocketAddr) = match gateway {
        IpAddr::V4(_) => (
            "0.0.0.0:0".parse().expect("literal"),
            SocketAddr::new(gateway, 9),
        ),
        IpAddr::V6(v6) => (
            "[::]:0".parse().expect("literal"),
            SocketAddr::V6(std::net::SocketAddrV6::new(v6, 9, 0, ifindex as u32)),
        ),
    };
    let Ok(socket) = UdpSocket::bind(bind) else {
        return;
    };
    // SAFETY: setsockopt reads `link.len()` bytes from the name.
    let bound = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            link.as_ptr().cast(),
            link.len() as libc::socklen_t,
        )
    };
    if bound == 0 {
        let _ = socket.send_to(b"hostd", to);
    }
}

/// The kernel's neighbour state for `gateway` on link `ifindex`, if it has
/// an entry.
pub fn state(ifindex: i32, gateway: IpAddr) -> std::io::Result<Option<u16>> {
    let family = match gateway {
        IpAddr::V4(_) => libc::AF_INET,
        IpAddr::V6(_) => libc::AF_INET6,
    } as u8;
    let want: Vec<u8> = match gateway {
        IpAddr::V4(a) => a.octets().to_vec(),
        IpAddr::V6(a) => a.octets().to_vec(),
    };
    let fd = netlink_socket()?;
    // nlmsghdr (16) + ndmsg (12).
    let mut request = Vec::with_capacity(28);
    request.extend_from_slice(&28u32.to_ne_bytes());
    request.extend_from_slice(&RTM_GETNEIGH.to_ne_bytes());
    request.extend_from_slice(&(NLM_F_REQUEST | NLM_F_DUMP).to_ne_bytes());
    request.extend_from_slice(&1u32.to_ne_bytes()); // seq
    request.extend_from_slice(&0u32.to_ne_bytes()); // pid
    request.extend_from_slice(&[family, 0, 0, 0]);
    request.extend_from_slice(&0i32.to_ne_bytes()); // any ifindex
    request.extend_from_slice(&[0u8; 4]); // state, flags, type
    // SAFETY: sends `request.len()` initialised bytes on a socket we own.
    let sent = unsafe { libc::send(fd.as_raw_fd(), request.as_ptr().cast(), request.len(), 0) };
    if sent < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut buf = vec![0u8; 64 * 1024];
    let mut found = None;
    loop {
        // SAFETY: receives into a buffer of exactly `buf.len()` bytes.
        let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let (done, hit) = parse(&buf[..n as usize], ifindex, &want);
        if found.is_none() {
            found = hit;
        }
        if done || n == 0 {
            return Ok(found);
        }
    }
}

fn netlink_socket() -> std::io::Result<OwnedFd> {
    // SAFETY: plain socket(2); the result is checked before use.
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a fresh fd we own.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let timeout = libc::timeval {
        tv_sec: 2,
        tv_usec: 0,
    };
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

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

/// Walks one datagram of netlink messages. Returns whether the dump is done
/// and the state of a matching neighbour, if any.
pub fn parse(buf: &[u8], ifindex: i32, want: &[u8]) -> (bool, Option<u16>) {
    let mut at = 0;
    let mut hit = None;
    while at + 16 <= buf.len() {
        let Some(len) = u32_at(buf, at).map(|l| l as usize) else {
            break;
        };
        let kind = u16_at(buf, at + 4).unwrap_or(0);
        if len < 16 || at + len > buf.len() {
            break;
        }
        match kind {
            NLMSG_DONE | NLMSG_ERROR => return (true, hit),
            RTM_NEWNEIGH if len >= 28 => {
                let msg = &buf[at + 16..at + len];
                let index = i32::from_ne_bytes(msg[4..8].try_into().unwrap_or_default());
                let state = u16_at(msg, 8).unwrap_or(0);
                let mut attr = 12;
                while attr + 4 <= msg.len() {
                    let alen = u16_at(msg, attr).unwrap_or(0) as usize;
                    let atype = u16_at(msg, attr + 2).unwrap_or(0) & 0x3fff;
                    if alen < 4 || attr + alen > msg.len() {
                        break;
                    }
                    if atype == NDA_DST && &msg[attr + 4..attr + alen] == want && index == ifindex {
                        hit = Some(state);
                    }
                    attr += align4(alen);
                }
            }
            _ => {}
        }
        at += align4(len);
    }
    (false, hit)
}

/// Nudges `gateway` and waits, within `window`, for the kernel to confirm
/// it. Bounded to one transaction's health window.
pub async fn reachable(link: &str, ifindex: i32, gateway: IpAddr, window: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let link = link.to_owned();
        let probe = tokio::task::spawn_blocking(move || {
            nudge(&link, ifindex, gateway);
            state(ifindex, gateway)
        })
        .await;
        if let Ok(Ok(Some(state))) = probe
            && confirmed(state)
        {
            return true;
        }
        if tokio::time::Instant::now() + Duration::from_millis(300) >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neigh_msg(ifindex: i32, state: u16, dst: &[u8]) -> Vec<u8> {
        let attr_len = 4 + dst.len();
        let len = 16 + 12 + align4(attr_len);
        let mut m = Vec::new();
        m.extend_from_slice(&(len as u32).to_ne_bytes());
        m.extend_from_slice(&RTM_NEWNEIGH.to_ne_bytes());
        m.extend_from_slice(&[0u8; 10]);
        m.extend_from_slice(&[libc::AF_INET as u8, 0, 0, 0]);
        m.extend_from_slice(&ifindex.to_ne_bytes());
        m.extend_from_slice(&state.to_ne_bytes());
        m.extend_from_slice(&[0, 0]);
        m.extend_from_slice(&(attr_len as u16).to_ne_bytes());
        m.extend_from_slice(&NDA_DST.to_ne_bytes());
        m.extend_from_slice(dst);
        m.resize(len, 0);
        m
    }

    #[test]
    fn parse_finds_the_gateway_on_the_link() {
        let mut buf = neigh_msg(3, NUD_STALE, &[192, 0, 2, 9]);
        buf.extend(neigh_msg(4, NUD_REACHABLE, &[192, 0, 2, 1]));
        buf.extend(neigh_msg(3, NUD_FAILED, &[192, 0, 2, 1]));
        assert_eq!(parse(&buf, 3, &[192, 0, 2, 1]), (false, Some(NUD_FAILED)));
        assert_eq!(
            parse(&buf, 4, &[192, 0, 2, 1]),
            (false, Some(NUD_REACHABLE))
        );
        assert_eq!(parse(&buf, 5, &[192, 0, 2, 1]), (false, None));
        let mut done = vec![0u8; 16];
        done[..4].copy_from_slice(&16u32.to_ne_bytes());
        done[4..6].copy_from_slice(&NLMSG_DONE.to_ne_bytes());
        assert!(parse(&done, 3, &[192, 0, 2, 1]).0);
        // Truncated input never panics.
        assert_eq!(parse(&buf[..20], 3, &[192, 0, 2, 1]), (false, None));
    }

    #[test]
    fn only_answered_states_confirm() {
        assert!(confirmed(NUD_REACHABLE));
        assert!(confirmed(NUD_STALE));
        assert!(confirmed(NUD_NOARP));
        assert!(!confirmed(NUD_INCOMPLETE));
        assert!(!confirmed(NUD_FAILED));
    }

    /// The dump runs unprivileged and answers for loopback's link.
    #[test]
    fn a_live_dump_completes() {
        assert!(state(1, "127.0.0.1".parse().unwrap()).is_ok());
    }
}
