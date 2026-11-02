use std::{
    io::Write,
    os::{fd::AsRawFd, unix::net::UnixStream},
};
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
/// The listener a numeric tcp_accept addresses: an Arc clone, so the
/// wait never holds the registry lock.
pub fn listener_for_accept(id: u64) -> MixResult<Arc<tcp_server::Listener>> {
    if let Some(l) = tcp_server::LISTENERS.lock().unwrap().get(&id).cloned() {
        return Ok(l);
    }
    if is_subscribed(ClientKey::listener(id)) {
        return Err(refusal(
            "SOCKET_SUBSCRIBED",
            format!(
                "tcp_accept: listener {id} is a tcp_on subscription — connections arrive as events; accept from the source id instead"
            ),
        ));
    }
    let connection = api::has_tcp(id) || is_subscribed(ClientKey::tcp(id));
    Err(refusal(
        "TCP_ACCEPT_HANDLE",
        if connection {
            format!("tcp_accept: handle {id} is a connection, not a listener")
        } else {
            format!("tcp_accept: unknown tcp listener {id}")
        },
    ))
}

/// Validate a numeric tcp_accept call: its listener and the absolute
/// deadline. The handle limit is enforced per accept attempt.
pub fn prepare_accept(
    args: &[Value],
) -> MixResult<(Arc<tcp_server::Listener>, Option<std::time::Instant>)> {
    let id = client_id_of(args.first(), "tcp_accept")?;
    let timeout = accept_timeout(args.get(1))?;
    Ok((listener_for_accept(id)?, deadline_after(timeout)))
}

/// Class C numeric accept: non-blocking accepts with readiness awaited
/// on an AsyncFd dup of the listener under the call's deadline, so the
/// read permit is released while no connection is pending. Several
/// waiters on one listener are safe: the loser of a race sees
/// WouldBlock and waits again. The connection is owned by `owner`
/// before this returns, so a task cancelled after the accept (while
/// the evaluator reacquires its permit) cannot leak it past
/// retirement.
///
/// The evaluator runs every numeric tcp_accept here on unix, not only
/// Class C ones: waiting on the runtime lets the shell's SIGTERM and
/// Ctrl-C select arms cancel the wait, as they cancel sleep().
/// `interruptible` (plain and Class S evaluation) also rechecks the
/// interrupt flag every ACCEPT_SLICE for embedders with no such select.
#[cfg(unix)]
pub async fn pull_accept(
    listener: Arc<tcp_server::Listener>,
    deadline: Option<std::time::Instant>,
    owner: Arc<TcpOwner>,
    interruptible: bool,
) -> MixResult<Value> {
    use tokio::io::Interest;
    use tokio::io::unix::AsyncFd;
    let dup = listener
        .sock
        .try_clone()
        .map_err(|e| refusal("SOCKET_PULL", e.to_string()))?;
    let ready = AsyncFd::with_interest(dup, Interest::READABLE)
        .map_err(|e| refusal("SOCKET_PULL", e.to_string()))?;
    let mut entered = false;
    loop {
        if let Some((id, _)) =
            super::try_accept(&listener.sock, Some(&owner)).map_err(super::AcceptError::into_mix)?
        {
            return Ok(Value::Number(id as f64));
        }
        if !entered {
            entered = true;
            super::ACCEPT_WAITS.fetch_add(1, Ordering::AcqRel);
        }
        match wait_ready(&ready, deadline, interruptible).await {
            Wait::Ready => {}
            Wait::Done => return Ok(Value::Nil),
            Wait::Failed(e) => {
                return Err(refusal(
                    "TCP_ACCEPT_FAILED",
                    format!("tcp_accept: readiness failed: {e}"),
                ));
            }
        }
    }
}

#[cfg(not(unix))]
pub async fn pull_accept(
    _listener: Arc<tcp_server::Listener>,
    _deadline: Option<std::time::Instant>,
    _owner: Arc<TcpOwner>,
    _interruptible: bool,
) -> MixResult<Value> {
    Err(refusal(
        "SOCKET_UNSUPPORTED",
        "tcp_accept inside a Class C body requires a unix platform; the plain blocking builtin keeps Class S semantics",
    ))
}

/// One accepted connection as an ordered subscription record. The
/// connection is already a live handle, so a record that is never
/// delivered must be discarded with `discard_record`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn accepted_record(handle: u64, peer: std::net::SocketAddr) -> SocketRecord {
    SocketRecord {
        kind: "accepted",
        data: serde_json::to_vec(&serde_json::json!({
            "handle": handle,
            "peer": {"host": peer.ip().to_string(), "port": peer.port()},
        }))
        .unwrap_or_default(),
    }
}

/// `tcp_on` on a listener: move it out of the registry into a reader
/// owned by the evaluator generation. Refused while a numeric accept
/// is waiting on it (the reader must be the single acceptor).
pub fn subscribe_listener(
    queue: Arc<Queue>,
    id: String,
    listener_id: u64,
    owned: Arc<TcpOwner>,
) -> MixResult<SocketSource> {
    let listener = {
        let mut map = tcp_server::LISTENERS.lock().unwrap();
        let Some(listener) = map.remove(&listener_id) else {
            return Err(refusal(
                "TCP_ON_HANDLE",
                format!("unknown tcp listener {listener_id}"),
            ));
        };
        if Arc::strong_count(&listener) > 1 {
            map.insert(listener_id, listener);
            return Err(refusal(
                "SOCKET_BUSY",
                "a tcp_accept is already waiting on this listener",
            ));
        }
        listener
    };
    let keep = listener.clone();
    spawn_listener(queue, id, listener_id, listener, owned).inspect_err(|_| {
        tcp_server::LISTENERS
            .lock()
            .unwrap()
            .insert(listener_id, keep.clone());
    })
}

