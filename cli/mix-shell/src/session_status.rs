// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Status and execution on the enrolled connection for every Bus caller.
use crate::session_state::{self, Source, View};
use ::bus::native_client::session::boottime_ms;
use ::bus::native_client::{VerifiedCommand, VerifiedConnection};
use ::bus::native_session::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const MAX_REQUEST: usize = 2048;
const ADMISSION: Duration = Duration::from_secs(2);
pub(crate) const COMMAND: &str = "shell.status";

fn stamped(command: &::bus::native_client::IncomingCommand) -> bool {
    let origins: Vec<_> = command
        .headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("broker_origin"))
        .map(|(_, value)| value.as_str())
        .collect();
    origins == ["mesh"]
        || (origins == ["local"]
            && !command.headers.keys().any(|key| {
                ["source_peer", "permissions", "signed_ident"]
                    .iter()
                    .any(|name| key.eq_ignore_ascii_case(name))
            }))
}

/// Discovery is static public metadata: never serialise session state here.
fn manifest() -> Vec<::bus::CommandDescriptor> {
    use crate::session_execute::*;
    [
        ("HELP", "List served commands", true),
        (COMMAND, "Read shell status", true),
        (SUBMIT, "Submit shell evaluation", false),
        (RESULT, "Read evaluation result", true),
        (CANCEL, "Cancel shell evaluation", false),
        (TASK_SUBMIT, "Submit isolated task", false),
        (TASK_RESULT, "Read isolated task result", true),
        (TASK_CANCEL, "Cancel isolated task", false),
    ]
    .into_iter()
    .map(|(name, description, read_only)| {
        ::bus::CommandDescriptor::new(name, &[], description, read_only)
    })
    .collect()
}
/// Concurrent status dispatches, bounded equally for every caller.
pub(crate) const DISPATCH_SLOTS: usize = 4;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
    target: Source,
}

#[derive(Serialize)]
struct Capabilities {
    shell_phase: &'static str,
    cwd: &'static str,
    prompt_generation: &'static str,
    jobs: &'static str,
    job_signal: &'static str,
    foreground: &'static str,
    evaluation_submit: &'static str,
    evaluation_inspect: &'static str,
    input: &'static str,
    isolated_task: &'static str,
    events: &'static str,
}
impl Default for Capabilities {
    fn default() -> Self {
        // Reported from what this build can actually do, not from what the
        // caller is allowed to ask for. The owned editor is what makes an
        // admission possible; under rustyline the answer is the declared
        // limitation, which is UNSUPPORTED and never BUSY.
        let execution = crate::session_execute::available();
        Self {
            shell_phase: "snapshot",
            cwd: "last-observed-snapshot",
            prompt_generation: "snapshot",
            jobs: "UNSUPPORTED",
            job_signal: "UNSUPPORTED",
            foreground: "UNSUPPORTED",
            evaluation_submit: if execution {
                "idle-prompt-admission"
            } else {
                "UNSUPPORTED"
            },
            evaluation_inspect: if execution {
                "result-and-cancel"
            } else {
                "UNSUPPORTED"
            },
            input: "UNSUPPORTED",
            // Unlike evaluation submit, this does NOT depend on the editor:
            // a task is a separate process and does not touch the prompt, so
            // it is available on the rustyline path and while the shell is
            // busy. That independence is the mode's entire purpose.
            isolated_task: "supervised-process; poll-only (no watch/list)",
            events: "UNSUPPORTED",
        }
    }
}
#[derive(Serialize)]
struct Reply {
    version: u8,
    status: View,
    capabilities: Capabilities,
    freshness: &'static str,
}

/// Check the recipient's current attachment, independently of the caller.
pub(crate) async fn admitted(connection: &VerifiedConnection, bound: &SessionRecord) -> bool {
    // lease.check is recipient-only: broker delivery creates a dependency on
    // the CALLER, not on ourselves. Re-read our own attachment by record ID,
    // retaining request-start time so latency cannot extend its reported lease.
    let Ok(started) = boottime_ms() else {
        return false;
    };
    let Ok(current) = connection.session_self(bound.record_id).await else {
        return false;
    };
    current.record.state == BindingState::Attached
        && Source::from(&current.record) == Source::from(bound)
        && current.record.lease_remaining_ms.is_some_and(|remaining| {
            boottime_ms().is_ok_and(|now| now.saturating_sub(started) < remaining.0)
        })
}

pub(crate) async fn help(connection: &VerifiedConnection, event: &VerifiedCommand) {
    let body = serde_json::to_string(&manifest()).expect("static descriptors serialise");
    let _ = tokio::time::timeout(
        ADMISSION,
        connection.client().respond(event.command(), 0, &body),
    )
    .await;
}

