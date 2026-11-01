// SPDX-License-Identifier: MIT OR Apache-2.0
//! IPv4 datagram sockets. No optional networking dependency for one-shot sends.
use crate::{error::MixResult, native_events::refusal, value::Value};
use std::net::{Ipv4Addr, UdpSocket};
use std::os::fd::AsRawFd;
#[cfg(feature = "ws")]
use std::os::fd::FromRawFd;

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
#[cfg(feature = "ws")]
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

#[cfg(feature = "ws")]
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
                    "reuse" | "loop" => {
                        let Value::Bool(b) = v else {
                            return Err(refusal("UDP_ARGUMENT", format!("{k} must be bool")));
                        };
                        if k == "reuse" {
                            reuse = *b;
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
        // SAFETY: successful socket() returns a new owned descriptor.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        let sock = unsafe { UdpSocket::from_raw_fd(fd) };
        if reuse {
            option(&sock, libc::SO_REUSEADDR, &1i32, libc::SOL_SOCKET)?;
            option(&sock, libc::SO_REUSEPORT, &1i32, libc::SOL_SOCKET)?;
        }
        let addr = libc::sockaddr_in {
            sin_family: libc::AF_INET as _,
            sin_port: port.to_be(),
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(host.octets()),
            },
            sin_zero: [0; 8],
        };
        // SAFETY: addr is a valid IPv4 sockaddr and sock owns fd.
        if unsafe {
            libc::bind(
                fd,
                (&addr as *const libc::sockaddr_in).cast(),
                std::mem::size_of_val(&addr) as _,
            )
        } < 0
        {
            return Err(io(std::io::Error::last_os_error()));
        }
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

#[cfg(all(test, feature = "ws", target_os = "linux"))]
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
                "udp_send_from",
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
        use crate::builtins::socket_sources::{MAX_SOCKET_FRAMES, SocketRecord};
        let id = listen("127.0.0.1", 0, "{}");
        let port = handles::get(id).unwrap().sock.local_addr().unwrap().port();
        let mut ne = NativeEvents::default();
        ne.own_udp(id);
        let h = ne.udp_on(id, "test.overflow".into(), None).unwrap();
        let data = serde_json::to_vec(
            &serde_json::json!({"bytes":[], "text":"", "from_host":"127.0.0.1", "from_port":port}),
        )
        .unwrap();
        for _ in 0..MAX_SOCKET_FRAMES {
            assert!(ne.queue.socket_push(
                &h,
                SocketRecord {
                    kind: "datagram",
                    data: data.clone()
                }
            ));
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
}
