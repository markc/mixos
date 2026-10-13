// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Wi-Fi through iwd: `net.wifi.scan` and `net.wifi.connect`.
//!
//! hostd writes no files for Wi-Fi. A passphrase reaches iwd through an
//! agent hostd registers on the system bus for one connect only. The agent
//! answers only the process that owns `net.connman.iwd`, and only for the
//! network being connected. iwd itself saves the network, and only after a
//! successful connect. The receipt records what is needed to put things
//! back (the station's previous network, whether the target was already
//! known) and never the passphrase.
//!
//! hostd never starts iwd: it asks whether the name has an owner, and a
//! call to an absent iwd would make D-Bus start it.
use crate::{caller::Origin, link::valid_link, receipt::Receipt, service::Host, txn};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use zbus::zvariant::OwnedObjectPath;

pub const SCAN: &str = "net.wifi.scan";
pub const VERB: &str = "net.wifi.connect";
const IWD: &str = "net.connman.iwd";
const STATION: &str = "net.connman.iwd.Station";
const DEVICE: &str = "net.connman.iwd.Device";
const NETWORK: &str = "net.connman.iwd.Network";
const KNOWN: &str = "net.connman.iwd.KnownNetwork";
const AGENT_PATH: &str = "/hostd/agent";
const DEFAULT_WINDOW: u64 = 25;
const MAX_WINDOW: u64 = 60;
const STEP: Duration = Duration::from_secs(5);

fn bad(message: impl Into<String>) -> Value {
    json!({"status":"bad_request","message":message.into()})
}

fn internal(e: anyhow::Error) -> Value {
    json!({"status":"internal","message":format!("{e:#}")})
}

/// An SSID: 1–32 bytes of text with no control characters.
pub fn valid_ssid(ssid: &str) -> bool {
    (1..=32).contains(&ssid.len()) && !ssid.chars().any(char::is_control)
}

