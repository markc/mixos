// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Agents branch: agentd sessions (ACP agents on the Bus) on every node
//! that runs agentd, and the transcript of the selected one. Headless, like
//! the rest of the engine: the shell sends the calls and hands back replies
//! and topics.
//!
//! Status comes from agentd's own answers (`agent.session.list`, the session
//! view a read returns), never from topic seqs alone. Only the selected
//! session's events are held, in a bounded ring; the others keep status.
//! Topics are broker-local, so only the local node's sessions are followed
//! live; a remote session is read on select and on `refresh`.
use crate::{Effect, Engine, Row, RowKind};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

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
/// The local node's name until discovery reports it.
pub const LOCAL: &str = "local";

/// The service that answers for a node's agentd: `agent` on this broker
/// (`None`), `agent.<node>.bus` on a peer.
pub fn agent_service(node: Option<&str>) -> String {
    match node {
        None => "agent".to_owned(),
        Some(node) => format!("agent.{node}.bus"),
    }
}

/// One session as `agent.session.list` reports it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct SessionRow {
    /// The node whose agentd listed it (known from where Workshop asked).
    pub node: String,
    pub session: String,
    pub agent: String,
    pub cwd: String,
    pub parent: Option<String>,
    pub status: String,
    pub seq: u64,
    pub turn: u64,
    pub created_at: String,
    /// Epoch seconds; `None` while live, or from an agentd that does not
    /// report it yet.
    pub ended_at: Option<f64>,
    pub exit_code: Option<i64>,
    pub signal: Value,
    pub end_reason: Option<String>,
    /// The first prompt's first line, once agentd reports it.
    pub title: Option<String>,
    pub open_permissions: u64,
    /// The open permission requests' keys, to answer them.
    pub permission_requests: Vec<String>,
    /// Their cards: {request, title, options, asked_at, expires_at}.
    pub permissions: Vec<Value>,
}

impl SessionRow {
    fn from(value: &Value, node: &str) -> Option<Self> {
        let text = |key: &str| value[key].as_str().map(str::to_owned);
        Some(Self {
            node: node.to_owned(),
            session: value["session"].as_str()?.to_owned(),
            agent: text("agent").unwrap_or_default(),
            cwd: text("cwd").unwrap_or_default(),
            parent: text("parent"),
            status: value["status"].as_str().unwrap_or("unknown").to_owned(),
            seq: value["seq"].as_u64().unwrap_or_default(),
            turn: value["turn"].as_u64().unwrap_or_default(),
            created_at: text("created_at").unwrap_or_default(),
            ended_at: value["ended_at"].as_f64(),
            exit_code: value["exit_code"].as_i64(),
            signal: value["signal"].clone(),
            end_reason: text("end_reason"),
            title: text("title").filter(|title| !title.is_empty()),
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
            permissions: value["permissions"].as_array().cloned().unwrap_or_default(),
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

    /// The row's name: the agent and the last part of its cwd.
    pub fn label(&self) -> String {
        let place = std::path::Path::new(&self.cwd)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.cwd);
        format!("{} · {place}", self.agent)
    }

    /// The tree filter matches the label, the cwd, the agent and the node.
    fn matches(&self, filter: &str) -> bool {
        [&self.label(), &self.cwd, &self.agent, &self.node]
            .iter()
            .any(|text| text.to_lowercase().contains(filter))
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

    /// The oldest seq held (0: none).
    pub fn first(&self) -> u64 {
        self.events
            .front()
            .and_then(|event| event["seq"].as_u64())
            .unwrap_or_default()
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
    let (session, _) = rest.split_once('-')?;
    let valid = (4..=12).contains(&session.len())
        && session
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    valid.then(|| session.to_owned())
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
        "answer" => {
            let request = match &args["request"] {
                Value::String(request) => request.clone(),
                Value::Number(request) => request.to_string(),
                _ => return Err("request must be the request key".into()),
            };
            let option = args["option"]
                .as_str()
                .ok_or("option must be an offered optionId")?;
            return Ok((
                "agent.permission.respond".to_owned(),
                json!({"session":session()?,"request":request,"option":option}),
            ));
        }
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

/// One node's agentd, as its `agent.agents` and `agent.session.list`
/// answered.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct NodeAgents {
    /// `None` until agentd answered once; `Some(false)`: not on that node.
    pub available: Option<bool>,
    /// Why the last list failed; the last known rows stay.
    pub error: Option<String>,
    /// `agent.agents` as agentd answered it.
    pub instances: Value,
    /// Live first (newest first as agentd lists them), then ended.
    pub sessions: Vec<SessionRow>,
    #[serde(skip)]
    list_ticket: Option<u64>,
    #[serde(skip)]
    instances_ticket: Option<u64>,
    #[serde(skip)]
    list_again: bool,
}

impl NodeAgents {
    /// The node has a group in the tree: its agentd answered, or listing it
    /// failed for a reason other than a missing service.
    pub fn shown(&self) -> bool {
        self.available == Some(true) || (self.available.is_none() && self.error.is_some())
    }

    fn order(&mut self) {
        self.sessions.sort_by_key(|row| !row.live());
    }
}

/// The selected session and the node whose agentd runs it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Selected {
    pub node: String,
    pub session: String,
}

/// What `workshop.agents.filter` narrowed the list to.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct AgentFilter {
    pub node: Option<String>,
    pub agent: Option<String>,
}

/// Everything the Agents branch knows.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Agents {
    /// The name the local node's entry is kept under (empty: [`LOCAL`]).
    local: String,
    /// One entry per node whose agentd was asked.
    pub nodes: BTreeMap<String, NodeAgents>,
    /// The last acting call or read that failed.
    pub error: Option<String>,
    pub selected: Option<Selected>,
    pub filter: AgentFilter,
    pub transcript: Transcript,
    /// The selected session's `config_options`, from its ready event.
    pub config_options: Value,
    /// The composer's text for the selected session.
    pub draft: String,
    /// The selected session's legacy `modes` ({availableModes, currentModeId}).
    pub modes: Value,
    /// A prompt from the window is on its way: no second one.
    #[serde(skip)]
    prompting: bool,
    /// The New session form.
    pub new_agent: String,
    pub new_cwd: String,
    /// Every call in flight: ticket to the node asked.
    #[serde(skip)]
    tickets: BTreeMap<u64, String>,
    /// Acting calls in flight: ticket to the Bus command to answer.
    #[serde(skip)]
    replies: BTreeMap<u64, Option<u64>>,
    #[serde(skip)]
    read_ticket: Option<u64>,
    #[serde(skip)]
    next_ticket: u64,
}

impl Agents {
    /// The local node's name, as discovery found it.
    pub fn local_name(&self) -> &str {
        if self.local.is_empty() {
            LOCAL
        } else {
            &self.local
        }
    }

    pub fn is_local(&self, node: &str) -> bool {
        node == self.local_name()
    }

    pub fn node(&self, node: &str) -> Option<&NodeAgents> {
        self.nodes.get(node)
    }

    pub fn row(&self, node: &str, session: &str) -> Option<&SessionRow> {
        self.node(node)?
            .sessions
            .iter()
            .find(|row| row.session == session)
    }

    pub fn selected_row(&self) -> Option<&SessionRow> {
        let selected = self.selected.as_ref()?;
        self.row(&selected.node, &selected.session)
    }

