// SPDX-License-Identifier: MIT OR Apache-2.0
//! `net.wg.apply`: a networkd-native WireGuard link from typed fields.
//!
//! hostd writes three files, all its own, in `/etc/systemd/network`:
//! `50-hostd-<name>.netdev`, `50-hostd-<name>.network`, and the private key
//! `50-hostd-<name>.key` (mode 0640, group `systemd-network`, so networkd
//! can read it and nobody else can). The key is made once, kept across
//! later applies, and never leaves the host: replies carry only the public
//! key.
//!
//! The dead-man check: the link must come up configured with its
//! addresses, and every peer with an endpoint must complete a handshake
//! within the window, or the change is put back.
use crate::{
    caller::Origin,
    link::valid_link,
    neigh,
    net::{OWNED_PREFIX, is_owned_path},
    networkd::{self, Networkd},
    receipt::{self, FileChange, Receipt},
    service::Host,
    txn, wgnl,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    net::IpAddr,
    path::{Path, PathBuf},
    time::Duration,
};

pub const VERB: &str = "net.wg.apply";
const DEFAULT_WINDOW: u64 = 25;
const MAX_WINDOW: u64 = 60;
const MAX_ADDRESSES: usize = 8;
const MAX_PEERS: usize = 16;
const MAX_ALLOWED: usize = 32;
/// The group networkd reads key files as.
pub const KEY_GROUP: &str = "systemd-network";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub name: String,
    /// `address/prefix`, IPv4 or IPv6.
    pub addresses: Vec<String>,
    #[serde(default)]
    pub listen_port: Option<u16>,
    #[serde(default)]
    pub peers: Vec<PeerRequest>,
    /// Seconds for the link to come up and every peer with an endpoint to
    /// complete a handshake.
    #[serde(default)]
    pub window_s: Option<u64>,
    /// Defaults to true when any peer has an endpoint. False lets the first
    /// side of a new pair come up before its peer exists.
    #[serde(default)]
    pub require_handshake: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerRequest {
    /// Base64, 32 bytes.
    pub public_key: String,
    /// `host:port`, `ip:port` or `[ipv6]:port`.
    #[serde(default)]
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<String>,
    #[serde(default)]
    pub keepalive_s: Option<u16>,
}

#[derive(Debug)]
pub struct Plan {
    pub name: String,
    pub netdev: PathBuf,
    pub network: PathBuf,
    pub key: PathBuf,
    pub netdev_text: String,
    pub network_text: String,
    /// The key to write, base64, when this change makes the key.
    pub new_key: Option<String>,
    pub public_key: String,
    pub addresses: Vec<(IpAddr, u8)>,
    /// Peers whose handshake the dead-man check waits for, each with an
    /// address inside its allowed IPs: a datagram sent there through the
    /// link makes WireGuard start the handshake.
    pub must_handshake: Vec<([u8; 32], IpAddr)>,
    pub window: Duration,
}

fn bad(message: impl Into<String>) -> Value {
    json!({"status":"bad_request","message":message.into()})
}

fn cidr(text: &str, allow_any: bool) -> Option<(IpAddr, u8)> {
    let (addr, prefix) = text.split_once('/')?;
    let addr: IpAddr = addr.parse().ok()?;
    let prefix: u8 = prefix.parse().ok()?;
    let max = if addr.is_ipv4() { 32 } else { 128 };
    let usable = allow_any || (!addr.is_unspecified() && !addr.is_multicast());
    (prefix <= max && usable).then_some((addr, prefix))
}

pub fn decode_key(text: &str) -> Option<[u8; 32]> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .ok()?;
    <[u8; 32]>::try_from(bytes).ok()
}

