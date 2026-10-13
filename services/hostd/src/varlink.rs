// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A minimal Varlink client: one call per connection, bounded in size and
//! time. networkd and resolved speak it on Unix sockets.
use crate::frame;
use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::{io::BufReader, net::UnixStream};

/// networkd's `Describe` carries every link's addresses and routes; a host
/// with large routing tables can pass 4 MiB. These sockets are root's own
/// services, so the cap is generous.
const REPLY_LIMIT: usize = 32 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(3);

pub async fn call(socket: &Path, method: &str, parameters: Value) -> anyhow::Result<Value> {
    tokio::time::timeout(TIMEOUT, call_inner(socket, method, parameters))
        .await
        .with_context(|| format!("{method} timed out"))?
}

async fn call_inner(socket: &Path, method: &str, parameters: Value) -> anyhow::Result<Value> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    let (read, mut write) = stream.into_split();
    frame::write(
        &mut write,
        &json!({"method": method, "parameters": parameters}),
    )
    .await?;
    let reply = frame::read(&mut BufReader::new(read), REPLY_LIMIT)
        .await?
        .context("no reply")?;
    if let Some(error) = reply.get("error") {
        bail!("{method}: {error}");
    }
    Ok(reply.get("parameters").cloned().unwrap_or(Value::Null))
}
