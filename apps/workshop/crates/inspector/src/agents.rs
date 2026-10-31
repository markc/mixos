// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Agents branch: this node's agentd sessions (ACP agents on the Bus),
//! and the transcript of the selected one. Headless, like the rest of the
//! engine: the shell sends the calls and hands back replies and topics.
//!
//! Status comes from agentd's own answers (`agent.session.list`, the session
//! view a read returns), never from topic seqs alone. Only the selected
//! session's events are held, in a bounded ring; the others keep status.
use crate::{Effect, Engine, Row, RowKind};
use serde_json::{Value, json};
use std::collections::VecDeque;

/// The topics agentd publishes; Workshop subscribes to them at start.
pub const TOPICS: [&str; 3] = [
    "agent.session.updated",
    "agent.session.ended",
    "agent.permission.requested",
];
pub const RING_EVENTS: usize = 2000;
pub const RING_BYTES: usize = 8 * 1024 * 1024;
/// agentd's page size for `agent.session.read`.
const READ_LIMIT: u64 = 2000;

/// One session as `agent.session.list` reports it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct SessionRow {
    pub session: String,
    pub agent: String,
    pub cwd: String,
    pub status: String,
    pub seq: u64,
    pub open_permissions: u64,
    /// The open permission requests' keys, to answer them.
    pub permission_requests: Vec<String>,
}