/// Runs in the resident's bounded sibling task set, never in its receive arm.
pub(crate) async fn dispatch(
    connection: &VerifiedConnection,
    bound: &SessionRecord,
    event: &VerifiedCommand,
) {
    let command = event.command();
    if command.id.is_none() {
        return;
    }
    if command.command == "HELP" {
        help(connection, event).await;
        return;
    }
    if !stamped(command) {
        refuse(connection, event).await;
        return;
    }
    let capability = match command.command.as_str() {
        crate::session_execute::SUBMIT
        | crate::session_execute::RESULT
        | crate::session_execute::CANCEL
        | crate::session_execute::TASK_SUBMIT
        | crate::session_execute::TASK_RESULT
        | crate::session_execute::TASK_CANCEL
        | crate::session_execute::TASK_WATCH
        | crate::session_execute::TASK_LIST => Capability::Execute,
        _ => Capability::ReadState,
    };
    if !tokio::time::timeout(ADMISSION, admitted(connection, bound))
        .await
        .unwrap_or(false)
    {
        refuse(connection, event).await;
        return;
    }
    let response = if command.command != COMMAND && capability != Capability::Execute {
        (10, "{\"error_code\":\"UNSUPPORTED\"}".to_owned())
    } else if capability == Capability::Execute {
        crate::session_execute::dispatch(connection, bound, event).await
    } else {
        let request = (command.body.len() <= MAX_REQUEST)
            .then(|| serde_json::from_str::<Request>(&command.body).ok())
            .flatten();
        match request {
            Some(request) if request.version == 1 && request.target == Source::from(bound) => {
                let Some(status) = session_state::view() else {
                    refuse(connection, event).await;
                    return;
                };
                // A snapshot observed after the transport dropped is not
                // deliverable, but the refusal is still attempted: every
                // admitted request gets one uniform answer or a failed write,
                // never a silent drop that a caller cannot distinguish.
                if !connection.client().is_connected() {
                    refuse(connection, event).await;
                    return;
                }
                // Source changes atomically with the reducer's sequence. Never
                // relabel an old-generation snapshot with a new attachment.
                if status.snapshot.source.as_ref() != Some(&request.target) {
                    refuse(connection, event).await;
                    return;
                }
                let reply = Reply {
                    version: 1,
                    status,
                    capabilities: Capabilities::default(),
                    freshness: "last-observed; CLOCK_BOOTTIME milliseconds since shell state startup (includes suspend); a snapshot is information, never an execution permit",
                };
                (
                    0,
                    serde_json::to_string(&reply).expect("bounded snapshot serialises"),
                )
            }
            Some(request) if request.version == 1 => {
                (10, "{\"error_code\":\"STALE_GENERATION\"}".to_owned())
            }
            _ => (10, "{\"error_code\":\"INVALID_REQUEST\"}".to_owned()),
        }
    };
    let _ = tokio::time::timeout(
        ADMISSION,
        connection
            .client()
            .respond(command, response.0, &response.1),
    )
    .await;
}

pub(crate) async fn refuse(connection: &VerifiedConnection, event: &VerifiedCommand) {
    if event.command().id.is_some() {
        let _ = tokio::time::timeout(
            ADMISSION,
            connection
                .client()
                .respond(event.command(), 10, r#"{"error_code":"REFUSED"}"#),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_accepts_local_and_mesh_without_a_principal() {
        let mut command = ::bus::native_client::IncomingCommand {
            generation: 0,
            from: String::new(),
            command: COMMAND.into(),
            id: Some("1".into()),
            args: serde_json::Value::Null,
            body: "{}".into(),
            headers: Default::default(),
        };
        assert!(!stamped(&command));
        command
            .headers
            .insert("broker_origin".into(), "local".into());
        assert!(stamped(&command));
        command
            .headers
            .insert("source_peer".into(), "peer.example".into());
        assert!(!stamped(&command));
        command
            .headers
            .insert("broker_origin".into(), "mesh".into());
        assert!(stamped(&command));
        command
            .headers
            .insert("Broker_Origin".into(), "local".into());
        assert!(!stamped(&command));
    }

    #[test]
    fn help_contains_only_public_command_metadata() {
        let commands = manifest();
        assert_eq!(commands[0].name, "HELP");
        assert_eq!(commands.len(), 8);
        let value = serde_json::to_value(&commands).unwrap();
        let body = value.to_string();
        for secret in [
            "grant",
            "seed",
            "token",
            "public_key",
            "parent_key_hash",
            "record_id",
            "instance_id",
            "incarnation",
            "pane_id",
            "pane_generation",
            "binding_generation",
            "broker_epoch",
        ] {
            assert!(!body.contains(secret), "HELP exposed {secret}");
        }
        for command in value.as_array().unwrap() {
            let fields = command.as_object().unwrap();
            assert_eq!(fields.len(), 4);
            for field in ["name", "args", "description", "read_only"] {
                assert!(fields.contains_key(field));
            }
            assert!(command["args"].as_array().unwrap().is_empty());
        }
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.read_only)
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            [
                "HELP",
                COMMAND,
                crate::session_execute::RESULT,
                crate::session_execute::TASK_RESULT
            ]
        );
    }
    fn target() -> SessionRecord {
        SessionRecord {
            name: "test-pane".into(),
            record_assurance: RecordAssurance::SessionBound,
            owner_node: "alpha".into(),
            owner_uid: 1000,
            broker_epoch: HexBytes([1; 16]),
            record_id: HexBytes([2; 16]),
            instance_id: HexBytes([3; 16]),
            incarnation: HexBytes([4; 16]),
            role: Role::PaneShell,
            parent_instance: Some(HexBytes([5; 16])),
            parent_incarnation: Some(HexBytes([6; 16])),
            pane_id: Some(DecimalU64(1)),
            pane_generation: Some(DecimalU64(2)),
            binding_generation: DecimalU64(3),
            state: BindingState::Attached,
            capabilities: vec![Capability::ReadState],
            policy: Policy::Restricted,
            lease_remaining_ms: Some(DecimalU64(1000)),
        }
    }
    #[test]
    fn bounded_typed_request_rejects_unknown_fields_and_bad_counters() {
        let target = Source::from(&target());
        let mut value = serde_json::json!({"version":1,"target":target});
        assert!(serde_json::from_value::<Request>(value.clone()).is_ok());
        value["after_sequence"] = serde_json::json!(12);
        assert!(serde_json::from_value::<Request>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("after_sequence");
        value["command"] = serde_json::json!("execute");
        assert!(serde_json::from_value::<Request>(value).is_err());
    }
}
