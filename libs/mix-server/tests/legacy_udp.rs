// SPDX-License-Identifier: MIT OR Apache-2.0
use mix::value::Value;
use std::net::{UdpSocket, Ipv4Addr};
use mix::server_api::testing::record;
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use mix::builtins::call_builtin;
use mix::server_api::testing::*;
    fn listen(host: &str, port: u16, opts: &str) -> u64 {
mix_server::register().unwrap();
        let opts = json_value(serde_json::from_str(opts).unwrap());
        let Some(Value::Number(n)) = mix_server::listen(vec![
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
        let port = get_udp(id).unwrap().sock.local_addr().unwrap().port();
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
            let ev = tokio::time::timeout(std::time::Duration::from_secs(3), ne.event_queue().next(None))
                .await
                .unwrap()
                .unwrap();
            let body: serde_json::Value = serde_json::from_str(&ev.body).unwrap();
            assert_eq!(
                body["datagram"]["bytes"],
                serde_json::json!({"hex":hex_encode(&payload)})
            );
            let args = incoming_event_to_mix_value(&ev);
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
        assert!(get_udp(id).is_err());
        drop(ne);
        assert!(get_udp(other).is_err());
    }
    #[test]
    fn send_source_and_options() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let opts =
            json_value(serde_json::json!({"from":"127.0.0.2", "ttl":4}));
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
            let opts = json_value(serde_json::from_str(opts).unwrap());
            assert!(
                mix_server::listen(vec![
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
                    Some(&json_value(opts))
                )
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn overflow_has_one_terminal_after_ordered_records() {
        use mix::server_api::testing::MAX_SOCKET_FRAMES;
        let id = listen("127.0.0.1", 0, "{}");
        let port = get_udp(id).unwrap().sock.local_addr().unwrap().port();
        let mut ne = NativeEvents::default();
        ne.own_udp(id);
        let h = ne.udp_on(id, "test.overflow".into(), None).unwrap();
        let record = record(&[], std::net::SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
        for _ in 0..MAX_SOCKET_FRAMES {
            assert!(ne.event_queue().socket_push(&h, record.clone()));
        }
        UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .send_to(b"overflow", (Ipv4Addr::LOCALHOST, port))
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while get_udp(id).is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for _ in 0..MAX_SOCKET_FRAMES {
            let ev = ne.event_queue().next(None).await.unwrap();
            assert!(ev.body.contains("datagram"));
        }
        let ev = ne.event_queue().next(None).await.unwrap();
        assert!(ev.body.contains("overflow"));
        assert_eq!(ne.event_queue().socket_snapshot(), (0, 0, 0));
        ne.socket_unwatch("udp", &h).unwrap();
    }

    #[tokio::test]
    async fn rejected_reply_does_not_retire_listener() {
        let closed = UdpSocket::bind("127.0.0.1:0").unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        let id = listen("127.0.0.1", 0, "{}");
        let port = get_udp(id).unwrap().sock.local_addr().unwrap().port();
        drop(closed);
        // Connect only in this regression fixture to guarantee the kernel
        // surfaces ICMP ECONNREFUSED on recv_from on every Linux setup.
        // The production listener stays unconnected; its error path is identical.
        get_udp(id)
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
        let ev = tokio::time::timeout(std::time::Duration::from_secs(3), ne.event_queue().next(None))
            .await
            .unwrap()
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&ev.body).unwrap();
        assert_eq!(body["datagram"]["text"], "alive", "{body}");
        assert!(get_udp(id).is_ok());
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
                json_value(serde_json::json!({"ttl":4})),
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
                    assert!(get_udp(id).is_ok());
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
            let sock = get_udp(first).unwrap();
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