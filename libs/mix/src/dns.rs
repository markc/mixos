// SPDX-License-Identifier: MIT OR Apache-2.0
//! `dns` feature — the `dns_query` builtin.
//!
//! `dns_lookup` asks the system resolver for addresses and sees nothing
//! else. `dns_query` asks a chosen server one question of a chosen type and
//! returns the whole answer: the rcode, the `aa`/`ra`/`tc` flags, and every
//! record in the answer, authority and additional sections with its TTL.
//! That is what tells an authoritative answer from a cached one, or a
//! REFUSED out-of-zone query from NXDOMAIN, when diagnosing split horizon.
//!
//! The wire format is hickory-proto (pure Rust); the transport is a plain
//! blocking UDP socket, with TCP on request or when the UDP answer comes
//! back truncated. One question, one server, no retries across servers, no
//! search list: the name is always taken as fully qualified. Operational
//! failures (timeout, unreachable, a malformed reply) are returned as
//! `{ok: false, error_code, error}`, like `ssh_exec`; bad arguments raise.

use crate::builtins::opt_invalid;
use crate::error::{MixError, MixResult};
use crate::value::Value;
use hickory_proto::op::{Edns, Message, MessageType, Query};
use hickory_proto::rr::{Name, Record, RecordType};
use indexmap::IndexMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::str::FromStr;
use std::time::{Duration, Instant};

const CALLER: &str = "dns_query()";

/// The UDP payload size advertised with EDNS (RFC 9715's safe default).
const EDNS_PAYLOAD: u16 = 1232;

struct QueryOpts {
    server: Option<String>,
    port: u16,
    timeout: Duration,
    tcp: bool,
    recurse: bool,
}

fn parse_opts(v: Option<&Value>) -> MixResult<QueryOpts> {
    let mut opts = QueryOpts {
        server: None,
        port: 53,
        timeout: Duration::from_secs(5),
        tcp: false,
        recurse: true,
    };
    let m = match v {
        None | Some(Value::Nil) => return Ok(opts),
        Some(Value::Map(m)) => m,
        Some(other) => {
            return Err(opt_invalid(
                CALLER,
                format!("options must be a map or nil, got {}", other.type_name()),
            ));
        }
    };
    for (k, v) in m.iter() {
        match (k.as_str(), v) {
            ("server", Value::String(s)) if !s.is_empty() => opts.server = Some(s.clone()),
            ("port", Value::Number(n)) => {
                opts.port =
                    crate::numeric::as_exact_integer("dns_query(): port", *n, 1, 65535)? as u16;
            }
            ("timeout", Value::Number(n)) => {
                opts.timeout = crate::numeric::as_duration("dns_query(): timeout", *n)?;
                if opts.timeout.is_zero() {
                    return Err(opt_invalid(CALLER, "timeout must be greater than 0"));
                }
            }
            ("tcp", Value::Bool(b)) => opts.tcp = *b,
            ("recurse", Value::Bool(b)) => opts.recurse = *b,
            ("server" | "port" | "timeout" | "tcp" | "recurse", other) => {
                let want = match k.as_str() {
                    "server" => "a non-empty string",
                    "port" | "timeout" => "a number",
                    _ => "a bool",
                };
                return Err(opt_invalid(
                    CALLER,
                    format!("option '{k}' must be {want}, got {}", other.type_name()),
                ));
            }
            _ => {
                return Err(opt_invalid(
                    CALLER,
                    format!(
                        "unknown option '{k}' (supported: server, port, timeout, tcp, recurse)"
                    ),
                ));
            }
        }
    }
    Ok(opts)
}

/// The first `nameserver` in /etc/resolv.conf, or 127.0.0.1 as glibc does
/// when there is none.
fn system_server() -> String {
    std::fs::read_to_string("/etc/resolv.conf")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some("nameserver"))
                    .then(|| words.next().map(str::to_string))
                    .flatten()
            })
        })
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