impl SessionRow {
    fn from(value: &Value) -> Option<Self> {
        Some(Self {
            session: value["session"].as_str()?.to_owned(),
            agent: value["agent"].as_str().unwrap_or_default().to_owned(),
            cwd: value["cwd"].as_str().unwrap_or_default().to_owned(),
            status: value["status"].as_str().unwrap_or("unknown").to_owned(),
            seq: value["seq"].as_u64().unwrap_or_default(),
            open_permissions: value["open_permissions"].as_u64().unwrap_or_default(),
            permission_requests: value["permission_requests"]
                .as_array()
                .map(|keys| {
                    keys.iter()
                        .map(|key| match key {
                            Value::String(key) => key.clone(),
                            other => other.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// The one badge a row shows: an open question outranks everything.
    pub fn badge(&self) -> &str {
        if self.open_permissions > 0 && self.status != "ended" {
            "question"
        } else {
            &self.status
        }
    }

    pub fn live(&self) -> bool {
        self.status != "ended"
    }
}

/// The selected session's events, oldest first, bounded.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Transcript {
    pub session: String,
    #[serde(skip)]
    pub events: VecDeque<Value>,
    #[serde(skip)]
    bytes: usize,
    /// The highest seq held with no gap before it (0: none yet).
    pub seq: u64,
    /// Events before the ring's oldest were evicted.
    pub evicted: bool,
    /// Some events are missing (a hole in agentd's log).
    pub gap: bool,
    /// The events folded for reading, kept in step with `events`.
    #[serde(skip)]
    pub items: Vec<Item>,
    /// A read is filling the ring; live events wait for it.
    pub filling: bool,
    /// The highest seq a topic announced while filling.
    seen: u64,
}

impl Transcript {
    fn new(session: &str) -> Self {
        Self {
            session: session.to_owned(),
            ..Default::default()
        }
    }

    /// Append an event newer than any held. A seq past the next one is a
    /// hole (the log lost an event, or retention trimmed it): marked, never
    /// waited for. Older or repeated seqs are ignored.
    fn push(&mut self, event: Value) -> bool {
        let seq = event["seq"].as_u64().unwrap_or_default();
        if seq <= self.seq {
            return false;
        }
        if seq > self.seq + 1 {
            if self.events.is_empty() && self.seq == 0 {
                self.evicted = true;
            } else {
                self.gap = true;
            }
        }
        let size = event.to_string().len();
        if self.events.len() >= RING_EVENTS || self.bytes + size > RING_BYTES {
            // Evict a tenth at once, so the items are refolded rarely.
            while !self.events.is_empty()
                && (self.events.len() > RING_EVENTS * 9 / 10
                    || self.bytes + size > RING_BYTES * 9 / 10)
            {
                if let Some(old) = self.events.pop_front() {
                    self.bytes = self.bytes.saturating_sub(old.to_string().len());
                }
            }
            self.evicted = true;
            self.items = items(&self.events);
        }
        fold(&mut self.items, &event);
        self.bytes += size;
        self.events.push_back(event);
        self.seq = seq;
        true
    }
}

/// One line of the transcript as a person reads it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum Item {
    Prompt {
        text: String,
    },
    Message {
        text: String,
    },
    Thought {
        text: String,
    },
    Tool {
        id: String,
        title: String,
        tool: String,
        status: String,
        /// The latest raw update, so nothing is lost by compacting.
        raw: Value,
    },
    Plan {
        entries: Vec<String>,
    },
    Stop {
        reason: String,
    },
    Error {
        message: String,
    },
    Note {
        kind: String,
        text: String,
    },
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Object(_) => content["text"].as_str().unwrap_or_default().to_owned(),
        Value::Array(blocks) => blocks.iter().map(text_of).collect(),
        _ => String::new(),
    }
}

/// Fold events into transcript items: chunks merge into paragraphs, a tool
/// call and its updates become one row.
pub fn items(events: &VecDeque<Value>) -> Vec<Item> {
    let mut out = Vec::new();
    for event in events {
        fold(&mut out, event);
    }
    out
}

/// Fold one more event into `out`.
pub fn fold(out: &mut Vec<Item>, event: &Value) {
    {
        let kind = event["kind"].as_str().unwrap_or_default();
        let update = &event["update"];
        match kind {
            "prompt" => out.push(Item::Prompt {
                text: text_of(&event["content"]),
            }),
            "agent_message_chunk" | "agent_thought_chunk" => {
                let chunk = text_of(&update["content"]);
                let thought = kind == "agent_thought_chunk";
                match out.last_mut() {
                    Some(Item::Message { text }) if !thought => text.push_str(&chunk),
                    Some(Item::Thought { text }) if thought => text.push_str(&chunk),
                    _ if thought => out.push(Item::Thought { text: chunk }),
                    _ => out.push(Item::Message { text: chunk }),
                }
            }
            "tool_call" | "tool_call_update" => {
                let id = update["toolCallId"].as_str().unwrap_or_default().to_owned();
                let existing = out.iter_mut().rev().find_map(|item| match item {
                    Item::Tool {
                        id: known,
                        title,
                        tool,
                        status,
                        raw,
                    } if *known == id => Some((title, tool, status, raw)),
                    _ => None,
                });
                let title = update["title"].as_str().map(str::to_owned);
                let tool = update["kind"].as_str().map(str::to_owned);
                let status = update["status"].as_str().map(str::to_owned);
                if let Some((t, k, s, raw)) = existing {
                    if let Some(title) = title {
                        *t = title;
                    }
                    if let Some(tool) = tool {
                        *k = tool;
                    }
                    if let Some(status) = status {
                        *s = status;
                    }
                    *raw = update.clone();
                } else {
                    out.push(Item::Tool {
                        id,
                        title: title.unwrap_or_default(),
                        tool: tool.unwrap_or_default(),
                        status: status.unwrap_or_else(|| "pending".into()),
                        raw: update.clone(),
                    });
                }
            }
            "plan" => out.push(Item::Plan {
                entries: update["entries"]
                    .as_array()
                    .map(|entries| {
                        entries
                            .iter()
                            .map(|entry| {
                                format!(
                                    "[{}] {}",
                                    entry["status"].as_str().unwrap_or("pending"),
                                    entry["content"].as_str().unwrap_or_default()
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            }),
            "stop" => out.push(Item::Stop {
                reason: event["stop"].as_str().unwrap_or_default().to_owned(),
            }),
            "error" | "protocol_error" | "resume_failed" => out.push(Item::Error {
                message: event["error"]["message"]
                    .as_str()
                    .or_else(|| event["message"].as_str())
                    .unwrap_or(kind)
                    .to_owned(),
            }),
            // Noise a person does not need line by line.
            "available_commands_update"
            | "usage_update"
            | "session_info_update"
            | "config_option_update"
            | "current_mode_update"
            | "initialized"
            | "ready" => {}
            _ => out.push(Item::Note {
                kind: kind.to_owned(),
                text: note_text(event),
            }),
        }
    }
}

/// The agentd session a caller key belongs to, when the caller is a
/// session's own mixos-mcp (`mcp-s-<session>-…`, agentd's `sender_session`
/// rule), locally (`local:NAME`) or over the mesh (`mesh:NAME@peer`).
pub fn agent_caller_session(caller: &str) -> Option<String> {
    let name = caller.strip_prefix("local:").or_else(|| {
        caller
            .strip_prefix("mesh:")
            .map(|rest| rest.split('@').next().unwrap_or(rest))
    })?;
    let rest = name.strip_prefix("mcp-s-")?;
    let (session, tail) = rest.split_once('-')?;
    let valid = (4..=12).contains(&session.len())
        && session
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    (valid && !tail.is_empty()).then(|| session.to_owned())
}

/// A `workshop.agents.*` acting command as the agentd command and body it
/// becomes. `session` defaults to the selected one.
pub fn agent_request(
    command: &str,
    args: &Value,
    selected: Option<&str>,
) -> Result<(String, Value), String> {
    let session = || {
        args["session"]
            .as_str()
            .or(selected)
            .map(str::to_owned)
            .ok_or_else(|| "no session given and none selected".to_owned())
    };
    let op = command.strip_prefix("workshop.agents.").unwrap_or_default();
    let body = match op {
        "new" => {
            let agent = args["agent"].as_str().ok_or("agent must be a string")?;
            let cwd = args["cwd"].as_str().ok_or("cwd must be a string")?;
            let mut body = json!({"agent":agent,"cwd":cwd});
            for key in ["mode", "additional_dirs"] {
                if !args[key].is_null() {
                    body[key] = args[key].clone();
                }
            }
            body
        }
        "prompt" => {
            let text = args["text"].as_str().ok_or("text must be a string")?;
            json!({"session":session()?,"content":text})
        }
        "cancel" | "close" | "resume" => json!({"session":session()?}),
        "set" => {
            let mut body = json!({"session":session()?});
            if let Some(mode) = args["mode"].as_str() {
                body["mode"] = json!(mode);
            } else if let (Some(option), false) = (args["option"].as_str(), args["value"].is_null())
            {
                body["option"] = json!(option);
                body["value"] = args["value"].clone();
            } else {
                return Err("set needs mode, or option and value".into());
            }
            body
        }
        _ => return Err(format!("unknown command {command}")),
    };
    Ok((format!("agent.session.{op}"), body))
}

fn note_text(event: &Value) -> String {
    for key in ["path", "option", "via", "command"] {
        if let Some(text) = event[key].as_str() {
            return text.to_owned();
        }
    }
    String::new()
}

/// Everything the Agents branch knows.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Agents {
    /// `None` until agentd answered once; `Some(false)`: not on this node.
    pub available: Option<bool>,
    pub error: Option<String>,
    /// `agent.agents` as agentd answered it.
    pub instances: Value,
    pub sessions: Vec<SessionRow>,
    pub selected: Option<String>,
    pub transcript: Transcript,
    /// The selected session's `config_options`, from its ready event.
    pub config_options: Value,
    /// The composer's text for the selected session.
    pub draft: String,
    /// The New session form.
    pub new_agent: String,
    pub new_cwd: String,
    /// Acting calls in flight: ticket to the Bus command to answer.
    #[serde(skip)]
    replies: std::collections::BTreeMap<u64, Option<u64>>,
    #[serde(skip)]
    list_ticket: Option<u64>,
    #[serde(skip)]
    list_again: bool,
    #[serde(skip)]
    read_ticket: Option<u64>,
    #[serde(skip)]
    next_ticket: u64,
}

impl Agents {
    pub fn selected_row(&self) -> Option<&SessionRow> {
        let id = self.selected.as_ref()?;
        self.sessions.iter().find(|row| &row.session == id)
    }

    pub fn items(&self) -> &[Item] {
        &self.transcript.items
    }

    /// Live sessions first (newest first as agentd lists them), then ended.
    fn order(&mut self) {
        self.sessions.sort_by_key(|row| !row.live());
    }
}

impl Engine {
    fn agent_call(&mut self, command: &str, body: Value) -> u64 {
        self.agents.next_ticket += 1;
        let ticket = self.agents.next_ticket;
        self.push_workshop(Effect::Agent {
            ticket,
            command: command.to_owned(),
            body,
        });
        ticket
    }

    /// Read agentd's instances and sessions again. Coalesced: one list is in
    /// flight at a time, and one more follows if asked for meanwhile.
    pub fn refresh_agents(&mut self) {
        if self.agents.list_ticket.is_some() {
            self.agents.list_again = true;
            return;
        }
        // The live count changes with every session: read it each time.
        self.agent_call("agent.agents", json!({}));
        let ticket = self.agent_call("agent.session.list", json!({}));
        self.agents.list_ticket = Some(ticket);
    }

    /// Select one session: its transcript is read back from the start.
    pub fn select_session(&mut self, session: &str) {
        if self.agents.selected.as_deref() == Some(session) && self.agents.transcript.seq > 0 {
            return;
        }
        self.agents.selected = Some(session.to_owned());
        self.agents.transcript = Transcript::new(session);
        self.agents.config_options = Value::Null;
        self.agents.draft.clear();
        self.read_transcript();
    }

    fn read_transcript(&mut self) {
        let Some(session) = self.agents.selected.clone() else {
            return;
        };
        self.agents.transcript.filling = true;
        let since = self.agents.transcript.seq;
        let ticket = self.agent_call(
            "agent.session.read",
            json!({"session":session,"since":since,"limit":READ_LIMIT}),
        );
        self.agents.read_ticket = Some(ticket);
    }

    /// One reply from agentd: `rc` and the decoded body, or a transport error.
    pub fn agent_completed(
        &mut self,
        ticket: u64,
        command: &str,
        result: Result<(u8, Value), String>,
    ) {
        if let Some(reply) = self.agents.replies.remove(&ticket) {
            self.acted(ticket, command, reply, &result);
            return;
        }
        let (rc, body) = match result {
            Ok(reply) => reply,
            Err(message) => {
                // A timeout or a transport fault: keep what is known.
                if command == "agent.session.list" && self.agents.list_ticket == Some(ticket) {
                    self.agents.list_ticket = None;
                    self.agents.list_again = false;
                }
                if command == "agent.session.read" && self.agents.read_ticket == Some(ticket) {
                    self.agents.read_ticket = None;
                    self.agents.transcript.filling = false;
                }
                self.agents.error = Some(message);
                return;
            }
        };
        match command {
            "agent.agents" => {
                if rc == 0 {
                    self.agents.available = Some(true);
                    self.agents.error = None;
                    self.agents.instances = body;
                } else {
                    self.agent_unavailable(rc, &body);
                }
            }
            "agent.session.list" => {
                if self.agents.list_ticket != Some(ticket) {
                    return;
                }
                self.agents.list_ticket = None;
                if rc == 0 {
                    self.agents.available = Some(true);
                    self.agents.error = None;
                    self.agents.sessions = body["sessions"]
                        .as_array()
                        .map(|rows| rows.iter().filter_map(SessionRow::from).collect())
                        .unwrap_or_default();
                    self.agents.order();
                } else {
                    self.agent_unavailable(rc, &body);
                }
                if std::mem::take(&mut self.agents.list_again) {
                    self.refresh_agents();
                }
            }
            "agent.session.read" => {
                if self.agents.read_ticket != Some(ticket) {
                    return;
                }
                self.agents.read_ticket = None;
                if rc != 0 {
                    self.agents.transcript.filling = false;
                    self.agents.error = body["message"].as_str().map(str::to_owned);
                    return;
                }
                if body["session"]["session"].as_str() != self.agents.selected.as_deref() {
                    return;
                }
                if let Some(row) = SessionRow::from(&body["session"]) {
                    self.update_row(row);
                }
                let before = self.agents.transcript.seq;
                let events = body["events"].as_array().cloned().unwrap_or_default();
                for event in events {
                    self.take_event(event);
                }
                let more = body["more"] == true;
                let transcript = &mut self.agents.transcript;
                let progressed = transcript.seq > before;
                if progressed && (more || transcript.seen > transcript.seq) {
                    self.read_transcript();
                } else {
                    // What a topic announced is not in the log (yet, or at
                    // all): live events carry on from here.
                    transcript.seen = transcript.seq;
                    transcript.filling = false;
                }
            }
            _ => {}
        }
    }

    fn agent_unavailable(&mut self, rc: u8, body: &Value) {
        // Only the broker's "no such service" means agentd is not on this
        // node; any other refusal keeps the rows and shows the message.
        let missing = rc == 10
            && body["message"]
                .as_str()
                .is_some_and(|message| message.contains("not found"));
        if !missing {
            self.agents.error = Some(
                body["message"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("agentd answered rc {rc}")),
            );
            return;
        }
        self.agents.available = Some(false);
        self.agents.error = Some(
            body["message"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("agentd answered rc {rc}")),
        );
        self.agents.sessions.clear();
        self.agents.instances = Value::Null;
    }

    fn update_row(&mut self, row: SessionRow) {
        match self
            .agents
            .sessions
            .iter_mut()
            .find(|r| r.session == row.session)
        {
            Some(known) => *known = row,
            None => self.agents.sessions.insert(0, row),
        }
        self.agents.order();
    }

    /// Keep one event of the selected session, in order.
    fn take_event(&mut self, event: Value) {
        if event["kind"] == "ready" || event["kind"] == "resumed" {
            self.agents.config_options = event["config_options"].clone();
        }
        if event["kind"] == "config_option_update" {
            self.agents.config_options = event["update"]["configOptions"].clone();
        }
        self.agents.transcript.push(event);
    }

    /// One agentd topic delivery.
    pub fn agent_topic(&mut self, topic: &str, body: &Value) {
        let Some(session) = body["session"].as_str() else {
            return;
        };
        match topic {
            "agent.session.updated" => {
                let kind = body["kind"].as_str().unwrap_or_default();
                let known = self
                    .agents
                    .sessions
                    .iter()
                    .any(|row| row.session == session);
                // Status changes come from agentd's answer, not from here.
                if !known
                    || matches!(
                        kind,
                        "prompt"
                            | "ready"
                            | "stop"
                            | "error"
                            | "resumed"
                            | "resume_failed"
                            | "permission_answered"
                            | "ended"
                    )
                {
                    self.refresh_agents();
                }
                if self.agents.selected.as_deref() != Some(session) {
                    return;
                }
                let seq = body["seq"].as_u64().unwrap_or_default();
                let transcript = &mut self.agents.transcript;
                if transcript.filling {
                    transcript.seen = transcript.seen.max(seq);
                    return;
                }
                if seq <= transcript.seq {
                    return;
                }
                // A gap, or a marker for an event too big for a topic: read
                // the log from where the ring stops.
                if seq > transcript.seq + 1 || body["truncated"] == true {
                    transcript.seen = seq;
                    self.read_transcript();
                    return;
                }
                self.take_event(body.clone());
            }
            "agent.session.ended" | "agent.permission.requested" => self.refresh_agents(),
            _ => {}
        }
    }

    /// Topic deliveries were dropped: read agentd's state again rather than
    /// trust what the topics said.
    pub fn agents_resync(&mut self) {
        self.refresh_agents();
        if self.agents.selected.is_some() && !self.agents.transcript.filling {
            self.read_transcript();
        }
    }

    /// The Agents branch of the tree.
    pub fn agents_tree(&self) -> Row {
        let mut children = Vec::new();
        if self.agents.available == Some(true) {
            children.push(Row {
                key: "agents:new".into(),
                kind: RowKind::AgentsNew,
                expanded: false,
                children: Vec::new(),
            });
        }
        if self.agents.available == Some(false) {
            children.push(Row {
                key: "agents:down".into(),
                kind: RowKind::AgentsDown,
                expanded: false,
                children: Vec::new(),
            });
        }
        let (live, ended): (Vec<_>, Vec<_>) =
            self.agents.sessions.iter().partition(|row| row.live());
        let row = |s: &SessionRow| Row {
            key: format!("agent:{}", s.session),
            kind: RowKind::AgentSession(s.session.clone()),
            expanded: false,
            children: Vec::new(),
        };
        children.extend(live.into_iter().map(row));
        if !ended.is_empty() {
            children.push(Row {
                key: "agents:ended".into(),
                kind: RowKind::AgentsEnded(ended.len()),
                expanded: self.ui.expanded.contains("agents:ended"),
                children: ended.into_iter().map(row).collect(),
            });
        }
        Row {
            key: "agents".into(),
            kind: RowKind::Agents,
            expanded: self.ui.expanded.contains("agents"),
            children,
        }
    }

    /// `workshop.agents.*`. Returns false for any other command.
    pub(crate) fn agents_command(&mut self, id: u64, command: &str, args: &Value) -> bool {
        match command {
            "workshop.agents.select" => {
                let Some(session) = args["session"].as_str() else {
                    self.push_workshop(Effect::Reply {
                        id,
                        rc: 10,
                        body: json!({"error_code":"ARGUMENT","message":"session must be a string"}),
                    });
                    return true;
                };
                if !self
                    .agents
                    .sessions
                    .iter()
                    .any(|row| row.session == session)
                {
                    self.push_workshop(Effect::Reply {
                        id,
                        rc: 14,
                        body: json!({"error_code":"UNKNOWN_SESSION","message":format!("no agent session {session}")}),
                    });
                    return true;
                }
                self.ui.expanded.insert("agents".into());
                self.ui.row_key = Some(format!("agent:{session}"));
                self.ui.selected = None;
                self.workshop.selected = None;
                self.select_session(session);
                self.push_workshop(Effect::Reply {
                    id,
                    rc: 0,
                    body: self.agents_info(),
                });
                true
            }
            "workshop.agents.new"
            | "workshop.agents.prompt"
            | "workshop.agents.cancel"
            | "workshop.agents.close"
            | "workshop.agents.resume"
            | "workshop.agents.set" => {
                if let Some(session) = self.caller.as_deref().and_then(agent_caller_session) {
                    self.push_workshop(Effect::Reply {
                        id,
                        rc: 17,
                        body: json!({"error_code":"USE_AGENT","message":format!("agent session {session}: call agent.* directly, not through Workshop")}),
                    });
                    return true;
                }
                match agent_request(command, args, self.agents.selected.as_deref()) {
                    Ok((agent_command, body)) => self.act(&agent_command, body, Some(id)),
                    Err(message) => self.push_workshop(Effect::Reply {
                        id,
                        rc: 10,
                        body: json!({"error_code":"ARGUMENT","message":message}),
                    }),
                }
                true
            }
            "workshop.agents" => {
                self.push_workshop(Effect::Reply {
                    id,
                    rc: 0,
                    body: self.agents_info(),
                });
                true
            }
            _ => false,
        }
    }

    /// Send one acting command to agentd; `reply` is the Bus command to
    /// answer with agentd's reply (`None` for the window's own actions).
    fn act(&mut self, command: &str, body: Value, reply: Option<u64>) {
        let ticket = self.agent_call(command, body);
        self.agents.replies.insert(ticket, reply);
    }

    /// An acting call came back: answer its Bus command, follow the result.
    fn acted(
        &mut self,
        _ticket: u64,
        command: &str,
        reply: Option<u64>,
        result: &Result<(u8, Value), String>,
    ) {
        let (rc, body) = match result {
            Ok((rc, body)) => (*rc, body.clone()),
            Err(message) => (10, json!({"error_code":"UNAVAILABLE","message":message})),
        };
        if rc != 0 {
            self.agents.error = body["message"].as_str().map(str::to_owned);
        } else {
            self.agents.error = None;
        }
        if rc == 0
            && command == "agent.session.new"
            && let Some(session) = body["session"].as_str()
        {
            let session = session.to_owned();
            self.ui.expanded.insert("agents".into());
            self.ui.row_key = Some(format!("agent:{session}"));
            self.select_session(&session);
        }
        if let Some(id) = reply {
            self.push_workshop(Effect::Reply { id, rc, body });
        }
        self.refresh_agents();
    }

    /// Start a session from the New session form.
    pub fn agents_start(&mut self) {
        let agent = self.agents.new_agent.trim().to_owned();
        let cwd = self.agents.new_cwd.trim().to_owned();
        if agent.is_empty() || cwd.is_empty() {
            return;
        }
        self.act("agent.session.new", json!({"agent":agent,"cwd":cwd}), None);
    }

    /// Send the composer's text to the selected session.
    pub fn agents_send(&mut self) {
        let text = self.agents.draft.trim().to_owned();
        let Some(session) = self.agents.selected.clone() else {
            return;
        };
        if text.is_empty() || !self.agents_can_send() {
            return;
        }
        self.agents.draft.clear();
        self.act(
            "agent.session.prompt",
            json!({"session":session,"content":text}),
            None,
        );
    }

    /// Cancel, close or resume the selected session.
    pub fn agents_action(&mut self, op: &str) {
        let Some(session) = self.agents.selected.clone() else {
            return;
        };
        if matches!(op, "cancel" | "close" | "resume") {
            self.act(
                &format!("agent.session.{op}"),
                json!({"session":session}),
                None,
            );
        }
    }

    /// Set one config option of the selected session.
    pub fn agents_set(&mut self, option: &str, value: &str) {
        let Some(session) = self.agents.selected.clone() else {
            return;
        };
        self.act(
            "agent.session.set",
            json!({"session":session,"option":option,"value":value}),
            None,
        );
    }

    /// The composer can send: a selected session that is ready.
    pub fn agents_can_send(&self) -> bool {
        self.agents
            .selected_row()
            .is_some_and(|row| matches!(row.status.as_str(), "ready" | "starting"))
    }

    /// The live count and cap from `agent.agents`.
    pub fn agents_at_cap(&self) -> bool {
        let live = self.agents.instances["live"].as_u64().unwrap_or_default();
        let cap = self.agents.instances["max_live"]
            .as_u64()
            .unwrap_or(u64::MAX);
        live >= cap
    }

    /// A session row's name: the agent and the last part of its cwd.
    pub fn session_label(&self, session: &str) -> String {
        match self
            .agents
            .sessions
            .iter()
            .find(|row| row.session == session)
        {
            Some(row) => {
                let place = std::path::Path::new(&row.cwd)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&row.cwd);
                format!("{} · {place}", row.agent)
            }
            None => session.to_owned(),
        }
    }

    /// What `workshop.info` and `workshop.agents` report.
    pub fn agents_info(&self) -> Value {
        json!({
            "available": self.agents.available,
            "error": self.agents.error,
            "sessions": self.agents.sessions,
            "selected": self.agents.selected,
            "transcript": {
                "seq": self.agents.transcript.seq,
                "events": self.agents.transcript.events.len(),
                "evicted": self.agents.transcript.evicted,
                "gap": self.agents.transcript.gap,
                "filling": self.agents.transcript.filling,
                "items": self.agents.items(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let mut engine = Engine::new(|key| key.to_owned());
        engine.take_effects();
        engine
    }

    fn calls(engine: &mut Engine) -> Vec<(u64, String, Value)> {
        engine
            .take_effects()
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Agent {
                    ticket,
                    command,
                    body,
                } => Some((ticket, command, body)),
                _ => None,
            })
            .collect()
    }

    fn chunk(seq: u64, text: &str) -> Value {
        json!({"session":"s1","seq":seq,"kind":"agent_message_chunk",
            "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}})
    }

    fn listed(engine: &mut Engine, sessions: Value) {
        let list = calls(engine)
            .into_iter()
            .find(|(_, command, _)| command == "agent.session.list")
            .expect("a list call");
        engine.agent_completed(
            list.0,
            "agent.session.list",
            Ok((0, json!({"sessions":sessions}))),
        );
    }

    #[test]
    fn sessions_become_rows_live_first_with_one_badge() {
        let mut e = engine();
        e.refresh_agents();
        listed(
            &mut e,
            json!([
                {"session":"old","agent":"claude","cwd":"/w","status":"ended","seq":9},
                {"session":"s1","agent":"claude","cwd":"/w","status":"running","seq":3,"open_permissions":1},
                {"session":"s2","agent":"codex","cwd":"/w","status":"ready","seq":2}
            ]),
        );
        let tree = e.agents_tree();
        let keys: Vec<_> = tree.children.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(keys, ["agents:new", "agent:s1", "agent:s2", "agents:ended"]);
        assert_eq!(tree.children[3].children[0].key, "agent:old");
        assert_eq!(e.agents.sessions[0].badge(), "question");
        assert_eq!(e.agents.sessions[1].badge(), "ready");
    }

    #[test]
    fn a_missing_agentd_is_one_row_not_an_empty_tree() {
        let mut e = engine();
        e.refresh_agents();
        let effects = calls(&mut e);
        let (ticket, _, _) = effects
            .iter()
            .find(|(_, c, _)| c == "agent.session.list")
            .unwrap();
        e.agent_completed(
            *ticket,
            "agent.session.list",
            Ok((10, json!({"message":"Service 'agent' not found"}))),
        );
        let tree = e.agents_tree();
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].kind, RowKind::AgentsDown);
    }

    #[test]
    fn lists_are_coalesced_while_one_is_in_flight() {
        let mut e = engine();
        e.refresh_agents();
        e.refresh_agents();
        e.refresh_agents();
        let lists: Vec<_> = calls(&mut e)
            .into_iter()
            .filter(|(_, c, _)| c == "agent.session.list")
            .collect();
        assert_eq!(lists.len(), 1);
        e.agent_completed(
            lists[0].0,
            "agent.session.list",
            Ok((0, json!({"sessions":[]}))),
        );
        let again: Vec<_> = calls(&mut e)
            .into_iter()
            .filter(|(_, c, _)| c == "agent.session.list")
            .collect();
        assert_eq!(
            again.len(),
            1,
            "one follow-up list for the asks made meanwhile"
        );
    }

    #[test]
    fn chunks_merge_and_tool_updates_fold_into_one_row() {
        let events: VecDeque<Value> = [
            json!({"seq":1,"kind":"prompt","content":[{"type":"text","text":"hi"}]}),
            chunk(2, "Hel"),
            chunk(3, "lo"),
            json!({"seq":4,"kind":"tool_call","update":{"toolCallId":"t1","title":"Read a.txt","kind":"read","status":"pending"}}),
            json!({"seq":5,"kind":"tool_call_update","update":{"toolCallId":"t1","status":"completed"}}),
            chunk(6, "done"),
            json!({"seq":7,"kind":"usage_update","update":{}}),
            json!({"seq":8,"kind":"stop","stop":"end_turn"}),
        ]
        .into();
        let out = items(&events);
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], Item::Prompt { text: "hi".into() });
        assert_eq!(
            out[1],
            Item::Message {
                text: "Hello".into()
            }
        );
        match &out[2] {
            Item::Tool {
                title,
                status,
                tool,
                ..
            } => {
                assert_eq!(
                    (title.as_str(), status.as_str(), tool.as_str()),
                    ("Read a.txt", "completed", "read")
                );
            }
            other => panic!("expected a tool row, got {other:?}"),
        }
        assert_eq!(
            out[3],
            Item::Message {
                text: "done".into()
            }
        );
        assert_eq!(
            out[4],
            Item::Stop {
                reason: "end_turn".into()
            }
        );
    }

    fn select_and_fill(e: &mut Engine, events: Vec<Value>, next: u64) {
        fill_with(e, events, next, 0);
    }

    fn fill_with(e: &mut Engine, events: Vec<Value>, next: u64, open: u64) {
        e.select_session("s1");
        let (ticket, command, body) = calls(e).pop().unwrap();
        assert_eq!(command, "agent.session.read");
        assert_eq!(body["since"], 0);
        e.agent_completed(
            ticket,
            "agent.session.read",
            Ok((0, json!({"session":{"session":"s1","status":"running","seq":next,"open_permissions":open},"events":events,"more":false,"next":next}))),
        );
    }

    #[test]
    fn selecting_fills_from_the_log_then_appends_live_events_in_order() {
        let mut e = engine();
        select_and_fill(&mut e, vec![chunk(1, "a"), chunk(2, "b")], 2);
        assert!(!e.agents.transcript.filling);
        e.agent_topic("agent.session.updated", &chunk(3, "c"));
        e.agent_topic("agent.session.updated", &chunk(3, "c"));
        assert_eq!(e.agents.transcript.seq, 3);
        assert_eq!(e.agents.items(), vec![Item::Message { text: "abc".into() }]);
        assert!(
            calls(&mut e)
                .iter()
                .all(|(_, c, _)| c != "agent.session.read")
        );
    }

    #[test]
    fn a_seq_gap_or_a_truncated_marker_reads_back_from_the_ring() {
        let mut e = engine();
        select_and_fill(&mut e, vec![chunk(1, "a")], 1);
        e.agent_topic("agent.session.updated", &chunk(4, "d"));
        let read = calls(&mut e)
            .into_iter()
            .find(|(_, c, _)| c == "agent.session.read")
            .unwrap();
        assert_eq!(read.2["since"], 1);
        assert!(e.agents.transcript.filling);
        // A live event while filling waits for the read.
        e.agent_topic("agent.session.updated", &chunk(5, "e"));
        e.agent_completed(
            read.0,
            "agent.session.read",
            Ok((0, json!({"session":{"session":"s1","status":"running"},"events":[chunk(2,"b"),chunk(3,"c"),chunk(4,"d")],"more":false,"next":4}))),
        );
        let again = calls(&mut e)
            .into_iter()
            .find(|(_, c, _)| c == "agent.session.read")
            .unwrap();
        assert_eq!(
            again.2["since"], 4,
            "the event seen while filling is read too"
        );
        let mut e2 = engine();
        select_and_fill(&mut e2, vec![chunk(1, "a")], 1);
        e2.agent_topic("agent.session.updated", &json!({"session":"s1","seq":2,"kind":"agent_message_chunk","truncated":true,"bytes":300000}));
        assert!(
            calls(&mut e2)
                .iter()
                .any(|(_, c, b)| c == "agent.session.read" && b["since"] == 1)
        );
    }

    #[test]
    fn the_ring_is_bounded_and_marks_eviction() {
        let mut t = Transcript::new("s1");
        for seq in 1..=(RING_EVENTS as u64 + 5) {
            assert!(t.push(chunk(seq, "x")));
        }
        assert!(t.events.len() <= RING_EVENTS);
        assert!(t.evicted);
        assert_eq!(t.items, items(&t.events), "the kept items match the ring");
        assert!(!t.push(chunk(1, "late")), "an old seq is never appended");
        assert!(!t.gap);
    }

    #[test]
    fn a_hole_in_the_log_is_marked_and_never_reread_in_a_loop() {
        let mut e = engine();
        select_and_fill(&mut e, vec![chunk(1, "a")], 1);
        // seq 2 was published but never logged; seq 3 shows the gap.
        e.agent_topic("agent.session.updated", &chunk(3, "c"));
        let read = calls(&mut e)
            .into_iter()
            .find(|(_, c, _)| c == "agent.session.read")
            .unwrap();
        e.agent_completed(
            read.0,
            "agent.session.read",
            Ok((0, json!({"session":{"session":"s1","status":"running"},"events":[chunk(3, "c")],"more":false,"next":3}))),
        );
        assert_eq!(e.agents.transcript.seq, 3);
        assert!(e.agents.transcript.gap);
        assert!(!e.agents.transcript.filling);
        assert!(
            calls(&mut e)
                .iter()
                .all(|(_, c, _)| c != "agent.session.read"),
            "no second read"
        );
        // A read that brings nothing new does not loop either.
        e.agent_topic("agent.session.updated", &chunk(5, "e"));
        let read = calls(&mut e)
            .into_iter()
            .find(|(_, c, _)| c == "agent.session.read")
            .unwrap();
        e.agent_completed(
            read.0,
            "agent.session.read",
            Ok((0, json!({"session":{"session":"s1","status":"running"},"events":[],"more":false,"next":3}))),
        );
        assert!(!e.agents.transcript.filling);
        assert!(
            calls(&mut e)
                .iter()
                .all(|(_, c, _)| c != "agent.session.read")
        );
    }

    #[test]
    fn a_timeout_keeps_the_rows_and_only_a_missing_service_is_down() {
        let mut e = engine();
        e.refresh_agents();
        listed(
            &mut e,
            json!([{"session":"s1","agent":"claude","status":"ready","seq":1,"permission_requests":["7"],"open_permissions":1}]),
        );
        assert_eq!(e.agents.sessions[0].permission_requests, ["7"]);
        e.refresh_agents();
        let list = calls(&mut e)
            .into_iter()
            .find(|(_, c, _)| c == "agent.session.list")
            .unwrap();
        e.agent_completed(list.0, "agent.session.list", Err("timed out".into()));
        assert_eq!(e.agents.sessions.len(), 1);
        assert_eq!(e.agents.available, Some(true));
        assert_eq!(e.agents.error.as_deref(), Some("timed out"));
        e.refresh_agents();
        let list = calls(&mut e)
            .into_iter()
            .find(|(_, c, _)| c == "agent.session.list")
            .unwrap();
        e.agent_completed(
            list.0,
            "agent.session.list",
            Ok((10, json!({"message":"Service 'agent' not found"}))),
        );
        assert_eq!(e.agents.available, Some(false));
        assert!(e.agents.sessions.is_empty());
    }

    #[test]
    fn a_topic_gap_reads_status_and_the_transcript_again() {
        let mut e = engine();
        select_and_fill(&mut e, vec![chunk(1, "a")], 1);
        e.agents_resync();
        let commands: Vec<_> = calls(&mut e).into_iter().map(|(_, c, _)| c).collect();
        assert!(commands.contains(&"agent.session.list".to_owned()));
        assert!(commands.contains(&"agent.session.read".to_owned()));
    }

    #[test]
    fn row_status_survives_ring_eviction() {
        let mut e = engine();
        e.refresh_agents();
        listed(
            &mut e,
            json!([{"session":"s1","agent":"claude","status":"running","seq":1,"open_permissions":1}]),
        );
        fill_with(&mut e, vec![chunk(1, "a")], 1, 1);
        for seq in 2..=(RING_EVENTS as u64 + 10) {
            e.agent_topic("agent.session.updated", &chunk(seq, "x"));
        }
        assert!(e.agents.transcript.evicted);
        assert_eq!(e.agents.selected_row().unwrap().badge(), "question");
    }

    #[test]
    fn select_command_refuses_unknown_sessions_and_reports_the_selection() {
        let mut e = engine();
        e.refresh_agents();
        listed(
            &mut e,
            json!([{"session":"s1","agent":"claude","status":"ready","seq":0}]),
        );
        assert!(e.agents_command(7, "workshop.agents.select", &json!({"session":"nope"})));
        let reply = e
            .take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                Effect::Reply { id: 7, rc, .. } => Some(rc),
                _ => None,
            });
        assert_eq!(reply, Some(14));
        assert!(e.agents_command(8, "workshop.agents.select", &json!({"session":"s1"})));
        let effects = e.take_effects();
        assert!(effects.iter().any(|effect| matches!(effect, Effect::Agent { command, .. } if command == "agent.session.read")));
        let body = effects.iter().find_map(|effect| match effect {
            Effect::Reply { id: 8, body, .. } => Some(body.clone()),
            _ => None,
        });
        assert_eq!(body.unwrap()["selected"], "s1");
        assert_eq!(e.ui.row_key.as_deref(), Some("agent:s1"));
    }

    #[test]
    fn a_session_caller_is_recognised_locally_and_over_the_mesh() {
        assert_eq!(
            agent_caller_session("local:mcp-s-ab12cd-1f2e").as_deref(),
            Some("ab12cd")
        );
        assert_eq!(
            agent_caller_session("mesh:mcp-s-ab12cd-x@peer").as_deref(),
            Some("ab12cd")
        );
        assert_eq!(agent_caller_session("local:workshop"), None);
        assert_eq!(
            agent_caller_session("local:mcp-s-AB12-x"),
            None,
            "upper case is not agentd's form"
        );
        assert_eq!(agent_caller_session("local:mcp-s-abc-x"), None, "too short");
        assert_eq!(agent_caller_session("local:mcp-s-ab12cd-"), None);
        assert_eq!(agent_caller_session("anon"), None);
    }

    fn reply_to(effects: &[Effect], wanted: u64) -> Option<(u8, Value)> {
        effects.iter().find_map(|effect| match effect {
            Effect::Reply { id, rc, body } if *id == wanted => Some((*rc, body.clone())),
            _ => None,
        })
    }

    #[test]
    fn an_agent_session_cannot_act_through_workshop() {
        let mut e = engine();
        for (caller, command) in [
            ("local:mcp-s-ab12cd-1f2e", "workshop.agents.prompt"),
            ("mesh:mcp-s-ab12cd-1f2e@peer", "workshop.agents.new"),
            ("local:mcp-s-ab12cd-1f2e", "workshop.agents.set"),
        ] {
            e.caller = Some(caller.into());
            assert!(e.agents_command(
                5,
                command,
                &json!({"session":"s1","text":"x","agent":"a","cwd":"/w"})
            ));
            let effects = e.take_effects();
            assert_eq!(
                reply_to(&effects, 5).unwrap().0,
                17,
                "{command} from {caller}"
            );
            assert!(
                !effects
                    .iter()
                    .any(|effect| matches!(effect, Effect::Agent { .. }))
            );
        }
        // select and agents stay open to agents.
        e.caller = Some("local:mcp-s-ab12cd-1f2e".into());
        assert!(e.agents_command(6, "workshop.agents", &json!({})));
        assert_eq!(reply_to(&e.take_effects(), 6).unwrap().0, 0);
    }

    #[test]
    fn a_prompt_goes_to_the_selected_session_and_agentd_answers_the_caller() {
        let mut e = engine();
        select_and_fill(&mut e, vec![chunk(1, "a")], 1);
        e.caller = Some("local:human-cli".into());
        assert!(e.agents_command(9, "workshop.agents.prompt", &json!({"text":"hello"})));
        let (ticket, command, body) = calls(&mut e).pop().unwrap();
        assert_eq!(command, "agent.session.prompt");
        assert_eq!(body, json!({"session":"s1","content":"hello"}));
        e.agent_completed(
            ticket,
            &command,
            Ok((15, json!({"error_code":"AGENT_RUNNING","message":"busy"}))),
        );
        let effects = e.take_effects();
        assert_eq!(
            reply_to(&effects, 9).unwrap().0,
            15,
            "agentd's own rc reaches the caller"
        );
        assert_eq!(e.agents.error.as_deref(), Some("busy"));
    }

    #[test]
    fn a_new_session_is_selected_once_agentd_answers() {
        let mut e = engine();
        e.agents.new_agent = "claude".into();
        e.agents.new_cwd = "/w".into();
        e.agents_start();
        let (ticket, command, body) = calls(&mut e).pop().unwrap();
        assert_eq!(
            (command.as_str(), body),
            ("agent.session.new", json!({"agent":"claude","cwd":"/w"}))
        );
        e.agent_completed(
            ticket,
            &command,
            Ok((0, json!({"session":"n1","status":"starting"}))),
        );
        assert_eq!(e.agents.selected.as_deref(), Some("n1"));
        assert_eq!(e.ui.row_key.as_deref(), Some("agent:n1"));
        let commands: Vec<_> = calls(&mut e).into_iter().map(|(_, c, _)| c).collect();
        assert!(commands.contains(&"agent.session.read".to_owned()));
        assert!(commands.contains(&"agent.session.list".to_owned()));
    }

    #[test]
    fn the_composer_sends_only_to_a_ready_session_and_clears() {
        let mut e = engine();
        e.refresh_agents();
        listed(
            &mut e,
            json!([{"session":"s1","agent":"claude","status":"running","seq":1}]),
        );
        select_and_fill(&mut e, vec![chunk(1, "a")], 1);
        e.refresh_agents();
        listed(
            &mut e,
            json!([{"session":"s1","agent":"claude","status":"running","seq":1}]),
        );
        e.agents.draft = "next".into();
        e.agents_send();
        assert!(calls(&mut e).is_empty(), "a running turn takes no prompt");
        assert_eq!(e.agents.draft, "next");
        e.refresh_agents();
        listed(
            &mut e,
            json!([{"session":"s1","agent":"claude","status":"ready","seq":2}]),
        );
        e.agents_send();
        let sent: Vec<_> = calls(&mut e)
            .into_iter()
            .filter(|(_, c, _)| c == "agent.session.prompt")
            .collect();
        assert_eq!(sent.len(), 1);
        assert!(e.agents.draft.is_empty());
    }

    #[test]
    fn requests_map_to_agentd_commands() {
        assert_eq!(
            agent_request(
                "workshop.agents.set",
                &json!({"option":"model","value":"opus"}),
                Some("s1")
            )
            .unwrap(),
            (
                "agent.session.set".to_owned(),
                json!({"session":"s1","option":"model","value":"opus"})
            )
        );
        assert_eq!(
            agent_request(
                "workshop.agents.close",
                &json!({"session":"s2"}),
                Some("s1")
            )
            .unwrap()
            .1,
            json!({"session":"s2"})
        );
        assert!(agent_request("workshop.agents.prompt", &json!({"text":"x"}), None).is_err());
        assert!(agent_request("workshop.agents.set", &json!({}), Some("s1")).is_err());
    }
}
