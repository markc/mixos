// SPDX-License-Identifier: MIT OR Apache-2.0
//! V1 placement through compd's existing, generation-fenced Bus commands.
use bus::native_client::SupervisedClient;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

pub(super) struct Placement {
    service: String,
    pending: BTreeMap<String, Value>,
}

impl Placement {
    pub fn new() -> Self {
        Self {
            service: std::env::var("MIXOS_COMP_SERVICE").unwrap_or_else(|_| "comp".into()),
            pending: BTreeMap::new(),
        }
    }

    pub fn topic(&self) -> String {
        format!("{}.props.changed", self.service)
    }

    pub async fn capture(&self, client: &SupervisedClient) -> Option<Value> {
        // No Bus command resolves portal xdg-foreign exports or X11 ids yet.
        // Capture the focused output BEFORE mapping our own chooser changes it.
        let snapshot = call(client, &self.service, "comp.props.get", json!({})).await?;
        focused(&snapshot)
    }

    pub fn opened(&mut self, request: String, placement: Option<Value>) {
        if let Some(placement) = placement {
            self.pending.insert(request, placement);
        }
    }

    pub fn closed(&mut self, request: &str) {
        self.pending.remove(request);
    }

    pub async fn changed(&mut self, client: &SupervisedClient) {
        if self.pending.is_empty() {
            return;
        }
        let Some(catalogue) = call(client, &self.service, "comp.windows.list", json!({})).await
        else {
            return;
        };
        let Some(windows) = catalogue["windows"].as_array() else {
            return;
        };
        let ids: Vec<_> = self.pending.keys().cloned().collect();
        for request in ids {
            let app_id = format!("dev.mixos.dopus.choose.{request}");
            let Some(window) = windows.iter().find(|w| {
                w["app_id"] == app_id && w["pid"] == std::process::id() && w["mapped"] != false
            }) else {
                continue;
            };
            let mut place = self.pending.remove(&request).unwrap();
            place["id"] = window["id"].clone();
            place["generation"] = window["generation"].clone();
            let target = json!({"id":window["id"], "generation":window["generation"]});
            if call(client, &self.service, "comp.window.place", place)
                .await
                .is_none()
            {
                tracing::warn!(request, "chooser focused-output placement failed");
            }
            let _ = call(client, &self.service, "comp.window.raise", target).await;
        }
    }
}

async fn call(
    client: &SupervisedClient,
    service: &str,
    command: &str,
    body: Value,
) -> Option<Value> {
    let (rc, body, _) = tokio::time::timeout(
        Duration::from_millis(700),
        client.call_with_headers_raw(service, command, &BTreeMap::new(), &body.to_string()),
    )
    .await
    .ok()?
    .ok()?;
    (rc == 0)
        .then(|| serde_json::from_str(&body).ok())
        .flatten()
}

fn focused(snapshot: &Value) -> Option<Value> {
    let outputs = snapshot["outputs"].as_object()?;
    let focused = snapshot["focus"]["window"]["id"]
        .as_u64()
        .and_then(|id| snapshot["windows"][format!("s{id}")]["output"].as_str());
    let pointer = snapshot["input"]["seats"]
        .as_object()
        .and_then(|seats| seats.values().find_map(|s| s["pointer"]["output"].as_str()));
    let (key, output) = focused
        .and_then(|key| outputs.get_key_value(key))
        .or_else(|| pointer.and_then(|name| outputs.iter().find(|(_, o)| o["name"] == name)))
        .or_else(|| outputs.iter().find(|(_, o)| o["default"] == true))?;
    let width = output["width"].as_f64()?;
    let height = output["height"].as_f64()?;
    let w = width.min(820.0);
    let h = height.min(600.0);
    Some(
        json!({"output":key, "x":(width-w)/2.0, "y":(height-h)/2.0,"width":w as u32,"height":h as u32}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn centre_is_output_local_and_uses_the_focused_window() {
        let snapshot = json!({"focus":{"window":{"id":7}}, "windows":{"s7":{"output":"o_right"}},
            "outputs":{"o_left":{"name":"left","default":true,"width":1280,"height":720},"o_right":{"name":"right","width":1920,"height":1080,"x":1280}}});
        assert_eq!(
            focused(&snapshot).unwrap(),
            json!({"output":"o_right","x":550.0,"y":240.0,"width":820,"height":600})
        );
    }
}