/// An operational failure, returned rather than raised.
fn failure(code: &str, msg: String, server: &str, started: Instant) -> Value {
    let mut map = IndexMap::new();
    map.insert("ok".to_string(), Value::Bool(false));
    map.insert("error_code".to_string(), Value::String(code.to_string()));
    map.insert("error".to_string(), Value::String(msg));
    map.insert("server".to_string(), Value::String(server.to_string()));
    map.insert(
        "duration_ms".to_string(),
        Value::Number(started.elapsed().as_millis() as f64),
    );
    Value::map(map)
}

/// The rcode as dig prints it.
fn rcode_name(code: u16) -> String {
    match code {
        0 => "NOERROR".into(),
        1 => "FORMERR".into(),
        2 => "SERVFAIL".into(),
        3 => "NXDOMAIN".into(),
        4 => "NOTIMP".into(),
        5 => "REFUSED".into(),
        6 => "YXDOMAIN".into(),
        7 => "YXRRSET".into(),
        8 => "NXRRSET".into(),
        9 => "NOTAUTH".into(),
        10 => "NOTZONE".into(),
        16 => "BADVERS".into(),
        n => format!("RCODE{n}"),
    }
}

fn record_value(r: &Record) -> Value {
    let mut map = IndexMap::new();
    map.insert("name".to_string(), Value::String(r.name().to_string()));
    map.insert(
        "type".to_string(),
        Value::String(r.record_type().to_string()),
    );
    map.insert("ttl".to_string(), Value::Number(f64::from(r.ttl())));
    map.insert("data".to_string(), Value::String(r.data().to_string()));
    Value::map(map)
}

fn records(rs: &[Record]) -> Value {
    Value::list(rs.iter().map(record_value).collect())
}

enum Failure {
    Timeout,
    Unreachable(std::io::Error),
    Io(std::io::Error),
    Protocol(String),
}

fn classify(e: std::io::Error) -> Failure {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::WouldBlock | K::TimedOut => Failure::Timeout,
        K::ConnectionRefused | K::ConnectionReset | K::HostUnreachable | K::NetworkUnreachable => {
            Failure::Unreachable(e)
        }
        _ => Failure::Io(e),
    }
}

fn remaining(deadline: Instant) -> Result<Duration, Failure> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(Failure::Timeout)
    } else {
        Ok(left)
    }
}

/// A reply is ours when it is a response with our id. Anything else on the
/// socket (a late reply to an earlier query, noise) is ignored.
fn ours(reply: &Message, id: u16) -> bool {
    reply.id() == id && reply.message_type() == MessageType::Response
}

fn via_udp(addr: SocketAddr, wire: &[u8], id: u16, deadline: Instant) -> Result<Message, Failure> {
    let bind: SocketAddr = if addr.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    let socket = UdpSocket::bind(bind).map_err(Failure::Io)?;
    // Connected: the kernel drops datagrams from anyone but the server, and
    // reports an ICMP port-unreachable as ECONNREFUSED.
    socket.connect(addr).map_err(classify)?;
    socket.send(wire).map_err(classify)?;
    let mut buf = vec![0u8; 65535];
    let mut malformed: Option<String> = None;
    loop {
        let left = match remaining(deadline) {
            Ok(left) => left,
            Err(timeout) => {
                return Err(malformed.map(Failure::Protocol).unwrap_or(timeout));
            }
        };
        socket.set_read_timeout(Some(left)).map_err(Failure::Io)?;
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => match classify(e) {
                Failure::Timeout => continue,
                other => return Err(other),
            },
        };
        match Message::from_vec(&buf[..n]) {
            Ok(reply) if ours(&reply, id) => return Ok(reply),
            Ok(_) => {}
            Err(e) => malformed = Some(format!("malformed reply: {e}")),
        }
    }
}

