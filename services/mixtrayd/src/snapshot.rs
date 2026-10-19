//! Builds the `mixtrayd.snapshot` reply: one object per popup tab. Each tab
//! carries its own `error`, so one failing source never blanks the others.

use std::path::PathBuf;

use bus::native_client::NodedClient;
use serde_json::{Value, json};

use crate::units;

/// Where the MixOS mix binary lives in the gold copy.
const MIX_BIN: &str = "/opt/mixos/bin/mix";

pub async fn build(client: &NodedClient) -> Value {
    let cos = tab_cos().await;
    let bus = tab_bus(client).await;
    let mix = tab_mix().await;
    let ssh = tab_ssh().await;
    json!({ "cos": cos, "bus": bus, "mix": mix, "ssh": ssh })
}

/// COS: the MixOS units and their systemd state.
async fn tab_cos() -> Value {
    match tokio::task::spawn_blocking(units::probe_all).await {
        Ok(units) => json!({ "units": units }),
        Err(error) => json!({ "units": [], "error": error.to_string() }),
    }
}

/// BUS: the services noded has registered, and the mesh peers it knows.
async fn tab_bus(client: &NodedClient) -> Value {
    let services = client.call("noded", "noded.list", Value::Null).await;
    let peers = client.call("noded", "noded.peers", Value::Null).await;
    let mut tab = json!({});
    match services {
        Ok(value) => tab["services"] = value,
        Err(error) => tab["error"] = json!(error.to_string()),
    }
    match peers {
        // noded wraps the peers in an authority record; the list is its routing view.
        Ok(value) => tab["peers"] = peer_list(value),
        // Peers are optional detail; a noded without the verb still shows services.
        Err(error) => tab["peers_error"] = json!(error.to_string()),
    }
    tab
}

/// The peer list from `noded.peers`. The reply holds an `authority` record
/// whose `routing_view` is the list; a bare list passes through unchanged.
pub fn peer_list(value: Value) -> Value {
    if value.is_array() {
        return value;
    }
    match value.pointer("/authority/routing_view") {
        Some(view @ Value::Array(_)) => view.clone(),
        _ => json!([]),
    }
}

/// MIX: the installed mix binary and its version. Scripts, runs and trash are
/// cosmix hub features with no MixOS source yet, so they are not reported.
async fn tab_mix() -> Value {
    let result = tokio::task::spawn_blocking(|| {
        let path = PathBuf::from(MIX_BIN);
        if !path.exists() {
            return json!({ "path": MIX_BIN, "error": "mix is not installed" });
        }
        match std::process::Command::new(&path).arg("--version").output() {
            Ok(out) if out.status.success() => json!({
                "path": MIX_BIN,
                "version": String::from_utf8_lossy(&out.stdout).trim(),
            }),
            Ok(out) => json!({
                "path": MIX_BIN,
                "error": format!("mix --version exited {}", out.status),
            }),
            Err(error) => json!({ "path": MIX_BIN, "error": error.to_string() }),
        }
    })
    .await;
    result.unwrap_or_else(|error| json!({ "error": error.to_string() }))
}

/// SSH: the concrete `Host` aliases in the user's ssh config.
async fn tab_ssh() -> Value {
    let result = tokio::task::spawn_blocking(|| {
        let Some(home) = std::env::var_os("HOME") else {
            return json!({ "error": "HOME is not set" });
        };
        let path = PathBuf::from(home).join(".ssh/config");
        match std::fs::read_to_string(&path) {
            Ok(text) => json!({ "hosts": ssh_hosts(&text) }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({ "hosts": [] }),
            Err(error) => json!({ "hosts": [], "error": error.to_string() }),
        }
    })
    .await;
    result.unwrap_or_else(|error| json!({ "error": error.to_string() }))
}

/// The concrete host aliases in an ssh config. Patterns (`*`, `?`) and
/// negations (`!`) are skipped, since they name no host to connect to.
pub fn ssh_hosts(text: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let mut words = line.split_whitespace();
        let Some(key) = words.next() else { continue };
        if !key.eq_ignore_ascii_case("host") {
            continue;
        }
        for word in words {
            if word.contains(['*', '?', '!']) {
                continue;
            }
            if !hosts.iter().any(|h| h == word) {
                hosts.push(word.to_string());
            }
        }
    }
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_hosts_keeps_concrete_aliases_and_skips_patterns() {
        let config = "\
# comment
Host alpha beta
    HostName 192.0.2.10
Host *
    ForwardAgent no
Host gamma !delta
Host web-?
host alpha
";
        assert_eq!(ssh_hosts(config), vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn ssh_hosts_of_an_empty_config_is_empty() {
        assert!(ssh_hosts("").is_empty());
    }

    #[test]
    fn peer_list_takes_the_routing_view_of_an_authority_record() {
        let reply = json!({
            "authority": {
                "epoch": 21,
                "routing_view": [{"name": "alpha"}, {"name": "beta"}],
            },
        });
        assert_eq!(peer_list(reply), json!([{"name": "alpha"}, {"name": "beta"}]));
    }

    #[test]
    fn peer_list_passes_a_bare_list_through_and_drops_anything_else() {
        assert_eq!(peer_list(json!([{"name": "x"}])), json!([{"name": "x"}]));
        assert_eq!(peer_list(json!({"unexpected": true})), json!([]));
    }
}
