// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Monitor's behaviour, with no UI and no I/O: which node is watched, what
//! is asked of its monitord and when, the page, the process table's sort,
//! filter and selection, and agent commands over the Bus.
//!
//! The shell feeds [`Engine`] events (replies, Bus commands, user edits, the
//! clock) and runs the [`Effect`]s it returns. Each fetch carries a ticket;
//! a reply whose ticket is not the one in flight for its kind is dropped, so
//! a late answer from a node Monitor stopped watching never lands.
use crate::history::{Rings, SEEDED, WINDOW};
use crate::model::{self, APP_ID, Column, Processes, Snapshot, Tab};
use design::{CaptionSide, Decorations, Mode, Scheme, Style};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Localises a catalogue key. The engine is headless; the shell supplies it.
pub type Label = fn(&str) -> String;

/// What a fetch asks monitord (or noded, for the peers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Fetch {
    Snapshot,
    Processes,
    History,
    Peers,
}

/// A signal the local process actions send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// End task: ask the process to finish (SIGTERM).
    Term,
    /// Kill: stop it at once (SIGKILL).
    Kill,
}

/// Work for the shell. Nothing here has happened yet.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Call `verb` on `target` with `body`, and report [`Engine::fetched`].
    Fetch {
        ticket: u64,
        kind: Fetch,
        target: String,
        verb: &'static str,
        body: Value,
    },
    /// Restore and focus the window and report [`Engine::shown`].
    Show { ticket: u64, reply: Option<u64> },
    /// Answer the Bus command `id`.
    Reply { id: u64, rc: u8, body: Value },
    /// Answer `id` with the command registry's description.
    Commands { id: u64 },
    /// Run registry command `command` and answer `id`.
    Execute { id: u64, command: String },
    /// Answer `id` with the whole Bus surface (`app.describe`), or only its
    /// verbs (`HELP`).
    Describe { id: u64, help: bool },
    /// Hand the window-level verb `verb` (`ui.click`, `window`, ...) to the
    /// toolkit drive layer.
    Drive { id: u64, verb: String, args: Value },
    /// Signal one local process as the person running Monitor, and report
    /// [`Engine::signalled`]. Only ever this node's processes.
    Signal { pid: u32, signal: Signal },
    /// Put `text` on the clipboard.
    Copy(String),
    /// Close: stop the Bus connection and the window.
    Exit,
}

/// A modal dialogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialog {
    About,
    Shortcuts,
}

/// The part of the state a person arranges, readable over the Bus
/// (`monitor.info`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiState {
    /// The watched node; `None` is this one.
    pub node: Option<String>,
    pub tab: Tab,
    /// The performance page's section: `cpu`, `memory`, `disk:<name>`,
    /// `net:<name>`.
    pub section: String,
    pub sort: Column,
    /// Against the column's natural order (largest first, names A to Z).
    pub reverse: bool,
    pub filter: String,
    pub tree: bool,
    pub selected: Option<u32>,
    pub paused: bool,
    pub dialog: Option<Dialog>,
    /// The window's own theme choice; `None` on an axis follows the session.
    #[serde(default, with = "by_name::scheme")]
    pub theme_scheme: Option<Scheme>,
    #[serde(default, with = "by_name::mode")]
    pub theme_mode: Option<Mode>,
    /// `None` follows the session, `Some(None)` is the scheme's own style.
    #[serde(default, with = "by_name::style")]
    pub theme_style: Option<Option<Style>>,
    #[serde(default, with = "by_name::decorations")]
    pub theme_decorations: Option<Decorations>,
    #[serde(default, with = "by_name::captions")]
    pub theme_captions: Option<CaptionSide>,
    /// The view should give the filter the keyboard; not saved.
    #[serde(skip)]
    pub focus_filter: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            node: None,
            tab: Tab::Processes,
            section: "cpu".into(),
            sort: Column::Cpu,
            reverse: false,
            filter: String::new(),
            tree: false,
            selected: None,
            paused: false,
            dialog: None,
            theme_scheme: None,
            theme_mode: None,
            theme_style: None,
            theme_decorations: None,
            theme_captions: None,
            focus_filter: false,
        }
    }
}

/// The theme choice saved by name ("forest", "dark").
mod by_name {
    macro_rules! named {
        ($module:ident, $ty:ty) => {
            pub mod $module {
                use serde::{Deserialize, Deserializer, Serializer};

                pub fn serialize<S: Serializer>(
                    value: &Option<$ty>,
                    s: S,
                ) -> Result<S::Ok, S::Error> {
                    match value {
                        Some(value) => s.serialize_some(value.name()),
                        None => s.serialize_none(),
                    }
                }

                pub fn deserialize<'de, D: Deserializer<'de>>(
                    d: D,
                ) -> Result<Option<$ty>, D::Error> {
                    match Option::<String>::deserialize(d)? {
                        None => Ok(None),
                        Some(name) => <$ty>::from_name(&name).map(Some).ok_or_else(|| {
                            serde::de::Error::custom(format!(
                                "unknown {} {name:?}",
                                stringify!($module)
                            ))
                        }),
                    }
                }
            }
        };
    }
    named!(scheme, design::Scheme);
    named!(mode, design::Mode);
    named!(decorations, design::Decorations);
    named!(captions, design::CaptionSide);

