// SPDX-License-Identifier: MIT OR Apache-2.0
//! Workshop effects use citizen's existing connection and the shared macro Session.
use inspector::bus::Handle;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub async fn perform(bus: Handle, operation: &str, mut args: Value, running: BTreeMap<String, String>) -> Result<Value, String> {
    if operation == "catalogues" {
        let local = tokio::task::spawn_blocking(|| {
            let roots = macros::Roots::resolve().ok_or("macro roots unavailable")?;
            let library = macros::Library::scan(&roots).map_err(|error| error.to_string())?;
            let mut catalogues = serde_json::Map::new();
            let session = macros::Session::for_app("workshop");
            let global = macros::verbs::handle(&session, "reload", &json!({})).wait().map_err(|e| e.to_string())?;
            catalogues.insert("global".into(), global);
            for (app, catalogue) in library.apps {
                let session = macros::Session::with_roots(&app, Some(roots.clone()));
                session.replace_catalogue(catalogue);
                let mut list = macros::verbs::handle(&session, "list", &json!({})).wait().map_err(|e| e.to_string())?;
                list["running"] = json!(false);
                catalogues.insert(app, list);
            }
            Ok::<_, String>(catalogues)
        }).await.map_err(|error| error.to_string())??;
        let mut catalogues = local;
        for (app, service) in &running {
            bus.subscribe(&format!("{service}.command.executed")).await?;
            bus.subscribe(&format!("{service}.macros.finished")).await?;
            let reply = bus.call(service, &format!("{app}.macros.list"), json!({})).await?;
            if reply.rc == 0 {
                let mut list: Value = serde_json::from_str(&reply.body).map_err(|e| e.to_string())?;
                list["running"] = json!(true);
                catalogues.insert(app.clone(), list);
            } else {
                catalogues.insert(app.clone(), json!({"running":true,"macros":[],"error":reply.body}));
            }
        }
        return Ok(Value::Object(catalogues));
    }
    if operation == "observe" {
        if !args["on"].is_boolean() || args.as_object().is_none_or(|args| args.keys().any(|key| !matches!(key.as_str(), "on"|"filter"|"body"))) {
            return Ok(json!({"error_code":"INVALID_ARGUMENT","message":"expected on, optional filter and body"}));
        }
        let on = args["on"] == true;
        let body = args["body"].as_str().unwrap_or("redacted");
        if !matches!(body, "none"|"redacted") { return Ok(json!({"error_code":"INVALID_ARGUMENT","message":"body must be none or redacted"})); }
        let request = if on { json!({"filter":args.get("filter").cloned().unwrap_or(json!({})),"body":body,"capacity":1024}) }
            else { json!({"subscription_id":args["subscription_id"]}) };
        let reply = bus.call("noded", if on { "noded.observe.start" } else { "noded.observe.stop" }, request).await?;
        if reply.rc != 0 { return Ok(json!({"error_code":"UNAVAILABLE","message":format!("{} {}",reply.error.unwrap_or_default(),reply.body)})); }
        return serde_json::from_str(&reply.body).map_err(|e| e.to_string());
    }
    let app = args["app"].as_str().unwrap_or("global").to_owned();
    if app != "global" && (app.is_empty() || !app.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))) {
        return Ok(json!({"error_code":"INVALID_ARGUMENT","message":"invalid app"}));
    }
    let suffix = operation.strip_prefix("macros.").unwrap_or(operation).to_owned();
    if suffix == "open" && args["id"].is_null() { return Ok(json!({"branch":app})); }
    if matches!(suffix.as_str(), "copy"|"ced"|"dopus") {
        let owner = if app == "global" { "workshop" } else { &app }.to_owned();
        let id = args["id"].as_str().unwrap_or_default().to_owned();
        let operation = suffix.clone();
        let result = tokio::task::spawn_blocking(move || {
            let session = macros::Session::with_roots(&owner, macros::Roots::resolve());
            let catalogue = session.reload().map_err(|e| e.to_string())?;
            let entry = catalogue.entries.iter().find(|entry| entry.id == id).ok_or("unknown macro id")?;
            let roots = session.roots().ok_or("macro roots unavailable")?;
            if operation == "copy" {
                let path = roots.copy_to_user(&owner, entry).map_err(|e| e.to_string())?;
                return Ok::<_, String>(json!({"path":path,"changed":true}));
            }
            let program = if operation == "ced" { "/opt/mixos/bin/ced" } else { "/opt/mixos/bin/dopus" };
            let path = if operation == "ced" { entry.path.as_path() } else { entry.path.parent().ok_or("macro has no parent")? };
            let mut child = std::process::Command::new(program).arg(path)
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
                .spawn().map_err(|e| e.to_string())?;
            std::thread::spawn(move || { let _ = child.wait(); });
            Ok(json!({"opened":true}))
        }).await.map_err(|e| e.to_string())??;
        if suffix == "copy" {
            if let Some(service) = running.get(&app) { let _ = bus.call(service, &format!("{app}.macros.reload"), json!({})).await; }
            else if app == "global" { macros::Session::for_app("workshop").reload().map_err(|e| e.to_string())?; }
        }
        return Ok(result);
    }
    if let Some(object) = args.as_object_mut() { object.remove("app"); }
    let suffix = if suffix == "open" { "get" } else { &suffix };
    if app != "global" && running.contains_key(&app) {
        let reply = bus.call(&running[&app], &format!("{app}.macros.{suffix}"), args).await?;
        return serde_json::from_str(&reply.body).map_err(|e| format!("{}: {e}", reply.body));
    }
    if app != "global" && matches!(suffix, "run"|"stop"|"output"|"status") {
        return Ok(json!({"error_code":"UNAVAILABLE","message":format!("{app} is not running")}));
    }
    let suffix = suffix.to_owned();
    tokio::task::spawn_blocking(move || {
        let session = if app == "global" { macros::Session::for_app("workshop") }
            else { macros::Session::with_roots(&app, macros::Roots::resolve()) };
        match macros::verbs::handle(&session, &suffix, &args).wait() {
            Ok(answer) => Ok(answer),
            Err(error) => Ok(json!({"error_code":error.code,"message":error.message})),
        }
    }).await.map_err(|e| e.to_string())?
}
