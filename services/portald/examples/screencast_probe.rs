// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Drives one ScreenCast session against the running session portal, the way
//! a client does: CreateSession, SelectSources, Start, OpenPipeWireRemote.
//! Prints the stream it was given, holds the session for the given number of
//! seconds (default 20) so a consumer can attach to the node, then closes it.
//!
//!     cargo run -p portald --example screencast_probe -- 20
//!
//! Needs DBUS_SESSION_BUS_ADDRESS for a session whose portald serves
//! ScreenCast.
use std::collections::HashMap;
use std::time::Duration;
use futures_util::StreamExt;
use zbus::{Connection, MessageStream};
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};
use zbus::{MatchRule, message};

const BUS: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const CAST: &str = "org.freedesktop.portal.ScreenCast";

fn segment(connection: &Connection) -> String {
    connection
        .unique_name()
        .expect("unique name")
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_")
}

/// Calls `method` with a `handle_token` and waits for its Response.
async fn request<F>(
    connection: &Connection,
    token: &str,
    method: &str,
    body: impl FnOnce(HashMap<&'static str, Value<'static>>) -> F,
) -> anyhow::Result<HashMap<String, OwnedValue>>
where
    F: std::future::Future<Output = zbus::Result<zbus::Message>>,
{
    let path = format!("/org/freedesktop/portal/desktop/request/{}/{token}", segment(connection));
    let rule = MatchRule::builder()
        .msg_type(message::Type::Signal)
        .interface("org.freedesktop.portal.Request")?
        .member("Response")?
        .path(path)?
        .build();
    let mut responses = MessageStream::for_match_rule(rule, connection, None).await?;
    let mut options = HashMap::new();
    options.insert("handle_token", Value::from(token.to_owned()));
    body(options)
        .await
        .map_err(|error| anyhow::anyhow!("{method}: {error}"))?;
    let message = responses
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("{method}: no Response"))??;
    let (code, results): (u32, HashMap<String, OwnedValue>) = message.body().deserialize()?;
    anyhow::ensure!(code == 0, "{method} answered {code}");
    Ok(results)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let hold: u64 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(20);
    let connection = Connection::session().await?;

    let created = request(&connection, "probe_create", "CreateSession", |mut options| {
        options.insert("session_handle_token", Value::from("probe"));
        let connection = connection.clone();
        async move { connection.call_method(Some(BUS), PATH, Some(CAST), "CreateSession", &(options,)).await }
    })
    .await?;
    let session = OwnedObjectPath::try_from(
        created["session_handle"]
            .downcast_ref::<&str>()
            .map_err(|_| anyhow::anyhow!("session_handle is not a string"))?
            .to_owned(),
    )?;
    println!("session {session}");

    request(&connection, "probe_select", "SelectSources", |mut options| {
        options.insert("types", Value::from(1u32));
        options.insert("cursor_mode", Value::from(2u32));
        let connection = connection.clone();
        let session = session.clone();
        async move { connection.call_method(Some(BUS), PATH, Some(CAST), "SelectSources", &(&session, options)).await }
    })
    .await?;

    let started = request(&connection, "probe_start", "Start", |options| {
        let connection = connection.clone();
        let session = session.clone();
        async move { connection.call_method(Some(BUS), PATH, Some(CAST), "Start", &(&session, "", options)).await }
    })
    .await?;
    let streams: Vec<(u32, HashMap<String, OwnedValue>)> = started["streams"].try_clone()?.try_into()?;
    for (node, properties) in &streams {
        let mut keys: Vec<_> = properties.iter().collect();
        keys.sort_by(|a, b| a.0.cmp(b.0));
        println!("stream node {node}");
        for (key, value) in keys {
            println!("  {key} = {value:?}");
        }
    }

    let remote: OwnedFd = connection
        .call_method(
            Some(BUS),
            PATH,
            Some(CAST),
            "OpenPipeWireRemote",
            &(&session, HashMap::<&str, Value>::new()),
        )
        .await?
        .body()
        .deserialize()?;
    println!("pipewire remote fd {remote:?}");

    println!("holding the session for {hold}s");
    tokio::time::sleep(Duration::from_secs(hold)).await;
    connection
        .call_method(Some(BUS), session.as_str(), Some("org.freedesktop.portal.Session"), "Close", &())
        .await?;
    println!("closed");
    Ok(())
}