    pub fn is_selected(&self, node: &str, session: &str) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|s| s.node == node && s.session == session)
    }

    pub fn items(&self) -> &[Item] {
        &self.transcript.items
    }

    /// The nodes as the tree lists them: the local node first, then peers
    /// by name.
    pub fn ordered(&self) -> Vec<(&str, &NodeAgents)> {
        let local = self.local_name();
        let mut out: Vec<_> = self
            .nodes
            .get_key_value(local)
            .map(|(node, entry)| (node.as_str(), entry))
            .into_iter()
            .collect();
        out.extend(
            self.nodes
                .iter()
                .filter(|(node, _)| node.as_str() != local)
                .map(|(node, entry)| (node.as_str(), entry)),
        );
        out
    }

    /// The nodes whose list holds `session` (one id can be listed by two
    /// agentds sharing a log; they are never merged).
    pub fn nodes_listing(&self, session: &str) -> Vec<String> {
        self.ordered()
            .into_iter()
            .filter(|(_, entry)| entry.sessions.iter().any(|row| row.session == session))
            .map(|(node, _)| node.to_owned())
            .collect()
    }

    /// Rows that pass `workshop.agents.filter`.
    fn passes(&self, row: &SessionRow) -> bool {
        self.filter.node.as_ref().is_none_or(|node| *node == row.node)
            && self.filter.agent.as_ref().is_none_or(|agent| *agent == row.agent)
    }
}

/// A refusal: rc and body.
type Refusal = (u8, Value);

fn refusal(rc: u8, code: &str, message: String) -> Refusal {
    (rc, json!({"error_code":code,"message":message}))
}

impl Engine {
    /// Key the local node's entry, its tree rows and its calls in flight by
    /// the name discovery found (`local` until then).
    fn sync_local_agents(&mut self) {
        let name = self
            .snapshot
            .nodes
            .iter()
            .find(|node| node.local)
            .map(|node| node.name.clone())
            .unwrap_or_else(|| self.agents.local_name().to_owned());
        let old = self.agents.local_name().to_owned();
        if name == old {
            self.agents.local = name;
            return;
        }
        if let Some(mut entry) = self.agents.nodes.remove(&old) {
            for row in &mut entry.sessions {
                row.node = name.clone();
            }
            self.agents.nodes.insert(name.clone(), entry);
        }
        for node in self.agents.tickets.values_mut() {
            if *node == old {
                node.clone_from(&name);
            }
        }
        if let Some(selected) = &mut self.agents.selected
            && selected.node == old
        {
            selected.node.clone_from(&name);
        }
        let rekey = |key: &str| -> Option<String> {
            for prefix in ["agents:node:", "agents:new:", "agents:ended:", "agents:error:"] {
                if key.strip_prefix(prefix) == Some(old.as_str()) {
                    return Some(format!("{prefix}{name}"));
                }
            }
            key.strip_prefix(&format!("agent:{old}:"))
                .map(|session| format!("agent:{name}:{session}"))
        };
        self.ui.expanded = self
            .ui
            .expanded
            .iter()
            .map(|key| rekey(key).unwrap_or_else(|| key.clone()))
            .collect();
        if let Some(key) = self.ui.row_key.as_deref().and_then(rekey) {
            self.ui.row_key = Some(key);
        }
        self.agents.local = name;
    }

    /// The nodes whose agentd is listed: this node always (a missing
    /// service says it has none), and every peer that advertises
    /// `agent.<node>.bus`.
    pub fn agent_nodes(&self) -> Vec<String> {
        let mut nodes = vec![self.agents.local_name().to_owned()];
        for node in &self.snapshot.nodes {
            if !node.local
                && !nodes.contains(&node.name)
                && node
                    .services
                    .contains(&agent_service(Some(node.name.as_str())))
            {
                nodes.push(node.name.clone());
            }
        }
        nodes
    }

    /// The `Effect::Agent` node for `node`: `None` for this node.
    fn agent_target(&self, node: &str) -> Option<String> {
        (!self.agents.is_local(node)).then(|| node.to_owned())
    }

    fn agent_call(&mut self, node: &str, command: &str, body: Value) -> u64 {
        self.agents.next_ticket += 1;
        let ticket = self.agents.next_ticket;
        self.agents.tickets.insert(ticket, node.to_owned());
        let node = self.agent_target(node);
        self.push_workshop(Effect::Agent {
            ticket,
            node,
            command: command.to_owned(),
            body,
        });
        ticket
    }

    /// List every node's agentd again.
    pub fn refresh_agents(&mut self) {
        self.sync_local_agents();
        for node in self.agent_nodes() {
            self.refresh_node(&node);
        }
    }

    /// Read one node's instances and sessions again. Coalesced per node:
    /// one list is in flight at a time, and one more follows if asked for
    /// meanwhile.
    pub fn refresh_node(&mut self, node: &str) {
        let entry = self.agents.nodes.entry(node.to_owned()).or_default();
        if entry.list_ticket.is_some() {
            entry.list_again = true;
            return;
        }
        // The live count changes with every session: read it each time.
        let instances_ticket = self.agent_call(node, "agent.agents", json!({}));
        let ticket = self.agent_call(node, "agent.session.list", json!({}));
        if let Some(entry) = self.agents.nodes.get_mut(node) {
            entry.list_ticket = Some(ticket);
            entry.instances_ticket = Some(instances_ticket);
        }
    }

    /// Discovery finished: the local entry takes its node's name, peers
    /// that newly advertise agentd are listed, and peers that left the
    /// mesh are dropped (unless their session is selected).
    pub(crate) fn agents_discovered(&mut self) {
        self.sync_local_agents();
        let local = self.agents.local_name().to_owned();
        let selected = self.agents.selected.as_ref().map(|s| s.node.clone());
        let known: Vec<String> = self.snapshot.nodes.iter().map(|n| n.name.clone()).collect();
        self.agents.nodes.retain(|node, _| {
            *node == local || known.contains(node) || selected.as_ref() == Some(node)
        });
        // Invalidate even retained selections when their node leaves. A
        // late reply must not refresh an offline selection or resurrect it
        // after the node rejoins with a new generation of calls.
        self.agents.tickets.retain(|_, node| *node == local || known.contains(node));
        for (node, entry) in &mut self.agents.nodes {
            if *node != local && !known.contains(node) {
                entry.list_ticket = None;
                entry.instances_ticket = None;
                entry.list_again = false;
            }
        }
        for node in self.agent_nodes() {
            if node != local && self.agents.node(&node).is_none_or(|entry| entry.available == Some(false)) {
                self.refresh_node(&node);
            }
        }
    }

    /// The broker's service list changed: this node's agentd may have come
    /// up since it was found missing.
    pub(crate) fn agents_changed(&mut self) {
        let local = self.agents.local_name().to_owned();
        if self
            .agents
            .node(&local)
            .is_some_and(|entry| entry.available == Some(false))
        {
            self.refresh_node(&local);
        }
    }

    /// Select one session on `node`: its transcript is read back from the
    /// start.
    pub fn select_session(&mut self, node: &str, session: &str) {
        if self.agents.is_selected(node, session) && self.agents.transcript.seq > 0 {
            return;
        }
        self.agents.selected = Some(Selected {
            node: node.to_owned(),
            session: session.to_owned(),
        });
        self.agents.transcript = Transcript::new(session);
        self.agents.config_options = Value::Null;
        self.agents.modes = Value::Null;
        self.agents.draft.clear();
        self.agents.prompting = false;
        self.read_transcript();
    }

    fn read_transcript(&mut self) {
        let Some(selected) = self.agents.selected.clone() else {
            return;
        };
        self.agents.transcript.filling = true;
        let since = self.agents.transcript.seq;
        let ticket = self.agent_call(
            &selected.node,
            "agent.session.read",
            json!({"session":selected.session,"since":since,"limit":READ_LIMIT}),
        );
        self.agents.read_ticket = Some(ticket);
    }

    /// A node's agentd answered: it is available, and its group opens the
    /// first time it does.
    fn node_answered(&mut self, node: &str) -> &mut NodeAgents {
        if self
            .agents
            .node(node)
            .is_none_or(|entry| entry.available != Some(true))
        {
            self.ui.expanded.insert(format!("agents:node:{node}"));
        }
        let entry = self.agents.nodes.entry(node.to_owned()).or_default();
        entry.available = Some(true);
        entry.error = None;
        entry
    }

