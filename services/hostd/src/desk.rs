// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Desk power: restart or stop one desk instance, a systemd-nspawn machine on
//! this host. A typed request names the desk. Every local caller may act on
//! any desk; the host itself is never powered.
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
    caller::Origin,
    receipt::{Outcome, Receipt},
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub desk: String,
}

pub const RESTART: &str = "desk.restart";
pub const STOP: &str = "desk.stop";
const SYSTEMCTL: &str = "systemctl";
const SYSTEMCTL_BUDGET: Duration = Duration::from_secs(30);

/// The host unit that runs `command`'s cycle for `desk`.
pub fn unit(command: &str, desk: &str) -> String {
    let action = if command == RESTART {
        "restart"
    } else {
        "stop"
    };
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

/// Start one power cycle on `desk`, then record it. A power action has no undo,
/// so its receipt is never undoable: `hostd.undo` refuses the command. The
/// receipt records that the cycle started; its unit's journal says how it
/// ended.
pub async fn act(
    root: &Path,
    state: &Path,
    command: &str,
    desk: &str,
    origin: &Origin,
) -> Result<Value, Value> {
    act_with(SYSTEMCTL, root, state, command, desk, origin).await
}

/// `act`, with the program that starts the unit named: tests pass a
/// stand-in for `systemctl`.
async fn act_with(
    systemctl: &str,
    root: &Path,
    state: &Path,
    command: &str,
    desk: &str,
    origin: &Origin,
) -> Result<Value, Value> {
    if in_container(root) {
        tracing::warn!(target: "hostd::audit", command, desk, "desk power refused: hostd runs inside a container");
        return Err(
            json!({"status":"in_container","command":command,"desk":desk,
            "message":"this hostd runs inside a desk; desk power needs the host's hostd"}),
        );
    }
    let unit = unit(command, desk);
    // --no-block: answer once the job is queued. The cycle outlives the
    // caller, whose desk goes down part way through it.
    let run = tokio::time::timeout(
        SYSTEMCTL_BUDGET,
        tokio::process::Command::new(systemctl)
            .args(["start", "--no-block", &unit])
            .output(),
    )
    .await
    .map_err(|_| json!({"status":"timeout","command":command,"desk":desk}))?
    .map_err(|e| json!({"status":"internal","command":command,"message":e.to_string()}))?;
    if !run.status.success() {
        let message = String::from_utf8_lossy(&run.stderr).trim().to_owned();
        tracing::warn!(target: "hostd::audit", command, desk, %unit, %message, "desk power unit did not start");
        return Err(
            json!({"status":"unit_failed","command":command,"desk":desk,"unit":unit,"message":message}),
        );
    }
    let action = if command == RESTART {
        "restart"
    } else {
        "stop"
    };
    let mut receipt = Receipt::new(
        command,
        origin,
        json!({"desk": desk, "action": action, "unit": unit}),
    );
    receipt.outcome = Outcome::Applied;
    receipt
        .save(state)
        .map_err(|e| json!({"status":"internal","command":command,"message":format!("{e:#}")}))?;
    tracing::info!(target: "hostd::audit", command, desk, %unit, receipt = %receipt.id, "desk power cycle started");
    Ok(json!({"desk": desk, "action": action, "unit": unit, "receipt": receipt.id}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_machine_names() {
        for name in ["desk12", "desk-2"] {
            assert!(valid_name(name));
        }
        for name in ["", "12desk", "Desk", "desk_12", "desk/12", "../host"] {
            assert!(!valid_name(name));
        }
        assert!(!valid_name(&"a".repeat(33)));
    }

    #[tokio::test]
    async fn any_local_bus_caller_can_target_a_desk() {
        let dir = tempfile::tempdir().unwrap();
        let origin = Origin::LocalBus {
            from: "arbitrary".into(),
        };
        let out = act_with("true", dir.path(), dir.path(), RESTART, "desk-2", &origin)
            .await
            .unwrap();
        let receipt = Receipt::load(dir.path(), out["receipt"].as_str().unwrap()).unwrap();
        assert_eq!(receipt.origin, origin);
        assert_eq!(receipt.summary["desk"], "desk-2");
        assert_eq!(receipt.summary["unit"], "desk-restart@desk-2.service");
        assert_eq!(receipt.outcome, Outcome::Applied);
    }

    #[test]
    fn each_command_names_its_host_unit() {
        assert_eq!(unit(RESTART, "desk-2"), "desk-restart@desk-2.service");
        assert_eq!(unit(STOP, "desk-3"), "desk-stop@desk-3.service");
    }

    #[tokio::test]
    async fn a_unit_that_does_not_start_is_refused_without_a_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let origin = Origin::LocalBus {
            from: "agent".into(),
        };
        let err = act_with("false", dir.path(), &state, STOP, "desk-2", &origin)
            .await
            .unwrap_err();
        assert_eq!(err["status"], "unit_failed");
        assert_eq!(err["unit"], "desk-stop@desk-2.service");
        assert_eq!(std::fs::read_dir(&state).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_hostd_inside_a_desk_refuses_and_starts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("run/systemd")).unwrap();
        std::fs::write(dir.path().join("run/systemd/container"), "systemd-nspawn\n").unwrap();
        let origin = Origin::LocalBus {
            from: "agent".into(),
        };
        let err = act_with("false", dir.path(), dir.path(), RESTART, "desk-2", &origin)
            .await
            .unwrap_err();
        assert_eq!(err["status"], "in_container");
    }
}
