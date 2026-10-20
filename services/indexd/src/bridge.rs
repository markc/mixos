// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! One connection, one local Bus call and flushed response per NDJSON request.
use ::bus::native_client::NodedClient;
use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Deserialize)]
struct Request {
    id: Value,
    verb: String,
    body: Value,
}

pub async fn run<R, W>(mut input: R, mut output: W, client: &Result<NodedClient>) -> Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut line = Vec::new();
    loop {
        let mut transport_failed = false;
        line.clear();
        if (&mut input).take((crate::limits::BUS_BODY_LIMIT + 1) as u64)
            .read_until(b'\n', &mut line).await? == 0 {
            break;
        }
        let oversized = line.len() > crate::limits::BUS_BODY_LIMIT;
        if oversized {
            // Drain just this line in bounded chunks, then resume the protocol.
            while line.last() != Some(&b'\n') {
                line.clear();
                if (&mut input).take(8192).read_until(b'\n', &mut line).await? == 0 {
                    break;
                }
            }
        }
        let reply = if oversized || std::str::from_utf8(&line).is_err() {
            json!({"id":null,"rc":-1,"error":"invalid bridge request"})
        } else { match serde_json::from_slice::<Request>(&line) {
            Ok(request) => match match client {
                Ok(client) => {
                    client
                        .call_with_headers_raw(
                            "indexd",
                            &request.verb,
                            &Default::default(),
                            &request.body.to_string(),
                        )
                        .await
                }
                Err(error) => Err(anyhow::anyhow!(error.to_string())),
            } {
                Ok((rc, body, _)) => match serde_json::from_str::<Value>(&body) {
                    Ok(body) => json!({"id":request.id,"rc":rc,"body":body}),
                    Err(error) => {
                        json!({"id":request.id,"rc":-1,"error":format!("invalid Bus reply JSON: {error}")})
                    }
                },
                Err(error) => {
                    transport_failed = true;
                    json!({"id":request.id,"rc":-1,"error":error.to_string()})
                }
            },
            Err(error) => {
                let id = serde_json::from_slice::<Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").cloned())
                    .unwrap_or(Value::Null);
                json!({"id":id,"rc":-1,"error":format!("invalid bridge request: {error}")})
            }
        }};
        output.write_all(reply.to_string().as_bytes()).await?;
        output.write_all(b"\n").await?;
        output.flush().await?;
        anyhow::ensure!(!transport_failed, "bridge Bus connection failed");
    }
    Ok(())
}
