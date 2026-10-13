// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `net.state`: who manages this host's network, and what they report.
//!
//! Links and addresses are not repeated here: every session reads them
//! itself over rtnetlink. hostd reports the management layer the session
//! can't see from rtnetlink: networkd's per-link state and `.network` file,
//! resolved's DNS configuration, iwd's stations, and which networkd files
//! hostd owns. A source that is absent or fails is reported in `errors`,
//! never fatal to the whole reply.
use crate::varlink;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Files hostd writes, and the only ones it will ever change.
pub const OWNED_PREFIX: &str = "50-hostd-";

pub struct Sources {
    pub root: PathBuf,
}

impl Sources {
    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
    fn networkd(&self) -> PathBuf {
        self.path("run/systemd/netif/io.systemd.Network")
    }
    fn resolved(&self) -> PathBuf {
        self.path("run/systemd/resolve/io.systemd.Resolve")
    }
    fn network_dir(&self) -> PathBuf {
        self.path("etc/systemd/network")
    }
}

/// The three sources run at once, each with its own deadline (3 s for
/// Varlink, 3 s per D-Bus step), so the whole reply arrives in about 6 s at
/// worst: well inside the read deadline, with partial results intact.
pub async fn collect(sources: &Sources) -> Value {
    let networkd_up = sources.networkd().exists();
    let resolved_up = sources.resolved().exists();

    let networkd = async {
        if !networkd_up {
            return Ok(vec![]);
        }
        varlink::call(
            &sources.networkd(),
            "io.systemd.Network.Describe",
            json!({}),
        )
        .await
        .map(|reply| links(&reply))
    };
    let resolved = async {
        if !resolved_up {
            return Ok(vec![]);
        }
        varlink::call(
            &sources.resolved(),
            "io.systemd.Resolve.DumpDNSConfiguration",
            json!({}),
        )
        .await
        .map(|reply| dns(&reply))
    };
    // Each D-Bus step has its own deadline, so a stalled iwd still leaves the
    // manager discovery.
    let dbus = async {
        let (conn, names) = bounded(system_bus()).await?;
        let wifi = if names.iwd {
            Some(bounded(iwd_stations(&conn)).await)
        } else {
            None
        };
        anyhow::Ok((names, wifi))
    };
    let (links, dns, dbus) = tokio::join!(networkd, resolved, dbus);

    let mut errors = Map::new();
    let mut keep = |source: &str, result: anyhow::Result<Vec<Value>>| match result {
        Ok(found) => found,
        Err(error) => {
            errors.insert(source.into(), json!(format!("{error:#}")));
            vec![]
        }
    };
    let links = keep("networkd", links);
    let dns = keep("resolved", dns);
    let (bus_names, wifi) = match dbus {
        Ok((names, wifi)) => (names, keep("iwd", wifi.unwrap_or(Ok(vec![])))),
        Err(error) => (BusNames::default(), keep("dbus", Err(error))),
    };
    let owned = match owned_files(&sources.network_dir()) {
        Ok(names) => json!(names),
        Err(error) => {
            errors.insert("owned_files".into(), json!(error.to_string()));
            Value::Null
        }
    };
    json!({
        "managers": {
            "networkd": networkd_up,
            "resolved": resolved_up,
            "network_manager": bus_names.network_manager,
            "iwd": bus_names.iwd,
        },
        "links": links,
        "dns": dns,
        "wifi": wifi,
        "owned_files": owned,
        "errors": errors,
    })
}

/// networkd's `Describe` reply, reduced to what a caller acts on.
pub fn links(reply: &Value) -> Vec<Value> {
    let Some(interfaces) = reply.get("Interfaces").and_then(Value::as_array) else {
        return vec![];
    };
    interfaces
        .iter()
        .map(|link| {
            let file = link.get("NetworkFile").and_then(Value::as_str);
            json!({
                "name": link.get("Name"),
                "index": link.get("Index"),
                "type": link.get("Type"),
                "admin": link.get("AdministrativeState"),
                "operational": link.get("OperationalState"),
                "network_file": file,
                "hostd_owned": file.is_some_and(is_owned_path),
            })
        })
        .collect()
}

/// resolved's DNS configuration: one entry for the global scope (no
/// `ifname`) and one per link that has servers or search domains.
pub fn dns(reply: &Value) -> Vec<Value> {
    let Some(scopes) = reply.get("configuration").and_then(Value::as_array) else {
        return vec![];
    };
    let addresses = |key: &str, scope: &Value| -> Vec<Value> {
        scope
            .get(key)
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|s| s.get("addressString").cloned())
                    .collect()
            })
            .unwrap_or_default()
    };
    scopes
        .iter()
        .filter_map(|scope| {
            let servers = addresses("servers", scope);
            let search: Vec<Value> = scope
                .get("searchDomains")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(|d| d.get("name").cloned()).collect())
                .unwrap_or_default();
            let link = scope.get("ifname").and_then(Value::as_str);
            if link.is_some() && servers.is_empty() && search.is_empty() {
                return None;
            }
            Some(json!({
                "scope": link.unwrap_or("global"),
                "default_route": scope.get("defaultRoute").and_then(Value::as_bool).unwrap_or(false),
                "current": scope.pointer("/currentServer/addressString"),
                "servers": servers,
                "search": search,
            }))
        })
        .collect()
}

pub fn is_owned_path(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(OWNED_PREFIX))
        && Path::new(path).parent() == Some(Path::new("/etc/systemd/network"))
}

