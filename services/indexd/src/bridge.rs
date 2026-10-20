// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! One connection, one local Bus call and flushed response per NDJSON request.
use ::bus::native_client::NodedClient;
use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

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
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line).await? == 0 {
            break;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
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
                Err(error) => json!({"id":request.id,"rc":-1,"error":error.to_string()}),
            },
            Err(error) => {
                let id = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").cloned())
                    .unwrap_or(Value::Null);
                json!({"id":id,"rc":-1,"error":format!("invalid bridge request: {error}")})
            }
        };
        output.write_all(reply.to_string().as_bytes()).await?;
        output.write_all(b"\n").await?;
        output.flush().await?;
    }
    Ok(())
}
