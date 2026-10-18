// SPDX-License-Identifier: MIT OR Apache-2.0
//! Typed calls to a local Bus port over a Unix socket.
use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::{bus, PortReply, RC_ERROR};

/// Call a local port, preserving application return codes separately from transport errors.
pub async fn call_port_typed(
    socket_path: &str,
    command: &str,
    args: serde_json::Value,
) -> Result<PortReply> {
    let mut stream = tokio::net::UnixStream::connect(socket_path).await?;

    // Build Bus request
    let mut msg = bus::BusMessage::new();
    msg.set("command", command);
    if !args.is_null() {
        msg.body = serde_json::to_string(&args)?;
    }

    // Write request and signal end
    stream.write_all(&msg.to_bytes()).await?;
    stream.shutdown().await?;

    // Read Bus response (byte-capped to bound memory on a misbehaving
    // or hostile port; see `bus::MAX_MESSAGE_BYTES`).
    let mut buf = Vec::new();
    let mut limited = stream.take(bus::MAX_MESSAGE_BYTES as u64 + 1);
    limited.read_to_end(&mut buf).await?;
    if buf.len() > bus::MAX_MESSAGE_BYTES {
        anyhow::bail!("Bus response exceeds {} byte limit", bus::MAX_MESSAGE_BYTES);
    }
    let raw = String::from_utf8(buf)?;
    let resp = bus::parse(&raw)?;

    let rc: u8 = resp.get("rc").and_then(|s| s.parse().ok()).unwrap_or(0);
    // `rc < RC_ERROR` (< 10) is the SUCCESS band — 0, the warning 5, and any
    // other sub-error status — matching NodedClient::call_typed (which treats
    // only `rc >= 10` as an application error). The two typed paths MUST agree
    // on the mid-band (1-9), or the same reply would classify differently over
    // a local socket vs the broker.
    if rc < RC_ERROR {
        let value = if resp.body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&resp.body)?
        };
        Ok(PortReply::Ok { rc, value })
    } else {
        Ok(PortReply::AppError {
            rc,
            message: resp.error_message(),
        })
    }
}

#[cfg(test)]
mod call_port_typed_tests {
    use super::*;
    use crate::bus::BusMessage;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    /// Spawn a one-shot Bus port that replies with the given `rc`/`body`,
    /// returning its socket path. The listener accepts a single connection,
    /// drains the request to EOF (the client shutdown()s its write half),
    /// writes the canned reply, and closes.
    async fn one_shot_port(
        rc: &str,
        body: &str,
    ) -> (std::path::PathBuf, tokio::task::JoinHandle<()>) {
        let dir = std::env::temp_dir();
        // Unique-ish name without Math.random: pid + a monotonic-ish nanos.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = dir.join(format!("mixos-cpt-{}-{}.sock", std::process::id(), nanos));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let (rc, body) = (rc.to_string(), body.to_string());
        let handle = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = Vec::new();
                let mut limited = (&mut stream).take(64 * 1024);
                let _ = limited.read_to_end(&mut buf).await;
                let mut reply = BusMessage::new();
                reply.set("rc", &rc);
                reply.body = body;
                let _ = stream.write_all(&reply.to_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (path, handle)
    }

    #[tokio::test]
    async fn typed_success_is_ok_value() {
        let (path, h) = one_shot_port("0", r#"{"pong":true}"#).await;
        let out = call_port_typed(path.to_str().unwrap(), "ping", serde_json::Value::Null)
            .await
            .expect("transport ok");
        match out {
            PortReply::Ok { value, .. } => assert_eq!(value, serde_json::json!({"pong": true})),
            other => panic!("expected Ok, got {other:?}"),
        }
        h.await.unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn typed_app_error_preserves_rc_not_err() {
        // A peer rc>=10 reply is an APPLICATION error → Ok(AppError), NOT the
        // transport Err (the whole point of the typed variant).
        let (path, h) = one_shot_port("10", r#"{"error":"boom"}"#).await;
        let out = call_port_typed(path.to_str().unwrap(), "do", serde_json::Value::Null)
            .await
            .expect("app error must NOT be a transport Err");
        match out {
            PortReply::AppError { rc, message } => {
                assert_eq!(rc, 10);
                assert!(message.contains("boom"), "message: {message}");
            }
            other => panic!("expected AppError, got {other:?}"),
        }
        h.await.unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn typed_connect_refused_is_transport_err() {
        // No socket at this path → a pure transport failure → Err.
        let path = std::env::temp_dir().join("mixos-cpt-nonexistent.sock");
        let _ = std::fs::remove_file(&path);
        let r = call_port_typed(path.to_str().unwrap(), "x", serde_json::Value::Null).await;
        assert!(r.is_err(), "a missing socket must be a transport Err");
    }

}