fn encode_key(key: &[u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(key)
}

/// The X25519 public key for a WireGuard private key.
pub fn public_of(private: &[u8; 32]) -> [u8; 32] {
    curve25519_dalek::montgomery::MontgomeryPoint::mul_base_clamped(*private).to_bytes()
}

/// A new private key: 32 random bytes from the kernel, clamped as X25519
/// expects (WireGuard clamps too, so this only makes it canonical).
fn new_private() -> anyhow::Result<[u8; 32]> {
    use std::io::Read;
    let mut key = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut key)?;
    key[0] &= 248;
    key[31] &= 127;
    key[31] |= 64;
    Ok(key)
}

/// `host:port`, `ip:port` or `[ipv6]:port`, with nothing that could end a
/// line or a value in the `.netdev` file.
pub fn valid_endpoint(text: &str) -> bool {
    let Some((host, port)) = text.rsplit_once(':') else {
        return false;
    };
    let port_ok = port.parse::<u16>().is_ok_and(|p| p > 0);
    let host_ok = if let Some(v6) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        v6.parse::<std::net::Ipv6Addr>().is_ok()
    } else {
        host.parse::<std::net::Ipv4Addr>().is_ok()
            || ((1..=253).contains(&host.len())
                && host.split('.').all(|label| {
                    (1..=63).contains(&label.len())
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                }))
    };
    port_ok && host_ok
}

fn paths(root: &Path, name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = root.join("etc/systemd/network");
    (
        dir.join(format!("{OWNED_PREFIX}{name}.netdev")),
        dir.join(format!("{OWNED_PREFIX}{name}.network")),
        dir.join(format!("{OWNED_PREFIX}{name}.key")),
    )
}

pub fn plan(root: &Path, request: Request) -> Result<Plan, Value> {
    if !valid_link(&request.name) {
        return Err(bad("name must be 1-15 characters of [A-Za-z0-9._-]"));
    }
    let window = request.window_s.unwrap_or(DEFAULT_WINDOW);
    if !(5..=MAX_WINDOW).contains(&window) {
        return Err(bad(format!("window_s must be 5-{MAX_WINDOW}")));
    }
    if request.addresses.is_empty() || request.addresses.len() > MAX_ADDRESSES {
        return Err(bad(format!("1-{MAX_ADDRESSES} addresses")));
    }
    if request.peers.len() > MAX_PEERS {
        return Err(bad(format!("at most {MAX_PEERS} peers")));
    }
    if request.listen_port == Some(0) {
        return Err(bad("listen_port must be 1-65535"));
    }
    let (netdev, network, key) = paths(root, &request.name);

    // Keep the key hostd made before; make one only for a new link.
    let (private, new_key) = match std::fs::read_to_string(&key) {
        Ok(text) => (
            decode_key(&text).ok_or_else(|| bad("the existing key file is not a WireGuard key"))?,
            None,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let k =
                new_private().map_err(|e| json!({"status":"internal","message":e.to_string()}))?;
            (k, Some(encode_key(&k)))
        }
        Err(e) => return Err(json!({"status":"internal","message":e.to_string()})),
    };
    let public_key = encode_key(&public_of(&private));

    let mut netdev_text = format!(
        "# Written by hostd ({VERB}). hostd replaces this file; edits are lost.\n\
         [NetDev]\nName={}\nKind=wireguard\n\n[WireGuard]\nPrivateKeyFile={}\n",
        request.name,
        key.strip_prefix(root)
            .map(|p| Path::new("/").join(p))
            .unwrap_or(key.clone())
            .display()
    );
    if let Some(port) = request.listen_port {
        netdev_text.push_str(&format!("ListenPort={port}\n"));
    }
    let mut must_handshake = vec![];
    let mut any_endpoint = false;
    for peer in &request.peers {
        let key = decode_key(&peer.public_key).ok_or_else(|| {
            bad(format!(
                "{:?} is not a WireGuard public key",
                peer.public_key
            ))
        })?;
        if peer.allowed_ips.is_empty() || peer.allowed_ips.len() > MAX_ALLOWED {
            return Err(bad(format!("each peer needs 1-{MAX_ALLOWED} allowed_ips")));
        }
        let mut allowed = vec![];
        let mut probe = None;
        for a in &peer.allowed_ips {
            let (addr, prefix) =
                cidr(a, true).ok_or_else(|| bad(format!("{a:?} is not address/prefix")))?;
            allowed.push(format!("{addr}/{prefix}"));
            probe.get_or_insert(probe_target(addr, prefix));
        }
        netdev_text.push_str(&format!(
            "\n[WireGuardPeer]\nPublicKey={}\nAllowedIPs={}\n",
            encode_key(&key),
            allowed.join(",")
        ));
        if let Some(endpoint) = &peer.endpoint {
            if !valid_endpoint(endpoint) {
                return Err(bad(format!("{endpoint:?} is not host:port")));
            }
            netdev_text.push_str(&format!("Endpoint={endpoint}\n"));
            any_endpoint = true;
            // `allowed` is never empty here, so neither is `probe`.
            must_handshake.push((key, probe.expect("checked above")));
        }
        if let Some(seconds) = peer.keepalive_s {
            if !(1..=3600).contains(&seconds) {
                return Err(bad("keepalive_s must be 1-3600"));
            }
            netdev_text.push_str(&format!("PersistentKeepalive={seconds}\n"));
        }
    }
    if !request.require_handshake.unwrap_or(any_endpoint) {
        must_handshake.clear();
    }

    let mut network_text = format!(
        "# Written by hostd ({VERB}). hostd replaces this file; edits are lost.\n\
         [Match]\nName={}\n\n[Network]\n",
        request.name
    );
    let mut addresses = vec![];
    for a in &request.addresses {
        let (addr, prefix) =
            cidr(a, false).ok_or_else(|| bad(format!("{a:?} is not address/prefix")))?;
        network_text.push_str(&format!("Address={addr}/{prefix}\n"));
        addresses.push((addr, prefix));
    }

    Ok(Plan {
        name: request.name,
        netdev,
        network,
        key,
        netdev_text,
        network_text,
        new_key,
        public_key,
        addresses,
        must_handshake,
        window: Duration::from_secs(window),
    })
}