    /// One reply from agentd: `rc` and the decoded body, or a transport error.
    pub fn agent_completed(
        &mut self,
        ticket: u64,
        command: &str,
        result: Result<(u8, Value), String>,
    ) {
        let Some(node) = self.agents.tickets.remove(&ticket) else {
            return;
        };
        if let Some(reply) = self.agents.replies.remove(&ticket) {
            self.acted(&node, command, reply, &result);
            return;
        }
        // Validate before both success and failure handling. In particular,
        // an older instances timeout cannot poison a newer successful list.
        match command {
            "agent.agents" => {
                let Some(entry) = self.agents.nodes.get_mut(&node) else { return };
                if entry.instances_ticket != Some(ticket) { return; }
                entry.instances_ticket = None;
            }
            "agent.session.list" => {
                if self.agents.node(&node).is_none_or(|entry| entry.list_ticket != Some(ticket)) { return; }
            }
            _ => {}
        }
        let (rc, body) = match result {
            Ok(reply) => reply,
            Err(message) => {
                // A timeout or a transport fault: keep what is known.
                if command == "agent.session.read" {
                    if self.agents.read_ticket == Some(ticket) {
                        self.agents.read_ticket = None;
                        self.agents.transcript.filling = false;
                        self.agents.error = Some(format!("agent.session.read: {message}"));
                    }
                    return;
                }
                let entry = self.agents.nodes.entry(node.clone()).or_default();
                let mut again = false;
                if command == "agent.session.list" && entry.list_ticket == Some(ticket) {
                    entry.list_ticket = None;
                    again = std::mem::take(&mut entry.list_again);
                }
                entry.error = Some(message);
                if again { self.refresh_node(&node); }
                return;
            }
        };
        match command {
            "agent.agents" => {
                if rc == 0 {
                    self.node_answered(&node).instances = body;
                } else {
                    self.agent_unavailable(&node, rc, &body);
                }
            }
            "agent.session.list" => {
                let entry = self.agents.nodes.entry(node.clone()).or_default();
                if entry.list_ticket != Some(ticket) {
                    return;
                }
                entry.list_ticket = None;
                if rc == 0 {
                    let rows = body["sessions"]
                        .as_array()
                        .map(|rows| {
                            rows.iter()
                                .filter_map(|row| SessionRow::from(row, &node))
                                .collect()
                        })
                        .unwrap_or_default();
                    let entry = self.node_answered(&node);
                    entry.sessions = rows;
                    entry.order();
                } else {
                    self.agent_unavailable(&node, rc, &body);
                }
                if self
                    .agents
                    .nodes
                    .get_mut(&node)
                    .is_some_and(|entry| std::mem::take(&mut entry.list_again))
                {
                    self.refresh_node(&node);
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
                let Some(session) = body["session"]["session"].as_str() else {
                    return;
                };
                if !self.agents.is_selected(&node, session) {
                    return;
                }
                if let Some(row) = SessionRow::from(&body["session"], &node) {
                    self.update_row(&node, row);
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

    fn agent_unavailable(&mut self, node: &str, rc: u8, body: &Value) {
        // Only the broker's "no such service" means agentd is not on that
        // node; any other refusal keeps the rows and shows the message.
        let missing = rc == 10
            && body["message"]
                .as_str()
                .is_some_and(|message| message.contains("not found"));
        let entry = self.agents.nodes.entry(node.to_owned()).or_default();
        entry.error = Some(
            body["message"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("agentd answered rc {rc}")),
        );
        if missing {
            entry.available = Some(false);
            entry.sessions.clear();
            entry.instances = Value::Null;
        }
    }

    fn update_row(&mut self, node: &str, row: SessionRow) {
        let entry = self.agents.nodes.entry(node.to_owned()).or_default();
        match entry.sessions.iter_mut().find(|r| r.session == row.session) {
            Some(known) => *known = row,
            None => entry.sessions.insert(0, row),
        }
        entry.order();
    }

    /// Keep one event of the selected session, in order.
    fn take_event(&mut self, event: Value) {
        if event["kind"] == "ready" || event["kind"] == "resumed" {
            self.agents.config_options = event["config_options"].clone();
            self.agents.modes = event["modes"].clone();
        }
        if event["kind"] == "current_mode_update"
            && let Some(mode) = event["update"]["currentModeId"].as_str()
            && self.agents.modes.is_object()
        {
            self.agents.modes["currentModeId"] = json!(mode);
        }
        if event["kind"] == "config_option_update" {
            self.agents.config_options = event["update"]["configOptions"].clone();
        }
        self.agents.transcript.push(event);
    }

    /// One agentd topic delivery. Topics are broker-local: they are the
    /// local node's agentd, never a peer's.
    pub fn agent_topic(&mut self, topic: &str, body: &Value) {
        let Some(session) = body["session"].as_str() else {
            return;
        };
        let local = self.agents.local_name().to_owned();
        match topic {
            "agent.session.updated" => {
                let kind = body["kind"].as_str().unwrap_or_default();
                let known = self.agents.row(&local, session).is_some();
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
                    self.refresh_node(&local);
                }
                if !self.agents.is_selected(&local, session) {
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
            "agent.session.ended" | "agent.permission.requested" => self.refresh_node(&local),
            _ => {}
        }
    }

    /// Topic deliveries were dropped: read the local agentd's state again
    /// rather than trust what the topics said.
    pub fn agents_resync(&mut self) {
        let local = self.agents.local_name().to_owned();
        self.refresh_node(&local);
        if self
            .agents
            .selected
            .as_ref()
            .is_some_and(|s| s.node == local)
            && !self.agents.transcript.filling
        {
            self.read_transcript();
        }
    }

    /// Some session row matches the tree's text filter.
    pub(crate) fn agents_match(&self, filter: &str) -> bool {
        self.agents
            .nodes
            .values()
            .flat_map(|entry| &entry.sessions)
            .any(|row| row.matches(filter))
    }

    /// The Agents branch of the tree: one group per node with agentd.
    pub fn agents_tree(&self) -> Row {
        let mut children = Vec::new();
        for (node, entry) in self.agents.ordered() {
            if !entry.shown()
                || self
                    .agents
                    .filter
                    .node
                    .as_ref()
                    .is_some_and(|wanted| wanted != node)
            {
                continue;
            }
            children.push(self.agents_node_tree(node, entry));
        }
        let answered = self
            .agents
            .nodes
            .values()
            .any(|entry| entry.available.is_some() || entry.error.is_some());
        if answered && !self.agents.nodes.values().any(NodeAgents::shown) {
            children.push(Row {
                key: "agents:down".into(),
                kind: RowKind::AgentsDown,
                expanded: false,
                children: Vec::new(),
            });
        }
        Row {
            key: "agents".into(),
            kind: RowKind::Agents,
            expanded: self.ui.expanded.contains("agents"),
            children,
        }
    }

    fn agents_node_tree(&self, node: &str, entry: &NodeAgents) -> Row {
        let mut children = Vec::new();
        if entry.available == Some(true) {
            children.push(Row {
                key: format!("agents:new:{node}"),
                kind: RowKind::AgentsNew(node.to_owned()),
                expanded: false,
                children: Vec::new(),
            });
        }
        if let Some(error) = &entry.error {
            let service = agent_service(self.agent_target(node).as_deref());
            children.push(Row {
                key: format!("agents:error:{node}"),
                kind: RowKind::AgentsError(format!("{service}: {error}")),
                expanded: false,
                children: Vec::new(),
            });
        }
        let (live, ended): (Vec<_>, Vec<_>) = entry
            .sessions
            .iter()
            .filter(|row| self.agents.passes(row))
            .partition(|row| row.live());
        let row = |s: &SessionRow| Row {
            key: format!("agent:{node}:{}", s.session),
            kind: RowKind::AgentSession {
                node: node.to_owned(),
                session: s.session.clone(),
            },
            expanded: false,
            children: Vec::new(),
        };
        children.extend(live.into_iter().map(row));
        if !ended.is_empty() {
            let key = format!("agents:ended:{node}");
            children.push(Row {
                expanded: self.ui.expanded.contains(&key),
                key,
                kind: RowKind::AgentsEnded {
                    node: node.to_owned(),
                    count: ended.len(),
                },
                children: ended.into_iter().map(row).collect(),
            });
        }
        let key = format!("agents:node:{node}");
        Row {
            expanded: self.ui.expanded.contains(&key),
            key,
            kind: RowKind::AgentsNode(node.to_owned()),
            children,
        }
    }

    /// The node a session-addressed command means: `node` when given (and
    /// it lists the session), else the one node that lists it. Two nodes
    /// listing one id is AMBIGUOUS_SESSION; none is UNKNOWN_SESSION.
    fn session_node(&self, session: &str, node: &Value) -> Result<String, Refusal> {
        match node {
            Value::String(node) => {
                if self.agents.row(node, session).is_some() {
                    Ok(node.clone())
                } else {
                    Err(refusal(
                        14,
                        "UNKNOWN_SESSION",
                        format!("no agent session {session} on {node}"),
                    ))
                }
            }
            Value::Null => {
                let nodes = self.agents.nodes_listing(session);
                match nodes.as_slice() {
                    [] => Err(refusal(
                        14,
                        "UNKNOWN_SESSION",
                        format!("no agent session {session}"),
                    )),
                    [node] => Ok(node.clone()),
                    _ => Err((
                        10,
                        json!({"error_code":"AMBIGUOUS_SESSION","nodes":nodes,
                            "message":format!("agent session {session} is listed by {}; give node", nodes.join(", "))}),
                    )),
                }
            }
            _ => Err(refusal(10, "ARGUMENT", "node must be a string".into())),
        }
    }

    /// The node an acting twin goes to and the session it defaults to:
    /// `node` when given, the session's node when a session is given, else
    /// the selection's (the local node for `new`, or with nothing selected).
    fn acting_node(&self, op: &str, args: &Value) -> Result<(String, Option<String>), Refusal> {
        let selected = self.agents.selected.as_ref();
        match &args["node"] {
            Value::String(node) if !self.agent_nodes().contains(node) => Err(refusal(14, "UNKNOWN_NODE", format!("unknown node {node}"))),
            Value::String(node) => Ok((
                node.clone(),
                selected
                    .filter(|s| s.node == *node)
                    .map(|s| s.session.clone()),
            )),
            Value::Null if op == "workshop.agents.new" => {
                Ok((self.agents.local_name().to_owned(), None))
            }
            Value::Null => match args["session"].as_str() {
                Some(session) => match self.session_node(session, &Value::Null) {
                    Ok(node) => Ok((node, None)),
                    // agentd answers for a session no list holds (yet).
                    Err((14, _)) => Ok((self.agents.local_name().to_owned(), None)),
                    Err(refused) => Err(refused),
                },
                None => Ok(selected
                    .map(|s| (s.node.clone(), Some(s.session.clone())))
                    .unwrap_or_else(|| (self.agents.local_name().to_owned(), None))),
            },
            _ => Err(refusal(10, "ARGUMENT", "node must be a string".into())),
        }
    }

    /// `workshop.agents.*`. Returns false for any other command.
    pub(crate) fn agents_command(&mut self, id: u64, command: &str, args: &Value) -> bool {
        let reply = |engine: &mut Self, (rc, body): Refusal| {
            engine.push_workshop(Effect::Reply { id, rc, body });
        };
        match command {
            "workshop.agents.select" => {
                let Some(session) = args["session"].as_str() else {
                    reply(
                        self,
                        refusal(10, "ARGUMENT", "session must be a string".into()),
                    );
                    return true;
                };
                let node = match self.session_node(session, &args["node"]) {
                    Ok(node) => node,
                    Err(refused) => {
                        reply(self, refused);
                        return true;
                    }
                };
                self.ui.expanded.insert("agents".into());
                self.ui.expanded.insert(format!("agents:node:{node}"));
                self.ui.row_key = Some(format!("agent:{node}:{session}"));
                self.ui.selected = None;
                self.workshop.selected = None;
                self.select_session(&node, session);
                let info = self.agents_info();
                reply(self, (0, info));
                true
            }
            "workshop.agents.refresh" => {
                self.sync_local_agents();
                let nodes = match &args["node"] {
                    Value::Null => self.agent_nodes(),
                    Value::String(node)
                        if self.agent_nodes().contains(node)
                            || self.agents.nodes.contains_key(node) =>
                    {
                        vec![node.clone()]
                    }
                    Value::String(node) => {
                        reply(
                            self,
                            refusal(14, "UNKNOWN_NODE", format!("no agentd known on {node}")),
                        );
                        return true;
                    }
                    _ => {
                        reply(
                            self,
                            refusal(10, "ARGUMENT", "node must be a string".into()),
                        );
                        return true;
                    }
                };
                for node in &nodes {
                    self.refresh_node(node);
                }
                // The selected session is read on from where its ring stops.
                if self
                    .agents
                    .selected
                    .as_ref()
                    .is_some_and(|s| nodes.contains(&s.node))
                    && !self.agents.transcript.filling
                {
                    self.read_transcript();
                }
                reply(self, (0, json!({"refreshing":nodes})));
                true
            }
            "workshop.agents.filter" => {
                match self.agents_filter(args) {
                    Ok(body) => reply(self, (0, body)),
                    Err(refused) => reply(self, refused),
                }
                true
            }
            "workshop.agents.new"
            | "workshop.agents.prompt"
            | "workshop.agents.cancel"
            | "workshop.agents.close"
            | "workshop.agents.resume"
            | "workshop.agents.set"
            | "workshop.agents.answer" => {
                if let Some(session) = self.caller.as_deref().and_then(agent_caller_session) {
                    reply(
                        self,
                        refusal(
                            17,
                            "USE_AGENT",
                            format!(
                                "agent session {session}: call agent.* directly, not through Workshop"
                            ),
                        ),
                    );
                    return true;
                }
                let routed = self.acting_node(command, args).and_then(|(node, session)| {
                    agent_request(command, args, session.as_deref())
                        .map(|request| (node, request))
                        .map_err(|message| refusal(10, "ARGUMENT", message))
                });
                match routed {
                    Ok((node, (agent_command, body))) => {
                        self.act(&node, &agent_command, body, Some(id))
                    }
                    Err(refused) => reply(self, refused),
                }
                true
            }
            "workshop.agents" => {
                let info = self.agents_info();
                reply(self, (0, info));
                true
            }
            _ => false,
        }
    }

    /// `workshop.agents.filter {ended?, node?, agent?}`: `ended` opens or
    /// closes every node's Ended row; `node` and `agent` narrow the list (a
    /// string sets, null clears, absent leaves). Answers the counts shown.
    fn agents_filter(&mut self, args: &Value) -> Result<Value, Refusal> {
        let text = |key: &str| -> Result<Option<Option<String>>, Refusal> {
            match args.get(key) {
                None => Ok(None),
                Some(Value::Null) => Ok(Some(None)),
                Some(Value::String(text)) => Ok(Some(Some(text.clone()))),
                Some(_) => Err(refusal(
                    10,
                    "ARGUMENT",
                    format!("{key} must be a string or null"),
                )),
            }
        };
        let ended = match args.get("ended") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(open)) => Some(*open),
            Some(_) => {
                return Err(refusal(10, "ARGUMENT", "ended must be a bool".into()));
            }
        };
        let (node, agent) = (text("node")?, text("agent")?);
        if let Some(node) = node {
            self.agents.filter.node = node;
        }
        if let Some(agent) = agent {
            self.agents.filter.agent = agent;
        }
        if let Some(open) = ended {
            let keys: Vec<String> = self
                .agents
                .nodes
                .keys()
                .map(|node| format!("agents:ended:{node}"))
                .collect();
            for key in keys {
                if open {
                    self.ui.expanded.insert(key);
                } else {
                    self.ui.expanded.remove(&key);
                }
            }
        }
        let rows: Vec<&SessionRow> = self
            .agents
            .nodes
            .values()
            .filter(|entry| entry.shown())
            .flat_map(|entry| &entry.sessions)
            .filter(|row| self.agents.passes(row))
            .collect();
        let live = rows.iter().filter(|row| row.live()).count();
        Ok(json!({"filter":self.agents.filter,"live":live,"ended":rows.len() - live}))
    }

    /// Send one acting command to `node`'s agentd; `reply` is the Bus
    /// command to answer with agentd's reply (`None` for the window's own
    /// actions).
    fn act(&mut self, node: &str, command: &str, body: Value, reply: Option<u64>) {
        let ticket = self.agent_call(node, command, body);
        self.agents.replies.insert(ticket, reply);
    }

    /// An acting call came back: answer its Bus command, follow the result.
    fn acted(
        &mut self,
        node: &str,
        command: &str,
        reply: Option<u64>,
        result: &Result<(u8, Value), String>,
    ) {
        let (rc, body) = match result {
            Ok((rc, body)) => (*rc, body.clone()),
            Err(message) => (10, json!({"error_code":"UNAVAILABLE","message":message})),
        };
        if command == "agent.session.prompt" {
            self.agents.prompting = false;
        }
        if rc == 0 && command == "agent.session.new" && reply.is_none() {
            self.agents.new_cwd.clear();
        }
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
            self.ui.expanded.insert(format!("agents:node:{node}"));
            self.ui.row_key = Some(format!("agent:{node}:{session}"));
            self.select_session(node, &session);
        }
        if let Some(id) = reply {
            self.push_workshop(Effect::Reply { id, rc, body });
        }
        self.refresh_node(node);
        // A peer's events never reach this broker as topics: read on.
        if !self.agents.is_local(node)
            && self.agents.selected.as_ref().is_some_and(|s| s.node == node)
            && !self.agents.transcript.filling
        {
            self.read_transcript();
        }
    }

    /// A window action driven over the Bus by an agent session's own
    /// mixos-mcp is refused, like its Bus twin (USE_AGENT).
    fn refused_for_agent(&mut self) -> bool {
        let Some(session) = self.caller.as_deref().and_then(agent_caller_session) else {
            return false;
        };
        self.agents.error = Some(format!(
            "USE_AGENT: agent session {session} drove this window; it calls agent.* directly"
        ));
        true
    }

    /// Answer an open permission question of the selected session. Only a
    /// person at the window answers: a pass driven over the Bus (ui.click,
    /// ui.key, ui.pointer) is refused (HUMAN_ONLY), whoever drove it.
    pub fn agents_answer(&mut self, request: &str, option: &str) {
        if self.driven {
            self.agents.error = Some(
                "HUMAN_ONLY: a permission is answered by a person at the window, not over the Bus"
                    .into(),
            );
            return;
        }
        let Some(selected) = self.agents.selected.clone() else {
            return;
        };
        self.act(
            &selected.node,
            "agent.permission.respond",
            json!({"session":selected.session,"request":request,"option":option}),
            None,
        );
    }

    /// The node the New session form starts on: the one its row names.
    pub fn agents_form_node(&self) -> String {
        self.ui
            .row_key
            .as_deref()
            .and_then(|key| key.strip_prefix("agents:new:"))
            .map(str::to_owned)
            .unwrap_or_else(|| self.agents.local_name().to_owned())
    }

    /// Start a session from the New session form.
    pub fn agents_start(&mut self) {
        let node = self.agents_form_node();
        if self.refused_for_agent()
            || self
                .agents
                .node(&node)
                .is_none_or(|entry| entry.instances.is_null())
        {
            return;
        }
        let agent = self.agents.new_agent.trim().to_owned();
        let cwd = self.agents.new_cwd.trim().to_owned();
        if agent.is_empty() || cwd.is_empty() {
            return;
        }
        self.act(
            &node,
            "agent.session.new",
            json!({"agent":agent,"cwd":cwd}),
            None,
        );
    }

    /// Send the composer's text to the selected session.
    pub fn agents_send(&mut self) {
        if self.refused_for_agent() {
            return;
        }
        let text = self.agents.draft.trim().to_owned();
        let Some(selected) = self.agents.selected.clone() else {
            return;
        };
        if text.is_empty() || !self.agents_can_send() {
            return;
        }
        self.agents.draft.clear();
        self.agents.prompting = true;
        self.act(
            &selected.node,
            "agent.session.prompt",
            json!({"session":selected.session,"content":text}),
            None,
        );
    }

    /// Cancel, close or resume the selected session.
    pub fn agents_action(&mut self, op: &str) {
        if self.refused_for_agent() {
            return;
        }
        let Some(selected) = self.agents.selected.clone() else {
            return;
        };
        if matches!(op, "cancel" | "close" | "resume") {
            self.act(
                &selected.node,
                &format!("agent.session.{op}"),
                json!({"session":selected.session}),
                None,
            );
        }
    }

    /// Set one config option of the selected session.
    pub fn agents_set(&mut self, option: &str, value: &str) {
        if self.refused_for_agent() {
            return;
        }
        let Some(selected) = self.agents.selected.clone() else {
            return;
        };
        self.act(
            &selected.node,
            "agent.session.set",
            json!({"session":selected.session,"option":option,"value":value}),
            None,
        );
    }

    /// Set the selected session's mode (agents that report `modes`, not a
    /// mode config option).
    pub fn agents_set_mode(&mut self, mode: &str) {
        if self.refused_for_agent() {
            return;
        }
        let Some(selected) = self.agents.selected.clone() else {
            return;
        };
        self.act(
            &selected.node,
            "agent.session.set",
            json!({"session":selected.session,"mode":mode}),
            None,
        );
    }

    /// The composer can send: a selected session that is ready, or still
    /// starting (agentd queues a prompt until it is ready), with no prompt
    /// of the window's already on its way.
    pub fn agents_can_send(&self) -> bool {
        !self.agents.prompting
            && self
                .agents
                .selected_row()
                .is_some_and(|row| matches!(row.status.as_str(), "ready" | "starting"))
    }

    /// The live count and cap from `node`'s `agent.agents`.
    pub fn agents_at_cap(&self, node: &str) -> bool {
        let Some(instances) = self.agents.node(node).map(|entry| &entry.instances) else {
            return true;
        };
        let live = instances["live"].as_u64().unwrap_or_default();
        let Some(cap) = instances["max_live"].as_u64() else {
            return true;
        };
        live >= cap
    }

    /// A session row's name: the agent and the last part of its cwd.
    pub fn session_label(&self, node: &str, session: &str) -> String {
        self.agents
            .row(node, session)
            .map(SessionRow::label)
            .unwrap_or_else(|| session.to_owned())
    }

    /// What `workshop.agents` reports.
    pub fn agents_info(&self) -> Value {
        let nodes: Vec<Value> = self
            .agents
            .ordered()
            .into_iter()
            .map(|(node, entry)| {
                json!({"node":node,"local":self.agents.is_local(node),"available":entry.available,
                    "error":entry.error,"live":entry.instances["live"],"max_live":entry.instances["max_live"],
                    "sessions":entry.sessions})
            })
            .collect();
        let transcript = &self.agents.transcript;
        json!({
            "nodes": nodes,
            "selected": self.agents.selected,
            "error": self.agents.error,
            "filter": self.agents.filter,
            "transcript": {
                "first": transcript.first(),
                "last": transcript.seq,
                "events": transcript.events.len(),
                "evicted": transcript.evicted,
                "gap": transcript.gap,
                "filling": transcript.filling,
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
                    ..
                } => Some((ticket, command, body)),
                _ => None,
            })
            .collect()
    }

    fn local(e: &Engine) -> &NodeAgents {
        e.agents.node(LOCAL).expect("the local node is listed")
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
        assert_eq!(tree.children.len(), 1);
        let group = &tree.children[0];
        assert_eq!(group.key, "agents:node:local");
        assert!(group.expanded, "a node opens the first time it answers");
        let keys: Vec<_> = group.children.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "agents:new:local",
                "agent:local:s1",
                "agent:local:s2",
                "agents:ended:local"
            ]
        );
        assert_eq!(group.children[3].children[0].key, "agent:local:old");
        assert_eq!(local(&e).sessions[0].badge(), "question");
        assert_eq!(local(&e).sessions[1].badge(), "ready");
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
        e.select_session(LOCAL, "s1");
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
        assert_eq!(local(&e).sessions[0].permission_requests, ["7"]);
        e.refresh_agents();
        let list = calls(&mut e)
            .into_iter()
            .find(|(_, c, _)| c == "agent.session.list")
            .unwrap();
        e.agent_completed(list.0, "agent.session.list", Err("timed out".into()));
        assert_eq!(local(&e).sessions.len(), 1);
        assert_eq!(local(&e).available, Some(true));
        assert_eq!(local(&e).error.as_deref(), Some("timed out"));
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
        assert_eq!(local(&e).available, Some(false));
        assert!(local(&e).sessions.is_empty());
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
        assert_eq!(
            body.unwrap()["selected"],
            json!({"node":"local","session":"s1"})
        );
        assert_eq!(e.ui.row_key.as_deref(), Some("agent:local:s1"));
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
        assert_eq!(
            agent_caller_session("local:mcp-s-ab12cd-").as_deref(),
            Some("ab12cd"),
            "agentd's rule"
        );
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
        e.agents.nodes.entry(LOCAL.into()).or_default().instances =
            json!({"agents":[{"agent":"claude"}],"live":0,"max_live":4});
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
        assert!(e.agents.is_selected(LOCAL, "n1"));
        assert_eq!(e.ui.row_key.as_deref(), Some("agent:local:n1"));
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

    #[test]
    fn a_permission_is_answered_from_the_window_but_never_from_a_driven_pass() {
        let mut e = engine();
        select_and_fill(&mut e, vec![chunk(1, "a")], 1);
        e.driven = true;
        e.caller = Some("local:mix-test-client".into());
        e.agents_answer("902", "allow");
        assert!(calls(&mut e).is_empty(), "a ui.click is no answer");
        assert!(e.agents.error.as_deref().unwrap().starts_with("HUMAN_ONLY"));
        e.driven = false;
        e.caller = None;
        e.agents_answer("902", "allow");
        let (_, command, body) = calls(&mut e).pop().unwrap();
        assert_eq!(command, "agent.permission.respond");
        assert_eq!(
            body,
            json!({"session":"s1","request":"902","option":"allow"})
        );
    }

    #[test]
    fn the_answer_twin_maps_to_agentd_and_refuses_agents() {
        assert_eq!(
            agent_request(
                "workshop.agents.answer",
                &json!({"request":902,"option":"deny"}),
                Some("s1")
            )
            .unwrap(),
            (
                "agent.permission.respond".to_owned(),
                json!({"session":"s1","request":"902","option":"deny"})
            )
        );
        let mut e = engine();
        e.caller = Some("local:mcp-s-ab12cd-1f2e".into());
        assert!(e.agents_command(
            4,
            "workshop.agents.answer",
            &json!({"session":"s1","request":"9","option":"allow"})
        ));
        assert_eq!(reply_to(&e.take_effects(), 4).unwrap().0, 17);
    }

    #[test]
    fn an_agent_driving_the_window_is_refused_like_its_bus_twin() {
        let mut e = engine();
        select_and_fill(&mut e, vec![chunk(1, "a")], 1);
        e.refresh_agents();
        listed(
            &mut e,
            json!([{"session":"s1","agent":"claude","status":"ready","seq":1}]),
        );
        e.driven = true;
        e.caller = Some("local:mcp-s-ab12cd-1f2e".into());
        e.agents.draft = "hi".into();
        e.agents_send();
        e.agents_action("close");
        e.agents_set("model", "x");
        assert!(calls(&mut e).is_empty());
        assert!(e.agents.error.as_deref().unwrap().starts_with("USE_AGENT"));
        // Any other driver may act through the window (the open drive rule).
        e.caller = Some("local:mix-test-client".into());
        e.agents_send();
        assert_eq!(
            calls(&mut e)
                .into_iter()
                .filter(|(_, c, _)| c == "agent.session.prompt")
                .count(),
            1
        );
    }

    #[test]
    fn a_second_send_waits_for_the_first() {
        let mut e = engine();
        select_and_fill(&mut e, vec![chunk(1, "a")], 1);
        e.refresh_agents();
        listed(
            &mut e,
            json!([{"session":"s1","agent":"claude","status":"ready","seq":1}]),
        );
        e.agents.draft = "one".into();
        e.agents_send();
        let (ticket, command, _) = calls(&mut e)
            .into_iter()
            .find(|(_, c, _)| c == "agent.session.prompt")
            .unwrap();
        assert!(
            !e.agents_can_send(),
            "the row still says ready, the window knows better"
        );
        e.agent_completed(ticket, &command, Ok((0, json!({"session":"s1","turn":5}))));
        assert!(e.agents_can_send());
    }

    #[test]
    fn modes_follow_ready_and_mode_updates_and_cards_reach_the_row() {
        let mut e = engine();
        select_and_fill(
            &mut e,
            vec![
                json!({"session":"s1","seq":1,"kind":"ready","modes":{"availableModes":[{"id":"ask","name":"Ask"},{"id":"code","name":"Code"}],"currentModeId":"ask"}}),
                json!({"session":"s1","seq":2,"kind":"current_mode_update","update":{"sessionUpdate":"current_mode_update","currentModeId":"code"}}),
            ],
            2,
        );
        assert_eq!(e.agents.modes["currentModeId"], "code");
        e.agents_set_mode("ask");
        let (_, command, body) = calls(&mut e).pop().unwrap();
        assert_eq!(
            (command.as_str(), body),
            ("agent.session.set", json!({"session":"s1","mode":"ask"}))
        );
        e.refresh_agents();
        listed(
            &mut e,
            json!([{"session":"s1","agent":"claude","status":"running","seq":3,"open_permissions":1,
            "permission_requests":["902"],"permissions":[{"request":"902","title":"Write file","options":[{"optionId":"allow","kind":"allow_once"}],"expires_at":9e9}]}]),
        );
        assert_eq!(
            e.agents.selected_row().unwrap().permissions[0]["title"],
            "Write file"
        );
    }

    // ---- nodes ------------------------------------------------------------

    type Call = (u64, Option<String>, String, Value);

    /// alpha is this node, beta a peer with agentd, gamma a peer without.
    fn two_nodes() -> Engine {
        let mut e = engine();
        let node = |name: &str, local: bool, services: &[&str]| crate::model::Node {
            name: name.into(),
            local,
            services: services.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        };
        e.snapshot.nodes = vec![
            node("alpha", true, &["agent", "noded"]),
            node("beta", false, &["agent.beta.bus", "noded.beta.bus"]),
            node("gamma", false, &["noded.gamma.bus"]),
        ];
        e
    }

    fn node_calls(engine: &mut Engine) -> Vec<Call> {
        engine
            .take_effects()
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Agent {
                    ticket,
                    node,
                    command,
                    body,
                } => Some((ticket, node, command, body)),
                _ => None,
            })
            .collect()
    }