    pub mod style {
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S: Serializer>(
            value: &Option<Option<design::Style>>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match value {
                None => s.serialize_none(),
                Some(style) => s.serialize_some(super::super::style_name(*style)),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Option<design::Style>>, D::Error> {
            match Option::<String>::deserialize(d)? {
                None => Ok(None),
                Some(name) => super::super::style_from_name(&name)
                    .map(Some)
                    .ok_or_else(|| serde::de::Error::custom(format!("unknown style {name:?}"))),
            }
        }
    }
}

/// The scheme's own style, by name.
pub const OWN_STYLE: &str = "own";

pub fn style_name(style: Option<Style>) -> &'static str {
    style.map_or(OWN_STYLE, Style::name)
}

pub fn style_from_name(name: &str) -> Option<Option<Style>> {
    if name == OWN_STYLE {
        Some(None)
    } else {
        Style::from_name(name).map(Some)
    }
}

/// The session theme as the shell loaded it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub scheme: Scheme,
    pub style: Option<Style>,
    pub mode: Mode,
    pub decorations: Decorations,
    pub captions: CaptionSide,
}

/// Monitor's state and rules.
pub struct Engine {
    label: Label,
    pub ui: UiState,
    pub snapshot: Option<Snapshot>,
    pub processes: Option<Processes>,
    pub rings: Rings,
    /// The mesh nodes Monitor can watch, besides this one.
    pub peers: Vec<String>,
    pub status: String,
    /// Why the watched node's monitord isn't answering, when it isn't.
    pub error: Option<String>,
    pub connected: bool,
    pub quitting: bool,
    pub session: Session,
    next_ticket: u64,
    inflight: BTreeMap<Fetch, u64>,
    activations: BTreeSet<u64>,
    effects: Vec<Effect>,
}

impl Engine {
    /// A fresh engine watching this node; the first fetches are queued.
    pub fn new(label: Label) -> Self {
        let mut engine = Self {
            label,
            ui: UiState::default(),
            snapshot: None,
            processes: None,
            rings: Rings::default(),
            peers: Vec::new(),
            status: label("connecting"),
            error: None,
            connected: true,
            quitting: false,
            session: Session::default(),
            next_ticket: 0,
            inflight: BTreeMap::new(),
            activations: BTreeSet::new(),
            effects: Vec::new(),
        };
        engine.watch();
        engine.fetch(Fetch::Peers, "noded.peers", json!({}));
        engine
    }