/// A subscribed tcp_listen listener. The reader parks in poll(2)
/// on (listener, cancel) with no timer, and turns each accepted
/// connection into one ordered `accepted` record. Cancellation,
/// overflow and the single terminal record follow the connection
/// reader's rules; a listener has no sends.
#[cfg(target_os = "linux")]
pub fn spawn_listener(
    queue: Arc<Queue>,
    id: String,
    listener_id: u64,
    listener: Arc<tcp_server::Listener>,
    owned: Arc<TcpOwner>,
) -> MixResult<SocketSource> {
    let key = ClientKey::listener(listener_id);
    let (cancel, cancel_wake) =
        UnixStream::pair().map_err(|e| refusal("SOCKET_PAIR", e.to_string()))?;
    mark_listening(listener_id, listener.sock.local_addr().ok());
    let completed = Arc::new(AtomicBool::new(false));
    let worker_completed = completed.clone();
    let worker = std::thread::Builder::new()
                .name("mix-socket-accept".into())
                .spawn(move || {
                    let mut fds = [
                        libc::pollfd {
                            fd: listener.sock.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        },
                        libc::pollfd {
                            fd: cancel_wake.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        },
                    ];
                    let mut pending = false;
                    let closed = loop {
                        let timeout = if pending { 0 } else { -1 };
                        // SAFETY: two valid pollfds that outlive the call.
                        let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
                        if rc < 0 {
                            let e = std::io::Error::last_os_error();
                            if e.kind() == std::io::ErrorKind::Interrupted {
                                continue;
                            }
                            break Some(serde_json::json!({ "reason": format!("poll failed: {e}") }));
                        }
                        // Explicit close: no terminal marker.
                        if fds[1].revents != 0 {
                            break None;
                        }
                        if pending || fds[0].revents != 0 {
                            match accept_batch(&queue, &id, &listener.sock, &owned) {
                                DrainOutcome::More => pending = false,
                                DrainOutcome::Yield => pending = true,
                                DrainOutcome::Overflow => {
                                    break Some(serde_json::json!({
                                        "reason": "overflow",
                                        "message": "the socket frame queue exceeded its byte or frame bound; the listener was closed",
                                    }));
                                }
                                DrainOutcome::Terminal(reason) if reason == "handle_limit" => {
                                    break Some(serde_json::json!({
                                        "reason": "handle_limit",
                                        "message": format!(
                                            "{} live tcp handles is the limit; the listener was closed",
                                            tcp_server::MAX_TCP_HANDLES
                                        ),
                                    }));
                                }
                                DrainOutcome::Terminal(reason) => {
                                    break Some(serde_json::json!({ "reason": reason }));
                                }
                            }
                        }
                    };
                    drop(listener);
                    if let Some(closed) = closed {
                        // Unmark before publishing: a script that sees the
                        // terminal may at once watch the same key again.
                        unmark_subscribed(key);
                        queue.socket_closed(&id, closed);
                    }
                    worker_completed.store(true, Ordering::Release);
                });
    match worker {
        Ok(worker) => Ok(SocketSource::from_extension(key, cancel, worker, completed)),
        Err(e) => {
            unmark_subscribed(key);
            Err(refusal("SOCKET_THREAD", e.to_string()))
        }
    }
}

/// Accept what poll exposed, in bounded batches. Each connection
/// takes a permit (reserved before accept(2), so the limit holds)
/// and becomes a live handle owned by the generation before its
/// record is queued; a refused record closes it again (never a
/// handle nobody can name).
pub fn accept_batch(
    queue: &Queue,
    id: &str,
    listener: &std::net::TcpListener,
    owned: &Arc<TcpOwner>,
) -> DrainOutcome {
    for _ in 0..16 {
        match crate::try_accept(listener, Some(owned)) {
            Ok(Some((handle, peer))) => {
                if !queue.socket_push(id, accepted_record(handle, peer)) {
                    let conn = api::remove_tcp(handle);
                    drop(conn);
                    return DrainOutcome::Overflow;
                }
            }
            Ok(None) => return DrainOutcome::More,
            // No permit: the connection stays queued in the
            // backlog and is refused cleanly when the listener
            // closes.
            Err(crate::AcceptError::Limit) => {
                return DrainOutcome::Terminal("handle_limit".into());
            }
            Err(crate::AcceptError::Io(e)) => {
                return DrainOutcome::Terminal(e.to_string());
            }
        }
    }
    DrainOutcome::Yield
}

#[cfg(not(target_os = "linux"))]
pub fn spawn_listener(
    _queue: Arc<Queue>,
    _id: String,
    _listener_id: u64,
    _listener: Arc<tcp_server::Listener>,
    _owned: Arc<TcpOwner>,
) -> MixResult<SocketSource> {
    Err(refusal(
        "SOCKET_UNSUPPORTED",
        "tcp_on requires Linux poll(2)",
    ))
}