    fn session_on(session: &str, status: &str) -> Value {
        json!({"session":session,"agent":"claude","cwd":"/w/site","status":status,"seq":1})
    }

    /// Answer every list call in `calls` with that node's rows.
    fn answer_lists(e: &mut Engine, calls: &[Call], rows: &[(Option<&str>, Value)]) {
        for (ticket, node, command, _) in calls {
            if command != "agent.session.list" {
                continue;
            }
            let sessions = rows
                .iter()
                .find(|(n, _)| n.map(str::to_owned) == *node)
                .map(|(_, sessions)| sessions.clone())
                .unwrap_or(json!([]));
            e.agent_completed(
                *ticket,
                command,
                Ok((0, json!({ "sessions": sessions }))),
            );
        }
    }

    fn listed_two(e: &mut Engine, alpha: Value, beta: Value) {
        e.refresh_agents();
        let calls = node_calls(e);
        answer_lists(e, &calls, &[(None, alpha), (Some("beta"), beta)]);
    }

    fn chunk_of(session: &str, seq: u64, text: &str) -> Value {
        let mut event = chunk(seq, text);
        event["session"] = json!(session);
        event
    }

    #[test]
    fn effects_address_the_local_agent_and_peers_by_node() {
        assert_eq!(agent_service(None), "agent");
        assert_eq!(agent_service(Some("beta")), "agent.beta.bus");
        let mut e = two_nodes();
        e.refresh_agents();
        let lists: Vec<_> = node_calls(&mut e)
            .into_iter()
            .filter(|(_, _, c, _)| c == "agent.session.list")
            .map(|(_, node, _, _)| node)
            .collect();
        assert_eq!(
            lists,
            [None, Some("beta".to_owned())],
            "this node as `agent`, beta as agent.beta.bus, gamma not at all"
        );
    }

