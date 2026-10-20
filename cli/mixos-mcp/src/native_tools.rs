// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Semantic desktop tools are translations of existing owning-service Bus
//! contracts. Explicit service targets are never resolved by caption or focus.
//! Admission, canonicalisation and effects stay in the owning services.

use bus::PortReply;
use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::MixosMcp;

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct NativeObservation {
    service: String,
    command: String,
    /// Owning-service response; accepted/applied acknowledgements must not be
    /// interpreted as downstream completion. Query observable state to verify.
    result: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceTarget {
    /// Bus service name, for example "busviewer".
    service: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandInvocation {
    /// Bus service name, for example "busviewer".
    service: String,
    /// Dotted command id from app_commands, for example "file.refresh".
    id: String,
    /// Optional command parameters, forwarded unchanged to the application.
    #[serde(default)]
    #[schemars(with = "serde_json::Map<String, Value>")]
    params: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WindowObservation {
    service: String,
    app_id: Option<String>,
    title: Option<String>,
    title_contains: Option<String>,
    visible: Option<bool>,
    workspace: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WindowTarget {
    id: u64,
    /// Surface role generation, not a compositor process incarnation token.
    generation: u64,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowCondition {
    Mapped,
    Visible,
    Presented,
    Focused,
    Maximized,
    Unmaximized,
    Fullscreen,
    Unfullscreen,
    Unmapped,
    Gone,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WindowWait {
    service: String,
    window: WindowTarget,
    until: WindowCondition,
    /// Positive bound, at most 55 seconds (below the native client deadline).
    timeout_ms: u64,
}

fn exact_target(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 255
        || value
            .chars()
            .any(|c| c.is_control() || (field == "service" && c.is_whitespace()))
    {
        Err(format!(
            "{field} must be an explicit nonempty target, at most 255 bytes"
        ))
    } else {
        Ok(())
    }
}

fn wait_bound(timeout_ms: u64) -> Result<std::time::Duration, String> {
    if !(1..=55_000).contains(&timeout_ms) {
        return Err("timeout_ms must be in 1..=55000".into());
    }
    Ok(std::time::Duration::from_millis(timeout_ms))
}

impl MixosMcp {
    async fn native_observation(
        &self,
        service: String,
        command: &str,
        args: Value,
    ) -> Result<Json<NativeObservation>, String> {
        exact_target(&service, "service")?;
        if serde_json::to_vec(&args).map_err(|e| e.to_string())?.len() > 16_384 {
            return Err("native desktop request exceeds 16384 bytes; nothing was sent".into());
        }
        let noded = self.noded().await?;
        // One attempt only. A lost reply does not establish that work stopped.
        let result = match noded.call_typed(&service, command, args).await {
            Ok(PortReply::Ok { value, .. }) => value,
            Ok(PortReply::AppError { rc, message }) => {
                if message.trim().is_empty() {
                    return Err(format!("rc={rc}: unknown error"));
                }
                return Err(message);
            }
            Err(error) => {
                return Err(format!(
                    "native request failed: {error}; outcome unknown if delivered; verify effects before retrying"
                ));
            }
        };
        if serde_json::to_vec(&result)
            .map_err(|e| e.to_string())?
            .len()
            > 1024 * 1024
        {
            return Err("native desktop response exceeds 1 MiB".into());
        }
        Ok(Json(NativeObservation {
            service,
            command: command.into(),
            result,
        }))
    }
}

#[tool_router(router = native_tool_router, vis = "pub(crate)")]
impl MixosMcp {
    /// Describe one explicitly targeted application and its native verbs.
    #[tool(annotations(
        read_only_hint = true,
        destructive_hint = false,
        idempotent_hint = true
    ))]
    async fn app_describe(
        &self,
        Parameters(p): Parameters<ServiceTarget>,
    ) -> Result<Json<NativeObservation>, String> {
        self.native_observation(p.service, "app.describe", json!({}))
            .await
    }

    /// List an application's registry commands, preserving each row's metadata.
    #[tool(annotations(
        read_only_hint = true,
        destructive_hint = false,
        idempotent_hint = true
    ))]
    async fn app_commands(
        &self,
        Parameters(p): Parameters<ServiceTarget>,
    ) -> Result<Json<NativeObservation>, String> {
        exact_target(&p.service, "service")?;
        if p.service.contains(['@', '.']) {
            return Err(format!(
                "service must be a plain Bus service name (no '@' or '.'): {}",
                p.service
            ));
        }
        let command = format!("{}.commands", p.service);
        self.native_observation(p.service, &command, json!({}))
            .await
    }

    /// Execute a registry command once. The application checks enablement and
    /// parameters; its reply does not establish downstream completion.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false
    ))]
    async fn app_execute(
        &self,
        Parameters(p): Parameters<CommandInvocation>,
    ) -> Result<Json<NativeObservation>, String> {
        exact_target(&p.service, "service")?;
        if p.service.contains(['@', '.']) {
            return Err(format!(
                "service must be a plain Bus service name (no '@' or '.'): {}",
                p.service
            ));
        }
        exact_target(&p.id, "id")?;
        let command = format!("{}.execute", p.service);
        let mut args = json!({"id": p.id});
        if let Some(params) = p.params {
            args["params"] = Value::Object(params);
        }
        self.native_observation(p.service, &command, args).await
    }

    /// Observe compositor windows and their logical geometry/generations.
    /// Filters restrict observations; they never select a mutation target.
    #[tool(annotations(
        read_only_hint = true,
        destructive_hint = false,
        idempotent_hint = true
    ))]
    async fn desktop_windows(
        &self,
        Parameters(p): Parameters<WindowObservation>,
    ) -> Result<Json<NativeObservation>, String> {
        let mut args = serde_json::Map::new();
        for (key, value) in [
            ("app_id", p.app_id),
            ("title", p.title),
            ("title_contains", p.title_contains),
        ] {
            if let Some(value) = value {
                args.insert(key.into(), Value::String(value));
            }
        }
        if let Some(visible) = p.visible {
            args.insert("visible".into(), json!(visible));
        }
        if let Some(workspace) = p.workspace {
            args.insert("workspace".into(), json!(workspace));
        }
        self.native_observation(p.service, "comp.windows.list", Value::Object(args))
            .await
    }

    /// Wait for an explicitly identified compositor window's observable state.
    /// Generation fences surface role changes; it does not prove process identity.
    #[tool(annotations(
        read_only_hint = true,
        destructive_hint = false,
        idempotent_hint = true
    ))]
    async fn desktop_window_wait(
        &self,
        Parameters(p): Parameters<WindowWait>,
    ) -> Result<Json<NativeObservation>, String> {
        wait_bound(p.timeout_ms)?;
        self.native_observation(
            p.service,
            "comp.window.wait",
            json!({
                "match": p.window, "until": p.until, "timeout_ms": p.timeout_ms
            }),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_targets_and_waits_are_bounded() {
        assert!(exact_target("busviewer", "service").is_ok());
        for target in ["", " ", "app\n", "app\0"] {
            assert!(exact_target(target, "service").is_err());
        }
        assert!(wait_bound(0).is_err());
        assert!(wait_bound(55_001).is_err());
        assert!(wait_bound(55_000).is_ok());
    }

    #[test]
    fn semantic_inputs_reject_unknown_fields() {
        assert!(
            serde_json::from_value::<ServiceTarget>(json!({
                "service":"busviewer", "id":"file.refresh"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<CommandInvocation>(json!({
                "service":"busviewer", "id":"file.refresh", "focus":true
            }))
            .is_err()
        );
    }

    #[test]
    fn command_invocation_distinguishes_empty_params_from_absent_or_null() {
        let absent: CommandInvocation = serde_json::from_value(json!({
            "service":"busviewer", "id":"file.refresh"
        }))
        .unwrap();
        assert_eq!(absent.params, None);
        let empty: CommandInvocation = serde_json::from_value(json!({
            "service":"busviewer", "id":"file.refresh", "params":{}
        }))
        .unwrap();
        assert_eq!(empty.params, Some(serde_json::Map::new()));
        let null: CommandInvocation = serde_json::from_value(json!({
            "service":"busviewer", "id":"file.refresh", "params":null
        }))
        .unwrap();
        assert_eq!(null.params, None);
    }
}
