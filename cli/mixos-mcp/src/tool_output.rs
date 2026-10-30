// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Wire contracts shared by the legacy text tools. Typed/native handlers keep
//! their own schemas. Human-readable content remains alongside typed results.

use rmcp::model::{
    CallToolResponse, CallToolResult, ContentBlock as Content, Tool, ToolAnnotations,
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Serialize, JsonSchema)]
pub(crate) struct TextObservation {
    pub result: Value,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct BusObservation {
    pub result: Value,
    pub rc: u8,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct MixExecutionObservation {
    pub result: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct StatusSnapshot {
    pub started: String,
    pub uptime_secs: u64,
    pub broker_connected: bool,
    pub total_calls: u64,
    pub error_calls: u64,
    pub per_tool: std::collections::BTreeMap<String, u64>,
    pub recent: Vec<RecentCall>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct RecentCall {
    pub tool: String,
    pub at: String,
    pub ms: u64,
    pub ok: bool,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct TermListObservation {
    pub service: String,
    pub instance: Option<u64>,
    /// Parsed native tab rows, including each row's instance metadata.
    pub tabs: Vec<Value>,
    /// Parsed native pane rows, including each row's instance metadata.
    pub panes: Vec<Value>,
}

/// schemars writes a free-form `serde_json::Value` field as the boolean
/// schema `true`. JSON Schema allows that, but the TypeScript MCP SDK (Gemini
/// CLI) requires every property, item and branch schema to be an object, and
/// one boolean subschema makes it drop the whole `tools/list`. Rewrite
/// boolean subschemas in their object form: `true` is `{}`, `false` is
/// `{"not": {}}`. `additionalProperties: false` keeps its boolean.
fn object_subschemas(schema: &mut Value) {
    fn fix(sub: &mut Value) {
        match sub {
            Value::Bool(true) => *sub = json!({}),
            Value::Bool(false) => *sub = json!({"not": {}}),
            // The draft-7 tuple form of items is a list of schemas.
            Value::Array(subs) => subs.iter_mut().for_each(fix),
            _ => object_subschemas(sub),
        }
    }
    let Value::Object(map) = schema else {
        return;
    };
    for key in ["properties", "patternProperties", "$defs", "definitions", "dependentSchemas"] {
        if let Some(Value::Object(subs)) = map.get_mut(key) {
            subs.values_mut().for_each(fix);
        }
    }
    for key in [
        "items",
        "not",
        "if",
        "then",
        "else",
        "contains",
        "propertyNames",
        "unevaluatedItems",
        "unevaluatedProperties",
    ] {
        if let Some(sub) = map.get_mut(key) {
            fix(sub);
        }
    }
    // `additionalProperties: true` becomes `{}` (same meaning); `false`
    // stays, since it is what closes an object and clients accept it.
    if let Some(Value::Bool(true)) = map.get("additionalProperties") {
        map.insert("additionalProperties".into(), json!({}));
    } else if let Some(sub @ Value::Object(_)) = map.get_mut("additionalProperties") {
        object_subschemas(sub);
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(Value::Array(subs)) = map.get_mut(key) {
            subs.iter_mut().for_each(fix);
        }
    }
}

fn object_schema(schema: &rmcp::model::JsonObject) -> std::sync::Arc<rmcp::model::JsonObject> {
    let mut value = Value::Object(schema.clone());
    object_subschemas(&mut value);
    let Value::Object(map) = value else {
        unreachable!("an object stays an object")
    };
    std::sync::Arc::new(map)
}

pub(crate) fn decorate(mut tool: Tool) -> Tool {
    if tool.output_schema.is_none() {
        tool.output_schema = Some(if tool.name == "bus_call" {
            rmcp::handler::server::common::schema_for_output::<BusObservation>()
        } else if tool.name == "mix_execute" {
            rmcp::handler::server::common::schema_for_output::<MixExecutionObservation>()
        } else {
            rmcp::handler::server::common::schema_for_output::<TextObservation>()
        });
    }
    tool.input_schema = object_schema(&tool.input_schema);
    tool.output_schema = tool.output_schema.as_deref().map(object_schema);
    if tool.annotations.is_none() {
        let read_only = matches!(
            tool.name.as_ref(),
            "term_list"
                | "term_snapshot"
                | "bus_list_services"
                | "bus_node_info"
                | "bus_list_peers"
                | "noded_ping"
                | "log_tail"
                | "log_search"
                | "mcp_status"
        );
        // Retrieval/search can update scoring;
        // unknown tools stay conservative rather than claiming idempotence.
        tool.annotations = Some(
            ToolAnnotations::new()
                .read_only(read_only)
                .destructive(!read_only)
                .idempotent(read_only)
                .open_world(true),
        );
    }
    tool
}

pub(crate) fn normalize(tool: &str, response: CallToolResponse) -> CallToolResponse {
    let CallToolResponse::Complete(mut result) = response else {
        return response;
    };
    let texts = result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>();
    // Compatibility boundary for legacy handlers. Do not scan arbitrary log or
    // screen lines for ERROR:; an observed line is data, not a tool failure.
    let legacy_status = matches!(
        tool,
        "bus_list_services"
            | "bus_node_info"
            | "bus_list_peers"
            | "noded_ping"
            | "context_search"
            | "index_workspace"
            | "docs_feedback"
            | "journal_feedback"
            | "memory_feedback"
            | "journal_supersede"
            | "knowledge_digest"
    );
    if legacy_status
        && texts.len() == 1
        && (texts[0].starts_with("ERROR:") || texts[0].starts_with("ERROR "))
    {
        result.is_error = Some(true);
    }
    if !result.is_error.unwrap_or(false) && result.structured_content.is_none() {
        let text = texts.join("\n");
        let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        result.structured_content = Some(json!({"result": value}));
    }
    CallToolResponse::Complete(result)
}

pub(crate) fn failure(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![Content::text(message.into())])
}

pub(crate) fn bus_reply(reply: bus::PortReply) -> CallToolResult {
    match reply {
        bus::PortReply::Ok { rc, value } => {
            CallToolResult::structured(json!({"result": value, "rc": rc}))
        }
        bus::PortReply::AppError { rc, message } => {
            let mut result = CallToolResult::structured(json!({"result": {
                "outcome": "application_error", "rc": rc, "message": message
            }, "rc": rc}));
            result.is_error = Some(true);
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text_is_not_success_and_observed_error_lines_are_data() {
        let CallToolResponse::Complete(error) = normalize(
            "bus_list_services",
            CallToolResult::success(vec![Content::text("ERROR: refused")]).into(),
        ) else {
            panic!()
        };
        assert_eq!(error.is_error, Some(true));
        let CallToolResponse::Complete(observed) = normalize(
            "log_tail",
            CallToolResult::success(vec![Content::text("ERROR: previous command")]).into(),
        ) else {
            panic!()
        };
        assert!(!observed.is_error.unwrap_or(false));
        assert!(observed.structured_content.is_some());
    }

    /// Any boolean schema below the root, in any position the TypeScript
    /// MCP SDK checks.
    fn has_boolean_subschema(schema: &Value) -> bool {
        match schema {
            Value::Object(map) => map.iter().any(|(key, sub)| {
                let positional = matches!(
                    key.as_str(),
                    "items"
                        | "not"
                        | "if"
                        | "then"
                        | "else"
                        | "contains"
                        | "propertyNames"
                        | "unevaluatedItems"
                        | "unevaluatedProperties"
                );
                let open_object = key == "additionalProperties" && *sub == Value::Bool(true);
                let keyed = matches!(
                    key.as_str(),
                    "properties" | "patternProperties" | "$defs" | "definitions"
                );
                let listed = matches!(key.as_str(), "anyOf" | "oneOf" | "allOf" | "prefixItems");
                open_object
                    || (positional
                    && (sub.is_boolean()
                        || sub.as_array().is_some_and(|a| a.iter().any(Value::is_boolean))))
                    || (keyed
                        && sub
                            .as_object()
                            .is_some_and(|m| m.values().any(Value::is_boolean)))
                    || (listed
                        && sub
                            .as_array()
                            .is_some_and(|a| a.iter().any(Value::is_boolean)))
                    || has_boolean_subschema(sub)
            }),
            Value::Array(items) => items.iter().any(has_boolean_subschema),
            _ => false,
        }
    }

    #[test]
    fn output_schemas_have_no_boolean_subschemas() {
        for name in ["bus_call", "mix_execute", "log_tail", "term_list"] {
            let mut tool = Tool::default();
            tool.name = name.into();
            let tool = decorate(tool);
            let schema = Value::Object((*tool.output_schema.unwrap()).clone());
            assert!(!has_boolean_subschema(&schema), "{name}: {schema}");
        }
        let raw = schemars::schema_for!(TermListObservation);
        let mut tool = Tool::default();
        tool.name = "term_list".into();
        tool.output_schema = Some(std::sync::Arc::new(
            serde_json::to_value(&raw)
                .unwrap()
                .as_object()
                .unwrap()
                .clone(),
        ));
        let schema = Value::Object((*decorate(tool).output_schema.unwrap()).clone());
        assert!(!has_boolean_subschema(&schema), "term_list: {schema}");
    }

    #[test]
    fn boolean_subschemas_become_objects_and_false_stays_false_for_additional_properties() {
        let mut schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {"a": true, "b": {"type": "array", "items": true}, "c": false,
                "t": {"type": "array", "items": [{"type": "string"}, true]},
                "m": {"type": "object", "additionalProperties": true},
                "k": {"type": "array", "contains": true}},
            "anyOf": [true, {"properties": {"d": true}}]
        });
        object_subschemas(&mut schema);
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["properties"]["a"], json!({}));
        assert_eq!(schema["properties"]["b"]["items"], json!({}));
        assert_eq!(schema["properties"]["c"], json!({"not": {}}));
        assert_eq!(schema["properties"]["t"]["items"][1], json!({}));
        assert_eq!(schema["properties"]["m"]["additionalProperties"], json!({}));
        assert_eq!(schema["properties"]["k"]["contains"], json!({}));
        assert_eq!(schema["anyOf"][0], json!({}));
        assert_eq!(schema["anyOf"][1]["properties"]["d"], json!({}));
    }

    #[test]
    fn annotations_are_conservative_for_arbitrary_mutations() {
        for name in ["bus_call", "mix_execute", "term_type", "new_tool"] {
            let mut tool = Tool::default();
            tool.name = name.into();
            let annotations = decorate(tool).annotations.unwrap();
            assert_eq!(annotations.read_only_hint, Some(false));
            assert_eq!(annotations.idempotent_hint, Some(false));
        }
    }
}
