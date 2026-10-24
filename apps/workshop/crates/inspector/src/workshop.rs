// SPDX-License-Identifier: MIT OR Apache-2.0
//! Macro navigation and bounded traffic state, shared by GUI and headless lanes.
use crate::{Effect, Engine, Row, RowKind};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

pub const TRAFFIC_CAPACITY: usize = 1024;
pub const TRAFFIC_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default, serde::Serialize)]
pub struct Workshop {
    pub destination: String,
    pub next_ticket: u64,
    pub latest_open: u64,
    pub latest_catalogues: u64,
    pub target: String,
    pub confirm_delete: bool,
    pub arguments: Value,
    pub draft: Option<String>,
    pub pending: usize,
    pub catalogues: BTreeMap<String, Value>,
    pub selected: Option<(String, String)>,
    pub document: Value,
    pub message: String,
    #[serde(skip)]
    pub traffic: VecDeque<Value>,
    pub(crate) bytes: usize,
    pub dropped: u64,
    pub observe: String,
    pub subscription: Option<String>,
    pub dock_open: bool,
    pub paused: bool,
    pub filter: String,
}

impl Workshop {
    pub fn retain(&mut self, mut row: Value) {
        if self.paused {
            return;
        }
        redact(&mut row);
        if row.get("service").is_none() { row["service"] = row["to"].clone(); }
        let size = row.to_string().len();
        if size > TRAFFIC_BYTES {
            self.dropped += 1;
            return;
        }
        while self.traffic.len() >= TRAFFIC_CAPACITY || self.bytes + size > TRAFFIC_BYTES {
            if let Some(old) = self.traffic.pop_front() {
                self.bytes -= old.to_string().len();
                self.dropped += 1;
            }
        }
        self.bytes += size;
        self.traffic.push_back(row);
    }

    pub fn rows(&self, limit: usize, filter: &str) -> Vec<Value> {
        self.traffic
            .iter()
            .rev()
            .filter(|row| row.to_string().contains(filter))
            .take(limit.min(TRAFFIC_CAPACITY))
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
}

impl Workshop {
    pub fn stamp_last(&mut self, ts: String, ms: u64) {
        if let Some(mut row) = self.traffic.pop_back() {
            self.bytes -= row.to_string().len();
            row["ts"] = json!(ts);
            row["ms"] = json!(ms);
            self.retain(row);
        }
    }
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let key = key.to_ascii_lowercase();
                if [
                    "password",
                    "token",
                    "secret",
                    "credential",
                    "authorization",
                    "cookie",
                ]
                .iter()
                .any(|word| key.contains(word))
                {
                    *value = json!("[redacted]");
                } else {
                    redact(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact(value);
            }
        }
        _ => {}
    }
}

