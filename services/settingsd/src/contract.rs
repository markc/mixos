// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Contract 1.0.0's reads and topics (key space v2 §5.1, §5.2), answered
//! the same way by the authority and by a mirror from its cache: the
//! mirror computes nothing the authority does not.
use crate::authority::Authority;
use crate::publication;
use serde_json::{Value, json};
use settings::topics::{self, KeysBody, KeysContent, LookBody, LookContent, Resolved};
use settings::{ReadRequest, Revision, View};
use std::collections::BTreeMap;

impl Authority {
    /// What the registry resolves beside the authored choices.
    pub fn resolved(&self) -> Resolved {
        Resolved {
            mode: topics::resolved_mode(&self.accepted.values),
            accent: self.accent_hex(),
        }
    }

    /// The schema 3 snapshot.
    pub fn snapshot_v3(&self) -> Result<settings::compact::ValuesSnapshot, Value> {
        settings::compact::encode_values(&self.snapshot, &self.accepted.values)
            .map_err(|e| json!({"status":"validation_failed","message":e.to_string()}))
    }

    fn look(&self) -> LookContent {
        topics::look_content(
            &self.accepted.values,
            &self.resolved(),
            &self.snapshot.source_digest,
        )
    }

    /// Every topic's content, by its board key.
    fn contents(&self) -> BTreeMap<String, Value> {
        let mut out: BTreeMap<String, Value> =
            topics::contents(&self.accepted.values, &self.resolved())
                .into_iter()
                .map(|(prefix, content)| {
                    (
                        prefix.to_owned(),
                        serde_json::to_value(content).unwrap_or(Value::Null),
                    )
                })
                .collect();
        out.insert(
            publication::LOOK.into(),
            serde_json::to_value(self.look()).unwrap_or(Value::Null),
        );
        out.insert(
            publication::SNAPSHOT.into(),
            self.snapshot_v3()
                .ok()
                .and_then(|s| serde_json::to_value(s).ok())
                .unwrap_or(Value::Null),
        );
        out
    }

    /// Stage the current record on the board (after a commit or a load).
    pub fn stage(&mut self) {
        self.stage_with(None);
    }

    /// Stage with the revision that last changed each topic given (a mirror
    /// adopting its authority's stamps). Returns the topics that moved.
    pub fn stage_with(&mut self, changed_at: Option<&BTreeMap<String, Revision>>) -> Vec<String> {
        let contents = self.contents();
        let revision = self.accepted.revision;
        self.board.stage(revision, contents, changed_at)
    }

    /// One pending topic's retained name and body.
    pub fn topic(&self, key: &str) -> Option<(String, String)> {
        let profile = &self.accepted.binding.profile;
        // A content that could not be built (null) is no body: publication
        // fails and the topic stays pending.
        let content = self.board.content(key)?.clone();
        if content.is_null() {
            return None;
        }
        let changed_at = self.board.changed_at_of(key)?;
        let identity = (
            self.accepted.binding.clone(),
            self.accepted.incarnation.clone(),
            self.accepted.revision,
        );
        let (name, body) = match key {
            publication::SNAPSHOT => (topics::snapshot_topic(profile), content),
            publication::LOOK => {
                let look: LookContent = serde_json::from_value(content).ok()?;
                let body = LookBody {
                    schema: topics::TOPIC_SCHEMA,
                    binding: identity.0,
                    incarnation: identity.1,
                    revision: identity.2,
                    changed_at,
                    values: look.values,
                    resolved: look.resolved,
                    source_digest: look.source_digest,
                };
                (
                    topics::look_topic(profile),
                    serde_json::to_value(body).ok()?,
                )
            }
            prefix => {
                let keys: KeysContent = serde_json::from_value(content).ok()?;
                let body = KeysBody {
                    schema: topics::TOPIC_SCHEMA,
                    binding: identity.0,
                    incarnation: identity.1,
                    revision: identity.2,
                    changed_at,
                    values: keys.values,
                    resolved: keys.resolved,
                };
                (
                    topics::keys_topic(profile, prefix),
                    serde_json::to_value(body).ok()?,
                )
            }
        };
        Some((name, serde_json::to_string(&body).ok()?))
    }

    /// `settings.get` in a 1.0.0 view.
    pub fn read_v1(&self, request: &ReadRequest) -> Result<Value, Value> {
        let identity = json!({
            "binding": self.accepted.binding,
            "incarnation": self.accepted.incarnation,
            "revision": self.accepted.revision,
        });
        let mut answer = match request.view {
            View::Values => {
                let values = topics::view_values(
                    &self.accepted.values,
                    request.prefix.as_deref(),
                    request.keys.as_deref(),
                )
                .map_err(|d| json!({"status":"validation_failed","diagnostics":d}))?;
                // A prefix's stamp, for Prefs' local conflict check (§5.2).
                let changed_at = request
                    .prefix
                    .as_deref()
                    .and_then(topics::prefix_of)
                    .and_then(|p| self.board.changed_at_of(p));
                let mut answer = json!({"values": values, "resolved": self.resolved()});
                if let Some(at) = changed_at {
                    answer["changed_at"] = json!(at);
                }
                answer
            }
            View::Look => json!({"look": self.look()}),
            View::Snapshot => json!({
                "snapshot": self.snapshot_v3()?,
                "changed_at": self.board.changed_at(),
            }),
            View::Full | View::Summary => {
                return Err(json!({"status":"validation_failed","path":"view"}));
            }
        };
        answer["status"] = "current".into();
        if let (Some(answer), Some(identity)) = (answer.as_object_mut(), identity.as_object()) {
            answer.extend(identity.clone());
        }
        Ok(answer)
    }

    /// `settings.source` (ADR R14a, App. §K): the pinned source the look
    /// compiles, iff `digest` names it; `stale_digest` with the current one
    /// otherwise, so the follower rereads once. A mirror answers from its
    /// cache, which pins the same source.
    pub fn source(&self, request: &settings::SourceRequest) -> Result<Value, Value> {
        if request.binding != self.accepted.binding {
            return Err(json!({"status":"wrong_target","binding":self.accepted.binding}));
        }
        let current = &self.snapshot.source_digest;
        if &request.digest != current {
            return Err(json!({"status":"stale_digest","digest":current}));
        }
        let source = self
            .accepted
            .values
            .get("appearance.source")
            .and_then(Value::as_str)
            .unwrap_or(&self.accepted.embedded_source);
        Ok(json!({"status":"current","digest":current,"source":source}))
    }

    /// The record a mirror caches, sealed under this authority's `node`.
    pub fn cache(&self, node: &str) -> anyhow::Result<crate::mirror::Cache> {
        crate::mirror::Cache::of(&self.accepted, node)
    }
}
