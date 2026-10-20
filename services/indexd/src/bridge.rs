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
    timeout_secs: Option<u64>,
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
        if (&mut input)
            .take((crate::limits::BUS_BODY_LIMIT + 1) as u64)
            .read_until(b'\n', &mut line)
            .await?
            == 0
        {
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
        } else {
            match serde_json::from_slice::<Request>(&line) {
                Ok(request) => match match client {
                    Ok(client) => {
                        client
                            .call_with_headers_raw_within(
                                "indexd",
                                &request.verb,
                                &Default::default(),
                                &request.body.to_string(),
                                std::time::Duration::from_secs(
                                    request.timeout_secs.unwrap_or(60).min(600),
                                ),
                            )
                            .await
                    }
                    Err(error) => Err(anyhow::anyhow!(error.to_string())),
                } {
                    Ok((rc, body, error)) => bus_reply(request.id, rc, &body, error.as_deref()),
                    Err(error) => {
                        if matches!(
                            error.downcast_ref::<::bus::ClientError>(),
                            Some(::bus::ClientError::Timeout { .. })
                        ) {
                            json!({"id":request.id,"rc":-1,"kind":"timeout","error":error.to_string()})
                        } else {
                            transport_failed = true;
                            json!({"id":request.id,"rc":-1,"error":error.to_string()})
                        }
                    }
                },
                Err(error) => {
                    let id = serde_json::from_slice::<Value>(&line)
                        .ok()
                        .and_then(|v| v.get("id").cloned())
                        .unwrap_or(Value::Null);
                    json!({"id":id,"rc":-1,"error":format!("invalid bridge request: {error}")})
                }
            }
        };
        output.write_all(reply.to_string().as_bytes()).await?;
        output.write_all(b"\n").await?;
        output.flush().await?;
        anyhow::ensure!(!transport_failed, "bridge Bus connection failed");
    }
    Ok(())
}

fn bus_reply(id: Value, rc: u8, body: &str, error: Option<&str>) -> Value {
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_str::<Value>(body).unwrap_or_else(|_| json!({"error":body}))
    };
    let mut reply = json!({"id":id,"rc":rc,"body":body});
    if let Some(error) = error {
        reply["error"] = error.into();
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_bus_error_preserves_rc_and_body() {
        assert_eq!(
            bus_reply(json!(7), 42, "service unavailable", None),
            json!({"id":7,"rc":42,"body":{"error":"service unavailable"}})
        );
    }

    #[test]
    fn empty_body_is_null_and_header_error_is_forwarded() {
        assert_eq!(
            bus_reply(json!(8), 0, "", None),
            json!({"id":8,"rc":0,"body":null})
        );
        assert_eq!(
            bus_reply(json!(9), 10, "", Some("service missing")),
            json!({"id":9,"rc":10,"body":null,"error":"service missing"})
        );
        assert_eq!(
            bus_reply(
                json!(10),
                10,
                r#"{"error":"body reason"}"#,
                Some("header reason")
            )["error"],
            "header reason"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_reply_keeps_connection_and_bridge_alive() {
        let broker = test_broker::Broker::start();
        let owner = NodedClient::connect("indexd", &broker.url).await.unwrap();
        let mut incoming = owner.incoming_async().await.unwrap();
        let task = tokio::spawn(async move {
            while let Some(command) = incoming.recv().await {
                if command.command == "indexd.stats" {
                    owner.respond(&command, 0, "").await.unwrap();
                }
            }
        });
        let client = NodedClient::connect_anonymous(&broker.url).await;
        let input = b"{\"id\":1,\"verb\":\"indexd.feedback\",\"body\":{},\"timeout_secs\":0}\n{\"id\":2,\"verb\":\"indexd.stats\",\"body\":{}}\n";
        let mut output = Vec::new();
        run(&input[..], &mut output, &client).await.unwrap();
        let replies: Vec<Value> = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies[0]["kind"], "timeout");
        assert_eq!(replies[0]["rc"], -1);
        assert_eq!(replies[1], json!({"id":2,"rc":0,"body":null}));
        client.unwrap().close().await;
        task.abort();
    }
}
