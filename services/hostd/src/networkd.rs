// SPDX-License-Identifier: MIT OR Apache-2.0
//! Driving systemd-networkd: reload its configuration, reconfigure a link,
//! and wait for a link to reach a state. hostd runs as root, so networkd's
//! D-Bus methods need no polkit. Waiting is event-driven: each
//! `PropertiesChanged` from networkd triggers one fresh `Describe`.
use crate::varlink;
use anyhow::{Context, bail};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

const DEST: &str = "org.freedesktop.network1";
const PATH: &str = "/org/freedesktop/network1";
const MANAGER: &str = "org.freedesktop.network1.Manager";
const STEP: Duration = Duration::from_secs(5);

/// networkd's Varlink socket under `root`.
pub fn socket(root: &Path) -> std::path::PathBuf {
    root.join("run/systemd/netif/io.systemd.Network")
}

/// The `Describe` entry for one link.
pub async fn describe(root: &Path, link: &str) -> anyhow::Result<Option<Value>> {
    let reply = varlink::call(&socket(root), "io.systemd.Network.Describe", json!({})).await?;
    Ok(reply
        .get("Interfaces")
        .and_then(Value::as_array)
        .and_then(|links| {
            links
                .iter()
                .find(|l| l.get("Name").and_then(Value::as_str) == Some(link))
        })
        .cloned())
}

pub struct Networkd {
    conn: zbus::Connection,
}

impl Networkd {
    pub async fn connect() -> anyhow::Result<Self> {
        let conn = tokio::time::timeout(STEP, zbus::Connection::system())
            .await
            .context("system bus timed out")??;
        Ok(Self { conn })
    }

    async fn call<B>(&self, method: &str, body: &B) -> anyhow::Result<()>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        tokio::time::timeout(
            STEP,
            self.conn
                .call_method(Some(DEST), PATH, Some(MANAGER), method, body),
        )
        .await
        .with_context(|| format!("{method} timed out"))?
        .with_context(|| method.to_owned())?;
        Ok(())
    }

    /// Re-read `.network` and `.netdev` files, then reconfigure one link so
    /// it picks up its (possibly new) file.
    pub async fn reload_link(&self, ifindex: i32) -> anyhow::Result<()> {
        self.call("Reload", &()).await?;
        self.call("ReconfigureLink", &(ifindex,)).await
    }

    /// Waits until `check` accepts the link's `Describe` entry, or the
    /// window closes. Returns the last entry seen and whether it passed.
    pub async fn wait_for(
        &self,
        root: &Path,
        link: &str,
        window: Duration,
        check: impl Fn(&Value) -> bool,
    ) -> anyhow::Result<(bool, Value)> {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(DEST)?
            .interface("org.freedesktop.DBus.Properties")?
            .member("PropertiesChanged")?
            .build();
        // Subscribe before the first read, so no change slips between them.
        let mut changes = zbus::MessageStream::for_match_rule(rule, &self.conn, Some(64)).await?;
        let deadline = tokio::time::Instant::now() + window;
        let mut last = Value::Null;
        loop {
            if let Some(entry) = describe(root, link).await? {
                if check(&entry) {
                    return Ok((true, entry));
                }
                last = entry;
            }
            match tokio::time::timeout_at(deadline, changes.next()).await {
                Err(_) => return Ok((false, last)),
                Ok(None) => bail!("system bus closed while waiting"),
                Ok(Some(_)) => {}
            }
        }
    }
}