    #[test]
    fn ended_rows_select_through_the_bus_command_for_the_details_pane() {
        let mut e = two_nodes();
        listed_two(&mut e, json!([]), json!([session_on("old", "ended")]));
        e.ui.expanded.extend(["agents".into(), "agents:node:beta".into(), "agents:ended:beta".into()]);
        e.command(1, "workshop.select_row", r#"{"key":"agent:beta:old"}"#);
        let effects = e.take_effects();
        assert_eq!(reply_to(&effects, 1).unwrap().0, 0);
        let row = e.agents.selected_row().unwrap();
        assert_eq!((&*row.node, &*row.session, &*row.status), ("beta", "old", "ended"));
        assert_eq!(e.ui.row_key.as_deref(), Some("agent:beta:old"));
        assert!(effects.iter().any(|effect| matches!(effect, Effect::Agent {node: Some(node), command, ..} if node == "beta" && command == "agent.session.read")));
    }

    #[test]
    fn discovery_reprobes_missing_peers_and_retains_the_local_name() {
        let mut e = two_nodes();
        e.refresh_agents();
        let list = node_calls(&mut e).into_iter().find(|(_, n, c, _)| n.as_deref() == Some("beta") && c == "agent.session.list").unwrap();
        e.agent_completed(list.0, &list.2, Ok((10, json!({"message":"Service 'agent.beta.bus' not found"}))));
        assert_eq!(e.agents.node("beta").unwrap().available, Some(false));
        e.agents_discovered();
        assert!(node_calls(&mut e).iter().any(|(_, n, c, _)| n.as_deref() == Some("beta") && c == "agent.session.list"));
        e.snapshot.nodes.clear();
        e.sync_local_agents();
        assert_eq!(e.agents.local_name(), "alpha");
    }

    #[test]
    fn stale_instances_and_departed_node_replies_are_ignored() {
        let mut e = two_nodes();
        e.refresh_agents();
        let first = node_calls(&mut e);
        answer_lists(&mut e, &first, &[]);
        e.refresh_node("beta");
        let second = node_calls(&mut e);
        let latest = second.iter().find(|(_, _, c, _)| c == "agent.agents").unwrap();
        e.agent_completed(latest.0, &latest.2, Ok((0, json!({"live":2}))));
        let old = first.iter().find(|(_, n, c, _)| n.as_deref() == Some("beta") && c == "agent.agents").unwrap();
        e.agent_completed(old.0, &old.2, Err("old timeout".into()));
        assert_eq!(e.agents.node("beta").unwrap().error, None);
        assert_eq!(e.agents.node("beta").unwrap().instances["live"], 2);
        e.snapshot.nodes.retain(|node| node.name != "beta");
        e.agents_discovered();
        answer_lists(&mut e, &second, &[(Some("beta"), json!([session_on("ghost", "ready")]))]);
        assert!(e.agents.node("beta").is_none());
    }

    #[test]
    fn a_timed_out_list_honours_a_pending_refresh() {
        let mut e = two_nodes();
        e.refresh_agents();
        let first = node_calls(&mut e);
        e.agents_command(1, "workshop.agents.refresh", &json!({"node":"beta"}));
        assert_eq!(reply_to(&e.take_effects(), 1).unwrap().0, 0);
        let list = first.iter().find(|(_, n, c, _)| n.as_deref() == Some("beta") && c == "agent.session.list").unwrap();
        e.agent_completed(list.0, &list.2, Err("timed out".into()));
        assert!(node_calls(&mut e).iter().any(|(_, n, c, _)| n.as_deref() == Some("beta") && c == "agent.session.list"));
    }

    #[test]
    fn acting_twins_refuse_unknown_nodes_before_routing() {
        let mut e = two_nodes();
        e.refresh_agents();
        node_calls(&mut e);
        for op in ["new", "prompt", "close", "cancel", "resume", "set", "answer"] {
            e.agents_command(1, &format!("workshop.agents.{op}"), &json!({"node":"typo","session":"s1","agent":"claude","cwd":"/w","text":"hi","mode":"code","request":"1","option":"deny"}));
            let effects = e.take_effects();
            let (rc, body) = reply_to(&effects, 1).unwrap();
            assert_eq!((rc, &body["error_code"]), (14, &json!("UNKNOWN_NODE")));
            assert!(!effects.iter().any(|effect| matches!(effect, Effect::Agent {..})));
        }
    }

    #[test]
    fn two_nodes_list_local_first_and_a_timeout_keeps_the_rows() {
        let mut e = two_nodes();
        listed_two(
            &mut e,
            json!([session_on("s1", "ready")]),
            json!([session_on("s2", "running"), session_on("s3", "ended")]),
        );
        let tree = e.agents_tree();
        let groups: Vec<_> = tree.children.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(groups, ["agents:node:alpha", "agents:node:beta"]);
        let beta: Vec<_> = tree.children[1]
            .children
            .iter()
            .map(|r| r.key.as_str())
            .collect();
        assert_eq!(
            beta,
            ["agents:new:beta", "agent:beta:s2", "agents:ended:beta"]
        );
        assert_eq!(e.agents.row("beta", "s2").unwrap().node, "beta");
        let info = e.agents_info();
        assert_eq!(
            (&info["nodes"][0]["node"], &info["nodes"][0]["local"]),
            (&json!("alpha"), &json!(true))
        );
        assert_eq!(info["nodes"][1]["local"], false);

        e.refresh_node("beta");
        let list = node_calls(&mut e)
            .into_iter()
            .find(|(_, _, c, _)| c == "agent.session.list")
            .unwrap();
        e.agent_completed(list.0, "agent.session.list", Err("timed out".into()));
        assert_eq!(e.agents.node("beta").unwrap().sessions.len(), 2);
        let tree = e.agents_tree();
        let error = tree.children[1]
            .children
            .iter()
            .find(|r| r.key == "agents:error:beta")
            .expect("an error row under beta");
        assert_eq!(
            error.kind,
            RowKind::AgentsError("agent.beta.bus: timed out".into())
        );
        assert_eq!(e.agents.node("alpha").unwrap().error, None);
    }

    #[test]
    fn a_desk_without_agentd_shows_only_its_peers() {
        let mut e = two_nodes();
        e.refresh_agents();
        let calls = node_calls(&mut e);
        for (ticket, node, command, _) in &calls {
            if command != "agent.session.list" {
                continue;
            }
            let reply = match node {
                None => Ok((10, json!({"message":"Service 'agent' not found"}))),
                Some(_) => Ok((0, json!({"sessions":[session_on("s2", "ready")]}))),
            };
            e.agent_completed(*ticket, command, reply);
        }
        let tree = e.agents_tree();
        let keys: Vec<_> = tree.children.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["agents:node:beta"], "no local group, no down row");
    }

