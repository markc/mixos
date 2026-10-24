// SPDX-License-Identifier: MIT OR Apache-2.0
//! Workshop effects use citizen's existing connection and the shared macro Session.
use inspector::bus::Handle;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub async fn perform(
    bus: Handle,
    operation: &str,
    mut args: Value,
    running: BTreeMap<String, String>,
) -> Result<Value, String> {
    if operation == "catalogues" {
        let local = tokio::task::spawn_blocking(|| {
            let roots = macros::Roots::resolve().ok_or("macro roots unavailable")?;
            let library = macros::Library::scan(&roots).map_err(|error| error.to_string())?;
            let mut catalogues = serde_json::Map::new();
            let session = macros::Session::for_app("workshop");
            let global = macros::verbs::handle(&session, macros::verbs::Caller::Workshop, "reload", &json!({}))
                .wait()
                .map_err(|e| e.to_string())?;
            catalogues.insert("global".into(), global);
            for (app, catalogue) in library.apps {
                if !macros::valid_destination(&app) {
                    continue;
                }
                let session = macros::Session::with_roots(&app, Some(roots.clone()));
                session.replace_catalogue(catalogue);
                let mut list = macros::verbs::handle(&session, macros::verbs::Caller::Workshop, "list", &json!({}))
                    .wait()
                    .map_err(|e| e.to_string())?;
                list["running"] = json!(false);
                catalogues.insert(app, list);
            }
            Ok::<_, String>(catalogues)
        })
        .await
        .map_err(|error| error.to_string())??;
        let mut catalogues = local;
        for (app, service) in &running {
            if !macros::valid_destination(app) {
                continue;
            }
            let reply = bus
                .call(service, &format!("{app}.macros.list"), json!({}))
                .await?;
            if reply.rc == 0 {
                let mut list: Value =
                    serde_json::from_str(&reply.body).map_err(|e| e.to_string())?;
                list["running"] = json!(true);
                catalogues.insert(app.clone(), list);
            } else {
                catalogues.insert(
                    app.clone(),
                    json!({"running":true,"macros":[],"error":reply.body}),
                );
            }
        }
        return Ok(Value::Object(catalogues));
    }
    if operation == "observe" {
        return observe(&bus, &args).await;
    }
    if args.get("app").is_some_and(|app| !app.is_string()) {
        return Ok(
            json!({"error_code":"INVALID_ARGUMENT","message":"app must be a component name or global"}),
        );
    }
    let app = args["app"].as_str().unwrap_or("global").to_owned();
    if app != "global"
        && (!app
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase())
            || !app
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
    {
        return Ok(json!({"error_code":"INVALID_ARGUMENT","message":"invalid app"}));
    }
    let suffix = operation
        .strip_prefix("macros.")
        .unwrap_or(operation)
        .to_owned();
    if suffix == "open"
        && (!args["app"].is_string()
            || args
                .as_object()
                .is_none_or(|args| args.keys().any(|key| !matches!(key.as_str(), "app" | "id"))))
    {
        return Ok(
            json!({"error_code":"INVALID_ARGUMENT","message":"expected app and optional macro id"}),
        );
    }
    if suffix == "open" && args["id"].is_null() {
        return Ok(json!({"branch":app}));
    }
    if matches!(suffix.as_str(), "ced" | "dopus") {
        let owner = if app == "global" { "workshop" } else { &app }.to_owned();
        let id = args["id"].as_str().unwrap_or_default().to_owned();
        let operation = suffix.clone();
        let result = tokio::task::spawn_blocking(move || {
            let session = macros::Session::with_roots(&owner, macros::Roots::resolve());
            let catalogue = session.reload().map_err(|e| e.to_string())?;
            let entry = catalogue
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .ok_or("unknown macro id")?;
            let program = if operation == "ced" {
                "/opt/mixos/bin/ced"
            } else {
                "/opt/mixos/bin/dopus"
            };
            let path = if operation == "ced" {
                entry.path.as_path()
            } else {
                entry.path.parent().ok_or("macro has no parent")?
            };
            let mut child = std::process::Command::new(program)
                .arg(path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok::<_, String>(json!({"opened":true}))
        })
        .await
        .map_err(|e| e.to_string())??;
        return Ok(result);
    }
    if let Some(object) = args.as_object_mut() {
        object.remove("app");
    }
    if suffix == "open" {
        let mut document = request_macro(&bus, &app, "get", args, &running).await?;
        if document.get("error_code").is_none() {
            document["catalogue"] = request_macro(&bus, &app, "list", json!({}), &running).await?;
            document["catalogue"]["running"] = json!(running.contains_key(&app));
        }
        return Ok(document);
    }
    let destination = if suffix == "move" {
        if args["tier"] == "user-global" {
            Some("global".to_owned())
        } else {
            args["destination"].as_str().map(str::to_owned)
        }
    } else {
        None
    };
    let answer = request_macro(&bus, &app, &suffix, args, &running).await?;
    if answer.get("error_code").is_none()
        && let Some(destination) = destination
    {
        let refreshed = request_macro(&bus, &destination, "reload", json!({}), &running).await?;
        if refreshed.get("error_code").is_some() {
            return Ok(refreshed);
        }
    }
    Ok(answer)
}

async fn observe(bus: &Handle, args: &Value) -> Result<Value, String> {
    observe_with(args, |verb, args| bus.call("noded", verb, args)).await
}

async fn observe_with<F, Fut>(args: &Value, mut call: F) -> Result<Value, String>
where
    F: FnMut(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<inspector::bus::Reply, String>>,
{
    let invalid = !args["on"].is_boolean()
        || args.as_object().is_none_or(|args| {
            args.keys()
                .any(|key| !matches!(key.as_str(), "on" | "filter" | "body" | "subscription_id"))
        })
        || args.get("body").is_some_and(|body| !body.is_string())
        || args.get("filter").is_some_and(|filter| !filter.is_object());
    let body = args["body"].as_str().unwrap_or("redacted");
    if invalid || !matches!(body, "none" | "redacted") {
        return Ok(
            json!({"error_code":"INVALID_ARGUMENT","message":"expected on, optional filter object and body none or redacted"}),
        );
    }
    let on = args["on"] == true;
    // Establish the replacement before stopping the previous stream. Invalid
    // filters or admission refusals leave an existing observer intact.
    let started = if on {
        let reply = call("noded.observe.start", json!({"filter":args.get("filter").cloned().unwrap_or(json!({})),"body":body,"capacity":1024})).await?;
        if reply.rc != 0 {
            return Ok(
                json!({"error_code":"UNAVAILABLE","message":format!("{} {}",reply.error.unwrap_or_default(),reply.body)}),
            );
        }
        serde_json::from_str::<Value>(&reply.body).map_err(|error| error.to_string())?
    } else {
        json!({"stopped":true})
    };
    if let Some(subscription) = args["subscription_id"].as_str() {
        let stopped = call(
            "noded.observe.stop",
            json!({"subscription_id":subscription}),
        )
        .await;
        let failure = match stopped {
            Ok(reply) if reply.rc == 0 => None,
            Ok(reply) => Some(reply.body),
            Err(error) => Some(error.to_string()),
        };
        if let Some(error) = failure {
            if on {
                let cleanup = call(
                    "noded.observe.stop",
                    json!({"subscription_id":started["subscription_id"]}),
                )
                .await;
                if !matches!(cleanup, Ok(ref reply) if reply.rc == 0) {
                    // Transport failure can leave completion unknown. Retain
                    // the replacement id so it can still be stopped/retried.
                    let mut started = started;
                    started["warning"] = json!(format!(
                        "observe stop and replacement cleanup failed: {error}"
                    ));
                    return Ok(started);
                }
            }
            return Ok(json!({"error_code":"UNAVAILABLE","message":error}));
        }
    }
    Ok(started)
}

async fn request_macro(
    bus: &Handle,
    app: &str,
    suffix: &str,
    args: Value,
    running: &BTreeMap<String, String>,
) -> Result<Value, String> {
    if app != "global" && running.contains_key(app) {
        let reply = bus
            .call(&running[app], &format!("{app}.macros.{suffix}"), args)
            .await?;
        return serde_json::from_str(&reply.body).map_err(|e| format!("{}: {e}", reply.body));
    }
    if app != "global" && matches!(suffix, "run" | "stop" | "output" | "status") {
        return Ok(json!({"error_code":"UNAVAILABLE","message":format!("{app} is not running")}));
    }
    let suffix = suffix.to_owned();
    let app = app.to_owned();
    tokio::task::spawn_blocking(move || {
        let session = if app == "global" {
            macros::Session::for_app("workshop")
        } else {
            macros::Session::with_roots(&app, macros::Roots::resolve())
        };
        match macros::verbs::handle(&session, macros::verbs::Caller::Workshop, &suffix, &args).wait() {
            Ok(answer) => Ok(answer),
            Err(error) => Ok(json!({"error_code":error.code,"message":error.message})),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn failed_stop_cleans_up_the_replacement() {
        let mut calls = Vec::new();
        let answer = observe_with(&json!({"on":true,"subscription_id":"old"}), |verb, args| {
            calls.push((verb, args.clone()));
            let reply = if verb == "noded.observe.start" {
                Ok(inspector::bus::Reply {
                    rc: 0,
                    body: json!({"subscription_id":"new","body":"redacted"}).to_string(),
                    error: None,
                })
            } else if args["subscription_id"] == "old" {
                Err("forced stop failure".into())
            } else {
                Ok(inspector::bus::Reply {
                    rc: 0,
                    body: "{}".into(),
                    error: None,
                })
            };
            std::future::ready(reply)
        })
        .await
        .unwrap();
        assert_eq!(answer["error_code"], "UNAVAILABLE");
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[2].0, "noded.observe.stop");
        assert_eq!(calls[2].1["subscription_id"], "new");
    }
    #[test]
    fn observation_refusal_is_visible_and_leaves_topics_available() {
        let broker = test_broker::Broker::start();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (allowed, mut observed) =
            inspector::bus::start("workshop-observe-test", &broker.url, &[]).unwrap();
        let (denied, _) = inspector::bus::start("workshop-denied-test", &broker.url, &[]).unwrap();
        runtime.block_on(async {
            let started = perform(
                allowed.clone(),
                "observe",
                json!({"on":true}),
                BTreeMap::new(),
            )
            .await
            .unwrap();
            assert_eq!(started["body"], "redacted");
            assert!(started["subscription_id"].is_string());
            let invalid = perform(
                allowed.clone(),
                "observe",
                json!({"on":true,"body":true,"subscription_id":started["subscription_id"]}),
                BTreeMap::new(),
            )
            .await
            .unwrap();
            assert_eq!(invalid["error_code"], "INVALID_ARGUMENT");
            allowed
                .call("noded", "noded.ping", json!({}))
                .await
                .unwrap();
            use futures::StreamExt;
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if let Some(inspector::bus::Delivery::Command { id, verb, body }) =
                        observed.next().await
                    {
                        if verb == "noded.observe.event" {
                            assert_eq!(id, 0, "one-way events must not occupy reply slots");
                            let row: Value = serde_json::from_str(&body).unwrap();
                            if row["verb"] == "noded.ping" {
                                break row;
                            }
                        } else {
                            allowed.reply(id, 0, json!({}));
                        }
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(event["verb"], "noded.ping");
            let replacement = perform(
                allowed.clone(),
                "observe",
                json!({"on":true,"body":"none","subscription_id":started["subscription_id"]}),
                BTreeMap::new(),
            )
            .await
            .unwrap();
            assert_eq!(replacement["body"], "none");
            let stopped = perform(
                allowed.clone(),
                "observe",
                json!({"on":false,"subscription_id":replacement["subscription_id"]}),
                BTreeMap::new(),
            )
            .await
            .unwrap();
            assert_eq!(stopped["stopped"], true);
            let refused = perform(
                denied.clone(),
                "observe",
                json!({"on":true}),
                BTreeMap::new(),
            )
            .await
            .unwrap();
            assert_eq!(refused["error_code"], "UNAVAILABLE");
            assert!(
                refused["message"]
                    .as_str()
                    .unwrap()
                    .contains("observe_unauthorised")
            );
            denied.subscribe("ced.command.executed").await.unwrap();
            denied.unsubscribe("ced.command.executed").await.unwrap();
            for args in [
                json!({}),
                json!({"on":true,"body":"raw"}),
                json!({"on":true,"extra":true}),
                json!({"on":false,"body":false}),
                json!({"on":true,"filter":false}),
            ] {
                assert_eq!(
                    perform(denied.clone(), "observe", args, BTreeMap::new())
                        .await
                        .unwrap()["error_code"],
                    "INVALID_ARGUMENT"
                );
            }
            let stopped_app = perform(
                denied.clone(),
                "macros.run",
                json!({"app":"ced","id":"test"}),
                BTreeMap::new(),
            )
            .await
            .unwrap();
            assert_eq!(
                stopped_app,
                json!({"error_code":"UNAVAILABLE","message":"ced is not running"})
            );
            for args in [
                json!({}),
                json!({"app":true}),
                json!({"app":"Ced","id":"x"}),
                json!({"app":"ced","extra":true}),
            ] {
                assert_eq!(
                    perform(denied.clone(), "macros.open", args, BTreeMap::new())
                        .await
                        .unwrap()["error_code"],
                    "INVALID_ARGUMENT"
                );
            }
        });
        allowed.quit();
        denied.quit();
        allowed.wait_done().unwrap();
        denied.wait_done().unwrap();
    }
}