/// hostd's files in the networkd directory. A missing directory owns
/// nothing; any other failure to look is an error, never an empty list.
pub fn owned_files(dir: &Path) -> std::io::Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut names = vec![];
    for entry in entries {
        if let Ok(name) = entry?.file_name().into_string()
            && name.starts_with(OWNED_PREFIX)
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

#[derive(Default)]
struct BusNames {
    network_manager: bool,
    iwd: bool,
}

/// Every D-Bus step, connection setup included, is cut off here.
const DBUS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);

async fn bounded<T>(step: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    tokio::time::timeout(DBUS_DEADLINE, step)
        .await
        .map_err(|_| anyhow::anyhow!("no reply within {DBUS_DEADLINE:?}"))?
}

/// Which managers own their system bus names. An activatable but stopped
/// iwd is reported as absent and is never started.
async fn system_bus() -> anyhow::Result<(zbus::Connection, BusNames)> {
    let conn = zbus::Connection::system().await?;
    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
    let names = BusNames {
        network_manager: dbus
            .name_has_owner("org.freedesktop.NetworkManager".try_into()?)
            .await?,
        iwd: dbus.name_has_owner("net.connman.iwd".try_into()?).await?,
    };
    Ok((conn, names))
}

async fn iwd_stations(conn: &zbus::Connection) -> anyhow::Result<Vec<Value>> {
    use zbus::zvariant::{ObjectPath, Value as Z};
    let objects = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination("net.connman.iwd")?
        .path("/")?
        .build()
        .await?
        .get_managed_objects()
        .await?;
    let text = |v: Option<&zbus::zvariant::OwnedValue>| -> Option<String> {
        v.and_then(|v| <&str>::try_from(&**v as &Z).ok())
            .map(str::to_owned)
    };
    let flag = |v: Option<&zbus::zvariant::OwnedValue>| -> Option<bool> {
        v.and_then(|v| bool::try_from(&**v as &Z).ok())
    };
    let mut out = vec![];
    for (path, interfaces) in &objects {
        let Some(device) = interfaces.get("net.connman.iwd.Device") else {
            continue;
        };
        let station = interfaces.get("net.connman.iwd.Station");
        // The connected network's name lives on its own object.
        let ssid = station
            .and_then(|s| s.get("ConnectedNetwork"))
            .and_then(|v| ObjectPath::try_from(&**v as &Z).ok())
            .and_then(|network| {
                objects
                    .iter()
                    .find(|(p, _)| p.as_str() == network.as_str())
                    .and_then(|(_, ifaces)| ifaces.get("net.connman.iwd.Network"))
                    .and_then(|n| text(n.get("Name")))
            });
        out.push(json!({
            "object": path.as_str(),
            "device": text(device.get("Name")),
            "powered": flag(device.get("Powered")),
            "mode": text(device.get("Mode")),
            "state": station.and_then(|s| text(s.get("State"))),
            "ssid": ssid,
        }));
    }
    out.sort_by(|a, b| a["device"].as_str().cmp(&b["device"].as_str()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_paths_are_prefixed_and_in_the_network_dir() {
        assert!(is_owned_path("/etc/systemd/network/50-hostd-eth0.network"));
        assert!(!is_owned_path("/etc/systemd/network/20-wired.network"));
        assert!(!is_owned_path("/run/systemd/network/50-hostd-eth0.network"));
        assert!(!is_owned_path(
            "/etc/systemd/network/sub/50-hostd-x.network"
        ));
    }

    #[test]
    fn links_reduce_describe() {
        // Field names as networkd's Describe returns them (systemd 257+).
        let reply = json!({"Interfaces":[
            {"Index":2,"Name":"eth0","Type":"ether","AdministrativeState":"configured",
             "OperationalState":"routable","NetworkFile":"/etc/systemd/network/50-hostd-eth0.network",
             "Addresses":[]},
            {"Index":1,"Name":"lo","Type":"loopback","AdministrativeState":"unmanaged","OperationalState":"carrier"}
        ]});
        let l = links(&reply);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0]["hostd_owned"], true);
        assert_eq!(l[0]["admin"], "configured");
        assert_eq!(l[0]["operational"], "routable");
        assert_eq!(l[1]["hostd_owned"], false);
        assert_eq!(l[1]["network_file"], Value::Null);
        assert!(links(&json!({})).is_empty());
    }

    #[test]
    fn dns_keeps_global_and_configured_links() {
        let reply = json!({"configuration":[
            {"currentServer":{"addressString":"127.0.0.1"},
             "servers":[{"addressString":"127.0.0.1"}],"searchDomains":[{"name":"example.com"}]},
            {"ifname":"lo","defaultRoute":false},
            {"ifname":"br0","defaultRoute":true,"servers":[{"addressString":"192.0.2.1"}]}
        ]});
        let d = dns(&reply);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0]["scope"], "global");
        assert_eq!(d[0]["search"], json!(["example.com"]));
        assert_eq!(d[1]["scope"], "br0");
        assert_eq!(d[1]["default_route"], true);
        assert_eq!(d[1]["current"], Value::Null);
    }

    #[test]
    fn owned_files_lists_only_hostd_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "50-hostd-b.network",
            "50-hostd-a.netdev",
            "20-wired.network",
        ] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        assert_eq!(
            owned_files(dir.path()).unwrap(),
            vec!["50-hostd-a.netdev", "50-hostd-b.network"]
        );
        assert!(owned_files(&dir.path().join("missing")).unwrap().is_empty());
        // Not a directory: an error, not an empty list.
        assert!(owned_files(&dir.path().join("20-wired.network")).is_err());
    }
}