    #[test]
    fn the_local_entry_takes_the_name_discovery_finds() {
        let mut e = engine();
        e.refresh_agents();
        listed(&mut e, json!([session_on("s1", "ready")]));
        e.ui.row_key = Some("agent:local:s1".into());
        e.select_session(LOCAL, "s1");
        let read = calls(&mut e).pop().unwrap();
        let mut named = two_nodes();
        e.snapshot.nodes = std::mem::take(&mut named.snapshot.nodes);
        e.agents_discovered();
        assert_eq!(e.agents.local_name(), "alpha");
        assert!(e.agents.node(LOCAL).is_none());
        assert_eq!(e.agents.row("alpha", "s1").unwrap().node, "alpha");
        assert!(e.agents.is_selected("alpha", "s1"));
        assert_eq!(e.ui.row_key.as_deref(), Some("agent:alpha:s1"));
        assert!(e.ui.expanded.contains("agents:node:alpha"));
        // The read in flight answers for the renamed node, and beta is
        // listed now that discovery shows it.
        e.agent_completed(
            read.0,
            "agent.session.read",
            Ok((0, json!({"session":session_on("s1", "ready"),"events":[chunk(1, "a")],"more":false,"next":1}))),
        );
        assert_eq!(e.agents.transcript.seq, 1);
        assert!(
            node_calls(&mut e)
                .iter()
                .any(|(_, n, c, _)| c == "agent.session.list" && n.as_deref() == Some("beta"))
        );
    }

