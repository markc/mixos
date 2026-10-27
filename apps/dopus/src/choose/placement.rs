// SPDX-License-Identifier: MIT OR Apache-2.0
//! V1 placement through compd's existing, generation-fenced Bus commands.
use bus::native_client::SupervisedClient;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

pub(super) struct Placement {
    service: String,
    pending: BTreeMap<String, Value>,
    capturing: BTreeSet<String>,
    jobs: tokio::task::JoinSet<Report>,
    catalogue_pending: bool,
}

enum Report {
    Captured(String, Option<Value>),
    Catalogue(Option<Value>),
    Applied,
}

impl Placement {
    pub fn new() -> Self {
        Self {
            service: std::env::var("MIXOS_COMP_SERVICE").unwrap_or_else(|_| "comp".into()),
            pending: BTreeMap::new(),
            capturing: BTreeSet::new(),
            jobs: tokio::task::JoinSet::new(),
            catalogue_pending: false,
        }
    }

    pub fn topic(&self) -> String {
        format!("{}.props.changed", self.service)
    }

    pub fn opened(&mut self, request: String, client: Arc<SupervisedClient>) {
        self.capturing.insert(request.clone());
        let service = self.service.clone();
        self.jobs
            .spawn(async move { Report::Captured(request, capture(&client, &service).await) });
    }

    pub async fn completed(&mut self, client: &Arc<SupervisedClient>) -> Option<String> {
        let report = match self.jobs.join_next().await {
            Some(Ok(report)) => report,
            Some(Err(error)) => {
                tracing::warn!(%error, "chooser placement task failed");
                return None;
            }
            None => return std::future::pending().await,
        };
        match report {
            Report::Captured(id, position) => {
                if !self.capturing.remove(&id) {
                    return None;
                }
                if let Some(position) = position {
                    self.pending.insert(id.clone(), position);
                }
                Some(id)
            }
            Report::Catalogue(catalogue) => {
                self.catalogue_pending = false;
                if let Some(catalogue) = catalogue {
                    self.place(client, &catalogue);
                }
                None
            }
            Report::Applied => None,
        }
    }

    pub fn closed(&mut self, request: &str) {
        self.pending.remove(request);
        self.capturing.remove(request);
    }

    pub fn changed(&mut self, client: Arc<SupervisedClient>) {
        if self.pending.is_empty() || self.catalogue_pending {
            return;
        }
        self.catalogue_pending = true;
        let service = self.service.clone();
        self.jobs.spawn(async move {
            Report::Catalogue(call(&client, &service, "comp.windows.list", json!({})).await)
        });
    }

    fn place(&mut self, client: &Arc<SupervisedClient>, catalogue: &Value) {
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
            let client = client.clone();
            let service = self.service.clone();
            self.jobs.spawn(async move {
                if call(&client, &service, "comp.window.place", place)
                    .await
                    .is_none()
                {
                    tracing::warn!(request, "chooser focused-output placement failed");
                }
                let _ = call(&client, &service, "comp.window.raise", target).await;
                Report::Applied
            });
        }
    }
}

async fn capture(client: &SupervisedClient, service: &str) -> Option<Value> {
    capture_with(|path| call(client, service, "comp.props.get", json!({"path":path}))).await
}

async fn capture_with<F, Fut>(mut get: F) -> Option<Value>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Option<Value>>,
{
    // No Bus command resolves portal xdg-foreign exports or X11 ids yet.
    // Capture the focused output BEFORE mapping our own chooser changes it.
    let id = get("focus.window.id".into()).await;
    let output = if let Some(id) = id.and_then(|id| id.as_u64()) {
        get(format!("windows.s{id}.output")).await
    } else {
        None
    };
    if let Some(key) = output.and_then(|v| v.as_str().map(str::to_owned)) {
        let output = get(format!("outputs.{key}")).await?;
        return centred(&key, &output);
    }
    let seats = get("input.seats".into()).await;
    let pointer = seats
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|seats| seats.values().find_map(|s| s["pointer"]["output"].as_str()));
    let outputs = get("outputs".into()).await?;
    if let Some((key, output)) =
        pointer.and_then(|name| outputs.as_object()?.iter().find(|(_, o)| o["name"] == name))
    {
        return centred(key, output);
    }
    let (key, output) = outputs
        .as_object()?
        .iter()
        .find(|(_, o)| o["default"] == true)?;
    centred(key, output)
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

fn centred(key: &str, output: &Value) -> Option<Value> {
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
    fn capture_reads_only_the_focused_window_and_its_output() {
        let mut paths = Vec::new();
        let result = futures::executor::block_on(capture_with(|path| {
            let value = match path.as_str() {
                "focus.window.id" => json!(7),
                "windows.s7.output" => json!("o_right"),
                "outputs.o_right" => json!({"width":1920,"height":1080}),
                other => panic!("unexpected property path {other}"),
            };
            paths.push(path);
            std::future::ready(Some(value))
        }));
        assert_eq!(
            paths,
            ["focus.window.id", "windows.s7.output", "outputs.o_right"]
        );
        assert_eq!(result.unwrap(), json!({"output":"o_right","x":550.0,"y":240.0,"width":820,"height":600}));
    }

    #[test]
    fn failed_choose_reply_removes_pending_placement_and_capture() {
        struct Prepared;
        impl dopus_core::chooser::Presenter for Prepared {
            fn open(
                &mut self,
                _: &dopus_core::chooser::OpenRequest,
            ) -> Result<(), dopus_core::chooser::Unavailable> {
                Ok(())
            }
            fn input_ready(&mut self, _: &str) {}
            fn close(&mut self, _: &str) {}
        }
        let mut engine =
            dopus_core::chooser::Engine::new(1, super::super::defaults(), b"/tmp".to_vec())
                .unwrap();
        let mut presenter = Prepared;
        let answer = super::super::dispatch(
            &mut engine,
            &mut presenter,
            "files.choose",
            "{}",
            "local:a",
            1,
        );
        let id = answer.enable.clone().unwrap();
        let mut placement = Placement::new();
        let mut mapping = super::super::Mapping::default();
        mapping.opened(&id);
        placement.pending.insert(id.clone(), json!({"output":"o1"}));
        placement.capturing.insert(id);
        futures::executor::block_on(super::super::deliver(
            &mut engine,
            &mut presenter,
            &mut placement,
            &mut mapping,
            answer,
            async { false },
        ));
        assert!(placement.pending.is_empty());
        assert!(placement.capturing.is_empty());
        assert!(mapping.deadlines.is_empty());
        assert_eq!(engine.list()["requests"], json!([]));
    }
    #[test]
    fn capture_uses_pointer_output_before_default_without_a_focused_window() {
        let result = futures::executor::block_on(capture_with(|path| std::future::ready(match path.as_str() {
            "focus.window.id" => Some(Value::Null),
            "input.seats" => Some(json!({"seat0":{"pointer":{"output":"right"}}})),
            "outputs" => Some(json!({"o_left":{"name":"left","default":true,"width":1280,"height":720}, "o_right":{"name":"right","width":1920,"height":1080}})),
            other => panic!("unexpected property path {other}"),
        })));
        assert_eq!(result.unwrap()["output"], "o_right");
    }
}
