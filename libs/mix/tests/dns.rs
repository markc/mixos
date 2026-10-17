// SPDX-License-Identifier: MIT OR Apache-2.0
//! `dns_query` against a local fixture server: no network. The fixture is
//! authoritative for `example.test.` and the reverse zone of 192.0.2.0/24,
//! REFUSES everything else (as an authoritative-only server does), and
//! answers `big.example.test.` with TC over UDP so the TCP retry is
//! exercised. It mirrors the query's RD flag into RA, so a test can see
//! what `recurse` sent.
#![cfg(feature = "dns")]

use hickory_proto::op::{Message, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA, MX, PTR};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use mix::value::Value;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener, UdpSocket};
use std::str::FromStr;

async fn run_ok(source: &str) -> Value {
    match mix::run(source).await {
        Ok(v) => v,
        Err(e) => panic!("script failed: {e}\nsource: {source}"),
    }
}

async fn run_err(source: &str) -> String {
    match mix::run(source).await {
        Ok(v) => panic!("expected an error, got {v:?}\nsource: {source}"),
        Err(e) => e.to_string(),
    }
}

fn field(v: &Value, key: &str) -> Value {
    match v {
        Value::Map(m) => m
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("no '{key}' in {v:?}")),
        other => panic!("expected a map, got {other:?}"),
    }
}

fn list(v: &Value) -> Vec<Value> {
    match v {
        Value::List(l) => l.to_vec(),
        other => panic!("expected a list, got {other:?}"),
    }
}

fn answer(req: &Message, tcp: bool) -> Message {
    let q = req.queries()[0].clone();
    let mut resp = Message::response(req.id(), req.op_code());
    resp.add_query(q.clone());
    resp.set_recursion_desired(req.recursion_desired());
    resp.set_recursion_available(req.recursion_desired());
    let name = q.name().clone();
    let text = name.to_ascii().to_ascii_lowercase();
    let in_zone = text.ends_with("example.test.") || text.ends_with(".2.0.192.in-addr.arpa.");
    if !in_zone {
        resp.set_response_code(ResponseCode::Refused);
        return resp;
    }
    resp.set_authoritative(true);
    let rdata = match (text.as_str(), q.query_type()) {
        ("mail.example.test.", RecordType::AAAA) => Some(RData::AAAA(AAAA(
            Ipv6Addr::from_str("2001:db8::25").unwrap(),
        ))),
        ("example.test.", RecordType::MX) => Some(RData::MX(MX::new(
            10,
            Name::from_ascii("mail.example.test.").unwrap(),
        ))),
        ("25.2.0.192.in-addr.arpa.", RecordType::PTR) => Some(RData::PTR(PTR(Name::from_ascii(
            "mail.example.test.",
        )
        .unwrap()))),
        ("big.example.test.", RecordType::A) if !tcp => {
            resp.set_truncated(true);
            None
        }
        ("big.example.test.", RecordType::A) => Some(RData::A(A(Ipv4Addr::new(192, 0, 2, 80)))),
        _ => {
            resp.set_response_code(ResponseCode::NXDomain);
            None
        }
    };
    if let Some(rdata) = rdata {
        resp.add_answer(Record::from_rdata(name, 300, rdata));
    }
    resp
}

