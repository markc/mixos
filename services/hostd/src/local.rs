// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The local socket. Each frame is `{"command": ..., "body": {...}}` and each
//! reply `{"rc": 0|10, "body": {...}}`, NUL-terminated, many per connection.
use crate::{
    caller::{Origin, Peer},
    frame,
    service::{self, Host},
};
use anyhow::Context;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    os::{fd::FromRawFd, unix::fs::PermissionsExt},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::BufReader,
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    sync::Semaphore,
};

/// The default socket; the unit's `hostd.socket` listens here.
pub const SOCKET: &str = "/run/hostd/hostd.sock";
/// Requests are small; anything larger is refused before parsing.
const REQUEST_LIMIT: usize = 64 * 1024;
/// A connection idle this long between requests is closed.
const IDLE: Duration = Duration::from_secs(30);
/// A reply the caller doesn't read within this closes the connection.
const WRITE: Duration = Duration::from_secs(5);
/// Callers served at once; more wait in the kernel backlog.
const CONNECTIONS: usize = 32;
/// Connections one uid may hold at once, so one caller can't take every
/// slot and lock others (and the kill switch) out.
const PER_UID: usize = 8;
/// No connection lives longer than this, however busy.
const LIFETIME: Duration = Duration::from_secs(600);

/// The listener systemd passed (`LISTEN_PID`/`LISTEN_FDS`, fd 3), else a
/// socket bound at `path` for a run without socket activation.
pub fn listener(path: &Path) -> anyhow::Result<UnixListener> {
    let activated = std::env::var("LISTEN_PID").ok() == Some(std::process::id().to_string())
        && std::env::var("LISTEN_FDS").ok().as_deref() == Some("1");
    if activated {
        // SAFETY: systemd hands over exactly one listening socket at fd 3,
        // checked above; nothing else in this process owns that fd.
        let std = unsafe { std::os::unix::net::UnixListener::from_raw_fd(3) };
        std.set_nonblocking(true)?;
        return Ok(UnixListener::from_std(std)?);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::symlink_metadata(path).is_ok() {
        // Never take over a socket something is serving: a debug run must
        // not steal the system hostd's callers.
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            anyhow::bail!("{} is in use; another hostd is serving it", path.display());
        }
        std::fs::remove_file(path).with_context(|| format!("remove stale {}", path.display()))?;
    }
    let listener = UnixListener::bind(path).with_context(|| format!("bind {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
    Ok(listener)
}

pub async fn serve(listener: UnixListener, host: Host) -> anyhow::Result<()> {
    let slots = Arc::new(Semaphore::new(CONNECTIONS));
    let per_uid: Arc<Mutex<HashMap<u32, usize>>> = Arc::default();
    loop {
        let permit = slots.clone().acquire_owned().await?;
        let (stream, _) = listener.accept().await?;
        let uid = stream.peer_cred().map(|c| c.uid()).unwrap_or(u32::MAX);
        {
            let mut held = per_uid.lock().unwrap_or_else(|p| p.into_inner());
            let count = held.entry(uid).or_default();
            if *count >= PER_UID {
                // Dropping the stream closes it; the slot frees at once.
                tracing::warn!(uid, "connection refused: per-uid limit");
                continue;
            }
            *count += 1;
        }
        let (host, per_uid) = (host.clone(), per_uid.clone());
        tokio::spawn(async move {
            match tokio::time::timeout(LIFETIME, connection(stream, &host)).await {
                Ok(Err(error)) => tracing::debug!(%error, "local connection closed"),
                Err(_) => tracing::debug!(uid, "local connection reached its lifetime"),
                Ok(Ok(())) => {}
            }
            let mut held = per_uid.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(count) = held.get_mut(&uid) {
                *count -= 1;
                if *count == 0 {
                    held.remove(&uid);
                }
            }
            drop(permit);
        });
    }
}

async fn connection(stream: UnixStream, host: &Host) -> anyhow::Result<()> {
    let peer = Arc::new(Peer::of(&stream)?);
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    loop {
        let request = match tokio::time::timeout(IDLE, frame::read(&mut read, REQUEST_LIMIT)).await
        {
            Err(_) | Ok(Ok(None)) => return Ok(()),
            Ok(Err(error)) => {
                let body = json!({"status":"bad_request","message":error.to_string()});
                return send(&mut write, 10, body).await;
            }
            Ok(Ok(Some(request))) => request,
        };
        let reply = match request.get("command").and_then(Value::as_str) {
            Some(command) => {
                let body = request.get("body").cloned().unwrap_or_else(|| json!({}));
                // Identity is read for this request, not once per connection.
                let origin = Origin::Local(peer.caller());
                service::dispatch_from(host, command, &body, &origin, Some(peer.clone())).await
            }
            None => Err(json!({"status":"bad_request","message":"missing command"})),
        };
        let (rc, body) = match reply {
            Ok(body) => (0, body),
            Err(body) => (10, body),
        };
        send(&mut write, rc, body).await?;
    }
}

/// Writes one reply within `WRITE`. A caller that stops reading loses its
/// connection, and its slot, instead of holding it forever.
async fn send(write: &mut OwnedWriteHalf, rc: u8, body: Value) -> anyhow::Result<()> {
    tokio::time::timeout(WRITE, frame::write(write, &json!({"rc": rc, "body": body})))
        .await
        .context("caller stopped reading")?
}

/// One request from a client, for `hostd call` and tests.
pub async fn call(path: &Path, command: &str, body: Value) -> anyhow::Result<(u8, Value)> {
    let stream = UnixStream::connect(path)
        .await
        .with_context(|| format!("connect {}", path.display()))?;
    let (read, mut write) = stream.into_split();
    frame::write(&mut write, &json!({"command": command, "body": body})).await?;
    let reply = frame::read(&mut BufReader::new(read), 8 * 1024 * 1024)
        .await?
        .context("hostd closed without a reply")?;
    let rc = reply.get("rc").and_then(Value::as_u64).unwrap_or(10) as u8;
    Ok((rc, reply.get("body").cloned().unwrap_or(Value::Null)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_round_trip_and_refusals() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("run/hostd.sock");
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("proc/sys/kernel")).unwrap();
        std::fs::write(root.join("proc/sys/kernel/hostname"), "example\n").unwrap();
        let listener = listener(&socket).unwrap();
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o660);
        tokio::spawn(serve(listener, Host::fixture(&root)));

        let (rc, body) = call(&socket, "host.info", json!({})).await.unwrap();
        assert_eq!((rc, &body["hostname"]), (0, &json!("example")));
        let (rc, body) = call(&socket, "net.exec", json!({"cmd":"id"}))
            .await
            .unwrap();
        assert_eq!((rc, &body["status"]), (10, &json!("UNKNOWN_COMMAND")));
    }

    /// Callers that pipeline requests and never read the replies take
    /// every slot; the write deadline frees them for a legitimate caller.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stalled_readers_release_their_slots() {
        use tokio::io::AsyncWriteExt;
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("hostd.sock");
        tokio::spawn(serve(listener(&socket).unwrap(), Host::fixture(dir.path())));

        let mut frames = Vec::new();
        for _ in 0..2000 {
            frames.extend_from_slice(b"{\"command\":\"HELP\"}\0");
        }
        let mut stalled = Vec::new();
        for _ in 0..CONNECTIONS {
            let mut stream = UnixStream::connect(&socket).await.unwrap();
            // The server may stop reading once its replies back up, so a
            // short write here is expected; the stream is kept, never read.
            let _ =
                tokio::time::timeout(Duration::from_millis(200), stream.write_all(&frames)).await;
            stalled.push(stream);
        }
        // The stalled callers share this test's uid, so the per-uid limit
        // also refuses the next one until the write deadline frees a slot.
        let reply = tokio::time::timeout(WRITE + Duration::from_secs(10), async {
            loop {
                match call(&socket, "HELP", json!({})).await {
                    Ok(reply) => break reply,
                    Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
                }
            }
        })
        .await
        .expect("a legitimate caller was starved");
        assert_eq!(reply.0, 0);
        drop(stalled);
    }

    /// One uid can hold at most PER_UID connections; the next is closed
    /// at once instead of waiting for a slot.
    #[tokio::test]
    async fn one_uid_cannot_take_every_slot() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("hostd.sock");
        tokio::spawn(serve(listener(&socket).unwrap(), Host::fixture(dir.path())));
        let mut held = Vec::new();
        for _ in 0..PER_UID {
            held.push(UnixStream::connect(&socket).await.unwrap());
        }
        // Give the server time to account for every held connection.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(call(&socket, "HELP", json!({})).await.is_err());
        drop(held.pop());
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(call(&socket, "HELP", json!({})).await.unwrap().0, 0);
    }

    #[test]
    fn a_live_socket_is_never_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("hostd.sock");
        let _live = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(listener(&socket).is_err());
        drop(_live);
        // Now stale: replaced.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            listener(&socket).unwrap_or_else(|e| panic!("stale socket was not replaced: {e:#}"));
        });
    }
}
