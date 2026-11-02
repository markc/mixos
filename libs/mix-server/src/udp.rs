// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
pub fn listen(args: Vec<Value>) -> MixResult<Option<Value>> {
    let host = ip(&args[0])?;
    let port = match args[1] {
        Value::Number(n) if n.is_finite() && n.fract() == 0.0 && (0.0..=65535.0).contains(&n) => {
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
    let id = api::adopt_udp(sock, permit);
    Ok(Some(Value::Number(id as f64)))
}

/// `udp_recv(port[, {timeout, host, max}])` → one datagram as
/// `{bytes, text, from_host, from_port}`, or `nil` on timeout.
///
/// `timeout` seconds (default 30; 0 = wait forever, like http_get's 0);
/// `host` is the bind address (default "0.0.0.0"); `max` caps the datagram
/// read (default 65535 — a UDP payload cannot exceed that, so the default
/// never truncates; a datagram longer than `max` is truncated to it, which
/// is recvfrom(2)'s own contract). `text` is the payload decoded as UTF-8,
/// or nil when it is not valid UTF-8 (`bytes` always carries the truth).
pub fn builtin_udp_recv(args: Vec<Value>) -> MixResult<Option<Value>> {
    expect_args_between("udp_recv", &args, 1, 2)?;
    // Port 0 would bind an ephemeral port the caller can never learn from
    // the result map — a guaranteed silent timeout, so it is refused.
    let port = as_exact_integer(
        "udp_recv(): port",
        number_arg("udp_recv", &args, 0)?,
        1,
        65535,
    )?;
    let mut timeout_seconds = 30.0;
    let mut bind_host = "0.0.0.0".to_string();
    let mut max: usize = 65535;
    if let Some(opts) = args.get(1) {
        match opts {
            Value::Nil => {}
            Value::Map(m) => {
                for k in m.keys() {
                    if !matches!(k.as_str(), "timeout" | "host" | "max") {
                        return Err(MixError::RuntimeError {
                            span: None,
                            msg: format!(
                                "udp_recv(): unknown option '{k}' (supported: timeout, host, max)"
                            ),
                        });
                    }
                }
                if let Some(v) = m.get("timeout") {
                    timeout_seconds =
                        extract_number(v, InputPolicy::NumberOnly).ok_or_else(|| {
                            MixError::RuntimeError {
                                span: None,
                                msg: format!(
                                    "udp_recv(): option 'timeout' must be a number, got {}",
                                    v.type_name()
                                ),
                            }
                        })?;
                    as_duration("udp_recv(): option 'timeout'", timeout_seconds)?;
                }
                if let Some(v) = m.get("host") {
                    match v {
                        Value::String(s) => bind_host = s.to_string(),
                        other => {
                            return Err(MixError::RuntimeError {
                                span: None,
                                msg: format!(
                                    "udp_recv(): option 'host' must be a string, got {}",
                                    other.type_name()
                                ),
                            });
                        }
                    }
                }
                if let Some(v) = m.get("max") {
                    let n = extract_number(v, InputPolicy::NumberOnly).ok_or_else(|| {
                        MixError::RuntimeError {
                            span: None,
                            msg: format!(
                                "udp_recv(): option 'max' must be a number, got {}",
                                v.type_name()
                            ),
                        }
                    })?;
                    max = as_exact_integer("udp_recv(): option 'max'", n, 1, 65535)? as usize;
                }
            }
            other => {
                return Err(MixError::RuntimeError {
                    span: None,
                    msg: format!(
                        "udp_recv(): options must be a map or nil, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    }

    let sock = std::net::UdpSocket::bind(format!("{bind_host}:{port}")).map_err(|e| {
        MixError::RuntimeError {
            span: None,
            msg: format!("udp_recv: bind '{bind_host}:{port}': {e}"),
        }
    })?;
    if timeout_seconds > 0.0 {
        sock.set_read_timeout(Some(std::time::Duration::from_secs_f64(timeout_seconds)))
            .map_err(|e| MixError::RuntimeError {
                span: None,
                msg: format!("udp_recv: set timeout: {e}"),
            })?;
    }
    let mut buf = vec![0u8; max];
    match sock.recv_from(&mut buf) {
        Ok((n, from)) => {
            buf.truncate(n);
            let text = match std::str::from_utf8(&buf) {
                Ok(s) => Value::String(s.to_string()),
                Err(_) => Value::Nil,
            };
            let mut m = indexmap::IndexMap::new();
            m.insert("bytes".to_string(), Value::bytes(buf));
            m.insert("text".to_string(), text);
            m.insert(
                "from_host".to_string(),
                Value::String(from.ip().to_string()),
            );
            m.insert("from_port".to_string(), Value::Number(from.port() as f64));
            Ok(Some(Value::map(m)))
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            // Timeout is an ordinary answer for a datagram wait, not a fault.
            Ok(Some(Value::Nil))
        }
        Err(e) => Err(MixError::RuntimeError {
            span: None,
            msg: format!("udp_recv: {e}"),
        }),
    }
}