impl Engine {
    pub fn workshop_completion(&mut self, ticket: u64, operation: &str) -> bool {
        self.workshop.pending = self.workshop.pending.saturating_sub(1);
        (operation != "macros.open" || ticket == self.workshop.latest_open)
            && (operation != "catalogues" || ticket == self.workshop.latest_catalogues)
    }
    pub fn reveal_macro(&mut self) {
        fn ancestors(rows: &[Row], key: &str, path: &mut Vec<String>) -> bool {
            for row in rows {
                if row.key == key { return true; }
                path.push(row.key.clone());
                if ancestors(&row.children, key, path) { return true; }
                path.pop();
            }
            false
        }
        let mut path = Vec::new();
        if let Some(key) = &self.ui.row_key {
            ancestors(&self.tree(), key, &mut path);
            self.ui.expanded.extend(path);
        }
    }
    pub fn macro_command(&mut self, operation: &str) {
        let mut args = std::mem::replace(&mut self.workshop.arguments, json!({}));
        if !args.is_object() { args = json!({}); }
        if operation == "macros.remove" {
            if args["confirmed"] != true { self.workshop.confirm_delete = true; return; }
            self.workshop.confirm_delete = false;
            args.as_object_mut().unwrap().remove("confirmed");
        }
        if operation == "macros.put" && args.get("text").is_none() {
            args["text"] = json!(self.workshop.draft.as_deref().unwrap_or(self.workshop.document["text"].as_str().unwrap_or_default()));
        }
        if operation == "macros.move" && args.get("tier").is_none() {
            if self.workshop.selected.as_ref().is_some_and(|(app, _)| app == "global") {
                args["tier"] = json!("user-app");
                args["destination"] = json!(self.workshop.destination);
            } else { args["tier"] = json!("user-global"); }
        }
        if operation == "macros.run" && args.get("target").is_none() && !self.workshop.target.is_empty()
            && self.workshop.selected.as_ref().and_then(|(app, id)| self.workshop.catalogues.get(app)?.get("macros")?.as_array()?.iter().find(|entry| entry["id"] == *id)).is_some_and(|entry| !entry["targets"].is_null()) {
            args["target"] = json!(self.workshop.target);
        }
        self.macro_action(operation, args, None);
    }
    pub fn macro_tree(&self, rows: &mut Vec<Row>) {
        fn branch(engine: &Engine, app: &str) -> Row {
            let catalogue = &engine.workshop.catalogues[app];
            let key = format!("macros:{app}");
            let filter = engine.ui.filter.to_lowercase();
            let children = catalogue["macros"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|entry| entry.to_string().to_lowercase().contains(&filter))
                .map(|entry| Row {
                    key: format!("macro:{app}:{}", entry["id"].as_str().unwrap_or_default()),
                    kind: RowKind::Macro {
                        app: app.into(),
                        id: entry["id"].as_str().unwrap_or_default().into(),
                        title: format!(
                            "{} · {}",
                            entry["title"].as_str().unwrap_or_default(),
                            entry["tier"].as_str().unwrap_or_default()
                        ),
                    },
                    expanded: false,
                    children: Vec::new(),
                })
                .collect();
            Row {
                expanded: engine.ui.expanded.contains(&key) || !filter.is_empty(),
                key,
                kind: RowKind::MacroGroup(if app == "global" {
                    engine.text("global-macros")
                } else if catalogue["running"] == false {
                    format!("{app} · {}", engine.text("not-running"))
                } else {
                    engine.text("macros-branch")
                }),
                children,
            }
        }
        fn insert(engine: &Engine, rows: &mut [Row], app: &str) -> bool {
            for row in rows {
                if matches!(&row.kind, RowKind::Service(name) if name == app || engine.snapshot.services.get(name).and_then(|verbs| verbs.as_ref().ok()).is_some_and(|verbs| verbs.iter().any(|verb| verb.name == format!("{app}.macros.list")))) {
                    row.children.push(branch(engine, app));
                    return true;
                }
                if insert(engine, &mut row.children, app) {
                    return true;
                }
            }
            false
        }
        for app in self.workshop.catalogues.keys() {
            if app == "global" {
                rows.push(branch(self, app));
            } else if !insert(self, rows, app) {
                if let Some(node) = self.snapshot.nodes.iter().find(|node| node.local)
                    && let Some(parent) = rows.iter_mut().find(|row| row.key == format!("node:{}", node.name)) {
                    let key = format!("service:{}:{app}", node.name);
                    parent.children.push(Row { expanded: self.ui.expanded.contains(&key), key,
                        kind: RowKind::Service(app.clone()), children: vec![branch(self, app)] });
                } else { rows.push(branch(self, app)); }
            }
        }
    }

    pub fn macro_action(&mut self, operation: &str, mut args: Value, reply: Option<u64>) {
        if let Some((app, id)) = &self.workshop.selected {
            if args.get("app").is_none() {
                args["app"] = json!(app);
            }
            if !matches!(
                operation,
                "macros.stop"
                    | "macros.status"
                    | "macros.output"
                    | "macros.reload"
                    | "macros.list"
                    | "macros.new"
            ) && args.get("id").is_none()
            {
                args["id"] = json!(id);
            }
        }
        self.queue_workshop(operation, args, reply);
    }

    pub fn queue_workshop(&mut self, operation: &str, args: Value, reply: Option<u64>) {
        self.workshop.pending += 1;
        self.workshop.next_ticket += 1;
        let ticket = self.workshop.next_ticket;
        if operation == "macros.open" { self.workshop.latest_open = ticket; }
        if operation == "catalogues" { self.workshop.latest_catalogues = ticket; }
        self.push_workshop(Effect::Workshop {
            ticket,
            operation: operation.into(),
            args,
            reply,
        });
    }

