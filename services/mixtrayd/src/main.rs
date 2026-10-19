// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! mixtrayd: the MixOS tray's data service. It registers on the noded Bus as
//! `mixtrayd` and answers `mixtrayd.snapshot` with the four popup tabs (COS,
//! BUS, MIX, SSH), `mixtrayd.wait` with the next snapshot once a COS unit
//! changes, and `mixtrayd.control` with start, stop or restart for the COS
//! units hostd grants it. `mixtrayd snapshot`, `mixtrayd wait` and
//! `mixtrayd control` are the one-shot clients the popup runs, so nothing
//! polls.

mod control;
mod snapshot;
mod units;
mod watch;

use std::sync::Arc;
use std::time::Duration;

use bus::client_helpers::{connect_anonymous_default, connect_default};
use bus::native_client::{IncomingCommand, NodedClient};
use bus::{RC_ERROR, RC_SUCCESS};
use serde_json::{Value, json};
use tokio::sync::watch as revision;

const SERVICE: &str = "mixtrayd";
const VERB_SNAPSHOT: &str = "mixtrayd.snapshot";
const VERB_WAIT: &str = "mixtrayd.wait";
const VERB_CONTROL: &str = "mixtrayd.control";
const VERB_PING: &str = "mixtrayd.ping";
/// The longest one `mixtrayd.wait` blocks. The Bus client gives up on a call
/// after 60 s, so a wait answers well inside that and the popup asks again.
const WAIT: Duration = Duration::from_secs(45);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        None | Some("serve") => serve().await,
        Some("snapshot") => snapshot_client().await,
        Some("wait") => match args.get(2).map(|r| r.parse::<u64>()) {
            Some(Ok(rev)) => wait_client(rev).await,
            _ => anyhow::bail!("usage: mixtrayd wait REVISION"),
        },
        Some("control") => match (args.get(2), args.get(3)) {
            (Some(unit), Some(action)) => control_client(unit, action).await,
            _ => anyhow::bail!("usage: mixtrayd control UNIT start|stop|restart"),
        },
        Some("--version") => {
            println!("mixtrayd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => anyhow::bail!(
            "unknown argument {other:?}; usage: mixtrayd [serve|snapshot|wait REV|control UNIT ACTION]"
        ),
    }
}

/// Registers on the Bus, starts the unit watchers, and answers each command
/// in its own task, so a long `mixtrayd.wait` never holds up another verb.
async fn serve() -> anyhow::Result<()> {
    let (rev_tx, rev_rx) = revision::channel(0u64);
    watch::spawn(rev_tx);
    let client = Arc::new(connect_default(SERVICE).await?);
    let mut incoming = client
        .incoming_async()
        .await
        .ok_or_else(|| anyhow::anyhow!("the Bus client has no command stream"))?;
    while let Some(cmd) = incoming.recv().await {
        // Topic deliveries carry no command; only verbs get a reply.
        if cmd.command.is_empty() {
            continue;
        }
        let (client, rev) = (Arc::clone(&client), rev_rx.clone());
        tokio::spawn(async move { handle(&client, rev, &cmd).await });
    }
    Ok(())
}

async fn handle(client: &NodedClient, rev: revision::Receiver<u64>, cmd: &IncomingCommand) {
    let (rc, body) = match cmd.command.as_str() {
        VERB_SNAPSHOT => {
            let current = *rev.borrow();
            (RC_SUCCESS, snapshot_at(client, current).await.to_string())
        }
        VERB_WAIT => (RC_SUCCESS, wait_reply(client, rev, &cmd.args).await.to_string()),
        VERB_CONTROL => control_reply(&cmd.args).await,
        VERB_PING => (RC_SUCCESS, json!({ "service": SERVICE }).to_string()),
        other => (RC_ERROR, json!({ "error": format!("unknown verb {other}") }).to_string()),
    };
    if let Err(error) = client.respond(cmd, rc, &body).await {
        eprintln!("mixtrayd: reply to {} failed: {error}", cmd.command);
    }
}

/// A snapshot labelled with the revision it was read at. The revision is
/// taken before the read, so a change during the read shows up as a newer
/// revision on the caller's next wait.
async fn snapshot_at(client: &NodedClient, rev: u64) -> Value {
    let mut snap = snapshot::build(client).await;
    snap["rev"] = json!(rev);
    snap
}

/// Waits until the revision moves past the caller's, or `WAIT` passes, then
/// answers with a fresh snapshot either way.
async fn wait_reply(client: &NodedClient, mut rev: revision::Receiver<u64>, args: &Value) -> Value {
    let since = args.get("since").and_then(Value::as_u64);
    let current = *rev.borrow_and_update();
    if watch::must_wait(since, current) {
        let _ = tokio::time::timeout(WAIT, rev.changed()).await;
    }
    let current = *rev.borrow();
    snapshot_at(client, current).await
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
    let value = client.call(SERVICE, VERB_SNAPSHOT, Value::Null).await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// `mixtrayd wait REV`: blocks until a COS unit changes after revision REV
/// (or the daemon's wait limit passes), then prints the new snapshot.
async fn wait_client(rev: u64) -> anyhow::Result<()> {
    let client = connect_anonymous_default().await?;
    let value = client.call(SERVICE, VERB_WAIT, json!({ "since": rev })).await?;
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