fn via_tcp(addr: SocketAddr, wire: &[u8], id: u16, deadline: Instant) -> Result<Message, Failure> {
    let mut stream = TcpStream::connect_timeout(&addr, remaining(deadline)?).map_err(classify)?;
    let len = u16::try_from(wire.len()).map_err(|_| Failure::Protocol("query too long".into()))?;
    stream
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(Failure::Io)?;
    let mut framed = len.to_be_bytes().to_vec();
    framed.extend_from_slice(wire);
    stream.write_all(&framed).map_err(classify)?;
    loop {
        let mut len = [0u8; 2];
        read_by(
            &mut stream,
            &mut len,
            deadline,
            "the server closed the connection without a reply",
        )?;
        let mut body = vec![0u8; usize::from(u16::from_be_bytes(len))];
        read_by(&mut stream, &mut body, deadline, "the reply was cut short")?;
        let reply = Message::from_vec(&body)
            .map_err(|e| Failure::Protocol(format!("malformed reply: {e}")))?;
        if ours(&reply, id) {
            return Ok(reply);
        }
    }
}

/// `read_exact`, but the whole read ends by `deadline`: the socket timeout is
/// re-armed with what is left before every read, so a server that trickles a
/// frame byte by byte cannot stretch the query past its timeout.
fn read_by(
    stream: &mut TcpStream,
    buf: &mut [u8],
    deadline: Instant,
    short: &str,
) -> Result<(), Failure> {
    let mut got = 0;
    while got < buf.len() {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(Failure::Io)?;
        match stream.read(&mut buf[got..]) {
            Ok(0) => return Err(Failure::Protocol(short.to_string())),
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(classify(e)),
        }
    }
    Ok(())
}

/// Resolve a server host name within the query deadline. The system
/// resolver has no timeout of its own, so it runs on a helper thread; past
/// the deadline the answer is abandoned (the thread ends when getaddrinfo
/// returns) and the query reports a timeout.
fn resolve_server(server: &str, port: u16, deadline: Instant) -> Result<SocketAddr, Failure> {
    let (tx, rx) = std::sync::mpsc::channel();
    let host = server.to_string();
    std::thread::Builder::new()
        .name("dns_query-resolve".into())
        .spawn(move || {
            let _ = tx.send(
                (host.as_str(), port)
                    .to_socket_addrs()
                    .map(|mut a| a.next()),
            );
        })
        .map_err(Failure::Io)?;
    match rx.recv_timeout(remaining(deadline)?) {
        Ok(Ok(Some(addr))) => Ok(addr),
        Ok(Ok(None)) => Err(Failure::Protocol(format!(
            "server '{server}' has no address"
        ))),
        Ok(Err(e)) => Err(Failure::Protocol(format!(
            "cannot resolve server '{server}': {e}"
        ))),
        Err(_) => Err(Failure::Timeout),
    }
}

