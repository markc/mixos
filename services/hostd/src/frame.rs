// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! NUL-terminated JSON frames: the Varlink wire, also used by hostd's own
//! local socket so one client primitive (a Unix socket plus NUL framing)
//! serves both.
use anyhow::{Context, bail};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// Reads one frame. `Ok(None)` is a clean end of stream between frames.
pub async fn read<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    limit: usize,
) -> anyhow::Result<Option<Value>> {
    let mut buf = Vec::new();
    let mut limited = (&mut *reader).take(limit as u64 + 1);
    let n = limited.read_until(0, &mut buf).await?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&0) {
        if buf.len() > limit {
            bail!("frame exceeds {limit} bytes");
        }
        bail!("stream ended inside a frame");
    }
    buf.pop();
    Ok(Some(
        serde_json::from_slice(&buf).context("frame is not JSON")?,
    ))
}

pub async fn write<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> anyhow::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(0);
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn round_trip_and_limits() {
        let (a, b) = tokio::io::duplex(1024);
        let (_, mut w) = tokio::io::split(a);
        let (r, _) = tokio::io::split(b);
        let mut r = BufReader::new(r);
        write(&mut w, &json!({"command":"host.info"}))
            .await
            .unwrap();
        assert_eq!(
            read(&mut r, 64).await.unwrap(),
            Some(json!({"command":"host.info"}))
        );
        write(&mut w, &json!({"pad":"x".repeat(100)}))
            .await
            .unwrap();
        assert!(read(&mut r, 64).await.is_err());
        drop(w);
    }

    #[tokio::test]
    async fn clean_end_is_none() {
        let mut r = BufReader::new(&b""[..]);
        assert_eq!(read(&mut r, 64).await.unwrap(), None);
        let mut r = BufReader::new(&b"{}"[..]);
        assert!(read(&mut r, 64).await.is_err());
    }
}