    #[test]
    fn select_without_node_on_a_duplicated_id_is_ambiguous() {
        let mut e = two_nodes();
        listed_two(
            &mut e,
            json!([session_on("dup", "ready")]),
            json!([session_on("dup", "ended")]),
        );
        assert!(e.agents_command(3, "workshop.agents.select", &json!({"session":"dup"})));
        let (rc, body) = reply_to(&e.take_effects(), 3).unwrap();
        assert_eq!((rc, &body["error_code"]), (10, &json!("AMBIGUOUS_SESSION")));
        assert_eq!(body["nodes"], json!(["alpha", "beta"]));
        assert!(e.agents_command(
            4,
            "workshop.agents.select",
            &json!({"session":"dup","node":"gamma"})
        ));
        assert_eq!(reply_to(&e.take_effects(), 4).unwrap().0, 14);
        assert!(e.agents_command(
            5,
            "workshop.agents.select",
            &json!({"session":"dup","node":"beta"})
        ));
        let effects = e.take_effects();
        assert_eq!(reply_to(&effects, 5).unwrap().0, 0);
        assert!(effects.iter().any(|effect| matches!(effect,
            Effect::Agent { node: Some(node), command, .. } if node == "beta" && command == "agent.session.read")));
        assert_eq!(e.ui.row_key.as_deref(), Some("agent:beta:dup"));
    }

