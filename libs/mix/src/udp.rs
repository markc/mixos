// SPDX-License-Identifier: MIT OR Apache-2.0
//! IPv4 datagram sockets. No optional networking dependency for one-shot sends.
use crate::{error::MixResult, native_events::refusal, value::Value};
use std::net::{Ipv4Addr, UdpSocket};
use std::os::fd::AsRawFd;

pub(crate) fn ip(v: &Value) -> MixResult<Ipv4Addr> {
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
pub(crate) fn io(e: std::io::Error) -> crate::error::MixError {
    let code = match e.kind() {
        std::io::ErrorKind::AddrInUse => "UDP_ADDR_IN_USE",
        std::io::ErrorKind::PermissionDenied => "UDP_PERMISSION",
        std::io::ErrorKind::AddrNotAvailable => "UDP_ADDR_UNAVAILABLE",
        _ => "UDP_IO",
    };
    refusal(code, e.to_string())
}
fn option<T>(sock: &UdpSocket, name: i32, value: &T, level: i32) -> MixResult<()> {
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
pub(crate) fn interface(sock: &UdpSocket, ip: Ipv4Addr) -> MixResult<()> {
    option(
        sock,
        libc::IP_MULTICAST_IF,
        &libc::in_addr {
            s_addr: u32::from_ne_bytes(ip.octets()),
        },
        libc::IPPROTO_IP,
    )
}
pub(crate) fn ttl(v: &Value) -> MixResult<u32> {
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
pub(crate) fn map(v: Option<&Value>) -> MixResult<Option<&indexmap::IndexMap<String, Value>>> {
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
pub(crate) fn record(
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
    pub(crate) struct Socket {
        pub sock: UdpSocket,
        pub subscribed: std::sync::atomic::AtomicBool,
        _permit: TcpPermit,
    }
    pub(crate) static MAP: LazyLock<Mutex<HashMap<u64, Arc<Socket>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    pub(crate) fn get(id: u64) -> MixResult<Arc<Socket>> {
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
    #[cfg(feature = "net-server")]
    pub(crate) fn listen(args: Vec<Value>) -> MixResult<Option<Value>> {
        let host = ip(&args[0])?;
        let port = match args[1] {
            Value::Number(n)
                if n.is_finite() && n.fract() == 0.0 && (0.0..=65535.0).contains(&n) =>
            {
                n as u16
            }
            _ => {
                return Err(refusal(
                    "UDP_ARGUMENT",
                    "port must be an integer from 0 to 65535",
                ));
            }
        };
        let mut iface = Ipv4Addr::UNSPECIFIED;
        let mut reuse = false;
        let mut reuse_port = false;
        let mut groups = Vec::new();
        let mut hops = None;
        let mut loopback = None;
        if let Some(m) = map(args.get(2))? {
            for (k, v) in m {
                match k.as_str() {
                    "iface" => iface = ip(v)?,
                    "join" => {
                        let vs = match v {
                            Value::List(xs) => xs.as_slice(),
                            _ => std::slice::from_ref(v),
                        };
                        for x in vs {
                            let g = ip(x)?;
                            if !g.is_multicast() {
                                return Err(refusal(
                                    "UDP_ARGUMENT",
                                    "join requires multicast IPv4 groups",
                                ));
                            }
                            groups.push(g);
                        }
                    }
                    "reuse" | "reuse_port" | "loop" => {
                        let Value::Bool(b) = v else {
                            return Err(refusal("UDP_ARGUMENT", format!("{k} must be bool")));
                        };
                        if k == "reuse" {
                            reuse = *b;
                        } else if k == "reuse_port" {
                            reuse_port = *b;
                        } else {
                            loopback = Some(*b);
                        }
                    }
                    "ttl" => hops = Some(ttl(v)?),
                    _ => return Err(refusal("UDP_ARGUMENT", format!("unknown UDP option '{k}'"))),
                }
            }
        }
        let permit = TcpPermit::acquire("udp_listen")?;
        // socket2 handles native sockaddr layouts (including BSD sin_len)
        // and descriptor inheritance portably. Already provided by ws.
        let sock: UdpSocket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .map_err(io)?
        .into();
        if reuse {
            option(&sock, libc::SO_REUSEADDR, &1i32, libc::SOL_SOCKET)?;
        }
        if reuse_port {
            option(&sock, libc::SO_REUSEPORT, &1i32, libc::SOL_SOCKET)?;
        }
        let addr = std::net::SocketAddrV4::new(host, port);
        socket2::SockRef::from(&sock)
            .bind(&addr.into())
            .map_err(io)?;
        interface(&sock, iface)?;
        if let Some(n) = hops {
            sock.set_multicast_ttl_v4(n).map_err(io)?;
        }
        if let Some(b) = loopback {
            sock.set_multicast_loop_v4(b).map_err(io)?;
        }
        for g in groups {
            sock.join_multicast_v4(&g, &iface).map_err(io)?;
        }
        sock.set_nonblocking(true).map_err(io)?;
        let id =
            crate::builtins::tcp_client::NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        MAP.lock().unwrap().insert(
            id,
            Arc::new(Socket {
                sock,
                subscribed: false.into(),
                _permit: permit,
            }),
        );
        Ok(Some(Value::Number(id as f64)))
    }
}

#[cfg(all(test, feature = "net-client", target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{builtins::call_builtin, native_events::NativeEvents};
    fn listen(host: &str, port: u16, opts: &str) -> u64 {
        let opts = crate::native_events::json_value(serde_json::from_str(opts).unwrap());
        let Some(Value::Number(n)) = handles::listen(vec![
            Value::String(host.into()),
            Value::Number(port as f64),
            opts,
        ])
        .unwrap() else {
            panic!()
        };
        n as u64
    }
    #[tokio::test]
    async fn multicast_reuse_pump_and_retirement() {
        let opts =
            r#"{"reuse":true,"join":"239.255.255.250","iface":"127.0.0.1","loop":true,"ttl":4}"#;
        let id = listen("0.0.0.0", 0, opts);
        let port = handles::get(id).unwrap().sock.local_addr().unwrap().port();
        let other = listen("0.0.0.0", port, opts);
        let mut ne = NativeEvents::default();
        ne.own_udp(id);
        ne.own_udp(other);
        let h = ne.udp_on(id, "test.udp".into(), None).unwrap();
        assert!(ne.udp_on(id, "duplicate".into(), None).is_err());
        for payload in [vec![0, 255], vec![], b"hello".to_vec()] {
            call_builtin(
                "udp_send_via",
                vec![
                    Value::Number(id as f64),
                    Value::String("239.255.255.250".into()),
                    Value::Number(port as f64),
                    Value::bytes(payload.clone()),
                ],
            )
            .unwrap();
            let ev = tokio::time::timeout(std::time::Duration::from_secs(3), ne.queue.next(None))
                .await
                .unwrap()
                .unwrap();
            let body: serde_json::Value = serde_json::from_str(&ev.body).unwrap();
            assert_eq!(
                body["datagram"]["bytes"],
                serde_json::json!({"hex":crate::builtins::hex_encode(&payload)})
            );
            let args = crate::evaluator::incoming_event_to_mix_value(&ev);
            let Value::Map(ref m) = args else { panic!() };
            let Value::Map(a) = &m["args"] else { panic!() };
            let Value::Map(d) = &a["datagram"] else {
                panic!()
            };
            assert_eq!(d["bytes"], Value::bytes(payload.clone()));
            assert_eq!(d["from_host"], Value::String("127.0.0.1".into()));
            assert_eq!(d["from_port"], Value::Number(port as f64));
            if payload == [0, 255] {
                assert_eq!(d["text"], Value::Nil);
            }
        }
        ne.socket_unwatch("udp", &h).unwrap();
        assert!(handles::get(id).is_err());
        drop(ne);
        assert!(handles::get(other).is_err());
    }
    #[test]
    fn send_source_and_options() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let opts =
            crate::native_events::json_value(serde_json::json!({"from":"127.0.0.2", "ttl":4}));
        call_builtin(
            "udp_send",
            vec![
                Value::String("127.0.0.1".into()),
                Value::Number(receiver.local_addr().unwrap().port() as f64),
                Value::String("test".into()),
                opts,
            ],
        )
        .unwrap();
        let mut buf = [0; 16];
        let (n, from) = receiver.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"test");
        assert_eq!(from.ip().to_string(), "127.0.0.2");
        for opts in [
            r#"{"join":"::1"}"#,
            r#"{"join":"127.0.0.1"}"#,
            r#"{"reuse":1}"#,
            r#"{"reuse_port":1}"#,
            r#"{"loop":1}"#,
            r#"{"ttl":256}"#,
            r#"{"ttl":-1}"#,
            r#"{"iface":"bad"}"#,
            r#"{"unknown":true}"#,
            "1",
        ] {
            let opts = crate::native_events::json_value(serde_json::from_str(opts).unwrap());
            assert!(
                handles::listen(vec![
                    Value::String("127.0.0.1".into()),
                    Value::Number(0.0),
                    opts
                ])
                .is_err()
            );
        }
        let id = listen("127.0.0.1", 0, "{}");
        let mut ne = NativeEvents::default();
        ne.own_udp(id);
        for opts in [
            serde_json::json!({"max":0}),
            serde_json::json!({"max":65536}),
            serde_json::json!({"bad":1}),
        ] {
            assert!(
                ne.udp_on(
                    id,
                    "x".into(),
                    Some(&crate::native_events::json_value(opts))
                )
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn overflow_has_one_terminal_after_ordered_records() {
        use crate::builtins::socket_sources::MAX_SOCKET_FRAMES;
        let id = listen("127.0.0.1", 0, "{}");
        let port = handles::get(id).unwrap().sock.local_addr().unwrap().port();
        let mut ne = NativeEvents::default();
        ne.own_udp(id);
        let h = ne.udp_on(id, "test.overflow".into(), None).unwrap();
        let record = record(&[], std::net::SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
        for _ in 0..MAX_SOCKET_FRAMES {
            assert!(ne.queue.socket_push(&h, record.clone()));
        }
        UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .send_to(b"overflow", (Ipv4Addr::LOCALHOST, port))
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while handles::get(id).is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for _ in 0..MAX_SOCKET_FRAMES {
            let ev = ne.queue.next(None).await.unwrap();
            assert!(ev.body.contains("datagram"));
        }
        let ev = ne.queue.next(None).await.unwrap();
        assert!(ev.body.contains("overflow"));
        assert_eq!(ne.queue.socket_snapshot(), (0, 0, 0));
        ne.socket_unwatch("udp", &h).unwrap();
    }

    #[tokio::test]
    async fn rejected_reply_does_not_retire_listener() {
        let closed = UdpSocket::bind("127.0.0.1:0").unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        let id = listen("127.0.0.1", 0, "{}");
        let port = handles::get(id).unwrap().sock.local_addr().unwrap().port();
        drop(closed);
        // Connect only in this regression fixture to guarantee the kernel
        // surfaces ICMP ECONNREFUSED on recv_from on every Linux setup.
        // The production listener stays unconnected; its error path is identical.
        handles::get(id)
            .unwrap()
            .sock
            .connect((Ipv4Addr::LOCALHOST, closed_port))
            .unwrap();
        let mut ne = NativeEvents::default();
        ne.own_udp(id);
        ne.udp_on(id, "test.peer".into(), None).unwrap();
        call_builtin(
            "udp_send_via",
            vec![
                Value::Number(id as f64),
                Value::String("127.0.0.1".into()),
                Value::Number(closed_port as f64),
                Value::String("reply".into()),
            ],
        )
        .unwrap();
        // Let the local ICMP response arrive before sending the next request.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        UdpSocket::bind((Ipv4Addr::LOCALHOST, closed_port))
            .unwrap()
            .send_to(b"alive", (Ipv4Addr::LOCALHOST, port))
            .unwrap();
        let ev = tokio::time::timeout(std::time::Duration::from_secs(3), ne.queue.next(None))
            .await
            .unwrap()
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&ev.body).unwrap();
        assert_eq!(body["datagram"]["text"], "alive", "{body}");
        assert!(handles::get(id).is_ok());
        for errno in [
            libc::ECONNREFUSED,
            libc::ENETUNREACH,
            libc::EHOSTUNREACH,
            libc::ECONNRESET,
        ] {
            assert!(peer_error(&std::io::Error::from_raw_os_error(errno)));
        }
        assert!(!peer_error(&std::io::Error::from_raw_os_error(libc::EBADF)));
    }

    #[test]
    fn literal_destinations_option_errors_and_compact_records() {
        let id = listen("127.0.0.1", 0, "{}");
        for host in ["localhost", "example.invalid", "::1", "127.0.0.1:1900"] {
            let err = call_builtin(
                "udp_send_via",
                vec![
                    Value::Number(id as f64),
                    Value::String(host.into()),
                    Value::Number(1900.0),
                    Value::String("probe".into()),
                ],
            )
            .unwrap_err();
            assert_eq!(err.info().unwrap().code, "UDP_ARGUMENT");
        }
        // Only the new options form changes its I/O error codes; the
        // original three-argument one-shot send keeps its existing errors.
        let err = call_builtin(
            "udp_send",
            vec![
                Value::String("127.0.0.1".into()),
                Value::Number(1.0),
                Value::bytes(vec![0; 65536]),
                crate::native_events::json_value(serde_json::json!({"ttl":4})),
            ],
        )
        .unwrap_err();
        assert_eq!(err.info().unwrap().code, "UDP_IO");
        let peer = std::net::SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1900);
        let payload = vec![255; 65507];
        let rec = record(&payload, peer);
        assert_eq!(rec.data.len(), payload.len() + 6);
        let body = record_body(&rec.data);
        assert_eq!(body["from_port"], 1900);
        assert_eq!(body["text"], serde_json::Value::Null);
        assert_eq!(
            body["bytes"]["hex"].as_str().unwrap(),
            "ff".repeat(payload.len())
        );
        assert!(handles::close(id).unwrap());
        assert!(!handles::close(id).unwrap());
    }

    #[test]
    fn close_and_subscribe_share_one_registry_transaction() {
        for _ in 0..64 {
            let id = listen("127.0.0.1", 0, "{}");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let start = barrier.clone();
            let subscriber = std::thread::spawn(move || {
                start.wait();
                handles::subscribe(id).map_err(|e| e.info().unwrap().code.clone())
            });
            let closer = std::thread::spawn(move || {
                barrier.wait();
                handles::close(id).map_err(|e| e.info().unwrap().code.clone())
            });
            let subscribed = subscriber.join().unwrap();
            let closed = closer.join().unwrap();
            match subscribed {
                Ok(sock) => {
                    assert_eq!(closed.unwrap_err(), "SOCKET_SUBSCRIBED");
                    assert!(handles::get(id).is_ok());
                    handles::MAP.lock().unwrap().remove(&id);
                    drop(sock);
                }
                Err(err) => {
                    assert_eq!(err, "UDP_HANDLE");
                    assert!(closed.unwrap());
                }
            }
        }
    }

    #[test]
    fn reuse_address_and_port_are_independent() {
        fn enabled(sock: &UdpSocket, name: i32) -> bool {
            let mut value = 0_i32;
            let mut size = std::mem::size_of_val(&value) as libc::socklen_t;
            // SAFETY: sock is live; value and size are valid output storage.
            assert_eq!(
                unsafe {
                    libc::getsockopt(
                        sock.as_raw_fd(),
                        libc::SOL_SOCKET,
                        name,
                        (&mut value as *mut i32).cast(),
                        &mut size,
                    )
                },
                0
            );
            value != 0
        }
        for (addr, port) in [(false, false), (true, false), (false, true), (true, true)] {
            let opts = serde_json::json!({"reuse":addr, "reuse_port":port}).to_string();
            let first = listen("0.0.0.0", 0, &opts);
            let sock = handles::get(first).unwrap();
            assert_eq!(enabled(&sock.sock, libc::SO_REUSEADDR), addr);
            assert_eq!(enabled(&sock.sock, libc::SO_REUSEPORT), port);
            if addr || port {
                let second = listen("0.0.0.0", sock.sock.local_addr().unwrap().port(), &opts);
                assert!(handles::close(second).unwrap());
            }
            assert!(handles::close(first).unwrap());
        }
    }
}
