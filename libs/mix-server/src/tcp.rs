// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
pub mod tcp_server {
    use super::tcp_client::{Ownership, TcpPermit};
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock, Mutex};

    /// A listening socket with its permit and (once recorded) its
    /// generation ownership, both released when the last clone drops.
    pub struct Listener {
        pub sock: std::net::TcpListener,
        _permit: TcpPermit,
        pub ownership: Mutex<Option<Ownership>>,
    }

    impl Listener {
        pub fn new(sock: std::net::TcpListener, permit: TcpPermit) -> Self {
            Self {
                sock,
                _permit: permit,
                ownership: Mutex::new(None),
            }
        }
    }

    /// Live, unsubscribed listeners. An Arc, so a waiting accept holds the
    /// socket without holding the registry lock.
    pub static LISTENERS: LazyLock<Mutex<HashMap<u64, Arc<Listener>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    /// Live server-side TCP sockets (listeners plus the connections they
    /// accepted) beyond which tcp_listen and accepts refuse, so a
    /// connection flood cannot exhaust the process's descriptors.
    /// tcp_connect clients are not counted.
    pub const MAX_TCP_HANDLES: usize = 1024;
    pub const DEFAULT_BACKLOG: i32 = 128;
    pub const MAX_BACKLOG: i32 = 4096;
}

/// Stop listening now. A Class C accept may still hold a clone of the
/// listener; shutdown(2) wakes it instead of leaving the port open until
/// the last clone drops.
pub fn close_listener(listener: std::sync::Arc<tcp_server::Listener>) {
    let _ = socket2::SockRef::from(&listener.sock).shutdown(std::net::Shutdown::Both);
}

/// Why an accept attempt produced no connection.
pub enum AcceptError {
    /// Every handle permit is taken; the connection was left queued.
    Limit,
    Io(std::io::Error),
}

impl AcceptError {
    pub fn into_mix(self) -> MixError {
        match self {
            AcceptError::Limit => api::refusal(
                "TCP_HANDLE_LIMIT",
                format!(
                    "tcp_accept: {} live tcp handles is the limit; close some with tcp_close()",
                    tcp_server::MAX_TCP_HANDLES
                ),
            ),
            AcceptError::Io(e) => api::refusal("TCP_ACCEPT_FAILED", format!("tcp_accept: {e}")),
        }
    }
}

/// One non-blocking accept. A permit is reserved before accept(2), so the
/// limit is never exceeded, and dropped again when nothing is pending.
/// Ok(None) means nothing was pending. With an owner, the connection is
/// owned before its handle exists anywhere else.
pub fn try_accept(
    listener: &std::net::TcpListener,
    owner: Option<&std::sync::Arc<tcp_client::TcpOwner>>,
) -> Result<Option<(u64, std::net::SocketAddr)>, AcceptError> {
    loop {
        let Some(permit) = tcp_client::TCP_PERMITS.try_acquire() else {
            return Err(AcceptError::Limit);
        };
        match listener.accept() {
            Ok((stream, _)) => return Ok(Some(adopt_accepted(stream, permit, owner))),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) if accept_retryable(&e) => continue,
            Err(e) => return Err(AcceptError::Io(e)),
        }
    }
}

/// accept(2) errors that concern one pending connection rather than the
/// listener: retry, as the Linux man page advises.
pub fn accept_retryable(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        e.kind(),
        ErrorKind::Interrupted | ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset
    ) || accept_retryable_os(e.raw_os_error())
}

#[cfg(all(target_os = "linux"))]
pub fn accept_retryable_os(code: Option<i32>) -> bool {
    code.is_some_and(|c| {
        matches!(
            c,
            libc::EPROTO
                | libc::ENETDOWN
                | libc::ENOPROTOOPT
                | libc::EHOSTDOWN
                | libc::ENONET
                | libc::EHOSTUNREACH
                | libc::EOPNOTSUPP
                | libc::ENETUNREACH
        )
    })
}

#[cfg(all(not(target_os = "linux")))]
pub fn accept_retryable_os(_code: Option<i32>) -> bool {
    false
}

