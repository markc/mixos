// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Unit control: start, stop or restart one system unit on this host. The
//! unit and action are typed request arguments, available to every local caller.
//!
//! hostd runs `systemctl ACTION -- UNIT` itself, so a caller needs no polkit
//! rule and no sudo. The command never touches hostd's own units: a restart of
//! hostd during a write would interrupt its transaction.
use crate::{
    caller::Origin,
    receipt::{Outcome, Receipt},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub const COMMAND: &str = "unit.control";
const SYSTEMCTL: &str = "systemctl";
/// `systemctl` waits for the job, so this is the longest a unit may take to
/// start or stop before the caller is told it timed out.
const BUDGET: Duration = Duration::from_secs(60);
/// Units hostd never controls while it is serving transactions.
const REFUSED: &[&str] = &["hostd.service", "hostd.socket"];
pub const ACTIONS: &[&str] = &["start", "stop", "restart"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub unit: String,
    pub action: String,
}

/// A systemd unit name with a known suffix and a literal, bounded stem.
/// Reject options, paths, patterns, whitespace and control characters.
pub fn valid_unit(name: &str) -> bool {
    let Some((stem, suffix)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && !stem.starts_with('-')
        && matches!(
            suffix,
            "service"
                | "socket"
                | "target"
                | "device"
                | "mount"
                | "automount"
                | "swap"
                | "timer"
                | "path"
                | "slice"
                | "scope"
        )
        && stem.len() <= 120
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ':'))
}

pub fn valid_action(action: &str) -> bool {
    ACTIONS.contains(&action)
}

/// Run `action` on `unit`, then record it. A unit action has no undo, so
/// its receipt is never undoable: `hostd.undo` refuses it.
pub async fn act(state: &Path, unit: &str, action: &str, origin: &Origin) -> Result<Value, Value> {
    act_with(SYSTEMCTL, state, unit, action, origin).await
}

/// `act`, with the program that runs the action named: tests pass a
/// stand-in for `systemctl`.
async fn act_with(
    systemctl: &str,
    state: &Path,
    unit: &str,
    action: &str,
    origin: &Origin,
) -> Result<Value, Value> {
    if !valid_unit(unit) || !valid_action(action) {
        return Err(json!({"status":"bad_request","command":COMMAND}));
    }
    if REFUSED.contains(&unit) {
        return Err(json!({"status":"refused","command":COMMAND,"unit":unit,
            "message":"hostd does not control its own units"}));
    }
    let run = tokio::time::timeout(
        BUDGET,
        tokio::process::Command::new(systemctl)
            .args([action, "--", unit])
            .output(),
    )
    .await
    .map_err(|_| json!({"status":"timeout","command":COMMAND,"unit":unit,"action":action}))?
    .map_err(|e| json!({"status":"internal","command":COMMAND,"message":e.to_string()}))?;
    if !run.status.success() {
        let message = String::from_utf8_lossy(&run.stderr).trim().to_owned();
        tracing::warn!(target: "hostd::audit", command = COMMAND, unit, action, %message, "unit action failed");
        return Err(json!({"status":"unit_failed","command":COMMAND,"unit":unit,
            "action":action,"message":message}));
    }
    let mut receipt = Receipt::new(COMMAND, origin, json!({"unit": unit, "action": action}));
    receipt.outcome = Outcome::Applied;
    receipt
        .save(state)
        .map_err(|e| json!({"status":"internal","command":COMMAND,"message":format!("{e:#}")}))?;
    tracing::info!(target: "hostd::audit", command = COMMAND, unit, action, receipt = %receipt.id, "unit action done");
    Ok(json!({"unit": unit, "action": action, "receipt": receipt.id}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::Caller;

    fn origin() -> Origin {
        Origin::Local(Caller {
            uid: 1002,
            gid: 1002,
            pid: Some(1),
            cgroup: Some(
                "/user.slice/user-1002.slice/user@1002.service/app.slice/mixtrayd.service".into(),
            ),
            verified: true,
            host_ns: false,
        })
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("state")).expect("state dir");
        dir
    }

    #[test]
    fn unit_names_have_known_suffixes_and_never_contain_options_or_controls() {
        assert!(valid_unit("noded.service"));
        assert!(valid_unit("monitord.service"));
        assert!(valid_unit("getty@tty1.service"));
        assert!(!valid_unit("noded"));
        assert!(!valid_unit(".service"));
        assert!(!valid_unit("../noded.service"));
        assert!(!valid_unit("a b.service"));
        assert!(valid_unit("noded.socket"));
        assert!(!valid_unit("--job-mode=replace-irreversibly"));
        assert!(!valid_unit("-option.service"));
        assert!(!valid_unit("a\n.service"));
        assert!(!valid_unit("a.unknown"));
        assert!(!valid_unit(&format!("{}.service", "a".repeat(121))));
    }

    #[test]
    fn only_start_stop_and_restart_are_actions() {
        assert!(valid_action("start"));
        assert!(valid_action("stop"));
        assert!(valid_action("restart"));
        assert!(!valid_action("kill"));
        assert!(!valid_action("daemon-reload"));
    }

    #[tokio::test]
    async fn hostd_never_controls_its_own_units() {
        let dir = fixture();
        let state = dir.path().join("state");
        let err = act_with("true", &state, "hostd.service", "restart", &origin())
            .await
            .expect_err("refused");
        assert_eq!(err["status"], "refused");
    }

    #[tokio::test]
    async fn a_unit_action_is_recorded_and_never_undoable() {
        let dir = fixture();
        let state = dir.path().join("state");
        let out = act_with("true", &state, "noded.service", "restart", &origin())
            .await
            .expect("done");
        assert_eq!(out["unit"], "noded.service");
        assert_eq!(out["action"], "restart");
        let receipt = Receipt::load(&state, out["receipt"].as_str().unwrap()).expect("saved");
        assert_eq!(receipt.command, COMMAND);
        assert_eq!(receipt.outcome, Outcome::Applied);
        assert_eq!(receipt.summary["unit"], "noded.service");
    }

    #[tokio::test]
    async fn a_unit_that_fails_is_refused_without_a_receipt() {
        let dir = fixture();
        let state = dir.path().join("state");
        let err = act_with("false", &state, "webd.service", "start", &origin())
            .await
            .expect_err("failed");
        assert_eq!(err["status"], "unit_failed");
        assert!(std::fs::read_dir(&state).expect("state").next().is_none());
    }
}