/// A WPA passphrase: 8–63 printable ASCII characters, or 64 hex digits.
pub fn valid_passphrase(p: &str) -> bool {
    ((8..=63).contains(&p.len()) && p.bytes().all(|b| (0x20..=0x7e).contains(&b)))
        || (p.len() == 64 && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanRequest {
    pub device: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectRequest {
    pub device: String,
    pub ssid: String,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub window_s: Option<u64>,
}

impl ConnectRequest {
    pub fn check(&self) -> Result<Duration, Value> {
        if !valid_link(&self.device) {
            return Err(bad("device must be 1-15 characters of [A-Za-z0-9._-]"));
        }
        if !valid_ssid(&self.ssid) {
            return Err(bad("ssid must be 1-32 bytes without control characters"));
        }
        if self
            .passphrase
            .as_deref()
            .is_some_and(|p| !valid_passphrase(p))
        {
            return Err(bad(
                "passphrase must be 8-63 printable characters or 64 hex digits",
            ));
        }
        let window = self.window_s.unwrap_or(DEFAULT_WINDOW);
        if !(5..=MAX_WINDOW).contains(&window) {
            return Err(bad(format!("window_s must be 5-{MAX_WINDOW}")));
        }
        Ok(Duration::from_secs(window))
    }
}

/// A system bus connection, after checking iwd is running. Never starts it.
///
/// Every method call on the connection is bounded by `timeout` (property
/// reads, agent registration and teardown included), so a stalled iwd can
/// never hold the writer lock. A connect passes its window here, because
/// iwd answers `Connect` only when the connection is up or has failed.
async fn iwd(timeout: Duration) -> anyhow::Result<zbus::Connection> {
    let conn = tokio::time::timeout(
        STEP,
        zbus::connection::Builder::system()?
            .method_timeout(timeout)
            .build(),
    )
    .await??;
    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
    if !dbus.name_has_owner(IWD.try_into()?).await? {
        anyhow::bail!("iwd is not running");
    }
    Ok(conn)
}

async fn proxy<'a>(
    conn: &'a zbus::Connection,
    path: &'a str,
    iface: &'a str,
) -> anyhow::Result<zbus::Proxy<'a>> {
    Ok(zbus::Proxy::new(conn, IWD, path, iface).await?)
}

/// The station object for a device name.
async fn station(conn: &zbus::Connection, device: &str) -> anyhow::Result<Option<String>> {
    let objects = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination(IWD)?
        .path("/")?
        .build()
        .await?
        .get_managed_objects()
        .await?;
    for (path, ifaces) in &objects {
        let Some(dev) = ifaces.get(DEVICE) else {
            continue;
        };
        let name = dev
            .get("Name")
            .and_then(|v| <&str>::try_from(&**v as &zbus::zvariant::Value).ok());
        if name == Some(device) && ifaces.contains_key(STATION) {
            return Ok(Some(path.as_str().to_owned()));
        }
    }
    Ok(None)
}

#[derive(Debug, Clone)]
struct Network {
    path: OwnedObjectPath,
    name: String,
    security: String,
    signal: i16,
    known: bool,
}

async fn networks(conn: &zbus::Connection, station: &str) -> anyhow::Result<Vec<Network>> {
    let st = proxy(conn, station, STATION).await?;
    let ordered: Vec<(OwnedObjectPath, i16)> =
        tokio::time::timeout(STEP, st.call("GetOrderedNetworks", &())).await??;
    let mut out = vec![];
    for (path, signal) in ordered {
        let n = proxy(conn, path.as_str(), NETWORK).await?;
        let name: String = n.get_property("Name").await?;
        let security: String = n.get_property("Type").await?;
        let known = n
            .get_property::<OwnedObjectPath>("KnownNetwork")
            .await
            .is_ok();
        out.push(Network {
            path,
            name,
            security,
            signal,
            known,
        });
    }
    Ok(out)
}

/// Starts a scan and waits (bounded) for it to finish.
async fn scan_now(conn: &zbus::Connection, station: &str) -> anyhow::Result<()> {
    let st = proxy(conn, station, STATION).await?;
    // A scan already running is fine: wait for it instead.
    let _ = tokio::time::timeout(STEP, st.call::<_, _, ()>("Scan", &())).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if !st.get_property::<bool>("Scanning").await.unwrap_or(false) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    anyhow::bail!("scan did not finish in 15 s")
}

/// `net.wifi.scan`: refreshes and lists the networks a device can see.
pub async fn scan(device: &str) -> Result<Value, Value> {
    let conn = iwd(STEP)
        .await
        .map_err(|e| json!({"status":"no_iwd","message":e.to_string()}))?;
    let station = station(&conn, device)
        .await
        .map_err(internal)?
        .ok_or_else(|| json!({"status":"no_such_station","device":device}))?;
    scan_now(&conn, &station).await.map_err(internal)?;
    let list = networks(&conn, &station).await.map_err(internal)?;
    let networks: Vec<Value> = list
        .iter()
        .map(|n| {
            json!({"ssid": n.name, "security": n.security,
                        "signal_dbm": f64::from(n.signal) / 100.0, "known": n.known})
        })
        .collect();
    Ok(json!({"device": device, "networks": networks}))
}

/// The network a station is connected to now, by SSID.
async fn connected_ssid(conn: &zbus::Connection, station: &str) -> anyhow::Result<Option<String>> {
    let st = proxy(conn, station, STATION).await?;
    let state: String = st.get_property("State").await?;
    if state != "connected" {
        return Ok(None);
    }
    let Ok(path) = st.get_property::<OwnedObjectPath>("ConnectedNetwork").await else {
        return Ok(None);
    };
    let n = proxy(conn, path.as_str(), NETWORK).await?;
    Ok(Some(n.get_property("Name").await?))
}

/// The passphrase agent, alive for one connect.
struct Agent {
    /// The unique bus name of the process that owns `net.connman.iwd`.
    iwd: String,
    network: OwnedObjectPath,
    passphrase: String,
}

#[zbus::interface(name = "net.connman.iwd.Agent")]
impl Agent {
    fn release(&self) {}

    /// Answers iwd only, and only for the network being connected.
    fn request_passphrase(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        network: OwnedObjectPath,
    ) -> zbus::fdo::Result<String> {
        let from_iwd = header.sender().is_some_and(|s| s.as_str() == self.iwd);
        if !from_iwd || network != self.network {
            return Err(zbus::fdo::Error::AccessDenied(
                "not for this caller or network".into(),
            ));
        }
        Ok(self.passphrase.clone())
    }

    fn cancel(&self, _reason: String) {}
}

/// Connects `station` to `network`, offering `passphrase` through a
/// one-connect agent when the network needs it.
async fn connect_network(
    conn: &zbus::Connection,
    network: &Network,
    passphrase: Option<&str>,
    timeout: Duration,
) -> anyhow::Result<()> {
    let n = proxy(conn, network.path.as_str(), NETWORK).await?;
    let Some(passphrase) = passphrase else {
        tokio::time::timeout(timeout, n.call::<_, _, ()>("Connect", &())).await??;
        return Ok(());
    };
    let owner = zbus::fdo::DBusProxy::new(conn)
        .await?
        .get_name_owner(IWD.try_into()?)
        .await?;
    let agent = Agent {
        iwd: owner.as_str().to_owned(),
        network: network.path.clone(),
        passphrase: passphrase.to_owned(),
    };
    conn.object_server().at(AGENT_PATH, agent).await?;
    let manager = proxy(conn, "/net/connman/iwd", "net.connman.iwd.AgentManager").await?;
    let path = OwnedObjectPath::try_from(AGENT_PATH)?;
    let result = async {
        manager.call::<_, _, ()>("RegisterAgent", &(&path,)).await?;
        tokio::time::timeout(timeout, n.call::<_, _, ()>("Connect", &())).await??;
        anyhow::Ok(())
    }
    .await;
    let _ = manager.call::<_, _, ()>("UnregisterAgent", &(&path,)).await;
    let _ = conn.object_server().remove::<Agent, _>(AGENT_PATH).await;
    result
}

async fn find(
    conn: &zbus::Connection,
    station: &str,
    ssid: &str,
) -> anyhow::Result<Option<Network>> {
    let found = networks(conn, station)
        .await?
        .into_iter()
        .find(|n| n.name == ssid);
    if found.is_some() {
        return Ok(found);
    }
    scan_now(conn, station).await?;
    Ok(networks(conn, station)
        .await?
        .into_iter()
        .find(|n| n.name == ssid))
}

/// `net.wifi.connect`, under the writer lock, in its own task.
pub async fn connect(
    host: Host,
    request: ConnectRequest,
    window: Duration,
    grant: String,
    origin: Origin,
) -> Result<Value, Value> {
    let conn = iwd(window + STEP)
        .await
        .map_err(|e| json!({"status":"no_iwd","message":e.to_string()}))?;
    let station = station(&conn, &request.device)
        .await
        .map_err(internal)?
        .ok_or_else(|| json!({"status":"no_such_station","device":request.device}))?;
    let target = find(&conn, &station, &request.ssid)
        .await
        .map_err(internal)?
        .ok_or_else(|| json!({"status":"no_such_network","ssid":request.ssid}))?;
    match (
        target.security.as_str(),
        target.known,
        request.passphrase.is_some(),
    ) {
        ("open", _, true) => return Err(bad("an open network takes no passphrase")),
        ("psk", false, false) => return Err(bad("this network needs a passphrase")),
        ("psk", true, true) => {
            return Err(bad(
                "this network is known; forget it first to change its passphrase",
            ));
        }
        ("open" | "psk", _, _) => {}
        (other, _, _) => {
            return Err(json!({"status":"unsupported","security":other}));
        }
    }
    let previous = connected_ssid(&conn, &station).await.map_err(internal)?;

    let mut receipt = Receipt::new(
        VERB,
        &grant,
        &origin,
        json!({"link": request.device, "ssid": request.ssid, "security": target.security,
               "was_known": target.known, "previous": previous, "window_s": window.as_secs()}),
    );
    txn::begin(&host, &receipt)?;

    let attempt = async {
        connect_network(&conn, &target, request.passphrase.as_deref(), window).await?;
        let deadline = tokio::time::Instant::now() + window;
        while tokio::time::Instant::now() < deadline {
            if connected_ssid(&conn, &station).await?.as_deref() == Some(request.ssid.as_str()) {
                return anyhow::Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        Ok(false)
    }
    .await;
    let (passed, reason) = match attempt {
        Ok(true) => (true, None),
        Ok(false) => (false, Some("not connected in time".to_owned())),
        Err(e) => (false, Some(format!("{e:#}"))),
    };
    let suspended = txn::conclude(&host, &mut receipt, passed, reason).await?;
    let reply = json!({
        "receipt": receipt.id, "outcome": receipt.outcome, "reason": receipt.reason,
        "grant": receipt.grant, "grant_suspended": suspended,
        "device": request.device, "ssid": request.ssid,
    });
    if passed {
        Ok(reply)
    } else {
        Err(json!({"status":"rolled_back","detail": reply}))
    }
}

fn field<'a>(receipt: &'a Receipt, key: &str) -> Option<&'a str> {
    receipt.summary.get(key).and_then(Value::as_str)
}

/// Bounds for put-back and the checks around it.
const PUT_BACK: Duration = Duration::from_secs(25);

/// The network a station is attached to now, whether connected or still
/// connecting (iwd sets `ConnectedNetwork` from the start of a connect),
/// and the station's state.
async fn attached(
    conn: &zbus::Connection,
    station: &str,
) -> anyhow::Result<(String, Option<String>)> {
    let st = proxy(conn, station, STATION).await?;
    let state: String = st.get_property("State").await?;
    if state == "disconnected" {
        return Ok((state, None));
    }
    let Ok(path) = st.get_property::<OwnedObjectPath>("ConnectedNetwork").await else {
        return Ok((state, None));
    };
    let n = proxy(conn, path.as_str(), NETWORK).await?;
    Ok((state, Some(n.get_property("Name").await?)))
}

/// Waits (bounded) until the station is attached to `want`, connected,
/// or (for `None`) disconnected.
async fn settle(conn: &zbus::Connection, station: &str, want: Option<&str>) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let (state, on) = attached(conn, station).await?;
        let done = match want {
            None => state == "disconnected",
            Some(ssid) => state == "connected" && on.as_deref() == Some(ssid),
        };
        if done {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("station did not settle ({state}, on {on:?}, want {want:?})");
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Whether the station is still where this change left it, or where it
/// started: attached to the target, to the previous network, or to nothing.
/// Attached anywhere else means someone has moved it since, and hostd
/// leaves it alone. A station that no longer exists has nothing of its own
/// to put back (the target can still be forgotten). An iwd that can't be
/// reached is an error, never "someone else's": the receipt stays in flight
/// and is retried.
pub async fn owns(receipt: &Receipt) -> anyhow::Result<bool> {
    let (Some(device), Some(ssid)) = (field(receipt, "link"), field(receipt, "ssid")) else {
        anyhow::bail!("receipt names no device or ssid");
    };
    let conn = iwd(PUT_BACK).await?;
    let Some(station) = station(&conn, device).await? else {
        return Ok(true);
    };
    let (_, on) = attached(&conn, &station).await?;
    Ok(match on.as_deref() {
        None => true,
        Some(s) => s == ssid || Some(s) == field(receipt, "previous"),
    })
}

/// Whether undoing this receipt would undo the station's current state:
/// the station is attached to this change's network, and no later Wi-Fi
/// change on the device is still in force. Returning to an SSID through a
/// later change doesn't revive an older receipt.
pub async fn current(host: &Host, receipt: &Receipt) -> anyhow::Result<bool> {
    let (Some(device), Some(ssid)) = (field(receipt, "link"), field(receipt, "ssid")) else {
        anyhow::bail!("receipt names no device or ssid");
    };
    let newer_in_force = crate::receipt::all(&host.state)?.iter().any(|r| {
        r.verb == VERB
            && r.id > receipt.id
            && field(r, "link") == Some(device)
            && matches!(
                r.outcome,
                crate::receipt::Outcome::Applied
                    | crate::receipt::Outcome::Applying
                    | crate::receipt::Outcome::Undoing
            )
    });
    if newer_in_force {
        return Ok(false);
    }
    let conn = iwd(PUT_BACK).await?;
    let Some(station) = station(&conn, device).await? else {
        return Ok(false);
    };
    Ok(attached(&conn, &station).await?.1.as_deref() == Some(ssid))
}

/// Forgets a known network by SSID, wherever iwd lists it (no station
/// needed). Already forgotten is fine.
///
/// iwd keys a saved network by SSID and security type: only the profile
/// with both is forgotten, never another saved network with the same name.
async fn forget(conn: &zbus::Connection, ssid: &str, security: &str) -> anyhow::Result<()> {
    let objects = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination(IWD)?
        .path("/")?
        .build()
        .await?
        .get_managed_objects()
        .await?;
    for (path, ifaces) in &objects {
        let Some(known) = ifaces.get(KNOWN) else {
            continue;
        };
        let text = |key: &str| {
            known
                .get(key)
                .and_then(|v| <&str>::try_from(&**v as &zbus::zvariant::Value).ok())
        };
        if text("Name") == Some(ssid) && text("Type") == Some(security) {
            proxy(conn, path.as_str(), KNOWN)
                .await?
                .call::<_, _, ()>("Forget", &())
                .await?;
        }
    }
    Ok(())
}

/// Puts the station back (`txn::put_back`), and checks it got there:
///
/// - the target is forgotten if this change made it known (whether or not
///   the device still exists);
/// - a connect still in progress, or a connection to anything but the
///   previous network, is cancelled;
/// - the previous network is reconnected, or the station left disconnected
///   if there was none. If the previous network is no longer in range,
///   the station is left disconnected: that is the closest state left.
///
/// Every step is idempotent, so a failed put-back simply runs again.
pub async fn reapply(_host: &Host, receipt: &Receipt) -> anyhow::Result<()> {
    let (Some(device), Some(ssid)) = (field(receipt, "link"), field(receipt, "ssid")) else {
        anyhow::bail!("receipt names no device or ssid");
    };
    let was_known = receipt
        .summary
        .get("was_known")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    // Where the station started. It may be the target itself (a connect to
    // the network it was already on), which is then reconnected.
    let previous = field(receipt, "previous");
    let conn = iwd(PUT_BACK).await?;
    if !was_known {
        let security = field(receipt, "security")
            .ok_or_else(|| anyhow::anyhow!("receipt names no security type"))?;
        forget(&conn, ssid, security).await?;
    }
    let Some(station) = station(&conn, device).await? else {
        return Ok(());
    };
    let (state, on) = attached(&conn, &station).await?;
    let already = match previous {
        None => state == "disconnected",
        Some(prev) => state == "connected" && on.as_deref() == Some(prev),
    };
    if already {
        return Ok(());
    }
    if state != "disconnected" {
        proxy(&conn, &station, STATION)
            .await?
            .call::<_, _, ()>("Disconnect", &())
            .await?;
        settle(&conn, &station, None).await?;
    }
    if let Some(prev) = previous {
        match find(&conn, &station, prev).await? {
            Some(net) => {
                connect_network(&conn, &net, None, Duration::from_secs(20)).await?;
                settle(&conn, &station, Some(prev)).await?;
            }
            None => {
                tracing::warn!(target: "hostd::audit", receipt = %receipt.id, previous = %prev,
                    "previous network not in range; left disconnected");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssids_and_passphrases_are_checked() {
        assert!(valid_ssid("hostd-test"));
        assert!(valid_ssid("Café"));
        for bad in ["", "a\nb", "x\u{0}y", &"x".repeat(33)] {
            assert!(!valid_ssid(bad), "{bad:?}");
        }
        assert!(valid_passphrase("hostd-test-passphrase"));
        assert!(valid_passphrase(&"a".repeat(64)));
        for bad in ["short", "tab\there12", &"x".repeat(64), &"x".repeat(65)] {
            assert!(!valid_passphrase(bad), "{bad:?}");
        }
    }

    #[test]
    fn connect_requests_are_checked_and_strict() {
        let req = |v: Value| serde_json::from_value::<ConnectRequest>(v).unwrap();
        assert!(
            req(json!({"device":"wlan0","ssid":"home","passphrase":"hostd-test-passphrase"}))
                .check()
                .is_ok()
        );
        assert!(req(json!({"device":"../x","ssid":"home"})).check().is_err());
        assert!(req(json!({"device":"wlan0","ssid":""})).check().is_err());
        assert!(
            req(json!({"device":"wlan0","ssid":"home","window_s":1}))
                .check()
                .is_err()
        );
        assert!(
            serde_json::from_value::<ConnectRequest>(
                json!({"device":"wlan0","ssid":"h","bssid":"x"})
            )
            .is_err()
        );
    }
}