/// Wait until the listener is readable, Ctrl-C arrives, or `timeout`
/// passes (None waits forever). Ok(false) on timeout; a signal that
/// interrupts poll surfaces as Interrupted. The interrupt wake socket
/// (mix::interrupt::wake_fd) is in the poll set as the fast path: a
/// SIGINT on another thread still ends the wait. Correctness never rests
/// on the byte (another waiter may drain it): the caller bounds each wait
/// (ACCEPT_SLICE) and checks the interrupt flag between waits.
#[cfg(all(unix))]
pub fn wait_listener(
    listener: &std::net::TcpListener,
    timeout: Option<std::time::Duration>,
) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    let wake = mix::interrupt::wake_fd();
    let mut fds = [
        libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: wake.unwrap_or(-1),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let ms = match timeout {
        None => -1,
        Some(d) => d.as_millis().clamp(1, i32::MAX as u128) as i32,
    };
    let count = if wake.is_some() { 2 } else { 1 };
    // SAFETY: `count` valid pollfds that outlive the call.
    let rc = unsafe { libc::poll(fds.as_mut_ptr(), count, ms) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if fds[1].revents != 0 {
        mix::interrupt::drain_wake();
    }
    Ok(rc > 0)
}

/// Without poll(2): sleep briefly; the caller retries accept and checks
/// its deadline.
#[cfg(all(not(unix)))]
pub fn wait_listener(
    _listener: &std::net::TcpListener,
    timeout: Option<std::time::Duration>,
) -> std::io::Result<bool> {
    let tick = std::time::Duration::from_millis(20);
    std::thread::sleep(timeout.map_or(tick, |t| t.min(tick)));
    Ok(true)
}

/// `tcp_listen(host, port[, {backlog}])` → numeric listener handle.
pub fn builtin_tcp_listen(args: Vec<Value>) -> MixResult<Option<Value>> {
    use api::refusal;
    expect_args_between("tcp_listen", &args, 2, 3)?;
    let host = match &args[0] {
        Value::String(s) if !s.trim().is_empty() => s.trim().to_string(),
        Value::String(_) => {
            return Err(refusal(
                "TCP_LISTEN_ARGUMENT",
                "tcp_listen(): host must name an address, such as \"127.0.0.1\"",
            ));
        }
        other => {
            return Err(refusal(
                "TCP_LISTEN_ARGUMENT",
                format!(
                    "tcp_listen(): host must be a string, got {}",
                    other.type_name()
                ),
            ));
        }
    };
    let port = match &args[1] {
        Value::Number(n) if n.fract() == 0.0 && (0.0..=65535.0).contains(n) => *n as u16,
        other => {
            return Err(refusal(
                "TCP_LISTEN_ARGUMENT",
                format!(
                    "tcp_listen(): port must be an integer from 0 to 65535, got {}",
                    other.to_mix_string()
                ),
            ));
        }
    };
    let mut backlog = tcp_server::DEFAULT_BACKLOG;
    match args.get(2) {
        None | Some(Value::Nil) => {}
        Some(Value::Map(m)) => {
            for k in m.keys() {
                if k != "backlog" {
                    return Err(refusal(
                        "TCP_LISTEN_ARGUMENT",
                        format!("tcp_listen(): unknown option '{k}' (supported: backlog)"),
                    ));
                }
            }
            if let Some(v) = m.get("backlog") {
                backlog = match v {
                    Value::Number(n)
                        if n.fract() == 0.0
                            && (1.0..=tcp_server::MAX_BACKLOG as f64).contains(n) =>
                    {
                        *n as i32
                    }
                    other => {
                        return Err(refusal(
                            "TCP_LISTEN_ARGUMENT",
                            format!(
                                "tcp_listen(): option 'backlog' must be an integer from 1 to {}, got {}",
                                tcp_server::MAX_BACKLOG,
                                other.to_mix_string()
                            ),
                        ));
                    }
                };
            }
        }
        Some(other) => {
            return Err(refusal(
                "TCP_LISTEN_ARGUMENT",
                format!(
                    "tcp_listen(): options must be a map or nil, got {}",
                    other.type_name()
                ),
            ));
        }
    }

    use std::net::ToSocketAddrs;
    let resolve_host = host.trim_start_matches('[').trim_end_matches(']');
    let addr = (resolve_host, port)
        .to_socket_addrs()
        .map_err(|e| {
            refusal(
                "TCP_LISTEN_ADDRESS",
                format!("tcp_listen: resolve '{host}': {e}"),
            )
        })?
        .next()
        .ok_or_else(|| {
            refusal(
                "TCP_LISTEN_ADDRESS",
                format!("tcp_listen: no address for '{host}'"),
            )
        })?;
    let permit = tcp_client::TcpPermit::acquire("tcp_listen")?;

    let fail = |e: std::io::Error| {
        let code = match e.kind() {
            std::io::ErrorKind::AddrInUse => "TCP_LISTEN_ADDR_IN_USE",
            std::io::ErrorKind::PermissionDenied => "TCP_LISTEN_PERMISSION",
            std::io::ErrorKind::AddrNotAvailable => "TCP_LISTEN_ADDR_UNAVAILABLE",
            _ => "TCP_LISTEN_FAILED",
        };
        refusal(code, format!("tcp_listen: '{addr}': {e}"))
    };
    use socket2::{Domain, Protocol, Socket, Type};
    let sock =
        Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP)).map_err(fail)?;
    // std's TcpListener::bind sets SO_REUSEADDR on unix too: it lets a
    // restarted server rebind while connections from its previous run
    // sit in TIME_WAIT. On Linux it does NOT allow a second live listener
    // on the same address and port (that needs SO_REUSEPORT): bind still
    // fails with EADDRINUSE, which errors_carry_stable_codes asserts.
    #[cfg(unix)]
    sock.set_reuse_address(true).map_err(fail)?;
    sock.bind(&addr.into()).map_err(fail)?;
    sock.listen(backlog).map_err(fail)?;
    sock.set_nonblocking(true).map_err(fail)?;
    let listener: std::net::TcpListener = sock.into();

    let id = tcp_client::NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    tcp_server::LISTENERS.lock().unwrap().insert(
        id,
        std::sync::Arc::new(tcp_server::Listener::new(listener, permit)),
    );
    Ok(Some(Value::Number(id as f64)))
}

