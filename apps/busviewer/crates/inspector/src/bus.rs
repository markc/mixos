// SPDX-License-Identifier: MIT OR Apache-2.0
//! BusViewer's Bus: the app connection is the shared `citizen` library
//! (promoted from this file when Prefs became its second owner); what stays
//! here is BusViewer's own discovery of the services and their verbs.
#[cfg(feature = "testing")]
pub use citizen::Logged;
pub use citizen::{CallError, Delivery, Handle, Reply};
use serde_json::Value;

/// The app name: the worker thread, the closing refusal and the
/// `busviewer.ping` / `busviewer.show` activation verbs.
const APP: &str = "busviewer";

/// Register as `service` on the broker at `url`.
pub fn start(
    service: &str,
    url: &str,
) -> Result<(Handle, futures::channel::mpsc::Receiver<Delivery>), String> {
    citizen::start(APP, service, url)
}

/// Is a BusViewer already registered as `service`?
pub fn probe(url: &str, service: &str) -> bool {
    citizen::probe(url, service, APP)
}

/// Ask the running BusViewer to show its window.
pub fn forward(url: &str, service: &str) -> Result<(), String> {
    citizen::forward(url, service, APP)
}

/// Restore and focus this process's window through compd (`comp`).
pub async fn show(handle: Handle, comp: String) -> Result<Value, String> {
    citizen::show(handle, comp, crate::model::APP_ID).await
}

async fn json_call(handle: &Handle, service: &str, verb: &str) -> Result<Value, String> {
    let reply = handle
        .raw(service, verb, String::new())
        .await
        .map_err(|e| e.to_string())?;
    if reply.rc >= 10 {
        return Err(format!("rc = {}: {}", reply.rc, reply.body));
    }
    serde_json::from_str(&reply.body).map_err(|e| e.to_string())
}
async fn describe(handle: &Handle, service: &str) -> Result<Vec<crate::model::Verb>, String> {
    let help = match json_call(handle, service, "HELP").await {
        Ok(value) => crate::model::parse_verbs(&value),
        Err(error) => Err(error),
    };
    match help {
        Ok(verbs) => Ok(verbs),
        Err(help_error) => match json_call(handle, service, "app.describe").await {
            Ok(value) => crate::model::parse_verbs(&value),
            Err(error) => Err(format!("HELP: {help_error}\napp.describe: {error}")),
        },
    }
}
/// Eight descriptions at a time; failed citizens do not stop later probes.
pub async fn discover(handle: Handle) -> crate::model::Snapshot {
    use futures::{StreamExt, stream};
    let mut snapshot = crate::model::Snapshot::default();
    let names = match json_call(&handle, "noded", "noded.list")
        .await
        .and_then(|v| crate::model::services(&v))
    {
        Ok(names) => names,
        Err(error) => {
            snapshot.error = Some(error);
            return snapshot;
        }
    };
    match json_call(&handle, "noded", "noded.peers").await {
        Ok(value) => snapshot.peers = crate::model::peers(&value),
        Err(error) => snapshot.peer_error = Some(error),
    }
    let results = stream::iter(names.into_iter().map(|name| {
        let handle = handle.clone();
        async move {
            let result = describe(&handle, &name).await;
            (name, result)
        }
    }))
    .buffer_unordered(8)
    .collect::<Vec<_>>()
    .await;
    snapshot.services.extend(results);
    snapshot
}
