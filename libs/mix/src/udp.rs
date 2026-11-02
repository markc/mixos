// SPDX-License-Identifier: MIT OR Apache-2.0
//! IPv4 datagram sockets. No optional networking dependency for one-shot sends.
use crate::{error::MixResult, native_events::refusal, value::Value};
use std::net::{Ipv4Addr, UdpSocket};
use std::os::fd::AsRawFd;

pub fn ip(v: &Value) -> MixResult<Ipv4Addr> {
    match v {
        Value::String(s) => s.parse().map_err(|_| {
            refusal(
                "UDP_ARGUMENT",
                "expected an IPv4 address (IPv6 is not supported)",
            )
        }),
        _ => Err(refusal("UDP_ARGUMENT", "expected an IPv4 address string")),
    }
}
pub fn io(e: std::io::Error) -> crate::error::MixError {
    let code = match e.kind() {
        std::io::ErrorKind::AddrInUse => "UDP_ADDR_IN_USE",
        std::io::ErrorKind::PermissionDenied => "UDP_PERMISSION",
        std::io::ErrorKind::AddrNotAvailable => "UDP_ADDR_UNAVAILABLE",
        _ => "UDP_IO",
    };
    refusal(code, e.to_string())
}
pub fn option<T>(sock: &UdpSocket, name: i32, value: &T, level: i32) -> MixResult<()> {
    // SAFETY: value is a live socket-option value of the supplied size.
    let rc = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            level,
            name,
            (value as *const T).cast(),
            std::mem::size_of::<T>() as libc::socklen_t,
        )
    };
    if rc < 0 {
        Err(io(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}
pub fn interface(sock: &UdpSocket, ip: Ipv4Addr) -> MixResult<()> {
    option(
        sock,
        libc::IP_MULTICAST_IF,
        &libc::in_addr {
            s_addr: u32::from_ne_bytes(ip.octets()),
        },
        libc::IPPROTO_IP,
    )
}
pub fn ttl(v: &Value) -> MixResult<u32> {
    if let Value::Number(n) = v
        && n.is_finite()
        && n.fract() == 0.0
        && (0.0..=255.0).contains(n)
    {
        return Ok(*n as u32);
    }
    Err(refusal(
        "UDP_ARGUMENT",
        "ttl must be an integer from 0 to 255",
    ))
}
#[cfg(feature = "net-client")]
pub(crate) fn payload(v: &Value) -> MixResult<Vec<u8>> {
    match v {
        Value::String(s) => Ok(s.as_bytes().to_vec()),
        Value::Bytes(b) => Ok(b.to_vec()),
        Value::Buffer(b) => Ok(b.borrow().clone()),
        _ => Err(refusal(
            "UDP_ARGUMENT",
            "payload must be string, bytes or buffer",
        )),
    }
}
pub fn map(v: Option<&Value>) -> MixResult<Option<&indexmap::IndexMap<String, Value>>> {
    match v {
        None | Some(Value::Nil) => Ok(None),
        Some(Value::Map(m)) => Ok(Some(m)),
        _ => Err(refusal("UDP_ARGUMENT", "options must be a map or nil")),
    }
}

/// Errors about a previous datagram's peer do not retire a shared listener.
#[cfg(all(feature = "net-client", target_os = "linux"))]
pub(crate) fn peer_error(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ECONNREFUSED | libc::ENETUNREACH | libc::EHOSTUNREACH | libc::ECONNRESET)
    )
}

/// Fixed peer header (IPv4 octets, network-order port) followed by raw bytes.
/// Queue accounting includes the six metadata bytes, with no JSON expansion.
#[cfg(feature = "net-client")]
pub fn record(
    data: &[u8],
    peer: std::net::SocketAddrV4,
) -> crate::builtins::socket_sources::SocketRecord {
    let mut bytes = Vec::with_capacity(6 + data.len());
    bytes.extend_from_slice(&peer.ip().octets());
    bytes.extend_from_slice(&peer.port().to_be_bytes());
    bytes.extend_from_slice(data);
    crate::builtins::socket_sources::SocketRecord {
        kind: "datagram",
        data: bytes,
    }
}

#[cfg(feature = "net-client")]
pub(crate) fn record_body(data: &[u8]) -> serde_json::Value {
    // Only record() constructs these internal records; header length is fixed.
    let host = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
    let port = u16::from_be_bytes([data[4], data[5]]);
    let payload = &data[6..];
    serde_json::json!({"bytes":{"hex":crate::builtins::hex_encode(payload)}, "text":std::str::from_utf8(payload).ok(), "from_host":host.to_string(), "from_port":port})
}

#[cfg(feature = "net-client")]
pub(crate) mod handles {
    use super::*;
    use crate::builtins::tcp_client::TcpPermit;
    use std::{
        collections::HashMap,
        sync::{Arc, LazyLock, Mutex},
    };
    pub struct Socket {
        pub sock: UdpSocket,
        pub subscribed: std::sync::atomic::AtomicBool,
        pub(crate) _permit: TcpPermit,
    }
    pub(crate) static MAP: LazyLock<Mutex<HashMap<u64, Arc<Socket>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    pub fn get(id: u64) -> MixResult<Arc<Socket>> {
        MAP.lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| refusal("UDP_HANDLE", format!("unknown or closed UDP handle {id}")))
    }
    pub(crate) fn subscribe(id: u64) -> MixResult<Arc<Socket>> {
        let map = MAP.lock().unwrap();
        let sock = map
            .get(&id)
            .ok_or_else(|| refusal("UDP_HANDLE", format!("unknown or closed UDP handle {id}")))?;
        if sock
            .subscribed
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(refusal(
                "SOCKET_SUBSCRIBED",
                "UDP handle is already subscribed",
            ));
        }
        Ok(sock.clone())
    }
    pub(crate) fn close(id: u64) -> MixResult<bool> {
        let mut map = MAP.lock().unwrap();
        if let Some(sock) = map.get(&id)
            && sock.subscribed.load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(refusal(
                "SOCKET_SUBSCRIBED",
                "use udp_unwatch(source) to close a subscribed UDP socket",
            ));
        }
        Ok(map.remove(&id).is_some())
    }

}