    #[test]
    fn a_remote_session_is_read_once_and_refresh_reads_on_from_last() {
        let mut e = two_nodes();
        listed_two(
            &mut e,
            json!([]),
            json!([session_on("s2", "running")]),
        );
        e.select_session("beta", "s2");
        let read = node_calls(&mut e).pop().unwrap();
        assert_eq!(read.1.as_deref(), Some("beta"));
        e.agent_completed(
            read.0,
            "agent.session.read",
            Ok((0, json!({"session":session_on("s2", "running"),"events":[chunk_of("s2", 1, "a")],"more":false,"next":1}))),
        );
        // This broker's topics are the local agentd's, never beta's.
        e.agent_topic("agent.session.updated", &chunk_of("s2", 2, "b"));
        e.agent_topic("agent.session.updated", &chunk_of("s2", 5, "e"));
        assert_eq!(e.agents.transcript.seq, 1);
        assert!(
            node_calls(&mut e)
                .iter()
                .all(|(_, _, c, _)| c != "agent.session.read")
        );
        assert!(e.agents_command(6, "workshop.agents.refresh", &json!({"node":"beta"})));
        let effects = e.take_effects();
        assert_eq!(
            reply_to(&effects, 6).unwrap(),
            (0, json!({"refreshing":["beta"]}))
        );
        let agent: Vec<_> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Agent {
                    node, command, body, ..
                } => Some((node.clone(), command.clone(), body.clone())),
                _ => None,
            })
            .collect();
        assert!(agent.iter().all(|(n, _, _)| n.as_deref() == Some("beta")));
        assert!(agent.iter().any(|(_, c, _)| c == "agent.session.list"));
        let reread = agent
            .iter()
            .find(|(_, c, _)| c == "agent.session.read")
            .unwrap();
        assert_eq!(reread.2["since"], 1);
        assert!(e.agents_command(7, "workshop.agents.refresh", &json!({"node":"gamma"})));
        assert_eq!(reply_to(&e.take_effects(), 7).unwrap().0, 14);
    }

    #[test]
    fn acting_twins_go_to_the_session_node() {
        let mut e = two_nodes();
        listed_two(
            &mut e,
            json!([session_on("s1", "ready")]),
            json!([session_on("s2", "ready")]),
        );
        e.caller = Some("local:human-cli".into());
        assert!(e.agents_command(
            8,
            "workshop.agents.prompt",
            &json!({"session":"s2","text":"hi"})
        ));
        let (ticket, node, command, body) = node_calls(&mut e).pop().unwrap();
        assert_eq!(
            (node.as_deref(), command.as_str(), body),
            (
                Some("beta"),
                "agent.session.prompt",
                json!({"session":"s2","content":"hi"})
            )
        );
        e.agent_completed(ticket, &command, Ok((0, json!({"session":"s2","turn":1}))));
        assert_eq!(reply_to(&e.take_effects(), 8).unwrap().0, 0);
        assert!(e.agents_command(
            9,
            "workshop.agents.new",
            &json!({"agent":"claude","cwd":"/w","node":"beta"})
        ));
        assert_eq!(node_calls(&mut e).pop().unwrap().1.as_deref(), Some("beta"));
        assert!(e.agents_command(
            10,
            "workshop.agents.new",
            &json!({"agent":"claude","cwd":"/w"})
        ));
        assert_eq!(node_calls(&mut e).pop().unwrap().1, None);
    }

    #[test]
    fn the_old_session_key_is_unknown_to_select_row() {
        let mut e = engine();
        e.refresh_agents();
        listed(&mut e, json!([session_on("s1", "ready")]));
        e.ui.expanded.insert("agents".into());
        e.command(1, "workshop.select_row", r#"{"key":"agent:s1"}"#);
        let (_, body) = reply_to(&e.take_effects(), 1).unwrap();
        assert_eq!(body["error_code"], "ARGUMENT");
        e.command(2, "workshop.select_row", r#"{"key":"agent:local:s1"}"#);
        assert_eq!(reply_to(&e.take_effects(), 2).unwrap().0, 0);
        assert!(e.agents.is_selected(LOCAL, "s1"));
    }

    #[test]
    fn the_filter_narrows_by_node_and_agent_and_opens_ended_rows() {
        let mut e = two_nodes();
        listed_two(
            &mut e,
            json!([session_on("s1", "ready")]),
            json!([
                {"session":"s2","agent":"codex","cwd":"/w","status":"ready","seq":1},
                session_on("s3", "ended")
            ]),
        );
        assert!(e.agents_command(1, "workshop.agents.filter", &json!({"node":"beta"})));
        let (rc, body) = reply_to(&e.take_effects(), 1).unwrap();
        assert_eq!((rc, &body["live"], &body["ended"]), (0, &json!(1), &json!(1)));
        let groups: Vec<_> = e
            .agents_tree()
            .children
            .iter()
            .map(|r| r.key.clone())
            .collect();
        assert_eq!(groups, ["agents:node:beta"]);
        assert!(e.agents_command(
            2,
            "workshop.agents.filter",
            &json!({"node":null,"agent":"codex","ended":true})
        ));
        let (_, body) = reply_to(&e.take_effects(), 2).unwrap();
        assert_eq!((&body["live"], &body["ended"]), (&json!(1), &json!(0)));
        assert!(e.ui.expanded.contains("agents:ended:alpha"));
        assert!(e.ui.expanded.contains("agents:ended:beta"));
        assert!(e.agents_command(3, "workshop.agents.filter", &json!({"ended":"yes"})));
        assert_eq!(reply_to(&e.take_effects(), 3).unwrap().0, 10);
        // The text filter matches the node name too.
        e.ui.filter = "beta".into();
        assert!(e.agents_match("beta"));
        assert!(!e.agents_match("nowhere"));
    }
}
