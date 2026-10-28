//! Builds the `mixtrayd.snapshot` reply: one object per popup tab. Each tab
//! carries its own `error`, so one failing source never blanks the others.

use std::path::PathBuf;
use std::sync::Arc;

use bus::native_client::NodedClient;
use serde_json::{Value, json};

use crate::ssh::Ssh;
use crate::units;

/// Where the MixOS mix binary lives in the gold copy.
fn mix_bin() -> String {
    ::paths::bin("mix").to_string_lossy().into_owned()
}

pub async fn build(client: &NodedClient, ssh: &Arc<Ssh>) -> Value {
    let cos = tab_cos().await;
    let bus = tab_bus(client).await;
    let mix = tab_mix().await;
    let ssh = tab_ssh(ssh).await;
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
        // Peers are optional detail; a noded without the command still shows services.
        Err(error) => tab["peers_error"] = json!(error.to_string()),
    }
    tab
}

/// The peer list from `noded.peers`. The reply holds an `authority` record
/// whose `routing_view` is the list; a bare list is taken as the list.
/// The routing view lists every member, so retired (tombstoned) and
/// transport-only members are dropped: the tray never shows them.
pub fn peer_list(value: Value) -> Value {
    let list = if value.is_array() {
        value
    } else {
        match value.pointer("/authority/routing_view") {
            Some(view @ Value::Array(_)) => view.clone(),
            _ => return json!([]),
        }
    };
    let shown: Vec<Value> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|peer| {
            peer.get("class")
                .and_then(Value::as_str)
                .is_none_or(|class| class == "active-bus")
        })
        .cloned()
        .collect();
    Value::Array(shown)
}

/// MIX: the installed mix binary and its version. Scripts, runs and trash are
/// predecessor hub features with no MixOS source yet, so they are not reported.
async fn tab_mix() -> Value {
    let result = tokio::task::spawn_blocking(|| {
        let path = PathBuf::from(mix_bin());
        if !path.exists() {
            return json!({ "path": mix_bin(), "error": "mix is not installed" });
        }
        match std::process::Command::new(&path).arg("--version").output() {
            Ok(out) if out.status.success() => json!({
                "path": mix_bin(),
                "version": String::from_utf8_lossy(&out.stdout).trim(),
            }),
            Ok(out) => json!({
                "path": mix_bin(),
                "error": format!("mix --version exited {}", out.status),
            }),
            Err(error) => json!({ "path": mix_bin(), "error": error.to_string() }),
        }
    })
    .await;
    result.unwrap_or_else(|error| json!({ "error": error.to_string() }))
}

/// SSH: the host fragments, the trash, the keys and the latest probes.
async fn tab_ssh(ssh: &Arc<Ssh>) -> Value {
    let ssh = Arc::clone(ssh);
    tokio::task::spawn_blocking(move || ssh.tab())
        .await
        .unwrap_or_else(|error| json!({ "error": error.to_string() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_list_takes_the_routing_view_of_an_authority_record() {
        let reply = json!({
            "authority": {
                "epoch": 21,
                "routing_view": [{"name": "alpha"}, {"name": "beta"}],
            },
        });
        assert_eq!(
            peer_list(reply),
            json!([{"name": "alpha"}, {"name": "beta"}])
        );
    }

    #[test]
    fn peer_list_leaves_out_retired_and_transport_only_members() {
        let reply = json!({
            "authority": {
                "routing_view": [
                    {"class": "active-bus", "name": "alpha"},
                    {"class": "tombstoned", "name": "gamma"},
                    {"class": "transport-only", "name": "delta"},
                ],
            },
        });
        assert_eq!(
            peer_list(reply),
            json!([{"class": "active-bus", "name": "alpha"}])
        );
    }

    #[test]
    fn peer_list_passes_a_bare_list_through_and_drops_anything_else() {
        assert_eq!(peer_list(json!([{"name": "x"}])), json!([{"name": "x"}]));
        assert_eq!(peer_list(json!({"unexpected": true})), json!([]));
    }
}
