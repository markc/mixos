//! Start, stop and restart for the COS tab. A system unit is controlled
//! through hostd's `unit.control` verb: hostd is root, and its grant
//! (`mixtray-units`) names the units this daemon may act on, so no sudo and no
//! polkit prompt is involved. The daemon refuses anything outside the COS list.

use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::units;

/// hostd's local socket. Its peer is this daemon, so the grant's cgroup match
/// is the `mixtrayd.service` cgroup.
pub const HOSTD_SOCKET: &str = "/run/hostd/hostd.sock";
const HOSTD_VERB: &str = "unit.control";
/// hostd waits for the unit job, so this covers its own budget plus a margin.
const HOSTD_BUDGET: Duration = Duration::from_secs(75);
const ACTIONS: &[&str] = &["start", "stop", "restart"];

#[derive(Debug, Deserialize)]
pub struct Request {
    pub unit: String,
    pub action: String,
}

/// Parses and checks a control request, returning the unit and action to run.
pub fn check(args: &Value) -> Result<Request, String> {
    let request: Request =
        serde_json::from_value(args.clone()).map_err(|e| format!("bad request: {e}"))?;
    if !ACTIONS.contains(&request.action.as_str()) {
        return Err(format!(
            "action must be start, stop or restart, not {}",
            request.action
        ));
    }
    if !units::is_controllable(&request.unit) {
        return Err(format!("{} is not controlled from MixTray", request.unit));
    }
    Ok(request)
}

/// Runs one checked request through hostd, returning hostd's reply body on
/// success, or its error body (or a transport message) on failure.
pub async fn run(request: &Request) -> Result<Value, Value> {
    let body = json!({"unit": request.unit, "action": request.action});
    tokio::time::timeout(HOSTD_BUDGET, hostd_call(HOSTD_VERB, body))
        .await
        .unwrap_or_else(|_| Err(json!({"status": "timeout", "message": "hostd did not answer"})))
}

/// One NUL-framed request on hostd's socket. hostd replies `{rc, body}`:
/// rc 0 is success, anything else carries hostd's reason in `body`.
async fn hostd_call(verb: &str, body: Value) -> Result<Value, Value> {
    let transport = |e: String| json!({"status": "transport", "message": e});
    let mut stream = UnixStream::connect(HOSTD_SOCKET)
        .await
        .map_err(|e| transport(format!("cannot reach hostd: {e}")))?;
    let mut frame = serde_json::to_vec(&json!({"verb": verb, "body": body}))
        .map_err(|e| transport(e.to_string()))?;
    frame.push(0);
    stream
        .write_all(&frame)
        .await
        .map_err(|e| transport(e.to_string()))?;
    let mut reader = BufReader::new(stream);
    let mut reply = Vec::new();
    reader
        .read_until(0, &mut reply)
        .await
        .map_err(|e| transport(e.to_string()))?;
    if reply.last() == Some(&0) {
        reply.pop();
    }
    let reply: Value =
        serde_json::from_slice(&reply).map_err(|e| transport(format!("hostd reply: {e}")))?;
    match reply.get("rc").and_then(Value::as_u64) {
        Some(0) => Ok(reply.get("body").cloned().unwrap_or(Value::Null)),
        _ => Err(reply.get("body").cloned().unwrap_or(reply)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checked_request_names_a_controllable_unit_and_an_action() {
        let ok = check(&json!({"unit": "noded.service", "action": "restart"})).expect("ok");
        assert_eq!(ok.unit, "noded.service");
        assert_eq!(ok.action, "restart");
    }

    #[test]
    fn hostd_and_mixtray_itself_are_never_controlled() {
        assert!(check(&json!({"unit": "hostd.service", "action": "restart"})).is_err());
        assert!(check(&json!({"unit": "mixtrayd.service", "action": "stop"})).is_err());
    }

    #[test]
    fn only_the_three_actions_pass() {
        assert!(check(&json!({"unit": "webd.service", "action": "kill"})).is_err());
        assert!(check(&json!({"unit": "webd.service", "action": "start"})).is_ok());
    }

    #[test]
    fn extra_fields_are_ignored_and_a_missing_unit_is_refused() {
        assert!(check(&json!({"action": "start"})).is_err());
        assert!(check(&json!({"unit": "webd.service", "action": "start", "x": 1})).is_ok());
    }
}