/// Start the fixture on one port for both UDP and TCP; returns the port.
fn fixture() -> u16 {
    let (tcp, udp) = loop {
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = tcp.local_addr().unwrap().port();
        if let Ok(udp) = UdpSocket::bind(("127.0.0.1", port)) {
            break (tcp, udp);
        }
    };
    let port = udp.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let Ok((n, from)) = udp.recv_from(&mut buf) else {
                return;
            };
            let req = Message::from_vec(&buf[..n]).unwrap();
            // Noise first: a reply with the wrong id must be ignored.
            let mut stray = answer(&req, false);
            let mut header = *stray.header();
            header.set_id(req.id().wrapping_add(1));
            stray.set_header(header);
            let _ = udp.send_to(&stray.to_vec().unwrap(), from);
            let _ = udp.send_to(&answer(&req, false).to_vec().unwrap(), from);
        }
    });
    std::thread::spawn(move || {
        for stream in tcp.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut len = [0u8; 2];
            if stream.read_exact(&mut len).is_err() {
                continue;
            }
            let mut body = vec![0u8; usize::from(u16::from_be_bytes(len))];
            if stream.read_exact(&mut body).is_err() {
                continue;
            }
            let req = Message::from_vec(&body).unwrap();
            let wire = answer(&req, true).to_vec().unwrap();
            let mut framed = (wire.len() as u16).to_be_bytes().to_vec();
            framed.extend(wire);
            let _ = stream.write_all(&framed);
        }
    });
    port
}

fn query(name: &str, kind: &str, port: u16, extra: &str) -> String {
    format!(
        "dns_query(\"{name}\", \"{kind}\", {{server: \"127.0.0.1\", port: {port}, timeout: 5{extra}}})"
    )
}

#[tokio::test]
async fn an_authoritative_answer_carries_aa_and_its_records() {
    let port = fixture();
    let r = run_ok(&query("mail.example.test", "AAAA", port, "")).await;
    assert_eq!(field(&r, "ok"), Value::Bool(true));
    assert_eq!(field(&r, "rcode").to_mix_string(), "NOERROR");
    assert_eq!(field(&r, "aa"), Value::Bool(true));
    assert_eq!(field(&r, "transport").to_mix_string(), "udp");
    assert_eq!(field(&r, "server").to_mix_string(), "127.0.0.1");
    let answers = list(&field(&r, "answers"));
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(
        field(&answers[0], "name").to_mix_string(),
        "mail.example.test."
    );
    assert_eq!(field(&answers[0], "type").to_mix_string(), "AAAA");
    assert_eq!(field(&answers[0], "ttl").to_mix_string(), "300");
    assert_eq!(field(&answers[0], "data").to_mix_string(), "2001:db8::25");
    // Type names are case-insensitive; MX data reads as zone text.
    let r = run_ok(&query("example.test.", "mx", port, "")).await;
    let answers = list(&field(&r, "answers"));
    assert_eq!(
        field(&answers[0], "data").to_mix_string(),
        "10 mail.example.test."
    );
}

#[tokio::test]
async fn an_out_of_zone_query_is_refused_not_an_error() {
    let port = fixture();
    let r = run_ok(&query("www.example.org", "A", port, "")).await;
    assert_eq!(field(&r, "ok"), Value::Bool(true));
    assert_eq!(field(&r, "rcode").to_mix_string(), "REFUSED");
    assert_eq!(field(&r, "aa"), Value::Bool(false));
    assert!(list(&field(&r, "answers")).is_empty());
    let r = run_ok(&query("nope.example.test", "A", port, "")).await;
    assert_eq!(field(&r, "rcode").to_mix_string(), "NXDOMAIN");
    assert_eq!(field(&r, "aa"), Value::Bool(true));
}

#[tokio::test]
async fn recurse_sets_the_rd_flag() {
    let port = fixture();
    let on = run_ok(&query("mail.example.test", "AAAA", port, "")).await;
    assert_eq!(field(&on, "ra"), Value::Bool(true));
    let off = run_ok(&query(
        "mail.example.test",
        "AAAA",
        port,
        ", recurse: false",
    ))
    .await;
    assert_eq!(field(&off, "ra"), Value::Bool(false));
}

#[tokio::test]
async fn ptr_of_an_address_asks_its_reverse_name() {
    let port = fixture();
    let r = run_ok(&query("192.0.2.25", "PTR", port, "")).await;
    let answers = list(&field(&r, "answers"));
    assert_eq!(
        field(&answers[0], "name").to_mix_string(),
        "25.2.0.192.in-addr.arpa."
    );
    assert_eq!(
        field(&answers[0], "data").to_mix_string(),
        "mail.example.test."
    );
}

