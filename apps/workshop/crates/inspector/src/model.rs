// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The renderer-independent protocol model: services, commands, peers and the
//! body and reply bounds. Transplanted from `apps/workshop/src/model.rs`
//! (markc/mixos-iced), itself adapted from MixOS Workshop
//! 70e6c9233a02577099e9c14500aebf44856530e1; the truncation marker is now a
//! parameter, so the engine needs no string catalogue.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const APP_ID: &str = "dev.mixos.workshop";
pub const BODY_LIMIT: usize = 65_536;
pub const REPLY_LIMIT: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    pub args: String,
    pub description: String,
    pub read_only: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    /// Present for live discovery; fixtures without progress are complete.
    pub progress: Option<Progress>,
    pub nodes: Vec<Node>,
    pub identities: BTreeMap<String, bus::ServiceInfo>,
    pub services: BTreeMap<String, Result<Vec<Command>, String>>,
    pub peers: Vec<String>,
    pub peer_error: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Progress {
    pub total: usize,
    pub completed: usize,
    pub finished: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Node {
    pub name: String,
    pub local: bool,
    pub services: Vec<String>,
    pub info: Value,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub service: String,
    pub command: String,
}

impl Snapshot {
    pub fn command_name(&self, selection: &Selection) -> Option<&Command> {
        self.services
            .get(&selection.service)?
            .as_ref()
            .ok()?
            .iter()
            .find(|v| v.name == selection.command)
    }

    pub fn failures(&self) -> usize {
        self.services
            .values()
            .filter(|value| value.is_err())
            .count()
    }
}

pub fn parse_commands(value: &Value) -> Result<Vec<Command>, String> {
    let entries = value
        .as_array()
        .or_else(|| value.get("commands")?.as_array())
        .ok_or("Expected a HELP array or app.describe commands array")?;
    let mut command_names = Vec::new();
    for entry in entries {
        let name = entry
            .as_str()
            .or_else(|| entry.get("name")?.as_str())
            .filter(|name| !name.is_empty())
            .ok_or("Command has no name")?;
        command_names.push(Command {
            name: name.into(),
            args: entry
                .get("args")
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                })
                .unwrap_or_default(),
            description: entry
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            read_only: entry.get("read_only").and_then(Value::as_bool),
        });
    }
    command_names.sort_by(|a, b| a.name.cmp(&b.name));
    command_names.dedup_by(|a, b| a.name == b.name);
    Ok(command_names)
}

pub fn services(value: &Value) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = value
        .as_array()
        .ok_or("noded.list is not an array")?
        .iter()
        .filter_map(|v| v.get("name")?.as_str())
        .map(str::to_owned)
        .collect();
    names.push("noded".into());
    names.sort();
    names.dedup();
    Ok(names)
}

