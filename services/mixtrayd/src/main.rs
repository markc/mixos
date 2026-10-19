// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! mixtrayd: the MixOS tray's data service. It registers on the noded Bus as
//! `mixtrayd` and answers `mixtrayd.snapshot` with the four popup tabs (COS,
//! BUS, MIX, SSH). `mixtrayd snapshot` is the one-shot client the popup runs
//! when it opens, so nothing polls.

mod snapshot;
mod units;

use bus::client_helpers::{connect_anonymous_default, connect_default};
use bus::native_client::IncomingCommand;
use bus::{RC_ERROR, RC_SUCCESS};

const SERVICE: &str = "mixtrayd";
const VERB_SNAPSHOT: &str = "mixtrayd.snapshot";
const VERB_PING: &str = "mixtrayd.ping";

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve().await,
        Some("snapshot") => snapshot_client().await,
        Some("--version") => {
            println!("mixtrayd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown argument {other:?}; usage: mixtrayd [serve|snapshot]"),
    }
}

/// Registers on the Bus and answers commands until the connection closes.
async fn serve() -> anyhow::Result<()> {
    let client = connect_default(SERVICE).await?;
    let mut incoming = client
        .incoming_async()
        .await
        .ok_or_else(|| anyhow::anyhow!("the Bus client has no command stream"))?;
    while let Some(cmd) = incoming.recv().await {
        // Topic deliveries carry no command; only verbs get a reply.
        if cmd.command.is_empty() {
            continue;
        }
        handle(&client, &cmd).await;
    }
    Ok(())
}

async fn handle(client: &bus::native_client::NodedClient, cmd: &IncomingCommand) {
    let (rc, body) = match cmd.command.as_str() {
        VERB_SNAPSHOT => (RC_SUCCESS, snapshot::build(client).await.to_string()),
        VERB_PING => (RC_SUCCESS, serde_json::json!({ "service": SERVICE }).to_string()),
        other => (RC_ERROR, serde_json::json!({ "error": format!("unknown verb {other}") }).to_string()),
    };
    if let Err(error) = client.respond(cmd, rc, &body).await {
        eprintln!("mixtrayd: reply to {} failed: {error}", cmd.command);
    }
}

/// `mixtrayd snapshot`: asks the running daemon for a snapshot, prints it.
async fn snapshot_client() -> anyhow::Result<()> {
    let client = connect_anonymous_default().await?;
    let value = client
        .call(SERVICE, VERB_SNAPSHOT, serde_json::Value::Null)
        .await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
