// SPDX-License-Identifier: MIT OR Apache-2.0
//! Macro navigation and bounded traffic state, shared by GUI and headless lanes.
use crate::{Effect, Engine, Row, RowKind};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

pub const TRAFFIC_CAPACITY: usize = 1024;
pub const TRAFFIC_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub struct Workshop {
    pub pending: usize,
    pub catalogues: BTreeMap<String, Value>,
    pub selected: Option<(String, String)>,
    pub document: Value,
    pub message: String,
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
                if matches!(&row.kind, RowKind::Service(name) if name == app) {
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
            if app == "global" || !insert(self, rows, app) {
                rows.push(branch(self, app));
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
        self.push_workshop(Effect::Workshop {
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