    pub fn workshop_command(&mut self, id: u64, verb: &str, args: &Value) -> bool {
        if verb == "workshop.traffic" {
            let valid = args.as_object().is_some_and(|args| {
                args.keys()
                    .all(|key| matches!(key.as_str(), "limit" | "filter"))
            }) && args
                .get("limit")
                .is_none_or(|limit| limit.as_u64().is_some_and(|v| v <= TRAFFIC_CAPACITY as u64))
                && args.get("filter").is_none_or(Value::is_string);
            let body = if valid {
                json!({"rows":self.workshop.rows(args["limit"].as_u64().unwrap_or(100) as usize, args["filter"].as_str().unwrap_or("")),"dropped":self.workshop.dropped,"observe":self.workshop.observe})
            } else {
                json!({"error_code":"INVALID_ARGUMENT","message":"invalid traffic arguments"})
            };
            self.push_workshop(Effect::Reply {
                id,
                rc: if valid { 0 } else { 10 },
                body,
            });
            return true;
        }
        if verb == "noded.observe.event" {
            self.workshop.retain(args.clone());
            self.push_workshop(Effect::Reply {
                id,
                rc: 0,
                body: json!({}),
            });
            return true;
        }
        if verb == "workshop.observe"
            || verb == "workshop.macros.open"
            || verb
                .strip_prefix("workshop.macros.")
                .is_some_and(|suffix| macros::verbs::SUFFIXES.contains(&suffix))
        {
            if self.quitting {
                self.push_workshop(Effect::Reply {
                    id,
                    rc: 10,
                    body: json!({"error_code":"BUSY","message":"Workshop is closing"}),
                });
                return true;
            }
            self.queue_workshop(
                verb.strip_prefix("workshop.").unwrap_or(verb),
                args.clone(),
                Some(id),
            );
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newer_macro_navigation_wins_and_pending_work_fences_quit() {
        let mut engine = Engine::new(str::to_owned);
        engine.take_effects();
        engine.queue_workshop("macros.open", json!({"app":"ced","id":"first"}), None);
        let first = engine.workshop.latest_open;
        engine.queue_workshop("macros.open", json!({"app":"ced","id":"second"}), None);
        let second = engine.workshop.latest_open;
        assert!(!engine.workshop_completion(first, "macros.open"));
        assert_eq!(engine.workshop.pending, 1);
        assert!(engine.workshop_completion(second, "macros.open"));
        assert_eq!(engine.workshop.pending, 0);
        assert_ne!(first, second);
    }

    #[test]
    fn traffic_verb_bounds_filter_and_refusals() {
        let mut engine = Engine::new(str::to_owned);
        engine.take_effects();
        engine.workshop.retain(json!({"verb":"ced.execute"}));
        for (args, rc) in [(json!({"limit":1,"filter":"ced"}),0), (json!({"limit":1025}),10), (json!({"filter":true}),10), (json!({"extra":0}),10)] {
            engine.command(7, "workshop.traffic", &args.to_string());
            assert!(matches!(&engine.take_effects()[..], [Effect::Reply { id:7, rc:actual, .. }] if *actual == rc));
        }
    }
    #[test]
    fn traffic_bounds_count_bytes_and_rows() {
        let mut state = Workshop::default();
        for n in 0..TRAFFIC_CAPACITY + 3 {
            state.retain(json!({"seq":n}));
        }
        assert_eq!(state.traffic.len(), TRAFFIC_CAPACITY);
        assert_eq!(state.dropped, 3);
        state.retain(json!({"text":"x".repeat(TRAFFIC_BYTES)}));
        assert_eq!(state.dropped, 4);
        assert_eq!(state.rows(2, "").len(), 2);
        state.paused = true;
        state.retain(json!({}));
        assert_eq!(state.traffic.len(), TRAFFIC_CAPACITY);
    }

    #[test]
    fn topic_bodies_are_redacted_before_retention() {
        let mut state = Workshop::default();
        state.retain(json!({"payload":{"params":{"password":"never-retain","count":3}}}));
        assert_eq!(
            state.traffic[0]["payload"]["params"]["password"],
            "[redacted]"
        );
        assert_eq!(state.traffic[0]["payload"]["params"]["count"], 3);
    }
}