pub fn peers(value: &Value) -> Vec<String> {
    let local = value.get("node").and_then(Value::as_str);
    let mut names: Vec<String> = value
        .pointer("/authority/routing_view")
        .or_else(|| value.get("peers"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        // The routing view lists every member; tombstoned and transport-only
        // ones carry no broker, so only active Bus members are nodes here.
        .filter(|v| {
            v.get("class")
                .and_then(Value::as_str)
                .is_none_or(|class| class == "active-bus")
        })
        .filter_map(|v| v.get("name")?.as_str())
        .filter(|name| Some(*name) != local)
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    names
}

pub fn validate_body(body: &str) -> Result<(), String> {
    if body.len() > BODY_LIMIT {
        return Err(format!("JSON body exceeds {BODY_LIMIT} bytes"));
    }
    if !body.trim().is_empty() {
        serde_json::from_str::<Value>(body).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `body` pretty-printed when it is JSON (and within [`REPLY_LIMIT`]),
/// else unchanged.
pub fn pretty(body: &str) -> String {
    if body.len() > REPLY_LIMIT {
        return body.to_owned();
    }
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| body.to_owned())
}

/// `body` cut to [`REPLY_LIMIT`] bytes on a character boundary, with
/// `marker` on its own line when anything was cut.
pub fn bounded(body: &str, marker: &str) -> String {
    if body.len() <= REPLY_LIMIT {
        return body.into();
    }
    let mut end = REPLY_LIMIT;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n{marker}", &body[..end])
}

/// Every command the engine answers itself. The window-level commands
/// (`workshop.ui.*`, `workshop.window*`) belong to the toolkit drive layer.
pub const COMMANDS: [&str; 21] = [
    "HELP",
    "app.describe",
    "workshop.ping",
    "workshop.info",
    "workshop.show",
    "workshop.refresh",
    "workshop.select",
    "workshop.call",
    "workshop.actions",
    "workshop.execute",
    "workshop.quit",
    "workshop.tree",
    "workshop.reply",
    "workshop.filter",
    "workshop.body",
    "workshop.split",
    "workshop.dock",
    "workshop.expand",
    "workshop.select_row",
    "workshop.dialog",
    "workshop.theme",
];

/// The engine's Bus surface (`HELP` / `app.describe` add the drive commands).
pub fn describe() -> Value {
    json!({"schema":"workshop.v1","app_id":APP_ID,"commands":[
        {"name":"HELP","description":"List every command","read_only":true},
        {"name":"app.describe","description":"Describe the app and every command with its arguments","read_only":true},
        {"name":"workshop.ping","description":"Probe Workshop","read_only":true},
        {"name":"workshop.info","description":"Inspect discovery, selection, reply and UI state","read_only":true},
        {"name":"workshop.show","description":"Restore and focus the existing window","read_only":false},
        {"name":"workshop.refresh","description":"Refetch services and descriptions","read_only":false},
        {"name":"workshop.select","args":{"service":"string","command":"string"},"description":"Select an advertised command","read_only":false},
        {"name":"workshop.call","args":{"service":"optional string","command":"optional string","body":"optional JSON text"},"description":"Call exactly once; target's safety is unchanged","read_only":false},
        {"name":"workshop.actions","description":"List the app's commands with labels, shortcuts and enablement","read_only":true},
        {"name":"workshop.execute","args":{"id":"string"},"description":"Run one app command by id, as its menu item or shortcut would (file.refresh, file.quit, edit.format, edit.clear, edit.copy, bus.call, help.shortcuts, help.about)","read_only":false},
        {"name":"workshop.quit","description":"Close when idle","read_only":false},
        {"name":"workshop.tree","description":"The rows the services tree shows: key, kind, label, depth, expanded, selected, children","read_only":true},
        {"name":"workshop.reply","description":"The reply panel's text and the last reply's value","read_only":true},
        {"name":"workshop.filter","args":{"text":"string"},"description":"Set the services filter, as typing in the search field does","read_only":false},
        {"name":"workshop.body","args":{"text":format!("string, at most {BODY_LIMIT} bytes")},"description":"Set the JSON body; refused while a call holds it","read_only":false},
        {"name":"workshop.split","args":{"value":format!("number from 0 to 1 (kept within {} to {})", crate::engine::SERVICES_SPLIT.start(), crate::engine::SERVICES_SPLIT.end())},"description":"Set the services pane's share of the window","read_only":false},
        {"name":"workshop.dock","args":{"side":"auto, bottom or right","size":"optional number 0..1, clamped to pane limits","open":"optional bool"},"description":"Set the Traffic dock placement and share of the window","read_only":false},
        {"name":"workshop.expand","args":{"key":"row key","open":"bool"},"description":"Open or close a tree row that has children","read_only":false},
        {"name":"workshop.select_row","args":{"key":"row key"},"description":"Select any tree row; a command row also selects its command","read_only":false},
        {"name":"workshop.dialog","args":{"open":"\"about\", \"shortcuts\" or null"},"description":"Open a dialog, or close the open one","read_only":false},
        {"name":"workshop.theme","args":{"scheme":"optional scheme name (ocean, crimson, stone, forest, sunset, mono, pro, studio, classic, breeze, adwaita) or null to follow the session","style":"optional style (plain, pro, studio, classic, breeze, adwaita), own for the scheme's own, or null to follow the session","mode":"optional light, dark or null to follow the session","decorations":"optional auto, csd, ssd or null to follow the session","caption_side":"optional right, left or null to follow the session"},"description":"Choose the window's theme: a present key sets that axis, an absent one leaves it; answers the choice and every effective axis, including decorations and decorations_reason","read_only":false}
    ]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_command_is_described_once() {
        let described: Vec<_> = describe()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(described, COMMANDS);
    }

    #[test]
    fn descriptions_preserve_legacy_unknown_safety_and_sort() {
        let command_names =
            parse_commands(&json!({"commands":["quit",{"name":"ping","read_only":true},"ping"]}))
                .unwrap();
        assert_eq!(command_names.len(), 2);
        assert_eq!(command_names[0].name, "ping");
        assert_eq!(command_names[0].read_only, Some(true));
        assert_eq!(command_names[1].read_only, None);
        assert!(parse_commands(&json!({"title":"legacy"})).is_err());
    }

    #[test]
    fn authority_peers_override_stale_roster_and_exclude_self() {
        assert_eq!(
            peers(
                &json!({"node":"alpha","authority":{"routing_view":[{"name":"alpha"},{"name":"beta"}]},"peers":[{"name":"stale"}]})
            ),
            vec!["beta"]
        );
        assert_eq!(
            peers(&json!({"peers":[{"name":"beta"},{"name":"beta"}]})),
            vec!["beta"]
        );
    }

    #[test]
    fn retired_members_are_not_nodes() {
        assert_eq!(
            peers(&json!({"node":"alpha","authority":{"routing_view":[
                {"name":"alpha","class":"active-bus"},
                {"name":"beta","class":"active-bus"},
                {"name":"gone","class":"tombstoned"},
                {"name":"wire","class":"transport-only"}
            ]}})),
            vec!["beta"]
        );
    }

    #[test]
    fn body_and_reply_bounds_preserve_utf8_and_plain_errors() {
        assert!(validate_body("").is_ok());
        assert!(validate_body("42").is_ok());
        assert!(validate_body("{").is_err());
        assert!(validate_body(&" ".repeat(BODY_LIMIT + 1)).is_err());
        assert_eq!(pretty("permission denied"), "permission denied");
        let cut = bounded(&"é".repeat(REPLY_LIMIT), "[cut]");
        assert!(cut.ends_with("\n[cut]"));
        assert!(cut.len() <= REPLY_LIMIT + "\n[cut]".len());
    }
}
