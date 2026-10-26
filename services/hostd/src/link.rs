// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `net.link.apply`: give one link a networkd configuration from typed
//! fields, check it comes up, and put the previous file back if it doesn't.
//!
//! hostd writes only `/etc/systemd/network/50-hostd-<link>.network` and
//! refuses a link that another file already configures.
use crate::{
    caller::Origin,
    neigh,
    net::{OWNED_PREFIX, is_owned_path},
    networkd::{self, Networkd},
    receipt::{self, FileChange, Receipt},
    service::Host,
    txn,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{net::IpAddr, path::PathBuf, time::Duration};

pub const COMMAND: &str = "net.link.apply";
const DEFAULT_WINDOW: u64 = 20;
const MAX_WINDOW: u64 = 60;
const MAX_LIST: usize = 8;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub link: String,
    pub config: Config,
    /// Seconds the link has to come up before the change is rolled back.
    #[serde(default)]
    pub window_s: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Config {
    Dhcp {
        #[serde(default)]
        dns: Vec<IpAddr>,
        /// The metric of the routes DHCP installs. Higher loses to lower: a
        /// second uplink that should never take over the default route
        /// gets a high one.
        #[serde(default)]
        route_metric: Option<u32>,
    },
    Static {
        /// `address/prefix`, IPv4 or IPv6.
        addresses: Vec<String>,
        #[serde(default)]
        gateway: Option<IpAddr>,
        #[serde(default)]
        dns: Vec<IpAddr>,
        /// The metric of the default route through `gateway` (see Dhcp).
        #[serde(default)]
        route_metric: Option<u32>,
    },
}

/// A checked request: everything that reaches the host is validated here.
#[derive(Debug)]
pub struct Plan {
    pub link: String,
    pub path: PathBuf,
    pub text: String,
    pub window: Duration,
    /// Addresses, with prefix lengths, the link must carry once configured.
    pub addresses: Vec<(IpAddr, u8)>,
    /// Whether success needs a working default route (DHCP, or a gateway).
    pub routable: bool,
    /// The static gateway, which must be the installed default route's.
    pub gateway: Option<IpAddr>,
}

fn bad(message: impl Into<String>) -> Value {
    json!({"status":"bad_request","message":message.into()})
}

/// Kernel interface names: 1–15 bytes, no `/`, whitespace or `:`. hostd
/// also refuses anything but `[A-Za-z0-9._-]`, so the name is safe in a
/// file name and in a `[Match]` section.
pub fn valid_link(name: &str) -> bool {
    (1..=15).contains(&name.len())
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn parse_cidr(text: &str) -> Option<(IpAddr, u8)> {
    let (addr, prefix) = text.split_once('/')?;
    let addr: IpAddr = addr.parse().ok()?;
    let prefix: u8 = prefix.parse().ok()?;
    let max = if addr.is_ipv4() { 32 } else { 128 };
    (prefix <= max && !addr.is_unspecified() && !addr.is_multicast()).then_some((addr, prefix))
}

pub fn plan(root: &std::path::Path, request: Request) -> Result<Plan, Value> {
    if !valid_link(&request.link) {
        return Err(bad("link must be 1-15 characters of [A-Za-z0-9._-]"));
    }
    let window = request.window_s.unwrap_or(DEFAULT_WINDOW);
    if !(5..=MAX_WINDOW).contains(&window) {
        return Err(bad(format!("window_s must be 5-{MAX_WINDOW}")));
    }
    let mut text = format!(
        "# Written by hostd ({COMMAND}). hostd replaces this file; edits are lost.\n\
         [Match]\nName={}\n\n[Network]\n",
        request.link
    );
    // Sections that follow [Network], written after its last line.
    let mut tail = String::new();
    let (addresses, gateway, routable, dns) = match &request.config {
        Config::Dhcp { dns, route_metric } => {
            text.push_str("DHCP=yes\n");
            if let Some(m) = route_metric {
                tail.push_str(&format!(
                    "\n[DHCPv4]\nRouteMetric={m}\n\n[IPv6AcceptRA]\nRouteMetric={m}\n"
                ));
            }
            (vec![], None, true, dns)
        }
        Config::Static {
            addresses,
            gateway,
            dns,
            route_metric,
        } => {
            if addresses.is_empty() || addresses.len() > MAX_LIST {
                return Err(bad(format!("static needs 1-{MAX_LIST} addresses")));
            }
            let mut parsed = vec![];
            for a in addresses {
                let (addr, prefix) =
                    parse_cidr(a).ok_or_else(|| bad(format!("{a:?} is not address/prefix")))?;
                text.push_str(&format!("Address={addr}/{prefix}\n"));
                parsed.push((addr, prefix));
            }
            if let Some(gw) = gateway {
                if gw.is_unspecified() || gw.is_multicast() {
                    return Err(bad("gateway must be a unicast address"));
                }
                if !parsed.iter().any(|(a, _)| a.is_ipv4() == gw.is_ipv4()) {
                    return Err(bad("gateway needs an address of its own family"));
                }
                match route_metric {
                    None => text.push_str(&format!("Gateway={gw}\n")),
                    Some(m) => tail.push_str(&format!("\n[Route]\nGateway={gw}\nMetric={m}\n")),
                }
            } else if route_metric.is_some() {
                return Err(bad("route_metric needs a gateway"));
            }
            (parsed, *gateway, gateway.is_some(), dns)
        }
    };
    if dns.len() > MAX_LIST {
        return Err(bad(format!("at most {MAX_LIST} DNS servers")));
    }
    for server in dns {
        text.push_str(&format!("DNS={server}\n"));
    }
    text.push_str(&tail);
    Ok(Plan {
        path: root
            .join("etc/systemd/network")
            .join(format!("{OWNED_PREFIX}{}.network", request.link)),
        link: request.link,
        text,
        window: Duration::from_secs(window),
        addresses,
        routable,
        gateway,
    })
}

/// Whether networkd reports the link configured from hostd's file, carrying
/// every planned address with its prefix length, and, when the plan needs
/// one, an installed default route (through the planned gateway, if
/// static). Returns that route's gateway, which must then answer. Without a
/// default route to check, `Some(None)`; not healthy, `None`.
pub fn healthy(plan: &Plan, entry: &Value) -> Option<Option<IpAddr>> {
    let ours = entry
        .get("NetworkFile")
        .and_then(Value::as_str)
        .is_some_and(|f| f.ends_with(&format!("/{OWNED_PREFIX}{}.network", plan.link)));
    let configured = entry.get("AdministrativeState").and_then(Value::as_str) == Some("configured");
    let operational = entry.get("OperationalState").and_then(Value::as_str);
    let up = if plan.routable {
        operational == Some("routable")
    } else {
        matches!(operational, Some("routable" | "degraded" | "enslaved"))
    };
    let carried: Vec<(IpAddr, u8)> = list(entry, "Addresses")
        .filter_map(|a| Some((bytes_addr(a.get("Address")?)?, prefix(a, "PrefixLength")?)))
        .collect();
    let has_all = plan.addresses.iter().all(|a| carried.contains(a));
    if !(ours && configured && up && has_all) {
        return None;
    }
    if !plan.routable {
        return Some(None);
    }
    // The installed default route on this link, of the gateway's family.
    let default = list(entry, "Routes").find_map(|r| {
        let configured = r.get("ConfigState").and_then(Value::as_str) == Some("configured");
        let default = prefix(r, "DestinationPrefixLength") == Some(0);
        let gw = bytes_addr(r.get("Gateway")?)?;
        let family = plan
            .gateway
            .is_none_or(|want| want.is_ipv4() == gw.is_ipv4());
        (configured && default && family).then_some(gw)
    })?;
    if plan.gateway.is_some_and(|want| want != default) {
        return None;
    }
    Some(Some(default))
}

fn list<'a>(entry: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    entry
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn prefix(entry: &Value, key: &str) -> Option<u8> {
    entry.get(key)?.as_u64().and_then(|p| u8::try_from(p).ok())
}

/// networkd gives an address as a byte array.
fn bytes_addr(value: &Value) -> Option<IpAddr> {
    let bytes: Vec<u8> = value
        .as_array()?
        .iter()
        .map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
        .collect::<Option<_>>()?;
    match bytes.len() {
        4 => Some(IpAddr::from(<[u8; 4]>::try_from(bytes).ok()?)),
        16 => Some(IpAddr::from(<[u8; 16]>::try_from(bytes).ok()?)),
        _ => None,
    }
}

/// A link nobody's file configures but that already carries addresses
/// (set by hand, or by another manager such as NetworkManager). hostd's
/// rollback restores files only, so it can't give such a link its runtime
/// configuration back, and refuses to take it over. IPv6 link-local
/// addresses don't count: the kernel adds those to any link that is up.
pub fn configured_by_someone_else(entry: &Value) -> bool {
    entry.get("NetworkFile").and_then(Value::as_str).is_none()
        && list(entry, "Addresses")
            .filter_map(|a| bytes_addr(a.get("Address")?))
            .any(|a| match a {
                IpAddr::V4(_) => true,
                IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80,
            })
}

/// The non-secret request fields kept in the receipt and the audit record.
fn summary(plan: &Plan) -> Value {
    let addresses: Vec<String> = plan
        .addresses
        .iter()
        .map(|(a, p)| format!("{a}/{p}"))
        .collect();
    json!({"link": plan.link, "file": plan.path, "routable": plan.routable,
           "addresses": addresses, "gateway": plan.gateway, "window_s": plan.window.as_secs()})
}

fn ifindex_of(entry: &Value) -> anyhow::Result<i32> {
    entry
        .get("Index")
        .and_then(Value::as_i64)
        .and_then(|i| i32::try_from(i).ok())
        .ok_or_else(|| anyhow::anyhow!("link has no index"))
}

/// Waits for the link to be healthy, then for its gateway to answer, all
/// inside the plan's window.
async fn prove(
    host: &Host,
    networkd: &Networkd,
    plan: &Plan,
    ifindex: i32,
) -> anyhow::Result<(bool, Value)> {
    let started = tokio::time::Instant::now();
    let (passed, last) = networkd
        .wait_for(&host.root, &plan.link, plan.window, |e| {
            healthy(plan, e).is_some()
        })
        .await?;
    if !passed {
        return Ok((false, last));
    }
    let Some(Some(gateway)) = healthy(plan, &last) else {
        return Ok((true, last));
    };
    let left = plan
        .window
        .saturating_sub(started.elapsed())
        .max(Duration::from_secs(2));
    Ok((
        neigh::reachable(&plan.link, ifindex, gateway, left).await,
        last,
    ))
}

/// Runs the transaction. Called with the write lock held, in a task that
/// outlives the request, so a caller that hangs up never interrupts a
/// rollback.
pub async fn apply(host: Host, plan: Plan, origin: Origin) -> Result<Value, Value> {
    let internal = |e: anyhow::Error| json!({"status":"internal","message":format!("{e:#}")});
    let current = networkd::describe(&host.root, &plan.link)
        .await
        .map_err(internal)?
        .ok_or_else(|| json!({"status":"no_such_link","link":plan.link}))?;
    if let Some(file) = current.get("NetworkFile").and_then(Value::as_str)
        && !is_owned_path(file)
    {
        return Err(json!({"status":"foreign_config","link":plan.link,"file":file}));
    }
    if configured_by_someone_else(&current) {
        return Err(json!({"status":"unmanaged_configured","link":plan.link,
            "message":"the link carries addresses no networkd file set; hostd can't restore them"}));
    }
    let ifindex = ifindex_of(&current).map_err(internal)?;

    let networkd = Networkd::connect().await.map_err(internal)?;
    let mut receipt = Receipt::new(COMMAND, &origin, summary(&plan));
    // A file that can't be backed up stops the change before anything moves.
    let mut change = FileChange::backup(&plan.path).map_err(internal)?;
    change.installed = Some(receipt::digest(plan.text.as_bytes()));
    receipt.files.push(change);
    txn::begin(&host, &receipt)?;

    let attempt = async {
        receipt::write_atomic(&plan.path, plan.text.as_bytes(), 0o644)?;
        networkd.reload_link(ifindex).await?;
        prove(&host, &networkd, &plan, ifindex).await
    }
    .await;

    let (passed, last, reason) = match attempt {
        Ok((true, last)) => (true, last, None),
        Ok((false, last)) => (
            false,
            last,
            Some("health check did not pass in time".to_owned()),
        ),
        Err(e) => (false, Value::Null, Some(format!("{e:#}"))),
    };
    txn::conclude(&host, &mut receipt, passed, reason).await?;
    let reply = json!({
        "receipt": receipt.id,
        "outcome": receipt.outcome,
        "reason": receipt.reason,
        "link": {"admin": last.get("AdministrativeState"), "operational": last.get("OperationalState")},
    });
    if passed {
        Ok(reply)
    } else {
        Err(json!({"status":"rolled_back", "detail": reply}))
    }
}

fn receipt_link(receipt: &Receipt) -> anyhow::Result<&str> {
    receipt
        .summary
        .get("link")
        .and_then(Value::as_str)
        .filter(|l| valid_link(l))
        .ok_or_else(|| anyhow::anyhow!("receipt names no link"))
}

/// Makes networkd use the link's restored file (`txn::put_back`). A link
/// that has gone away needs nothing.
pub async fn reapply(host: &Host, receipt: &Receipt) -> anyhow::Result<()> {
    let link = receipt_link(receipt)?;
    let networkd = Networkd::connect().await?;
    if let Some(entry) = networkd::describe(&host.root, link).await? {
        networkd.reload_link(ifindex_of(&entry)?).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(value: Value) -> Request {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn link_names_are_checked() {
        assert!(valid_link("eth0"));
        assert!(valid_link("wg-home.1"));
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "a b",
            "eth0\n",
            "x:1",
            "abcdefghijklmnop",
        ] {
            assert!(!valid_link(bad), "{bad:?}");
        }
    }

    #[test]
    fn static_plan_renders_typed_fields_only() {
        let p = plan(
            std::path::Path::new("/"),
            request(json!({"link":"hd0","config":{"mode":"static",
                "addresses":["192.0.2.10/24","2001:db8::10/64"],"gateway":"192.0.2.1","dns":["192.0.2.53"]}})),
        )
        .unwrap();
        assert_eq!(
            p.path,
            PathBuf::from("/etc/systemd/network/50-hostd-hd0.network")
        );
        assert!(p.text.contains("[Match]\nName=hd0\n"));
        assert!(p.text.contains(
            "Address=192.0.2.10/24\nAddress=2001:db8::10/64\nGateway=192.0.2.1\nDNS=192.0.2.53\n"
        ));
        assert!(p.routable);
        assert_eq!(p.window, Duration::from_secs(20));
    }

    #[test]
    fn plans_refuse_bad_input() {
        let root = std::path::Path::new("/");
        let refused = [
            json!({"link":"../x","config":{"mode":"dhcp"}}),
            json!({"link":"hd0","config":{"mode":"static","addresses":[]}}),
            json!({"link":"hd0","config":{"mode":"static","addresses":["192.0.2.10"]}}),
            json!({"link":"hd0","config":{"mode":"static","addresses":["192.0.2.10/33"]}}),
            json!({"link":"hd0","config":{"mode":"static","addresses":["0.0.0.0/0"]}}),
            json!({"link":"hd0","config":{"mode":"static","addresses":["192.0.2.10/24\nDNS=1.1.1.1"]}}),
            json!({"link":"hd0","config":{"mode":"dhcp"},"window_s":1}),
            json!({"link":"hd0","config":{"mode":"dhcp"},"window_s":3600}),
        ];
        for value in refused {
            assert!(plan(root, request(value.clone())).is_err(), "{value}");
        }
        // Unknown fields never reach a plan.
        assert!(
            serde_json::from_value::<Request>(
                json!({"link":"hd0","config":{"mode":"dhcp","raw":"x"}})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<Request>(json!({"link":"hd0","config":{"mode":"shell"}}))
                .is_err()
        );
    }

    #[test]
    fn health_needs_our_file_the_addresses_and_the_right_state() {
        let p = plan(
            std::path::Path::new("/"),
            request(json!({"link":"hd0","config":{"mode":"static","addresses":["192.0.2.10/24"]}})),
        )
        .unwrap();
        let entry = |file: &str, admin: &str, oper: &str, addr: [u8; 4]| {
            json!({"NetworkFile": file, "AdministrativeState": admin, "OperationalState": oper,
                   "Addresses": [{"Address": addr, "PrefixLength": 24}]})
        };
        let ours = "/etc/systemd/network/50-hostd-hd0.network";
        let ok = |e: Value| healthy(&p, &e).is_some();
        assert!(ok(entry(ours, "configured", "degraded", [192, 0, 2, 10])));
        assert!(!ok(entry(ours, "configuring", "degraded", [192, 0, 2, 10])));
        assert!(!ok(entry(ours, "configured", "degraded", [192, 0, 2, 11])));
        assert!(!ok(entry(
            "/etc/systemd/network/hd0.network",
            "configured",
            "degraded",
            [192, 0, 2, 10]
        )));
        assert!(!ok(entry(ours, "configured", "carrier", [192, 0, 2, 10])));
        // The right address with the wrong prefix is not the plan.
        let mut wrong_prefix = entry(ours, "configured", "degraded", [192, 0, 2, 10]);
        wrong_prefix["Addresses"][0]["PrefixLength"] = json!(16);
        assert!(!ok(wrong_prefix));
    }

    #[test]
    fn a_gateway_must_be_the_installed_default_route() {
        let p = plan(
            std::path::Path::new("/"),
            request(json!({"link":"hd0","config":{"mode":"static",
                "addresses":["192.0.2.10/24"],"gateway":"192.0.2.1"}})),
        )
        .unwrap();
        let with_route = |gw: [u8; 4], state: &str| {
            json!({"NetworkFile": "/etc/systemd/network/50-hostd-hd0.network",
                   "AdministrativeState": "configured", "OperationalState": "routable",
                   "Addresses": [{"Address": [192, 0, 2, 10], "PrefixLength": 24}],
                   "Routes": [{"Destination": [0,0,0,0], "DestinationPrefixLength": 0,
                               "Gateway": gw, "ConfigState": state}]})
        };
        assert_eq!(
            healthy(&p, &with_route([192, 0, 2, 1], "configured")),
            Some(Some("192.0.2.1".parse().unwrap()))
        );
        assert_eq!(healthy(&p, &with_route([192, 0, 2, 9], "configured")), None);
        assert_eq!(
            healthy(&p, &with_route([192, 0, 2, 1], "configuring")),
            None
        );
        let mut no_routes = with_route([192, 0, 2, 1], "configured");
        no_routes["Routes"] = json!([]);
        assert_eq!(healthy(&p, &no_routes), None);
    }

    #[test]
    fn hand_configured_links_are_not_taken_over() {
        let link_local = json!({"Addresses": [{"Address": [254,128,0,0,0,0,0,0,0,0,0,0,0,0,0,1]}]});
        assert!(!configured_by_someone_else(&link_local));
        let manual = json!({"Addresses": [{"Address": [198, 51, 100, 7]}]});
        assert!(configured_by_someone_else(&manual));
        let ours = json!({"NetworkFile": "/etc/systemd/network/50-hostd-x.network",
                          "Addresses": [{"Address": [198, 51, 100, 7]}]});
        assert!(!configured_by_someone_else(&ours));
        assert!(!configured_by_someone_else(&json!({})));
    }

    #[test]
    fn route_metric_renders_a_route_section_after_network() {
        let p = plan(
            std::path::Path::new("/"),
            request(json!({"link":"hd0","config":{"mode":"static",
                "addresses":["192.0.2.10/24"],"gateway":"192.0.2.1","dns":["192.0.2.53"],
                "route_metric":4242}})),
        )
        .unwrap();
        assert!(!p.text.contains("\nGateway=192.0.2.1\nDNS"));
        assert!(
            p.text
                .ends_with("DNS=192.0.2.53\n\n[Route]\nGateway=192.0.2.1\nMetric=4242\n")
        );
        let d = plan(
            std::path::Path::new("/"),
            request(json!({"link":"hd0","config":{"mode":"dhcp","route_metric":600}})),
        )
        .unwrap();
        assert!(d.text.contains("[DHCPv4]\nRouteMetric=600\n"));
        let no_gw = request(json!({"link":"hd0","config":{"mode":"static",
            "addresses":["192.0.2.10/24"],"route_metric":10}}));
        assert!(plan(std::path::Path::new("/"), no_gw).is_err());
    }

    #[test]
    fn a_gateway_needs_an_address_of_its_family() {
        let refused = request(json!({"link":"hd0","config":{"mode":"static",
            "addresses":["2001:db8::10/64"],"gateway":"192.0.2.1"}}));
        assert!(plan(std::path::Path::new("/"), refused).is_err());
    }
}