/// `tcp_accept(listener[, {timeout}])` → connected handle | nil on
/// timeout. The blocking form, for non-unix targets and direct
/// call_builtin callers: on unix the evaluator runs every numeric form on
/// the runtime (socket_sources::pull_accept), and the source-id form.
pub fn builtin_tcp_accept(args: Vec<Value>) -> MixResult<Option<Value>> {
    use api::refusal;
    expect_args_between("tcp_accept", &args, 1, 2)?;
    let (listener, deadline) = socket_sources::prepare_accept(&args)?;
    let mut eintr = 0u32;
    let mut entered = false;
    loop {
        // The evaluator records ownership as soon as this returns, with no
        // await in between, so the plain form needs no owner here.
        if let Some((id, _)) = try_accept(&listener.sock, None).map_err(AcceptError::into_mix)? {
            return Ok(Some(Value::Number(id as f64)));
        }
        if mix::interrupt::is_interrupted() {
            return Ok(Some(Value::Nil));
        }
        let left = match deadline {
            None => None,
            Some(d) => {
                let left = d.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    return Ok(Some(Value::Nil));
                }
                Some(left)
            }
        };
        if !entered {
            entered = true;
            ACCEPT_WAITS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        let slice = left.map_or(ACCEPT_SLICE, |l| l.min(ACCEPT_SLICE));
        match wait_listener(&listener.sock, Some(slice)) {
            // Readable, woken by Ctrl-C, or the slice ended: the loop
            // retries accept, then checks the interrupt flag and the
            // deadline.
            Ok(_) => {}
            // A signal interrupts poll(2): the loop top stops on Ctrl-C;
            // anything else keeps waiting, boundedly.
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                eintr += 1;
                if eintr > 10_000 {
                    return Err(refusal(
                        "TCP_ACCEPT_FAILED",
                        "tcp_accept: wait interrupted repeatedly (a signal storm?)",
                    ));
                }
            }
            Err(e) => return Err(refusal("TCP_ACCEPT_FAILED", format!("tcp_accept: {e}"))),
        }
    }
}

/// Blocking tcp_accept calls that have reached their wait loop: a
/// synchronisation point for tests that must signal a waiter that is
/// really waiting.
pub static ACCEPT_WAITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many blocking tcp_accept calls have entered their wait loop.
/// Test synchronisation only; not part of the Mix contract.
#[doc(hidden)]
pub fn tcp_accept_waits_entered() -> usize {
    ACCEPT_WAITS.load(std::sync::atomic::Ordering::Acquire)
}
