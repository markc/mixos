// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! mixtrayd: the MixOS tray's data service. It registers on the noded Bus as
//! `mixtrayd` and answers `mixtrayd.snapshot` with the four popup tabs (COS,
//! BUS, MIX, SSH) and `mixtrayd.control` with start, stop or restart for the
//! COS units hostd grants it. `mixtrayd snapshot` and `mixtrayd control` are
//! the one-shot clients the popup runs, so nothing polls.

mod control;
mod snapshot;
mod units;

use bus::client_helpers::{connect_anonymous_default, connect_default};
use bus::native_client::IncomingCommand;
use bus::{RC_ERROR, RC_SUCCESS};
use serde_json::{Value, json};

const SERVICE: &str = "mixtrayd";
const VERB_SNAPSHOT: &str = "mixtrayd.snapshot";
const VERB_CONTROL: &str = "mixtrayd.control";
const VERB_PING: &str = "mixtrayd.ping";

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        None | Some("serve") => serve().await,
        Some("snapshot") => snapshot_client().await,
        Some("control") => match (args.get(2), args.get(3)) {
            (Some(unit), Some(action)) => control_client(unit, action).await,
            _ => anyhow::bail!("usage: mixtrayd control UNIT start|stop|restart"),
        },
        Some("--version") => {
            println!("mixtrayd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => anyhow::bail!(
            "unknown argument {other:?}; usage: mixtrayd [serve|snapshot|control UNIT ACTION]"
        ),
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
        VERB_CONTROL => control_reply(&cmd.args).await,
        VERB_PING => (RC_SUCCESS, json!({ "service": SERVICE }).to_string()),
        other => (RC_ERROR, json!({ "error": format!("unknown verb {other}") }).to_string()),
    };
    if let Err(error) = client.respond(cmd, rc, &body).await {
        eprintln!("mixtrayd: reply to {} failed: {error}", cmd.command);
    }
}

/// Checks one control request, runs it through hostd, and shapes the reply.
async fn control_reply(args: &Value) -> (u8, String) {
    let request = match control::check(args) {
        Ok(request) => request,
        Err(message) => {
            return (RC_ERROR, json!({ "status": "refused", "message": message }).to_string());
        }
    };
    match control::run(&request).await {
        Ok(hostd) => (
            RC_SUCCESS,
            json!({ "unit": request.unit, "action": request.action, "hostd": hostd }).to_string(),
        ),
        Err(error) => (
            RC_ERROR,
            json!({ "unit": request.unit, "action": request.action, "error": error }).to_string(),
        ),
    }
}

/// `mixtrayd snapshot`: asks the running daemon for a snapshot, prints it.
async fn snapshot_client() -> anyhow::Result<()> {
    let client = connect_anonymous_default().await?;
    let value = client
        .call(SERVICE, VERB_SNAPSHOT, Value::Null)
        .await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// `mixtrayd control UNIT ACTION`: asks the running daemon to act on one unit.
async fn control_client(unit: &str, action: &str) -> anyhow::Result<()> {
    let client = connect_anonymous_default().await?;
    let value = client
        .call(SERVICE, VERB_CONTROL, json!({ "unit": unit, "action": action }))
        .await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
