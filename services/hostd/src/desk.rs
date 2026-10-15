// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Desk power: restart or stop one desk instance, a systemd-nspawn machine on
//! this host. The verbs take no machine name. The desk comes from the
//! caller's own grant: its `desks` limit must name exactly one valid machine,
//! so policy decides which desk a session may restart or stop, and a request
//! can never name another. The host itself is never powered.
//!
//! The work is a full cycle, not one `machinectl` call: a restart stops the
//! session, reboots the machine, starts the session again and puts the
//! console back on the desk's VT; a stop returns the console to the VT that
//! was in front first. The caller's own processes end part way through, and
//! the steps need the console, so hostd runs none of them itself. It starts
//! the host's unit for the action (`desk-restart@DESK.service` or
//! `desk-stop@DESK.service`) and answers once systemd has queued it. The
//! host installs those units; hostd only names them.
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
const SYSTEMCTL: &str = "systemctl";
const SYSTEMCTL_BUDGET: Duration = Duration::from_secs(30);

/// The host unit that runs `verb`'s cycle for `desk`.
pub fn unit(verb: &str, desk: &str) -> String {
    let action = if verb == RESTART { "restart" } else { "stop" };
    format!("desk-{action}@{desk}.service")
}

/// Whether this hostd runs inside a container (a desk) rather than on the
/// host: from there no desk machine is visible, and a desk cannot restart
/// itself and bring its own session back.
pub fn in_container(root: &Path) -> bool {
    root.join("run/systemd/container").exists()
}

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

/// Start one power cycle on `desk`, then record it. The caller has passed the
/// grant gate with `desks` set to this same name. A power action has no undo,
/// so its receipt is never undoable: `hostd.undo` refuses the verb. The
/// receipt records that the cycle started; its unit's journal says how it
/// ended.
pub async fn act(
    root: &Path,
    state: &Path,
    verb: &str,
    desk: &str,
    grant: &str,
    origin: &Origin,
) -> Result<Value, Value> {
    act_with(SYSTEMCTL, root, state, verb, desk, grant, origin).await
}

/// `act`, with the program that starts the unit named: tests pass a
/// stand-in for `systemctl`.
async fn act_with(
    systemctl: &str,
    root: &Path,
    state: &Path,
    verb: &str,
    desk: &str,
    grant: &str,
    origin: &Origin,
) -> Result<Value, Value> {
    if in_container(root) {
        tracing::warn!(target: "hostd::audit", verb, desk, "desk power refused: hostd runs inside a container");
        return Err(json!({"status":"in_container","verb":verb,"desk":desk,
            "message":"this hostd runs inside a desk; desk power needs the host's hostd"}));
    }
    let unit = unit(verb, desk);
    // --no-block: answer once the job is queued. The cycle outlives the
    // caller, whose desk goes down part way through it.
    let run = tokio::time::timeout(
        SYSTEMCTL_BUDGET,
        tokio::process::Command::new(systemctl)
            .args(["start", "--no-block", &unit])
            .output(),
    )
    .await
    .map_err(|_| json!({"status":"timeout","verb":verb,"desk":desk}))?
    .map_err(|e| json!({"status":"internal","verb":verb,"message":e.to_string()}))?;
    if !run.status.success() {
        let message = String::from_utf8_lossy(&run.stderr).trim().to_owned();
        tracing::warn!(target: "hostd::audit", verb, desk, %unit, %message, "desk power unit did not start");
        return Err(json!({"status":"unit_failed","verb":verb,"desk":desk,"unit":unit,
            "message": message}));
    }
    let action = if verb == RESTART { "restart" } else { "stop" };
    let mut receipt = Receipt::new(
        verb,
        grant,
        origin,
        json!({"desk": desk, "action": action, "unit": unit}),
    );
    receipt.outcome = Outcome::Applied;
    receipt
        .save(state)
        .map_err(|e| json!({"status":"internal","verb":verb,"message":format!("{e:#}")}))?;
    tracing::info!(target: "hostd::audit", verb, desk, %unit, receipt = %receipt.id, "desk power cycle started");
    Ok(json!({"desk": desk, "action": action, "unit": unit, "receipt": receipt.id}))
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

    /// The top bar of desk32 as the host's hostd sees it: the desk's own
    /// cgroup path sits under its nspawn unit's payload.
    const HOST_VIEW: &str = "/machine.slice/systemd-nspawn@desk32.service/payload/system.slice/system-scenes.slice/scenes@3.service";

    #[test]
    fn the_host_matches_a_desk_session_by_its_full_cgroup() {
        let p = policy(&format!(
            r#"{{schema: 1, grants: [
              {{name: "desk32-power", caller: {{uid: 1002, cgroup: "{HOST_VIEW}/*"}}, verbs: ["desk.restart", "desk.stop"],
               limits: {{desks: ["desk32"]}}, rate: {{count: 2, per_s: 60}}}}
            ]}}"#
        ));
        assert_eq!(
            target(&p, &caller(HOST_VIEW), RESTART, now()),
            Ok("desk32".into())
        );
        // The same session's path as seen inside the desk does not match.
        let inside = "/system.slice/system-scenes.slice/scenes@3.service";
        assert_eq!(
            target(&p, &caller(inside), RESTART, now()),
            Err("not_granted")
        );
    }

    #[test]
    fn each_verb_names_its_host_unit() {
        assert_eq!(unit(RESTART, "desk32"), "desk-restart@desk32.service");
        assert_eq!(unit(STOP, "desk12"), "desk-stop@desk12.service");
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("state")).expect("state dir");
        dir
    }

    fn origin() -> Origin {
        Origin::Local(caller(HOST_VIEW))
    }

    #[tokio::test]
    async fn a_started_unit_is_recorded_and_never_undoable() {
        let dir = fixture();
        let state = dir.path().join("state");
        let out = act_with("true", dir.path(), &state, RESTART, "desk32", "g", &origin())
            .await
            .expect("started");
        assert_eq!(out["unit"], "desk-restart@desk32.service");
        assert_eq!(out["action"], "restart");
        let receipt = Receipt::load(&state, out["receipt"].as_str().unwrap()).expect("saved");
        assert_eq!(receipt.verb, RESTART);
        assert_eq!(receipt.outcome, Outcome::Applied);
        assert_eq!(receipt.summary["unit"], "desk-restart@desk32.service");
    }

    #[tokio::test]
    async fn a_unit_that_does_not_start_is_refused_without_a_receipt() {
        let dir = fixture();
        let state = dir.path().join("state");
        let err = act_with("false", dir.path(), &state, STOP, "desk32", "g", &origin())
            .await
            .unwrap_err();
        assert_eq!(err["status"], "unit_failed");
        assert_eq!(err["unit"], "desk-stop@desk32.service");
        assert_eq!(std::fs::read_dir(&state).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_hostd_inside_a_desk_refuses_and_starts_nothing() {
        let dir = fixture();
        std::fs::create_dir_all(dir.path().join("run/systemd")).unwrap();
        std::fs::write(dir.path().join("run/systemd/container"), "systemd-nspawn\n").unwrap();
        let state = dir.path().join("state");
        // A program that would fail proves it never ran: the refusal is
        // in_container, not unit_failed.
        let err = act_with("false", dir.path(), &state, RESTART, "desk12", "g", &origin())
            .await
            .unwrap_err();
        assert_eq!(err["status"], "in_container");
    }
}
