// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Desk power: restart or stop one desk instance, a systemd-nspawn machine on
//! this host. The verbs take no machine name. The desk comes from the
//! caller's own grant: its `desks` limit must name exactly one valid machine,
//! so policy decides which desk a session may restart or stop, and a request
//! can never name another. The host itself is never powered.
use crate::{
    caller::{Caller, Origin},
    policy::Policy,
    receipt::{Outcome, Receipt},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path, time::Duration};

pub const RESTART: &str = "desk.restart";
pub const STOP: &str = "desk.stop";
/// The grant limit that names the desk, one machine per grant.
pub const LIMIT: &str = "desks";
const MACHINECTL_BUDGET: Duration = Duration::from_secs(60);

/// A machine name as nspawn and machinectl take it: at most 32 characters,
/// lower case letters, digits and '-', starting with a letter.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The one desk this caller may act on with `verb`: every matching grant's
/// `desks` limit, which must name exactly one valid machine. Anything else is
/// refused by status, and the status says why.
pub fn target(
    policy: &Policy,
    caller: &Caller,
    verb: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<String, &'static str> {
    let grants = policy
        .grants_for(caller, verb, now)
        .map_err(|_| "not_granted")?;
    let desks: BTreeSet<&str> = grants
        .iter()
        .filter_map(|g| g.limits.get(LIMIT))
        .flatten()
        .map(String::as_str)
        .collect();
    match desks.len() {
        0 => Err("not_granted"),
        1 => {
            let desk = desks.into_iter().next().unwrap_or_default();
            if valid_name(desk) {
                Ok(desk.to_owned())
            } else {
                Err("bad_desk")
            }
        }
        _ => Err("ambiguous_desk"),
    }
}

/// Run one power action on `desk`, then record it. The caller has passed the
/// grant gate with `desks` set to this same name. A power action has no undo,
/// so its receipt is never undoable: `hostd.undo` refuses the verb.
pub async fn act(
    state: &Path,
    verb: &str,
    desk: &str,
    grant: &str,
    origin: &Origin,
) -> Result<Value, Value> {
    let action = if verb == RESTART {
        "reboot"
    } else {
        "poweroff"
    };
    let run = tokio::time::timeout(
        MACHINECTL_BUDGET,
        tokio::process::Command::new("machinectl")
            .arg(action)
            .arg(desk)
            .output(),
    )
    .await
    .map_err(|_| json!({"status":"timeout","verb":verb,"desk":desk}))?
    .map_err(|e| json!({"status":"internal","verb":verb,"message":e.to_string()}))?;
    if !run.status.success() {
        return Err(json!({"status":"machinectl_failed","verb":verb,"desk":desk,
            "message": String::from_utf8_lossy(&run.stderr).trim()}));
    }
    let mut receipt = Receipt::new(verb, grant, origin, json!({"desk": desk, "action": action}));
    receipt.outcome = Outcome::Applied;
    receipt
        .save(state)
        .map_err(|e| json!({"status":"internal","verb":verb,"message":format!("{e:#}")}))?;
    Ok(json!({"desk": desk, "action": action, "receipt": receipt.id}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(cgroup: &str) -> Caller {
        Caller {
            uid: 1002,
            gid: 1002,
            pid: Some(1),
            cgroup: Some(cgroup.into()),
            verified: true,
            host_ns: false,
        }
    }

    fn policy(text: &str) -> Policy {
        let policy: Policy = strict::from_str(text).expect("parses");
        policy.validate().expect("validates");
        policy
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        "2026-10-13T00:00:00Z".parse().unwrap()
    }

    const SESSION: &str = "/system.slice/system-scenes.slice/scenes@4.service";

    #[test]
    fn names_are_machine_names() {
        assert!(valid_name("desk12"));
        assert!(valid_name("desk-2"));
        assert!(!valid_name(""));
        assert!(!valid_name("12desk"));
        assert!(!valid_name("Desk"));
        assert!(!valid_name("desk_12"));
        assert!(!valid_name("desk/12"));
        assert!(!valid_name(&"a".repeat(33)));
    }

    #[test]
    fn the_grant_names_the_one_desk() {
        let p = policy(&format!(
            r#"{{schema: 1, grants: [
              {{name: "topbar", caller: {{cgroup: "{SESSION}"}}, verbs: ["desk.restart", "desk.stop"],
               limits: {{desks: ["desk12"]}}, expires: "2030-01-01T00:00:00Z", rate: {{count: 2, per_s: 60}}}}
            ]}}"#
        ));
        assert_eq!(
            target(&p, &caller(SESSION), RESTART, now()),
            Ok("desk12".into())
        );
        assert_eq!(
            target(&p, &caller(SESSION), STOP, now()),
            Ok("desk12".into())
        );
    }

    #[test]
    fn another_session_gets_no_desk() {
        let p = policy(&format!(
            r#"{{schema: 1, grants: [
              {{name: "topbar", caller: {{cgroup: "{SESSION}"}}, verbs: ["desk.stop"],
               limits: {{desks: ["desk12"]}}, expires: "2030-01-01T00:00:00Z", rate: {{count: 2, per_s: 60}}}}
            ]}}"#
        ));
        let other = "/system.slice/system-apps.slice/apps@4.service";
        assert_eq!(target(&p, &caller(other), STOP, now()), Err("not_granted"));
        // A verb the grant does not list is not granted either.
        assert_eq!(
            target(&p, &caller(SESSION), RESTART, now()),
            Err("not_granted")
        );
    }

    #[test]
    fn a_grant_without_a_desk_or_with_two_is_refused() {
        let none = policy(&format!(
            r#"{{schema: 1, grants: [
              {{name: "topbar", caller: {{cgroup: "{SESSION}"}}, verbs: ["desk.stop"],
               expires: "2030-01-01T00:00:00Z", rate: {{count: 2, per_s: 60}}}}
            ]}}"#
        ));
        assert_eq!(
            target(&none, &caller(SESSION), STOP, now()),
            Err("not_granted")
        );
        let two = policy(&format!(
            r#"{{schema: 1, grants: [
              {{name: "topbar", caller: {{cgroup: "{SESSION}"}}, verbs: ["desk.stop"],
               limits: {{desks: ["desk12", "desk13"]}}, expires: "2030-01-01T00:00:00Z", rate: {{count: 2, per_s: 60}}}}
            ]}}"#
        ));
        assert_eq!(
            target(&two, &caller(SESSION), STOP, now()),
            Err("ambiguous_desk")
        );
    }

    #[test]
    fn an_invalid_desk_name_in_policy_is_refused() {
        let bad = policy(&format!(
            r#"{{schema: 1, grants: [
              {{name: "topbar", caller: {{cgroup: "{SESSION}"}}, verbs: ["desk.stop"],
               limits: {{desks: ["../host"]}}, expires: "2030-01-01T00:00:00Z", rate: {{count: 2, per_s: 60}}}}
            ]}}"#
        ));
        assert_eq!(target(&bad, &caller(SESSION), STOP, now()), Err("bad_desk"));
    }
}