#[tokio::test]
async fn a_truncated_udp_answer_is_retried_over_tcp() {
    let port = fixture();
    let r = run_ok(&query("big.example.test", "A", port, "")).await;
    assert_eq!(field(&r, "transport").to_mix_string(), "tcp");
    assert_eq!(field(&r, "tc"), Value::Bool(false));
    let answers = list(&field(&r, "answers"));
    assert_eq!(field(&answers[0], "data").to_mix_string(), "192.0.2.80");
    // tcp:true goes straight to TCP.
    let r = run_ok(&query("mail.example.test", "AAAA", port, ", tcp: true")).await;
    assert_eq!(field(&r, "transport").to_mix_string(), "tcp");
    assert_eq!(field(&r, "aa"), Value::Bool(true));
}

#[tokio::test]
async fn operational_failures_are_returned_not_raised() {
    // A server that reads and never answers.
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = silent.local_addr().unwrap().port();
    let r = run_ok(&format!(
        "dns_query(\"a.example.test\", \"A\", {{server: \"127.0.0.1\", port: {port}, timeout: 0.3}})"
    ))
    .await;
    assert_eq!(field(&r, "ok"), Value::Bool(false));
    assert_eq!(field(&r, "error_code").to_mix_string(), "DNS_TIMEOUT");
    drop(silent);
    // Nothing listening: UDP sees the ICMP port-unreachable, TCP the RST.
    let closed = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    for extra in ["", ", tcp: true"] {
        let r = run_ok(&format!(
            "dns_query(\"a.example.test\", \"A\", {{server: \"127.0.0.1\", port: {port}, timeout: 2{extra}}})"
        ))
        .await;
        assert_eq!(field(&r, "ok"), Value::Bool(false), "{extra}");
        assert_eq!(
            field(&r, "error_code").to_mix_string(),
            "DNS_UNREACHABLE",
            "{extra}: {r:?}"
        );
    }
}

#[tokio::test]
async fn a_trickled_tcp_reply_cannot_outlast_the_timeout() {
    // Announces a long frame, then sends one byte every 100 ms: each read
    // succeeds well within any per-read timeout, so only a whole-query
    // deadline ends it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut len = [0u8; 2];
            let _ = stream.read_exact(&mut len);
            let mut body = vec![0u8; usize::from(u16::from_be_bytes(len))];
            let _ = stream.read_exact(&mut body);
            let _ = stream.write_all(&[0x01, 0x00]);
            for _ in 0..100 {
                if stream.write_all(&[0]).is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    });
    let started = std::time::Instant::now();
    let r = run_ok(&format!(
        "dns_query(\"a.example.test\", \"A\", {{server: \"127.0.0.1\", port: {port}, timeout: 0.5, tcp: true}})"
    ))
    .await;
    assert_eq!(
        field(&r, "error_code").to_mix_string(),
        "DNS_TIMEOUT",
        "{r:?}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn bad_arguments_raise() {
    for (source, fragment) in [
        (
            "dns_query(\"a.example.test\", \"NOPE\")",
            "unknown record type",
        ),
        (
            "dns_query(\"a.example.test\", \"A\", {servr: \"127.0.0.1\"})",
            "unknown option 'servr'",
        ),
        ("dns_query(\"a.example.test\", \"A\", {port: 0})", "port"),
        (
            "dns_query(\"a.example.test\", \"A\", {tcp: \"yes\"})",
            "option 'tcp' must be a bool",
        ),
        (
            "dns_query(\"a.example.test\", \"A\", {timeout: 0})",
            "timeout",
        ),
        ("dns_query(\"a.example.test\")", "expects"),
        // A valid duration that no clock can add: refused, not a panic.
        (
            "dns_query(\"a.example.test\", \"A\", {server: \"127.0.0.1\", timeout: 1e19})",
            "timeout is too large",
        ),
    ] {
        let err = run_err(source).await;
        assert!(err.contains(fragment), "{source}: {err}");
    }
}