/// `dns_query(name, type[, {server, port, timeout, tcp, recurse}])`.
pub fn builtin_dns_query(args: Vec<Value>) -> MixResult<Option<Value>> {
    if args.len() < 2 || args.len() > 3 {
        return Err(MixError::RuntimeError {
            span: None,
            msg: format!(
                "{CALLER} expects 2 or 3 args (name, type[, opts]), got {}",
                args.len()
            ),
        });
    }
    let (Value::String(name), Value::String(kind)) = (&args[0], &args[1]) else {
        return Err(MixError::structured(
            "TYPE_MISMATCH",
            format!(
                "{CALLER}: name and type must be strings, got {} and {}",
                args[0].type_name(),
                args[1].type_name()
            ),
        ));
    };
    let opts = parse_opts(args.get(2))?;
    let rtype = RecordType::from_str(&kind.to_ascii_uppercase()).map_err(|_| {
        opt_invalid(
            CALLER,
            format!("unknown record type '{kind}' (A, AAAA, MX, TXT, SRV, NS, SOA, PTR, CNAME, CAA, ...)"),
        )
    })?;
    // PTR for an address means its reverse name, as `dig -x` does.
    let qname = match (rtype, IpAddr::from_str(name)) {
        (RecordType::PTR, Ok(ip)) => Name::from(ip),
        _ => {
            let mut n = Name::from_utf8(name)
                .map_err(|e| opt_invalid(CALLER, format!("invalid name '{name}': {e}")))?;
            n.set_fqdn(true);
            n
        }
    };

    let started = Instant::now();
    let deadline = started
        .checked_add(opts.timeout)
        .ok_or_else(|| opt_invalid(CALLER, "timeout is too large"))?;
    let server = opts.server.clone().unwrap_or_else(system_server);
    // An address literal (IPv6 with or without brackets), else a host name
    // resolved by the system resolver.
    let literal = server.trim_start_matches('[').trim_end_matches(']');
    let addr = match IpAddr::from_str(literal) {
        Ok(ip) => SocketAddr::new(ip, opts.port),
        Err(_) => match resolve_server(&server, opts.port, deadline) {
            Ok(addr) => addr,
            Err(f) => {
                let (code, msg) = match f {
                    Failure::Timeout => (
                        "DNS_TIMEOUT",
                        format!(
                            "{CALLER}: resolving server '{server}' took longer than the timeout"
                        ),
                    ),
                    Failure::Protocol(m) => ("DNS_SERVER", format!("{CALLER}: {m}")),
                    Failure::Io(e) | Failure::Unreachable(e) => (
                        "DNS_SERVER",
                        format!("{CALLER}: cannot resolve server '{server}': {e}"),
                    ),
                };
                return Ok(Some(failure(code, msg, &server, started)));
            }
        },
    };

    let mut query = Message::query();
    query
        .add_query(Query::query(qname, rtype))
        .set_recursion_desired(opts.recurse);
    let mut edns = Edns::new();
    edns.set_max_payload(EDNS_PAYLOAD);
    query.set_edns(edns);
    let id = query.id();
    let wire = query
        .to_vec()
        .map_err(|e| opt_invalid(CALLER, format!("cannot encode the query: {e}")))?;

    let mut transport = if opts.tcp { "tcp" } else { "udp" };
    let mut result = if opts.tcp {
        via_tcp(addr, &wire, id, deadline)
    } else {
        via_udp(addr, &wire, id, deadline)
    };
    // A truncated UDP answer is retried over TCP for the whole answer.
    if matches!(&result, Ok(reply) if reply.truncated()) && !opts.tcp {
        transport = "tcp";
        result = via_tcp(addr, &wire, id, deadline);
    }
    let server_shown = addr.ip().to_string();
    let reply = match result {
        Ok(reply) => reply,
        Err(f) => {
            let (code, msg) = match f {
                Failure::Timeout => (
                    "DNS_TIMEOUT",
                    format!(
                        "{CALLER}: no reply from {addr} over {transport} within {:?}",
                        opts.timeout
                    ),
                ),
                Failure::Unreachable(e) => (
                    "DNS_UNREACHABLE",
                    format!("{CALLER}: {addr} over {transport}: {e}"),
                ),
                Failure::Io(e) => ("DNS_IO", format!("{CALLER}: {addr} over {transport}: {e}")),
                Failure::Protocol(m) => (
                    "DNS_PROTOCOL",
                    format!("{CALLER}: {addr} over {transport}: {m}"),
                ),
            };
            return Ok(Some(failure(code, msg, &server_shown, started)));
        }
    };

    let mut map = IndexMap::new();
    map.insert("ok".to_string(), Value::Bool(true));
    map.insert(
        "rcode".to_string(),
        Value::String(rcode_name(u16::from(reply.response_code()))),
    );
    map.insert("aa".to_string(), Value::Bool(reply.authoritative()));
    map.insert("ra".to_string(), Value::Bool(reply.recursion_available()));
    map.insert("tc".to_string(), Value::Bool(reply.truncated()));
    map.insert("answers".to_string(), records(reply.answers()));
    map.insert("authority".to_string(), records(reply.name_servers()));
    map.insert("additional".to_string(), records(reply.additionals()));
    map.insert("server".to_string(), Value::String(server_shown));
    map.insert("port".to_string(), Value::Number(f64::from(addr.port())));
    map.insert(
        "transport".to_string(),
        Value::String(transport.to_string()),
    );
    map.insert(
        "duration_ms".to_string(),
        Value::Number(started.elapsed().as_millis() as f64),
    );
    Ok(Some(Value::map(map)))
}