    /// The effects queued since the last call.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    fn label(&self, key: &str) -> String {
        (self.label)(key)
    }

    fn next(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }

    /// The watched node's monitord, as the Bus addresses it.
    pub fn target(&self) -> String {
        model::target(self.ui.node.as_deref())
    }

    /// The name the window shows for the watched node.
    pub fn host(&self) -> String {
        self.snapshot
            .as_ref()
            .map(|s| s.host.clone())
            .filter(|h| !h.is_empty())
            .or_else(|| self.ui.node.clone())
            .unwrap_or_else(|| self.label("this-node"))
    }

    pub fn fetching(&self, kind: Fetch) -> bool {
        self.inflight.contains_key(&kind)
    }

    fn fetch(&mut self, kind: Fetch, verb: &'static str, body: Value) {
        if self.inflight.contains_key(&kind) || self.quitting {
            return;
        }
        let ticket = self.next();
        self.inflight.insert(kind, ticket);
        let target = match kind {
            Fetch::Peers => "noded".to_owned(),
            _ => self.target(),
        };
        self.effects.push(Effect::Fetch {
            ticket,
            kind,
            target,
            verb,
            body,
        });
    }

    /// Forget an in-flight fetch: its reply, when it comes, is dropped.
    fn cancel(&mut self, kind: Fetch) {
        self.inflight.remove(&kind);
    }

    /// The `monitord.processes` body for the table as it is set.
    pub fn query(&self) -> Value {
        let mut q = json!({
            "sort": self.ui.sort.name(),
            "reverse": self.ui.reverse,
            "limit": 0,
            "tree": self.ui.tree,
        });
        if !self.ui.filter.trim().is_empty() {
            q["filter"] = json!(self.ui.filter.trim());
        }
        q
    }

    /// Start watching the node in `ui.node`: seed the graphs from its
    /// history, and read it now.
    fn watch(&mut self) {
        self.fetch(
            Fetch::History,
            "monitord.history",
            json!({"series": SEEDED, "since": WINDOW, "max_points": WINDOW}),
        );
        self.poll();
    }

    fn poll(&mut self) {
        self.fetch(Fetch::Snapshot, "monitord.snapshot", json!({}));
        if self.ui.tab == Tab::Processes {
            let query = self.query();
            self.fetch(Fetch::Processes, "monitord.processes", query);
        }
    }

    /// The clock: read the node again, unless paused.
    pub fn tick(&mut self) {
        if !self.ui.paused && !self.quitting {
            self.poll();
        }
    }

    /// Read the node now, paused or not.
    pub fn refresh(&mut self) {
        self.poll();
    }

    /// Ask for the table again at once: the sort, filter or tree changed.
    fn requery(&mut self) {
        if self.ui.tab == Tab::Processes {
            self.cancel(Fetch::Processes);
            let query = self.query();
            self.fetch(Fetch::Processes, "monitord.processes", query);
        }
    }

    /// A fetch answered: the reply body as JSON, or why it failed.
    pub fn fetched(&mut self, kind: Fetch, ticket: u64, result: Result<Value, String>) {
        if self.inflight.get(&kind) != Some(&ticket) {
            return;
        }
        self.inflight.remove(&kind);
        match (kind, result) {
            (Fetch::Snapshot, Ok(value)) => match serde_json::from_value::<Snapshot>(value) {
                Ok(snapshot) => self.snapshotted(snapshot),
                Err(error) => self.unreachable(error.to_string()),
            },
            (Fetch::Processes, Ok(value)) => match serde_json::from_value::<Processes>(value) {
                Ok(table) => {
                    if let Some(pid) = self.ui.selected
                        && !table.rows.iter().any(|p| p.pid == pid)
                    {
                        self.ui.selected = None;
                    }
                    self.processes = Some(table);
                }
                Err(error) => self.unreachable(error.to_string()),
            },
            (Fetch::History, Ok(value)) => self.rings.seed(&value),
            (Fetch::Peers, Ok(value)) => self.peers = model::peers(&value),
            // No history yet, or no mesh: the graphs fill live, the node
            // list stays as it was.
            (Fetch::History | Fetch::Peers, Err(_)) => {}
            (Fetch::Snapshot | Fetch::Processes, Err(error)) => self.unreachable(error),
        }
    }

    fn snapshotted(&mut self, snapshot: Snapshot) {
        // Another host answering at the same address (a node renamed, or
        // replaced) starts the graphs again.
        if self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.host != snapshot.host)
        {
            self.rings.clear();
        }
        self.rings.record(&snapshot);
        if !model::sections(Some(&snapshot)).contains(&self.ui.section) {
            self.ui.section = "cpu".into();
        }
        self.snapshot = Some(snapshot);
        self.error = None;
        self.status = self.label(if self.ui.paused { "paused" } else { "watching" });
    }

    fn unreachable(&mut self, error: String) {
        self.status = self.label("unreachable");
        self.error = Some(error);
    }

    // ---- actions (commands dispatch here) --------------------------------

    /// Watch `node` (`None`: this node). The readings, graphs and selection
    /// start again; replies still on their way from the last node are
    /// dropped.
    pub fn set_node(&mut self, node: Option<String>) -> Result<(), String> {
        if let Some(name) = &node
            && !model::valid_node(name)
        {
            return Err(self.label("invalid-node"));
        }
        if node == self.ui.node {
            return Ok(());
        }
        self.ui.node = node;
        self.snapshot = None;
        self.processes = None;
        self.rings.clear();
        self.error = None;
        self.ui.selected = None;
        self.status = self.label("connecting");
        for kind in [Fetch::Snapshot, Fetch::Processes, Fetch::History] {
            self.cancel(kind);
        }
        self.watch();
        Ok(())
    }

    pub fn set_tab(&mut self, tab: Tab) {
        if self.ui.dialog.is_some() {
            return;
        }
        self.ui.tab = tab;
        if tab == Tab::Processes && !self.ui.paused {
            let query = self.query();
            self.fetch(Fetch::Processes, "monitord.processes", query);
        }
    }

    pub fn show_processes(&mut self) {
        self.set_tab(Tab::Processes);
    }

    pub fn show_performance(&mut self) {
        self.set_tab(Tab::Performance);
    }

    pub fn set_section(&mut self, section: &str) -> Result<(), String> {
        if !model::sections(self.snapshot.as_ref())
            .iter()
            .any(|s| s == section)
        {
            return Err(self.label("invalid-section"));
        }
        self.ui.section = section.to_owned();
        Ok(())
    }

    /// Sort by `column`. With no direction, choosing the column already
    /// sorted by flips it, as a header click does.
    pub fn set_sort(&mut self, column: Column, reverse: Option<bool>) {
        match reverse {
            Some(reverse) => self.ui.reverse = reverse,
            None if column == self.ui.sort => self.ui.reverse = !self.ui.reverse,
            None => self.ui.reverse = false,
        }
        self.ui.sort = column;
        self.requery();
    }

    pub fn set_filter(&mut self, filter: String) {
        if self.ui.dialog.is_some() || filter == self.ui.filter {
            return;
        }
        self.ui.filter = filter;
        self.requery();
    }

    pub fn set_tree(&mut self, on: bool) {
        if on != self.ui.tree {
            self.ui.tree = on;
            self.requery();
        }
    }

    pub fn toggle_tree(&mut self) {
        let on = !self.ui.tree;
        self.set_tree(on);
    }

    pub fn toggle_pause(&mut self) {
        self.ui.paused = !self.ui.paused;
        self.status = self.label(if self.ui.paused { "paused" } else { "watching" });
        if !self.ui.paused {
            self.poll();
        }
    }

    /// Select the row for `pid`, or nothing. A pid not in the table is
    /// refused.
    pub fn select(&mut self, pid: Option<u32>) -> Result<(), String> {
        if let Some(pid) = pid
            && !self
                .processes
                .as_ref()
                .is_some_and(|t| t.rows.iter().any(|p| p.pid == pid))
        {
            return Err(self.label("no-such-process"));
        }
        self.ui.selected = pid;
        Ok(())
    }

    /// The selected process's row.
    pub fn selected(&self) -> Option<&model::Process> {
        let pid = self.ui.selected?;
        self.processes.as_ref()?.rows.iter().find(|p| p.pid == pid)
    }

    /// End task and Kill act only on this node's processes, as the person
    /// running Monitor: another node's are hostd's to touch.
    pub fn can_signal(&self) -> bool {
        self.ui.node.is_none() && self.ui.dialog.is_none() && self.selected().is_some()
    }

    pub fn end_selected(&mut self) {
        self.signal(Signal::Term);
    }

    pub fn kill_selected(&mut self) {
        self.signal(Signal::Kill);
    }

    fn signal(&mut self, signal: Signal) {
        if let (true, Some(pid)) = (self.can_signal(), self.ui.selected) {
            self.effects.push(Effect::Signal { pid, signal });
        }
    }

    /// A signal was sent, or refused by the kernel.
    pub fn signalled(&mut self, pid: u32, signal: Signal, result: Result<(), String>) {
        let name = self
            .processes
            .as_ref()
            .and_then(|t| t.rows.iter().find(|p| p.pid == pid))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| pid.to_string());
        self.status = match result {
            Ok(()) => format!(
                "{} {name} ({pid})",
                self.label(match signal {
                    Signal::Term => "ended",
                    Signal::Kill => "killed",
                })
            ),
            Err(error) => format!("{} {name} ({pid}): {error}", self.label("signal-refused")),
        };
        self.requery();
    }

    /// Copy the selected process's command line.
    pub fn copy_command(&mut self) {
        if let Some(p) = self.selected() {
            let text = if p.cmdline.is_empty() {
                p.name.clone()
            } else {
                p.cmdline.clone()
            };
            self.effects.push(Effect::Copy(text));
        }
    }

    pub fn has_selection(&self) -> bool {
        self.ui.dialog.is_none() && self.selected().is_some()
    }

    pub fn open(&mut self, dialog: Dialog) {
        if self.ui.dialog.is_none() && !self.quitting {
            self.ui.dialog = Some(dialog);
        }
    }

    pub fn close_dialog(&mut self) {
        self.ui.dialog = None;
    }

    /// Close. Monitor holds no work of its own to finish: it only reads.
    pub fn quit(&mut self) {
        if !self.quitting {
            self.quitting = true;
            self.effects.push(Effect::Exit);
        }
    }

    pub fn focus_filter(&mut self) {
        if self.ui.dialog.is_none() {
            self.ui.tab = Tab::Processes;
            self.ui.focus_filter = true;
        }
    }

    pub fn filter_focused(&mut self) {
        self.ui.focus_filter = false;
    }

    /// The Bus connection came up or went down.
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
        if connected {
            self.status = self.label("connecting");
            self.fetch(Fetch::Peers, "noded.peers", json!({}));
            self.poll();
        } else {
            self.status = self.label("disconnected");
            // Nothing in flight will answer now.
            self.inflight.clear();
        }
    }

    /// The services on the Bus changed: the mesh may have too.
    pub fn services_changed(&mut self) {
        self.fetch(Fetch::Peers, "noded.peers", json!({}));
    }

    pub fn effective_theme(&self) -> (Scheme, Mode) {
        let shown = self.effective();
        (shown.scheme, shown.mode)
    }

    pub fn effective(&self) -> Session {
        let ui = &self.ui;
        Session {
            scheme: ui.theme_scheme.unwrap_or(self.session.scheme),
            style: ui.theme_style.unwrap_or(self.session.style),
            mode: ui.theme_mode.unwrap_or(self.session.mode),
            decorations: ui.theme_decorations.unwrap_or(self.session.decorations),
            captions: ui.theme_captions.unwrap_or(self.session.captions),
        }
    }

    pub fn toggle_mode(&mut self) {
        let shown = self.effective_theme().1;
        self.set_mode(if shown == Mode::Dark {
            Mode::Light
        } else {
            Mode::Dark
        });
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.ui.theme_mode = Some(mode);
    }

    // ---- the Bus ------------------------------------------------------------

    pub fn show(&mut self, reply: Option<u64>) {
        let ticket = self.next();
        self.activations.insert(ticket);
        self.effects.push(Effect::Show { ticket, reply });
    }

    pub fn shown(&mut self, ticket: u64, reply: Option<u64>, result: Result<Value, String>) {
        if !self.activations.remove(&ticket) {
            return;
        }
        if let Some(id) = reply {
            match result {
                Ok(value) => self.effects.push(Effect::Reply {
                    id,
                    rc: 0,
                    body: value,
                }),
                Err(error) => self.error(id, "ACTIVATION", &error),
            }
        }
    }

    /// The whole state as `monitor.info` reports it.
    pub fn info(&self) -> Value {
        let snapshot = self.snapshot.as_ref().map(|s| {
            json!({
                "host": s.host, "ts": s.ts, "cpu": s.cpu.busy,
                "memory": {"used": s.memory.used, "total": s.memory.total},
                "uptime_s": s.uptime_s,
            })
        });
        json!({
            "schema": "monitor.v1", "app_id": APP_ID, "version": env!("CARGO_PKG_VERSION"),
            "pid": std::process::id(), "connected": self.connected, "target": self.target(),
            "host": self.host(), "status": self.status, "error": self.error, "peers": self.peers,
            "snapshot": snapshot,
            "processes": self.processes.as_ref().map(|t| json!({"total": t.total, "count": t.count})),
            "sections": model::sections(self.snapshot.as_ref()),
            "ui": self.ui,
        })
    }

    /// The process rows in the order the table shows them.
    pub fn rows(&self) -> Vec<Value> {
        self.processes
            .iter()
            .flat_map(|t| &t.rows)
            .map(|p| {
                json!({
                    "pid": p.pid, "name": p.name, "user": p.user, "cpu": p.cpu,
                    "memory": p.memory, "io": p.io(), "depth": p.depth,
                    "selected": self.ui.selected == Some(p.pid),
                })
            })
            .collect()
    }

    fn error(&mut self, id: u64, code: &str, message: &str) {
        self.effects.push(Effect::Reply {
            id,
            rc: 10,
            body: json!({"error_code": code, "message": message}),
        });
    }

    fn ok(&mut self, id: u64, body: Value) {
        self.effects.push(Effect::Reply { id, rc: 0, body });
    }

    /// A command an agent sent to Monitor over the Bus.
    pub fn command(&mut self, id: u64, verb: &str, body: &str) {
        let body = if body.trim().is_empty() { "{}" } else { body };
        let args = match serde_json::from_str::<Value>(body) {
            Ok(args) if args.is_object() => args,
            _ => return self.error(id, "ARGUMENT", "arguments must be a JSON object"),
        };
        if let Some(window) = verb
            .strip_prefix("monitor.")
            .filter(|v| v.starts_with("ui.") || v.starts_with("window"))
        {
            return self.effects.push(Effect::Drive {
                id,
                verb: window.to_owned(),
                args,
            });
        }
        if !model::VERBS.contains(&verb) {
            return self.error(id, "UNKNOWN_VERB", "unknown Monitor verb");
        }
        match verb {
            "HELP" => self.effects.push(Effect::Describe { id, help: true }),
            "app.describe" => self.effects.push(Effect::Describe { id, help: false }),
            "monitor.ping" => self.ok(
                id,
                json!({"schema": "monitor.v1", "version": env!("CARGO_PKG_VERSION")}),
            ),
            "monitor.info" => {
                let info = self.info();
                self.ok(id, info);
            }
            "monitor.rows" => {
                let rows = self.rows();
                self.ok(id, json!({"rows": rows}));
            }
            "monitor.commands" => self.effects.push(Effect::Commands { id }),
            "monitor.execute" => match args.get("id").and_then(Value::as_str) {
                Some(command) => self.effects.push(Effect::Execute {
                    id,
                    command: command.to_owned(),
                }),
                None => self.error(id, "ARGUMENT", "id must be a command id string"),
            },
            "monitor.show" if !self.quitting => self.show(Some(id)),
            "monitor.quit" => {
                self.ok(id, json!({"quitting": true}));
                self.quit();
            }
            "monitor.theme" => match self.theme(&args) {
                Ok(body) => self.ok(id, body),
                Err((code, message)) => self.error(id, code, &message),
            },
            "monitor.dialog" => match self.dialog(&args) {
                Ok(body) => self.ok(id, body),
                Err((code, message)) => self.error(id, code, &message),
            },
            _ if self.ui.dialog.is_some() || self.quitting => {
                let message = self.label("busy");
                self.error(id, "BUSY", &message);
            }
            _ => match self.edit(verb, &args) {
                Ok(body) => self.ok(id, body),
                Err((code, message)) => self.error(id, code, &message),
            },
        }
    }

    fn dialog(&mut self, args: &Value) -> Result<Value, (&'static str, String)> {
        let dialog = match args.get("open") {
            Some(Value::Null) => None,
            Some(Value::String(name)) if name == "about" => Some(Dialog::About),
            Some(Value::String(name)) if name == "shortcuts" => Some(Dialog::Shortcuts),
            _ => {
                return Err((
                    "ARGUMENT",
                    "open must be \"about\", \"shortcuts\" or null".into(),
                ));
            }
        };
        match dialog {
            None => self.close_dialog(),
            Some(_) if self.ui.dialog.is_some() || self.quitting => {
                return Err(("BUSY", self.label("busy")));
            }
            Some(dialog) => self.open(dialog),
        }
        Ok(json!({"dialog": self.ui.dialog}))
    }

    /// The state verbs: one edit as the window makes it.
    fn edit(&mut self, verb: &str, args: &Value) -> Result<Value, (&'static str, String)> {
        let text = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| ("ARGUMENT", format!("{key} must be a string")))
        };
        match verb {
            "monitor.refresh" => {
                self.refresh();
                Ok(json!({"refreshing": true}))
            }
            "monitor.node" => {
                let node = match args.get("node") {
                    Some(Value::Null) => None,
                    Some(Value::String(node)) => Some(node.clone()),
                    _ => return Err(("ARGUMENT", "node must be a node name or null".into())),
                };
                self.set_node(node).map_err(|e| ("ARGUMENT", e))?;
                Ok(json!({"node": self.ui.node, "target": self.target()}))
            }
            "monitor.tab" => {
                let tab = Tab::from_name(&text("tab")?).ok_or((
                    "ARGUMENT",
                    "tab must be processes or performance".to_owned(),
                ))?;
                self.set_tab(tab);
                Ok(json!({"tab": self.ui.tab}))
            }
            "monitor.section" => {
                self.set_section(&text("section")?)
                    .map_err(|e| ("ARGUMENT", e))?;
                Ok(json!({"section": self.ui.section}))
            }
            "monitor.sort" => {
                let column = Column::from_name(&text("column")?).ok_or((
                    "ARGUMENT",
                    "column must be name, pid, user, cpu, memory or io".to_owned(),
                ))?;
                let reverse = match args.get("reverse") {
                    None => None,
                    Some(Value::Bool(b)) => Some(*b),
                    Some(_) => return Err(("ARGUMENT", "reverse must be a bool".into())),
                };
                self.set_sort(column, reverse);
                Ok(json!({"sort": self.ui.sort, "reverse": self.ui.reverse}))
            }
            "monitor.filter" => {
                self.set_filter(text("text")?);
                Ok(json!({"filter": self.ui.filter}))
            }
            "monitor.tree" => {
                let on = args
                    .get("on")
                    .and_then(Value::as_bool)
                    .ok_or(("ARGUMENT", "on must be a bool".to_owned()))?;
                self.set_tree(on);
                Ok(json!({"tree": self.ui.tree}))
            }
            "monitor.select" => {
                let pid = match args.get("pid") {
                    Some(Value::Null) => None,
                    Some(v) => Some(
                        v.as_u64()
                            .and_then(|p| u32::try_from(p).ok())
                            .ok_or(("ARGUMENT", "pid must be a process id or null".to_owned()))?,
                    ),
                    None => return Err(("ARGUMENT", "pid is required".into())),
                };
                self.select(pid).map_err(|e| ("ARGUMENT", e))?;
                Ok(json!({"selected": self.ui.selected}))
            }
            _ => Err(("UNKNOWN_VERB", "unknown Monitor verb".into())),
        }
    }

    /// `monitor.theme`: a present key sets that axis of the window's theme
    /// choice (a name, or null to follow the session); an absent one
    /// leaves it. Every axis is checked before any applies.
    fn theme(&mut self, args: &Value) -> Result<Value, (&'static str, String)> {
        type Axis<T> = Result<Option<Option<T>>, (&'static str, String)>;
        fn axis<T>(args: &Value, key: &str, from_name: fn(&str) -> Option<T>) -> Axis<T> {
            match args.get(key) {
                None => Ok(None),
                Some(Value::Null) => Ok(Some(None)),
                Some(Value::String(name)) => from_name(name)
                    .map(|v| Some(Some(v)))
                    .ok_or_else(|| ("ARGUMENT", format!("unknown {key} {name:?}"))),
                Some(_) => Err(("ARGUMENT", format!("{key} must be a name or null"))),
            }
        }
        let scheme = axis(args, "scheme", Scheme::from_name)?;
        let style = axis(args, "style", style_from_name)?;
        let mode = axis(args, "mode", Mode::from_name)?;
        let decorations = axis(args, "decorations", Decorations::from_name)?;
        let captions = axis(args, "caption_side", CaptionSide::from_name)?;
        if let Some(v) = scheme {
            self.ui.theme_scheme = v;
        }
        if let Some(v) = style {
            self.ui.theme_style = v;
        }
        if let Some(v) = mode {
            self.ui.theme_mode = v;
        }
        if let Some(v) = decorations {
            self.ui.theme_decorations = v;
        }
        if let Some(v) = captions {
            self.ui.theme_captions = v;
        }
        let ui = &self.ui;
        let shown = self.effective();
        Ok(json!({
            "scheme": ui.theme_scheme.map(Scheme::name),
            "style": ui.theme_style.map(style_name),
            "mode": ui.theme_mode.map(Mode::name),
            "decorations": ui.theme_decorations.map(Decorations::name),
            "caption_side": ui.theme_captions.map(CaptionSide::name),
            "effective": {
                "scheme": shown.scheme.name(),
                "style": style_name(shown.style),
                "mode": shown.mode.name(),
                "decorations": shown.decorations.name(),
                "caption_side": shown.captions.name(),
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(key: &str) -> String {
        key.to_owned()
    }

    /// The tickets of the queued fetches, by kind.
    fn fetches(e: &mut Engine) -> BTreeMap<Fetch, (u64, String, Value)> {
        e.take_effects()
            .into_iter()
            .filter_map(|x| match x {
                Effect::Fetch {
                    ticket,
                    kind,
                    target,
                    body,
                    ..
                } => Some((kind, (ticket, target, body))),
                _ => None,
            })
            .collect()
    }

    fn snapshot(host: &str, busy: f64) -> Value {
        json!({"host": host, "cpu": {"busy": busy, "cores": 2,
            "per_core": [{"cpu": 0, "busy": busy}, {"cpu": 1, "busy": 0.0}]},
            "memory": {"total": 100, "used": 40},
            "disks": [{"name": "sda", "busy": 5.0}], "net": [{"name": "eth0", "rx_bps": 10.0}]})
    }

    fn table(pids: &[u32]) -> Value {
        json!({"total": pids.len(), "count": pids.len(),
            "rows": pids.iter().map(|p| json!({"pid": p, "name": format!("p{p}")})).collect::<Vec<_>>()})
    }

    /// An engine that has had one snapshot and one table.
    fn ready() -> Engine {
        let mut e = Engine::new(label);
        let f = fetches(&mut e);
        e.fetched(
            Fetch::Snapshot,
            f[&Fetch::Snapshot].0,
            Ok(snapshot("alpha", 25.0)),
        );
        e.fetched(
            Fetch::Processes,
            f[&Fetch::Processes].0,
            Ok(table(&[1, 42])),
        );
        e
    }

    #[test]
    fn a_new_engine_seeds_reads_and_lists_peers() {
        let mut e = Engine::new(label);
        let f = fetches(&mut e);
        let kinds: Vec<Fetch> = f.keys().copied().collect();
        assert_eq!(
            kinds,
            [
                Fetch::Snapshot,
                Fetch::Processes,
                Fetch::History,
                Fetch::Peers
            ]
        );
        assert_eq!(f[&Fetch::Snapshot].1, "monitord");
        assert_eq!(f[&Fetch::Peers].1, "noded");
        assert_eq!(f[&Fetch::Processes].2["sort"], "cpu");
    }

    #[test]
    fn readings_fill_the_graphs_and_status() {
        let e = ready();
        assert_eq!(e.host(), "alpha");
        assert_eq!(e.rings.get("cpu.busy"), [25.0]);
        assert_eq!(e.rings.get("cpu.core.1.busy"), [0.0]);
        assert_eq!(e.rings.get("disk.sda.busy"), [5.0]);
        assert_eq!(e.status, "watching");
        assert_eq!(e.processes.as_ref().unwrap().rows.len(), 2);
    }

    #[test]
    fn one_fetch_of_a_kind_at_a_time() {
        let mut e = Engine::new(label);
        fetches(&mut e);
        e.tick();
        assert!(fetches(&mut e).is_empty(), "everything is still in flight");
    }

    #[test]
    fn a_late_reply_from_the_last_node_is_dropped() {
        let mut e = Engine::new(label);
        let old = fetches(&mut e);
        e.set_node(Some("beta".into())).unwrap();
        let new = fetches(&mut e);
        assert_eq!(new[&Fetch::Snapshot].1, "monitord.beta.bus");
        e.fetched(
            Fetch::Snapshot,
            old[&Fetch::Snapshot].0,
            Ok(snapshot("alpha", 99.0)),
        );
        assert!(e.snapshot.is_none(), "alpha's answer must not land on beta");
        e.fetched(
            Fetch::Snapshot,
            new[&Fetch::Snapshot].0,
            Ok(snapshot("beta", 1.0)),
        );
        assert_eq!(e.host(), "beta");
        assert!(e.set_node(Some("bad.name".into())).is_err());
    }

    #[test]
    fn paused_ticks_read_nothing_but_refresh_does() {
        let mut e = ready();
        fetches(&mut e);
        e.toggle_pause();
        fetches(&mut e);
        let f = fetches(&mut e);
        assert!(f.is_empty());
        e.tick();
        assert!(fetches(&mut e).is_empty());
        e.refresh();
        assert!(fetches(&mut e).contains_key(&Fetch::Snapshot));
    }

    #[test]
    fn sorting_flips_on_the_same_column_and_requeries() {
        let mut e = ready();
        fetches(&mut e);
        e.set_sort(Column::Memory, None);
        assert_eq!((e.ui.sort, e.ui.reverse), (Column::Memory, false));
        let f = fetches(&mut e);
        assert_eq!(f[&Fetch::Processes].2["sort"], "memory");
        e.set_sort(Column::Memory, None);
        assert!(e.ui.reverse);
        e.set_filter("fire".into());
        let f = fetches(&mut e);
        assert_eq!(f[&Fetch::Processes].2["filter"], "fire");
    }

    #[test]
    fn signals_only_go_to_this_nodes_selected_process() {
        let mut e = ready();
        e.take_effects();
        e.end_selected();
        assert!(e.take_effects().is_empty(), "nothing selected");
        e.select(Some(42)).unwrap();
        e.kill_selected();
        assert_eq!(
            e.take_effects(),
            [Effect::Signal {
                pid: 42,
                signal: Signal::Kill
            }]
        );
        assert!(e.select(Some(7)).is_err());
        e.set_node(Some("beta".into())).unwrap();
        assert!(!e.can_signal());
    }

    #[test]
    fn a_vanished_process_loses_its_selection() {
        let mut e = ready();
        e.select(Some(42)).unwrap();
        e.take_effects();
        e.refresh();
        let f = fetches(&mut e);
        e.fetched(Fetch::Processes, f[&Fetch::Processes].0, Ok(table(&[1])));
        assert_eq!(e.ui.selected, None);
    }

    #[test]
    fn an_unreachable_node_says_why() {
        let mut e = Engine::new(label);
        let f = fetches(&mut e);
        e.fetched(
            Fetch::Snapshot,
            f[&Fetch::Snapshot].0,
            Err("Service 'monitord' not found".into()),
        );
        assert_eq!(e.status, "unreachable");
        assert!(e.error.as_deref().unwrap().contains("not found"));
    }

    fn reply(e: &mut Engine, verb: &str, body: Value) -> (u8, Value) {
        e.take_effects();
        e.command(1, verb, &body.to_string());
        e.take_effects()
            .into_iter()
            .find_map(|x| match x {
                Effect::Reply { rc, body, .. } => Some((rc, body)),
                _ => None,
            })
            .expect("a reply")
    }

    #[test]
    fn bus_verbs_edit_the_same_state() {
        let mut e = ready();
        assert_eq!(
            reply(&mut e, "monitor.tab", json!({"tab": "performance"})).0,
            0
        );
        assert_eq!(e.ui.tab, Tab::Performance);
        assert_eq!(
            reply(&mut e, "monitor.section", json!({"section": "disk:sda"})).0,
            0
        );
        assert_eq!(
            reply(&mut e, "monitor.section", json!({"section": "disk:nope"})).0,
            10
        );
        assert_eq!(
            reply(
                &mut e,
                "monitor.sort",
                json!({"column": "pid", "reverse": true})
            )
            .0,
            0
        );
        assert_eq!((e.ui.sort, e.ui.reverse), (Column::Pid, true));
        assert_eq!(reply(&mut e, "monitor.select", json!({"pid": 42})).0, 0);
        let (rc, rows) = reply(&mut e, "monitor.rows", json!({}));
        assert_eq!(rc, 0);
        assert_eq!(rows["rows"][1]["selected"], true);
        assert_eq!(
            reply(&mut e, "monitor.node", json!({"node": "beta"})).1["target"],
            "monitord.beta.bus"
        );
        assert_eq!(
            reply(&mut e, "monitor.dialog", json!({"open": "about"})).0,
            0
        );
        assert_eq!(
            reply(&mut e, "monitor.tab", json!({"tab": "processes"})).1["error_code"],
            "BUSY"
        );
        assert_eq!(
            reply(&mut e, "monitor.bogus", json!({})).1["error_code"],
            "UNKNOWN_VERB"
        );
        assert_eq!(
            reply(&mut e, "monitor.theme", json!({"mode": "dark"})).1["mode"],
            "dark"
        );
    }

    #[test]
    fn window_verbs_go_to_the_drive_layer() {
        let mut e = Engine::new(label);
        e.take_effects();
        e.command(5, "monitor.ui.tree", "{}");
        assert!(
            matches!(&e.take_effects()[..], [Effect::Drive { id: 5, verb, .. }] if verb == "ui.tree")
        );
    }
}