/// Configured from hostd's file, carrying every planned address.
pub fn healthy(plan: &Plan, entry: &Value) -> bool {
    let ours = entry
        .get("NetworkFile")
        .and_then(Value::as_str)
        .is_some_and(|f| f.ends_with(&format!("/{OWNED_PREFIX}{}.network", plan.name)));
    let configured = entry.get("AdministrativeState").and_then(Value::as_str) == Some("configured");
    let carried: Vec<(IpAddr, u8)> = entry
        .get("Addresses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let bytes: Vec<u8> = a
                .get("Address")?
                .as_array()?
                .iter()
                .map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
                .collect::<Option<_>>()?;
            let addr = match bytes.len() {
                4 => IpAddr::from(<[u8; 4]>::try_from(bytes).ok()?),
                16 => IpAddr::from(<[u8; 16]>::try_from(bytes).ok()?),
                _ => return None,
            };
            Some((
                addr,
                a.get("PrefixLength")?
                    .as_u64()
                    .and_then(|p| u8::try_from(p).ok())?,
            ))
        })
        .collect();
    ours && configured && plan.addresses.iter().all(|a| carried.contains(a))
}

fn ifindex_of(entry: &Value) -> Option<i32> {
    entry
        .get("Index")
        .and_then(Value::as_i64)
        .and_then(|i| i32::try_from(i).ok())
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// An address inside an allowed-IPs entry to send a handshake-starting
/// datagram to: the entry's own address, or the next one when the entry is
/// a network (`10.0.0.0/24` -> `10.0.0.1`), or a documentation address for
/// a default route (`0.0.0.0/0`), which WireGuard still sends to this peer.
pub fn probe_target(addr: IpAddr, prefix: u8) -> IpAddr {
    match addr {
        IpAddr::V4(v4) if v4.is_unspecified() => "192.0.2.1".parse().expect("literal"),
        IpAddr::V6(v6) if v6.is_unspecified() => "2001:db8::1".parse().expect("literal"),
        IpAddr::V4(v4) => {
            let host_bits = 32 - u32::from(prefix);
            let n = u32::from(v4);
            if host_bits > 0 && host_bits < 32 && n & ((1u32 << host_bits) - 1) == 0 {
                IpAddr::V4((n + 1).into())
            } else {
                addr
            }
        }
        IpAddr::V6(v6) => {
            let host_bits = 128 - u32::from(prefix);
            let n = u128::from(v6);
            if host_bits > 0 && host_bits < 128 && n & ((1u128 << host_bits) - 1) == 0 {
                IpAddr::V6((n + 1).into())
            } else {
                addr
            }
        }
    }
}

/// Waits, within `window`, for every listed peer to complete a handshake
/// after `since` (seconds since the epoch). Each round sends a datagram
/// through the link to every peer still waiting, because WireGuard starts a
/// handshake only when it has something to send: an idle pair without
/// keepalives would otherwise never handshake. Bounded to one window.
async fn handshakes(
    name: &str,
    ifindex: i32,
    peers: &[([u8; 32], IpAddr)],
    since: i64,
    window: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let link = name.to_owned();
        let state = tokio::task::spawn_blocking(move || wgnl::peers(&link)).await;
        let done = |want: &[u8; 32], state: &[wgnl::Peer]| {
            state
                .iter()
                .any(|p| &p.public_key == want && p.last_handshake >= since)
        };
        let state = match state {
            Ok(Ok(state)) => state,
            _ => vec![],
        };
        if peers.iter().all(|(key, _)| done(key, &state)) {
            return true;
        }
        for (key, target) in peers {
            if !done(key, &state) {
                let (link, target) = (name.to_owned(), *target);
                let _ =
                    tokio::task::spawn_blocking(move || neigh::nudge(&link, ifindex, target)).await;
            }
        }
        if tokio::time::Instant::now() + Duration::from_millis(500) >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn summary(plan: &Plan) -> Value {
    let addresses: Vec<String> = plan
        .addresses
        .iter()
        .map(|(a, p)| format!("{a}/{p}"))
        .collect();
    json!({"link": plan.name, "netdev": plan.netdev, "network": plan.network,
           "addresses": addresses, "public_key": plan.public_key,
           "handshake_peers": plan.must_handshake.len(), "window_s": plan.window.as_secs()})
}

/// Runs the transaction, under the writer lock, in its own task.
pub async fn apply(host: Host, plan: Plan, grant: String, origin: Origin) -> Result<Value, Value> {
    let internal = |e: anyhow::Error| json!({"status":"internal","message":format!("{e:#}")});
    let current = networkd::describe(&host.root, &plan.name)
        .await
        .map_err(internal)?;
    if let Some(entry) = &current {
        // An existing link must be hostd's own WireGuard link.
        let file = entry.get("NetworkFile").and_then(Value::as_str);
        if file.is_some_and(|f| !is_owned_path(f)) {
            return Err(json!({"status":"foreign_config","link":plan.name,"file":file}));
        }
        if !plan.netdev.exists() {
            return Err(json!({"status":"foreign_config","link":plan.name,
                "message":"a link of this name exists that hostd didn't create"}));
        }
        // It must be WireGuard and hold hostd's key for this name: a link
        // someone made with the same name is never deleted or reconfigured.
        let name = plan.name.clone();
        let live = tokio::task::spawn_blocking(move || wgnl::device_key(&name))
            .await
            .map_err(|e| internal(e.into()))?;
        let ours = decode_key(&plan.public_key);
        if !matches!(live, Ok(Some(key)) if Some(key) == ours) {
            return Err(json!({"status":"foreign_config","link":plan.name,
                "message":"the link isn't hostd's WireGuard link"}));
        }
    }

    let networkd = Networkd::connect().await.map_err(internal)?;
    let mut receipt = Receipt::new(VERB, &grant, &origin, summary(&plan));
    let mut files = vec![];
    if let Some(key) = &plan.new_key {
        let mut change = FileChange::backup(&plan.key).map_err(internal)?;
        change.installed = Some(receipt::digest(format!("{key}\n").as_bytes()));
        change.mode = 0o640;
        change.group = Some(KEY_GROUP.to_owned());
        files.push(change);
    }
    for (path, text) in [
        (&plan.netdev, &plan.netdev_text),
        (&plan.network, &plan.network_text),
    ] {
        let mut change = FileChange::backup(path).map_err(internal)?;
        change.installed = Some(receipt::digest(text.as_bytes()));
        files.push(change);
    }
    receipt.files = files;
    txn::begin(&host, &receipt)?;

    let since = now_epoch();
    let attempt = async {
        if let Some(key) = &plan.new_key {
            receipt::write_as(
                &plan.key,
                format!("{key}\n").as_bytes(),
                0o640,
                Some(KEY_GROUP),
            )?;
        }
        receipt::write_atomic(&plan.netdev, plan.netdev_text.as_bytes(), 0o644)?;
        receipt::write_atomic(&plan.network, plan.network_text.as_bytes(), 0o644)?;
        // networkd doesn't apply a changed .netdev to a link that exists:
        // delete hostd's own link so the reload creates it afresh.
        if let Some(index) = current.as_ref().and_then(ifindex_of) {
            tokio::task::spawn_blocking(move || wgnl::delete_link(index)).await??;
        }
        networkd.reload().await?;
        let started = tokio::time::Instant::now();
        let (up, last) = networkd
            .wait_for(&host.root, &plan.name, plan.window, |e| healthy(&plan, e))
            .await?;
        if !up || plan.must_handshake.is_empty() {
            return anyhow::Ok((up, last));
        }
        let left = plan
            .window
            .saturating_sub(started.elapsed())
            .max(Duration::from_secs(2));
        let ifindex = ifindex_of(&last).unwrap_or(0);
        Ok((
            handshakes(&plan.name, ifindex, &plan.must_handshake, since, left).await,
            last,
        ))
    }
    .await;

    let (passed, last, reason) = match attempt {
        Ok((true, last)) => (true, last, None),
        Ok((false, last)) => (
            false,
            last,
            Some(if plan.must_handshake.is_empty() {
                "the link did not come up in time".to_owned()
            } else {
                "the link or a peer handshake did not come up in time".to_owned()
            }),
        ),
        Err(e) => (false, Value::Null, Some(format!("{e:#}"))),
    };
    let suspended = txn::conclude(&host, &mut receipt, passed, reason).await?;
    let reply = json!({
        "receipt": receipt.id,
        "outcome": receipt.outcome,
        "reason": receipt.reason,
        "grant": grant,
        "grant_suspended": suspended,
        "public_key": plan.public_key,
        "link": {"admin": last.get("AdministrativeState"), "operational": last.get("OperationalState")},
    });
    if passed {
        Ok(reply)
    } else {
        Err(json!({"status":"rolled_back", "detail": reply}))
    }
}

/// Makes networkd use the restored files (`txn::put_back`): delete hostd's
/// link, then reload, so the previous `.netdev` (if any) creates it again.
pub async fn reapply(host: &Host, receipt: &Receipt) -> anyhow::Result<()> {
    let name = receipt_name(receipt)?;
    // `txn::put_back` checked ownership before restoring files; check again
    // here, right before the delete, so nothing else's link is ever removed.
    if !owns(receipt).await? {
        anyhow::bail!("link {name} is no longer hostd's own WireGuard link; left alone");
    }
    if let Some(index) = networkd::describe(&host.root, &name)
        .await?
        .as_ref()
        .and_then(ifindex_of)
    {
        tokio::task::spawn_blocking(move || wgnl::delete_link(index)).await??;
    }
    Networkd::connect().await?.reload().await
}

fn receipt_name(receipt: &Receipt) -> anyhow::Result<String> {
    Ok(receipt
        .summary
        .get("link")
        .and_then(Value::as_str)
        .filter(|l| valid_link(l))
        .ok_or_else(|| anyhow::anyhow!("receipt names no link"))?
        .to_owned())
}

/// Whether the link a receipt names is still hostd's: absent, or a
/// WireGuard link holding the key the receipt recorded. Anything else (a
/// link of another kind, or a WireGuard link with another key, made by
/// someone else since) isn't hostd's to delete or reconfigure.
pub async fn owns(receipt: &Receipt) -> anyhow::Result<bool> {
    let name = receipt_name(receipt)?;
    let recorded = receipt
        .summary
        .get("public_key")
        .and_then(Value::as_str)
        .and_then(decode_key);
    let live = tokio::task::spawn_blocking(move || wgnl::device_key(&name)).await?;
    Ok(match live {
        Ok(None) => true,
        Ok(Some(key)) => recorded == Some(key),
        // Not WireGuard (the kernel refuses the query), or unreadable.
        Err(_) => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(value: Value) -> Request {
        serde_json::from_value(value).unwrap()
    }

    const PEER: &str = "HIgo9xNzJMWLKASShiTqIybxZ0U3wGLiUeJ1PKf8ykw=";

    #[test]
    fn public_key_matches_the_rfc_7748_vector() {
        // RFC 7748 §6.1, Alice.
        let private: [u8; 32] =
            hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let public: [u8; 32] =
            hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        assert_eq!(public_of(&private), public);
    }

    fn hex(s: &str) -> [u8; 32] {
        let v: Vec<u8> = (0..32)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect();
        v.try_into().unwrap()
    }

    #[test]
    fn plan_makes_a_key_once_and_renders_typed_fields() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("etc/systemd/network")).unwrap();
        let p = plan(
            dir.path(),
            request(
                json!({"name":"wg-home","addresses":["10.99.0.1/24"],"listen_port":51820,
                "peers":[{"public_key":PEER,"endpoint":"198.51.100.7:51820",
                          "allowed_ips":["10.99.0.2/32","0.0.0.0/0"],"keepalive_s":25}]}),
            ),
        )
        .unwrap();
        assert!(p.new_key.is_some());
        assert!(p.netdev_text.contains("Kind=wireguard\n"));
        assert!(
            p.netdev_text
                .contains("PrivateKeyFile=/etc/systemd/network/50-hostd-wg-home.key\n")
        );
        assert!(p.netdev_text.contains("ListenPort=51820\n"));
        assert!(p.netdev_text.contains(&format!("PublicKey={PEER}\nAllowedIPs=10.99.0.2/32,0.0.0.0/0\nEndpoint=198.51.100.7:51820\nPersistentKeepalive=25\n")));
        assert!(p.network_text.contains("Address=10.99.0.1/24\n"));
        assert_eq!(p.must_handshake.len(), 1);
        // The handshake probe goes to an address inside the peer's first
        // allowed entry.
        assert_eq!(
            p.must_handshake[0].1,
            "10.99.0.2".parse::<IpAddr>().unwrap()
        );
        // The key never appears in what a caller sees.
        assert!(
            !summary(&p)
                .to_string()
                .contains(p.new_key.as_ref().unwrap())
        );
        // With a key file present, it's kept and its public key reported.
        std::fs::write(&p.key, format!("{}\n", p.new_key.as_ref().unwrap())).unwrap();
        let again = plan(
            dir.path(),
            request(json!({"name":"wg-home","addresses":["10.99.0.1/24"]})),
        )
        .unwrap();
        assert!(again.new_key.is_none());
        assert_eq!(again.public_key, p.public_key);
        assert!(again.must_handshake.is_empty());
    }

    #[test]
    fn require_handshake_false_skips_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let p = plan(
            dir.path(),
            request(json!({"name":"wga","addresses":["10.99.0.1/32"],"require_handshake":false,
                "peers":[{"public_key":PEER,"endpoint":"127.0.0.1:51821","allowed_ips":["10.99.0.2/32"]}]})),
        )
        .unwrap();
        assert!(p.must_handshake.is_empty());
    }

    #[test]
    fn plans_refuse_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let refused = [
            json!({"name":"../x","addresses":["10.0.0.1/24"]}),
            json!({"name":"wg0","addresses":[]}),
            json!({"name":"wg0","addresses":["10.0.0.1/24"],"listen_port":0}),
            json!({"name":"wg0","addresses":["10.0.0.1/24"],"peers":[{"public_key":"short","allowed_ips":["10.0.0.2/32"]}]}),
            json!({"name":"wg0","addresses":["10.0.0.1/24"],"peers":[{"public_key":PEER,"allowed_ips":[]}]}),
            json!({"name":"wg0","addresses":["10.0.0.1/24"],"peers":[{"public_key":PEER,"allowed_ips":["10.0.0.2/32"],"endpoint":"host\nDNS=1.1.1.1:51820"}]}),
            json!({"name":"wg0","addresses":["10.0.0.1/24"],"peers":[{"public_key":PEER,"allowed_ips":["10.0.0.2/32"],"keepalive_s":0}]}),
            json!({"name":"wg0","addresses":["10.0.0.1/24\nDNS=x"]}),
            json!({"name":"wg0","addresses":["10.0.0.1/24"],"window_s":2}),
        ];
        for value in refused {
            assert!(plan(dir.path(), request(value.clone())).is_err(), "{value}");
        }
        assert!(
            serde_json::from_value::<Request>(
                json!({"name":"wg0","addresses":[],"private_key":"x"})
            )
            .is_err()
        );
    }

    #[test]
    fn probe_targets_land_inside_the_allowed_entry() {
        let t = |a: &str, p: u8| probe_target(a.parse().unwrap(), p).to_string();
        assert_eq!(t("10.99.0.2", 32), "10.99.0.2");
        assert_eq!(t("10.99.0.0", 24), "10.99.0.1");
        assert_eq!(t("10.99.0.7", 24), "10.99.0.7");
        assert_eq!(t("0.0.0.0", 0), "192.0.2.1");
        assert_eq!(t("::", 0), "2001:db8::1");
        assert_eq!(t("2001:db8:1::", 64), "2001:db8:1::1");
    }

    #[test]
    fn endpoints_are_strict() {
        for ok in [
            "198.51.100.7:51820",
            "[2001:db8::1]:51820",
            "vpn.example.com:443",
        ] {
            assert!(valid_endpoint(ok), "{ok}");
        }
        for bad in [
            "198.51.100.7",
            "x:0",
            "-bad.example:1",
            "a b:1",
            "[::1:1",
            "host:99999",
            "h:1\n",
        ] {
            assert!(!valid_endpoint(bad), "{bad}");
        }
    }

    #[test]
    fn health_needs_our_file_and_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let p = plan(
            dir.path(),
            request(json!({"name":"wga","addresses":["10.99.0.1/24"]})),
        )
        .unwrap();
        let entry = |file: &str, admin: &str, addr: [u8; 4], prefix: u8| {
            json!({"NetworkFile": file, "AdministrativeState": admin,
                   "Addresses": [{"Address": addr, "PrefixLength": prefix}]})
        };
        let ours = "/etc/systemd/network/50-hostd-wga.network";
        assert!(healthy(&p, &entry(ours, "configured", [10, 99, 0, 1], 24)));
        assert!(!healthy(
            &p,
            &entry(ours, "configuring", [10, 99, 0, 1], 24)
        ));
        assert!(!healthy(&p, &entry(ours, "configured", [10, 99, 0, 1], 16)));
        assert!(!healthy(
            &p,
            &entry(
                "/etc/systemd/network/wga.network",
                "configured",
                [10, 99, 0, 1],
                24
            )
        ));
    }
}
