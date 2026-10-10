// SPDX-License-Identifier: MIT OR Apache-2.0
//! BusViewer's behaviour, with no UI and no I/O: discovery, selection, the
//! body, one call at a time, agent commands over the Bus, and quitting only
//! once accepted work is finished.
//!
//! The shell feeds [`Engine`] events (Bus deliveries, completions, user
//! edits) and runs the [`Effect`]s it returns. Every effect that leaves the
//! process carries a ticket, and a completion whose ticket is not the one in
//! flight is ignored, so a late reply can never land on a newer operation.
//!
//! Ported from the iced reducer (`apps/busviewer/src/app.rs`, markc/mixos-iced)
//! with the same rules; only the rendering and the effect plumbing differ.

use crate::bus::{CallError, Delivery, Reply};
use crate::model::{self, APP_ID, Selection, Snapshot};
use design::{Mode, Scheme};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Localises a catalogue key. The engine is headless; the shell supplies it.
pub type Label = fn(&str) -> String;

/// Work for the shell. Nothing here has happened yet.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Run discovery and report [`Engine::discovered`] with `ticket`.
    Discover { ticket: u64 },
    /// Send one call and report [`Engine::completed`] with `ticket`.
    Call { ticket: u64, target: Selection, body: String },
    /// Restore and focus the window and report [`Engine::shown`].
    Show { ticket: u64, reply: Option<u64> },
    /// Answer the Bus command `id`.
    Reply { id: u64, rc: u8, body: Value },
    /// Answer `id` with the command registry's description.
    Commands { id: u64 },
    /// Run registry command `command` and answer `id`.
    Execute { id: u64, command: String },
    /// Answer `id` with the whole Bus surface (`app.describe`), or only its
    /// verbs (`HELP`): the engine's and the window's drive verbs.
    Describe { id: u64, help: bool },
    /// Hand the window-level verb `verb` (without the `busviewer.` prefix:
    /// `ui.click`, `window`, …) to the toolkit drive layer.
    Drive { id: u64, verb: String, args: Value },
    /// Put `text` on the clipboard.
    Copy(String),
    /// Close: stop the Bus connection and the window.
    Exit,
}

/// A modal dialogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dialog {
    About,
    Shortcuts,
}

/// What the tree shows for one row.
#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Service(String),
    Verb(Selection),
    Error(String),
    Peers,
    Peer(String),
    NoPeers,
}

/// One row of the services tree, with its children.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: String,
    pub kind: RowKind,
    pub expanded: bool,
    pub children: Vec<Row>,
}

/// The part of the state a person arranges, saved and restored as one
/// value and readable over the Bus (`busviewer.info`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiState {
    pub selected: Option<Selection>,
    pub row_key: Option<String>,
    pub filter: String,
    pub expanded: BTreeSet<String>,
    pub body: String,
    pub split: f32,
    pub dialog: Option<Dialog>,
    /// The window's own theme choice (View › Theme, the title bar's
    /// light/dark button, `busviewer.theme`): `None` on an axis follows the
    /// session. The session's theme file is never written.
    #[serde(default, with = "by_name::scheme")]
    pub theme_scheme: Option<Scheme>,
    #[serde(default, with = "by_name::mode")]
    pub theme_mode: Option<Mode>,
    /// The view should give the services filter the keyboard (set by
    /// `view.search`, cleared by the view once it has); not saved.
    #[serde(skip)]
    pub focus_filter: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            selected: None,
            row_key: None,
            filter: String::new(),
            expanded: BTreeSet::new(),
            body: String::new(),
            split: 0.34,
            dialog: None,
            theme_scheme: None,
            theme_mode: None,
            focus_filter: false,
        }
    }
}

/// The theme choice saved by name ("forest", "dark"): the design's
/// [`Scheme`] and [`Mode`] carry no serde of their own.
mod by_name {
    macro_rules! named {
        ($module:ident, $ty:ty) => {
            pub mod $module {
                use serde::{Deserialize, Deserializer, Serializer};

                pub fn serialize<S: Serializer>(value: &Option<$ty>, s: S) -> Result<S::Ok, S::Error> {
                    match value {
                        Some(value) => s.serialize_some(value.name()),
                        None => s.serialize_none(),
                    }
                }

                pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<$ty>, D::Error> {
                    match Option::<String>::deserialize(d)? {
                        None => Ok(None),
                        Some(name) => <$ty>::from_name(&name)
                            .map(Some)
                            .ok_or_else(|| serde::de::Error::custom(format!("unknown {} {name:?}", stringify!($module)))),
                    }
                }
            }
        };
    }
    named!(scheme, design::Scheme);
    named!(mode, design::Mode);
}

#[derive(Clone, Debug)]
struct Call {
    ticket: u64,
    target: Selection,
    body: String,
    reply: Option<u64>,
}

/// BusViewer's state and rules.
pub struct Engine {
    label: Label,
    pub ui: UiState,
    pub snapshot: Snapshot,
    /// The rendered reply panel.
    pub reply: String,
    pub last_reply: Value,
    pub status: String,
    pub connected: bool,
    pub quitting: bool,
    /// `busviewer.split` set the split: the shell drops the panel width
    /// egui remembers, so the window takes the new one.
    pub split_requested: bool,
    /// The session theme's scheme and mode, as the shell loaded it: what an
    /// unchosen axis of the theme choice shows.
    pub session: (Scheme, Mode),
    next_ticket: u64,
    discovery: Option<(u64, Option<u64>)>,
    call: Option<Call>,
    activations: BTreeSet<u64>,
    refetch: bool,
    effects: Vec<Effect>,
}

impl Engine {
    /// A fresh engine; the first refresh is queued.
    pub fn new(label: Label) -> Self {
        let mut engine = Self {
            label,
            ui: UiState::default(),
            snapshot: Snapshot::default(),
            reply: String::new(),
            last_reply: Value::Null,
            status: label("connecting"),
            connected: true,
            quitting: false,
            split_requested: false,
            session: (Scheme::default(), Mode::default()),
            next_ticket: 0,
            discovery: None,
            call: None,
            activations: BTreeSet::new(),
            refetch: false,
            effects: Vec::new(),
        };
        engine.refresh(None);
        engine
    }

    /// The effects queued since the last call.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    fn label(&self, key: &str) -> String {
        (self.label)(key)
    }

    /// Discovery, a call or a window activation is in flight.
    pub fn busy(&self) -> bool {
        self.discovery.is_some() || self.call.is_some() || !self.activations.is_empty()
    }

    pub fn calling(&self) -> bool {
        self.call.is_some()
    }

    /// The selected verb is advertised and can be called now.
    pub fn callable(&self) -> bool {
        !self.busy()
            && self.connected
            && self.ui.dialog.is_none()
            && self.ui.selected.as_ref().is_some_and(|t| self.snapshot.verb(t).is_some())
    }

    fn next(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }

    /// The whole state as `busviewer.info` reports it.
    pub fn info(&self) -> Value {
        let mut info = self.state();
        info["ui"]["split"] = json!(rounded(self.ui.split));
        info
    }

    fn state(&self) -> Value {
        json!({"schema":"busviewer.v1","app_id":APP_ID,"version":env!("CARGO_PKG_VERSION"),"pid":std::process::id(),
            "connected":self.connected,"busy":self.busy(),"discovering":self.discovery.is_some(),"calling":self.call.is_some(),
            "selection":self.ui.selected,"body":self.ui.body,"reply":self.last_reply,"status":self.status,"snapshot":self.snapshot,
            "ui":self.ui})
    }

    /// `busviewer.theme`: a present `scheme` or `mode` sets that axis of the
    /// window's theme choice (a name, or null to follow the session); an
    /// absent one leaves it. Both are checked before either applies.
    fn theme(&mut self, args: &Value) -> Result<Value, (&'static str, String)> {
        type Axis<T> = Result<Option<Option<T>>, (&'static str, String)>;
        fn axis<T>(args: &Value, key: &str, from_name: fn(&str) -> Option<T>) -> Axis<T> {
            match args.get(key) {
                None => Ok(None),
                Some(Value::Null) => Ok(Some(None)),
                Some(Value::String(name)) => {
                    from_name(name).map(|v| Some(Some(v))).ok_or_else(|| ("ARGUMENT", format!("unknown {key} {name:?}")))
                }
                Some(_) => Err(("ARGUMENT", format!("{key} must be a name or null"))),
            }
        }
        let scheme = axis(args, "scheme", Scheme::from_name)?;
        let mode = axis(args, "mode", Mode::from_name)?;
        if let Some(scheme) = scheme {
            self.ui.theme_scheme = scheme;
        }
        if let Some(mode) = mode {
            self.ui.theme_mode = mode;
        }
        let (scheme, mode) = self.effective_theme();
        Ok(json!({"scheme":self.ui.theme_scheme.map(Scheme::name),"mode":self.ui.theme_mode.map(Mode::name),
            "effective":{"scheme":scheme.name(),"mode":mode.name()}}))
    }

    fn error(&mut self, id: u64, code: &str, message: &str) {
        self.effects.push(Effect::Reply { id, rc: 10, body: json!({"error_code":code,"message":message}) });
    }

    fn refusal(&self) -> &'static str {
        if self.connected { "BUSY" } else { "DISCONNECTED" }
    }

    // ---- actions (commands dispatch here) --------------------------------

    /// Refetch services; `reply` is the Bus command that asked, if any.
    pub fn refresh(&mut self, reply: Option<u64>) {
        if self.busy() || !self.connected || self.quitting {
            if let Some(id) = reply {
                let key = if self.connected { "busy" } else { "disconnected" };
                let message = self.label(key);
                self.error(id, self.refusal(), &message);
            } else if self.connected && !self.quitting {
                self.refetch = true;
            }
            return;
        }
        let ticket = self.next();
        self.discovery = Some((ticket, reply));
        self.refetch = false;
        self.status = self.label("discovering");
        self.effects.push(Effect::Discover { ticket });
    }

    /// Call the selected verb with the body, as the person would.
    pub fn call_selected(&mut self) {
        if let Some(target) = self.ui.selected.clone() {
            let body = self.ui.body.clone();
            self.start_call(target, body, None);
        }
    }

    fn start_call(&mut self, target: Selection, body: String, reply: Option<u64>) {
        let blocked = self.busy() || self.ui.dialog.is_some() || !self.connected || self.quitting;
        let error = if blocked {
            Some(self.label("busy"))
        } else if self.snapshot.verb(&target).is_none() {
            Some(self.label("invalid-target"))
        } else if body.len() > model::BODY_LIMIT {
            Some(self.label("body-too-large"))
        } else {
            model::validate_body(&body).err().map(|e| format!("{}: {e}", self.label("invalid-json")))
        };
        if let Some(error) = error {
            match reply {
                Some(id) => {
                    let code = if !self.connected {
                        "DISCONNECTED"
                    } else if blocked {
                        "BUSY"
                    } else {
                        "ARGUMENT"
                    };
                    self.error(id, code, &error);
                }
                None => self.status = error,
            }
            return;
        }
        let ticket = self.next();
        self.call = Some(Call { ticket, target: target.clone(), body: body.clone(), reply });
        self.status = self.label("calling");
        self.effects.push(Effect::Call { ticket, target, body });
    }

    /// Pretty-print the body when it is valid JSON that still fits.
    pub fn format_body(&mut self) {
        if self.busy() {
            return;
        }
        match model::validate_body(&self.ui.body) {
            Ok(()) => {
                let formatted = model::pretty(&self.ui.body);
                if formatted.len() > model::BODY_LIMIT {
                    self.status = self.label("body-too-large");
                } else {
                    self.ui.body = formatted;
                }
            }
            Err(error) => self.status = format!("{}: {error}", self.label("invalid-json")),
        }
    }

    pub fn clear_body(&mut self) {
        if !self.busy() {
            self.ui.body.clear();
        }
    }

    pub fn copy_reply(&mut self) {
        self.effects.push(Effect::Copy(self.reply.clone()));
    }

    pub fn open(&mut self, dialog: Dialog) {
        if !self.quitting {
            self.ui.dialog = Some(dialog);
        }
    }

    pub fn close_dialog(&mut self) {
        self.ui.dialog = None;
    }

    /// Close now, or as soon as the accepted operation finishes.
    pub fn quit(&mut self) {
        self.quitting = true;
        if self.busy() {
            self.status = self.label("quitting");
        } else {
            self.effects.push(Effect::Exit);
        }
    }

    // ---- edits ------------------------------------------------------------

    /// Ask the view to focus the services filter.
    pub fn focus_filter(&mut self) {
        if self.ui.dialog.is_none() {
            self.ui.focus_filter = true;
        }
    }

    /// The view has focused the filter.
    pub fn filter_focused(&mut self) {
        self.ui.focus_filter = false;
    }

    /// The scheme and mode the window shows: the choice, each unchosen
    /// axis the session's.
    pub fn effective_theme(&self) -> (Scheme, Mode) {
        (self.ui.theme_scheme.unwrap_or(self.session.0), self.ui.theme_mode.unwrap_or(self.session.1))
    }

    /// Choose the window's scheme and mode (`None` follows the session).
    pub fn set_theme(&mut self, scheme: Option<Scheme>, mode: Option<Mode>) {
        self.ui.theme_scheme = scheme;
        self.ui.theme_mode = mode;
    }

    /// The title bar's light/dark switch: the mode opposite the one shown,
    /// keeping the scheme choice.
    pub fn toggle_mode(&mut self) {
        let shown = self.effective_theme().1;
        self.set_mode(if shown == Mode::Dark { Mode::Light } else { Mode::Dark });
    }

    /// Show `mode`: the window's own toggle sends where it is going, so a
    /// click seen twice lands in the same place.
    pub fn set_mode(&mut self, mode: Mode) {
        self.ui.theme_mode = Some(mode);
    }

    pub fn set_filter(&mut self, filter: String) {
        if self.ui.dialog.is_none() {
            self.ui.filter = filter;
        }
    }

    /// Replace the body. Refused while a call is in flight (its body is
    /// frozen) or when the edit would pass the limit.
    pub fn set_body(&mut self, body: String) {
        if self.ui.dialog.is_some() || self.call.is_some() {
            return;
        }
        if body.len() > model::BODY_LIMIT {
            self.status = self.label("body-too-large");
        } else {
            self.ui.body = body;
        }
    }

    pub fn set_split(&mut self, split: f32) {
        if self.ui.dialog.is_none() {
            self.ui.split = split.clamp(0.2, 0.65);
        }
    }

    /// Open or close the tree row `key`.
    pub fn toggle(&mut self, key: &str) {
        if self.ui.dialog.is_some() {
            return;
        }
        if !self.ui.expanded.remove(key) {
            self.ui.expanded.insert(key.to_owned());
        }
    }

    /// Select the row `key`; a verb row also selects that verb.
    pub fn select_row(&mut self, row: &Row) {
        if self.ui.dialog.is_some() {
            return;
        }
        self.ui.row_key = Some(row.key.clone());
        self.ui.selected = match &row.kind {
            RowKind::Verb(target) => Some(target.clone()),
            _ => None,
        };
    }

    // ---- the tree ---------------------------------------------------------

    /// The services and mesh trees for the current filter. A filter opens
    /// every match; otherwise rows keep the person's expansion, and the
    /// selected verb's service is always open.
    pub fn tree(&self) -> Vec<Row> {
        let filter = self.ui.filter.to_lowercase();
        let open = |key: &str| self.ui.expanded.contains(key) || !filter.is_empty();
        let mut rows = Vec::new();
        for (service, result) in &self.snapshot.services {
            let key = format!("service:{service}");
            let service_matches = service.to_lowercase().contains(&filter);
            let verbs: Vec<_> = result
                .as_ref()
                .map(|verbs| {
                    verbs
                        .iter()
                        .filter(|v| {
                            service_matches
                                || format!("{} {} {}", v.name, v.args, v.description).to_lowercase().contains(&filter)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !service_matches && verbs.is_empty() {
                continue;
            }
            let mut children: Vec<Row> = verbs
                .into_iter()
                .map(|verb| Row {
                    key: format!("verb:{service}:{}", verb.name),
                    kind: RowKind::Verb(Selection { service: service.clone(), verb: verb.name.clone() }),
                    expanded: false,
                    children: Vec::new(),
                })
                .collect();
            if let Err(error) = result {
                children.push(Row { key: format!("error:{service}"), kind: RowKind::Error(error.clone()), expanded: false, children: Vec::new() });
            }
            let holds_selection = self.ui.selected.as_ref().is_some_and(|s| &s.service == service);
            rows.push(Row { expanded: open(&key) || holds_selection, key, kind: RowKind::Service(service.clone()), children });
        }
        let leaf = |key: String, kind| Row { key, kind, expanded: false, children: Vec::new() };
        let mesh = if let Some(error) = &self.snapshot.peer_error {
            vec![leaf("mesh:error".into(), RowKind::Error(error.clone()))]
        } else if self.snapshot.peers.is_empty() {
            vec![leaf("mesh:empty".into(), RowKind::NoPeers)]
        } else {
            self.snapshot
                .peers
                .iter()
                .filter(|peer| peer.to_lowercase().contains(&filter))
                .map(|peer| leaf(format!("peer:{peer}"), RowKind::Peer(peer.clone())))
                .collect()
        };
        rows.push(Row { key: "mesh".into(), kind: RowKind::Peers, expanded: open("mesh"), children: mesh });
        rows
    }

    // ---- events -----------------------------------------------------------

    /// A delivery from the Bus connection.
    pub fn delivery(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Command { id, verb, body } => self.command(id, &verb, &body),
            Delivery::Changed => {
                if self.busy() {
                    self.refetch = true;
                } else {
                    self.refresh(None);
                }
            }
            Delivery::Connected => {
                self.connected = true;
                self.refresh(None);
            }
            Delivery::Disconnected => {
                self.connected = false;
                self.status = self.label("disconnected");
            }
            // The shell reloads the theme; the engine has nothing to do.
            Delivery::Theme => {}
        }
    }

    /// Resolve the caller's target: both of service and verb, or neither
    /// (the current selection). Never a silent fallback from one to the other.
    fn target(&self, args: &Value) -> Result<Selection, String> {
        match (args.get("service"), args.get("verb")) {
            (None, None) => self.ui.selected.clone().ok_or_else(|| self.label("invalid-target")),
            (Some(service), Some(verb)) => Ok(Selection {
                service: service.as_str().ok_or("service must be a string")?.into(),
                verb: verb.as_str().ok_or("verb must be a string")?.into(),
            }),
            _ => Err("service and verb must be supplied together".into()),
        }
    }

    /// A command an agent sent to BusViewer over the Bus.
    pub fn command(&mut self, id: u64, verb: &str, body: &str) {
        let args = match serde_json::from_str::<Value>(body) {
            Ok(args) if args.is_object() => args,
            _ => return self.error(id, "ARGUMENT", "arguments must be a JSON object"),
        };
        if let Some(window) = verb.strip_prefix("busviewer.").filter(|v| v.starts_with("ui.") || v.starts_with("window")) {
            return self.effects.push(Effect::Drive { id, verb: window.to_owned(), args });
        }
        if !model::VERBS.contains(&verb) {
            return self.error(id, "UNKNOWN_VERB", "unknown BusViewer verb");
        }
        let idle = !self.busy() && self.ui.dialog.is_none() && !self.quitting;
        match verb {
            "busviewer.ping" => self.effects.push(Effect::Reply {
                id,
                rc: 0,
                body: json!({"schema":"busviewer.v1","version":env!("CARGO_PKG_VERSION")}),
            }),
            "busviewer.info" => self.effects.push(Effect::Reply { id, rc: 0, body: self.info() }),
            "HELP" => self.effects.push(Effect::Describe { id, help: true }),
            "app.describe" => self.effects.push(Effect::Describe { id, help: false }),
            "busviewer.tree"
            | "busviewer.reply"
            | "busviewer.filter"
            | "busviewer.body"
            | "busviewer.split"
            | "busviewer.expand"
            | "busviewer.select_row"
            | "busviewer.dialog" => match self.edit(verb, &args) {
                Ok(body) => self.effects.push(Effect::Reply { id, rc: 0, body }),
                Err((code, message)) => self.error(id, code, &message),
            },
            "busviewer.theme" => match self.theme(&args) {
                Ok(body) => self.effects.push(Effect::Reply { id, rc: 0, body }),
                Err((code, message)) => self.error(id, code, &message),
            },
            "busviewer.commands" => self.effects.push(Effect::Commands { id }),
            "busviewer.execute" => match args.get("id").and_then(Value::as_str) {
                Some(command) => self.effects.push(Effect::Execute { id, command: command.to_owned() }),
                None => self.error(id, "ARGUMENT", "id must be a command id string"),
            },
            "busviewer.show" if !self.quitting => self.show(Some(id)),
            "busviewer.refresh" if self.ui.dialog.is_none() => self.refresh(Some(id)),
            "busviewer.select" if idle => match self.target(&args) {
                Ok(target) if self.snapshot.verb(&target).is_some() => {
                    self.ui.row_key = Some(format!("verb:{}:{}", target.service, target.verb));
                    self.ui.selected = Some(target);
                    self.effects.push(Effect::Reply { id, rc: 0, body: self.info() });
                }
                _ => {
                    let message = self.label("invalid-target");
                    self.error(id, "ARGUMENT", &message);
                }
            },
            "busviewer.call" => {
                let target = match self.target(&args) {
                    Ok(target) => target,
                    Err(error) => return self.error(id, "ARGUMENT", &error),
                };
                let body = match args.get("body") {
                    None => self.ui.body.clone(),
                    Some(Value::String(body)) => body.clone(),
                    _ => return self.error(id, "ARGUMENT", "body must be JSON text"),
                };
                self.start_call(target, body, Some(id));
            }
            "busviewer.quit" if !self.busy() && self.ui.dialog.is_none() => {
                self.effects.push(Effect::Reply { id, rc: 0, body: json!({"quitting":true}) });
                self.quit();
            }
            "busviewer.show" | "busviewer.select" | "busviewer.refresh" | "busviewer.quit" => {
                let message = self.label("busy");
                self.error(id, "BUSY", &message);
            }
            _ => self.error(id, "UNKNOWN_VERB", "unknown BusViewer verb"),
        }
    }

    /// The state verbs: a read, or one edit as the window makes it. Refused
    /// under a dialog (as the window refuses them), and the body while a
    /// call holds it.
    fn edit(&mut self, verb: &str, args: &Value) -> Result<Value, (&'static str, String)> {
        let text = |key: &str| {
            args.get(key).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| ("ARGUMENT", format!("{key} must be a string")))
        };
        match verb {
            "busviewer.tree" => return Ok(json!({"rows":self.rows()})),
            "busviewer.reply" => return Ok(json!({"text":self.reply,"value":self.last_reply})),
            "busviewer.dialog" => {
                let dialog = match args.get("open") {
                    Some(Value::Null) => None,
                    Some(Value::String(name)) if name == "about" => Some(Dialog::About),
                    Some(Value::String(name)) if name == "shortcuts" => Some(Dialog::Shortcuts),
                    _ => return Err(("ARGUMENT", "open must be \"about\", \"shortcuts\" or null".into())),
                };
                match dialog {
                    None => self.close_dialog(),
                    Some(_) if self.ui.dialog.is_some() || self.quitting => return Err(("BUSY", self.label("busy"))),
                    Some(dialog) => self.open(dialog),
                }
                return Ok(json!({"dialog":dialog_name(self.ui.dialog)}));
            }
            _ => {}
        }
        if self.ui.dialog.is_some() {
            return Err(("BUSY", self.label("busy")));
        }
        match verb {
            "busviewer.filter" => {
                self.set_filter(text("text")?);
                Ok(json!({"filter":self.ui.filter,"rows":self.rows()}))
            }
            "busviewer.body" => {
                let body = text("text")?;
                if self.calling() {
                    return Err(("BUSY", self.label("busy")));
                }
                if body.len() > model::BODY_LIMIT {
                    return Err(("ARGUMENT", self.label("body-too-large")));
                }
                self.set_body(body);
                Ok(json!({"body":self.ui.body}))
            }
            "busviewer.split" => {
                let value = args
                    .get("value")
                    .and_then(Value::as_f64)
                    .filter(|v| (0.0..=1.0).contains(v))
                    .ok_or(("ARGUMENT", "value must be a number from 0 to 1".to_owned()))?;
                self.set_split(value as f32);
                self.split_requested = true;
                Ok(json!({"split":rounded(self.ui.split)}))
            }
            "busviewer.expand" => {
                let key = text("key")?;
                let open = args.get("open").and_then(Value::as_bool).ok_or(("ARGUMENT", "open must be a bool".to_owned()))?;
                // Only rows the window draws a chevron on open and close.
                if find(&self.tree(), &key).is_none_or(|row| row.children.is_empty()) {
                    return Err(("ARGUMENT", "no row with children has that key".into()));
                }
                if open != self.ui.expanded.contains(&key) {
                    self.toggle(&key);
                }
                let expanded = find(&self.tree(), &key).is_some_and(|row| row.expanded);
                Ok(json!({"key":key,"expanded":expanded,"rows":self.rows()}))
            }
            "busviewer.select_row" => {
                let key = text("key")?;
                let row = find(&self.tree(), &key).cloned().ok_or(("ARGUMENT", "no row has that key".to_owned()))?;
                self.select_row(&row);
                Ok(json!({"row_key":self.ui.row_key,"selection":self.ui.selected}))
            }
            _ => Err(("UNKNOWN_VERB", "unknown BusViewer verb".into())),
        }
    }

    /// The rows the tree shows, depth first, as `busviewer.tree` reports
    /// them: each row's key, kind, label (as the window draws it), depth,
    /// whether it is open and selected, and how many children it has.
    pub fn rows(&self) -> Vec<Value> {
        fn walk(engine: &Engine, rows: &[Row], depth: usize, out: &mut Vec<Value>) {
            for row in rows {
                let (kind, label) = match &row.kind {
                    RowKind::Service(name) => ("service", name.clone()),
                    RowKind::Verb(target) => ("verb", target.verb.clone()),
                    RowKind::Error(_) => ("error", engine.label("descriptions-failed")),
                    RowKind::Peers => ("peers", engine.label("peers")),
                    RowKind::Peer(name) => ("peer", name.clone()),
                    RowKind::NoPeers => ("no_peers", engine.label("no-peers")),
                };
                let mut entry = json!({"key":row.key,"kind":kind,"label":label,"depth":depth,"expanded":row.expanded,
                    "children":row.children.len(),"selected":engine.ui.row_key.as_deref() == Some(row.key.as_str())});
                match &row.kind {
                    RowKind::Verb(target) => entry["selection"] = json!(target),
                    RowKind::Error(error) => entry["error"] = json!(error),
                    _ => {}
                }
                out.push(entry);
                if row.expanded {
                    walk(engine, &row.children, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(self, &self.tree(), 0, &mut out);
        out
    }

    /// Restore and focus the window; `reply` is the Bus command that asked.
    pub fn show(&mut self, reply: Option<u64>) {
        let ticket = self.next();
        self.activations.insert(ticket);
        self.effects.push(Effect::Show { ticket, reply });
    }

    /// Discovery `ticket` finished.
    pub fn discovered(&mut self, ticket: u64, snapshot: Snapshot) {
        let Some((expected, reply)) = self.discovery else { return };
        if expected != ticket {
            return;
        }
        self.discovery = None;
        // A failed discovery keeps the last good snapshot and reports why.
        if snapshot.error.is_some() {
            self.snapshot.error = snapshot.error;
        } else {
            self.snapshot = snapshot;
        }
        let vanished = self.ui.selected.as_ref().is_some_and(|target| {
            self.snapshot.error.is_none()
                && !matches!(self.snapshot.services.get(&target.service), Some(Err(_)))
                && self.snapshot.verb(target).is_none()
        });
        if vanished {
            self.ui.selected = None;
            self.ui.row_key = None;
        }
        self.status = if !self.connected {
            self.label("disconnected")
        } else if let Some(error) = &self.snapshot.error {
            format!("{}: {error}", self.label("discovery-failed"))
        } else {
            format!(
                "{} — {} {} · {} {} · {} {}",
                self.label("connected"),
                self.snapshot.services.len(),
                self.label("services"),
                self.snapshot.peers.len(),
                self.label("peers"),
                self.snapshot.failures(),
                self.label("descriptions-failed")
            )
        };
        if let Some(id) = reply {
            let rc = if self.snapshot.error.is_some() { 10 } else { 0 };
            self.effects.push(Effect::Reply { id, rc, body: self.info() });
        }
        self.followup();
    }

    /// Call `ticket` finished.
    pub fn completed(&mut self, ticket: u64, result: Result<Reply, CallError>) {
        if self.call.as_ref().is_none_or(|call| call.ticket != ticket) {
            return;
        }
        let call = self.call.take().expect("matching call");
        let (service, verb, request) = (&call.target.service, &call.target.verb, &call.body);
        self.last_reply = match result {
            Ok(reply) => json!({"service":service,"verb":verb,"request_body":request,"rc":reply.rc,"body":reply.body}),
            Err(error) => json!({"service":service,"verb":verb,"request_body":request,
                "transport_error":error.message,"outcome_unknown":error.outcome_unknown,"retried":false}),
        };
        let rendered = if let Some(rc) = self.last_reply["rc"].as_u64() {
            let body = model::pretty(self.last_reply["body"].as_str().unwrap_or_default());
            format!("{service}  {verb}\n{} = {rc}\n\n{body}", self.label("rc"))
        } else {
            let error = self.last_reply["transport_error"].as_str().unwrap_or_default();
            format!("{service}  {verb}\n{}\n{error}", self.label("transport-error"))
        };
        self.reply = model::bounded(&rendered, &self.label("truncated"));
        self.status = self.label(if self.connected { "connected" } else { "disconnected" });
        if let Some(id) = call.reply {
            let rc = if self.last_reply["transport_error"].is_null() { 0 } else { 10 };
            self.effects.push(Effect::Reply { id, rc, body: self.last_reply.clone() });
        }
        self.followup();
    }

    /// Window activation `ticket` finished.
    pub fn shown(&mut self, ticket: u64, reply: Option<u64>, result: Result<Value, String>) {
        if !self.activations.remove(&ticket) {
            return;
        }
        if let Some(id) = reply {
            match result {
                Ok(value) => self.effects.push(Effect::Reply { id, rc: 0, body: value }),
                Err(error) => self.error(id, "ACTIVATION", &error),
            }
        }
        self.followup();
    }

    fn followup(&mut self) {
        if self.quitting {
            self.quit();
        } else if self.refetch {
            self.refresh(None);
        }
    }
}

/// The row `key` anywhere in `rows`.
pub fn find<'a>(rows: &'a [Row], key: &str) -> Option<&'a Row> {
    rows.iter().find_map(|r| if r.key == key { Some(r) } else { find(&r.children, key) })
}

/// `split` to 4 places, as the Bus reports it (an f32 widened to f64
/// would print 0.4000000059604645).
fn rounded(split: f32) -> f64 {
    (f64::from(split) * 10_000.0).round() / 10_000.0
}

/// A dialog as the Bus names it.
fn dialog_name(dialog: Option<Dialog>) -> Option<&'static str> {
    dialog.map(|d| match d {
        Dialog::About => "about",
        Dialog::Shortcuts => "shortcuts",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Verb;

    fn label(key: &str) -> String {
        key.to_owned()
    }

    fn target() -> Selection {
        Selection { service: "example".into(), verb: "echo".into() }
    }

    fn snapshot() -> Snapshot {
        let mut s = Snapshot::default();
        s.services.insert(
            "example".into(),
            Ok(vec![Verb { name: "echo".into(), args: String::new(), description: "Echo".into(), read_only: Some(true) }]),
        );
        s.services.insert("broken".into(), Err("HELP and app.describe unavailable".into()));
        s
    }

    /// An engine past its first discovery, with `snapshot()` loaded.
    fn engine() -> Engine {
        let mut e = Engine::new(label);
        let Some(Effect::Discover { ticket }) = e.take_effects().pop() else { panic!("initial discovery") };
        e.discovered(ticket, snapshot());
        e.take_effects();
        e
    }

    fn calls(effects: &[Effect]) -> usize {
        effects.iter().filter(|e| matches!(e, Effect::Call { .. })).count()
    }

    #[test]
    fn a_call_validates_and_is_sent_once() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.ui.body = "{".into();
        e.call_selected();
        assert_eq!(calls(&e.take_effects()), 0);
        assert!(e.status.starts_with("invalid-json"));
        e.ui.body = "{\"x\":1}".into();
        e.call_selected();
        let effects = e.take_effects();
        assert_eq!(calls(&effects), 1);
        let Effect::Call { ticket, .. } = effects[0] else { panic!() };
        e.call_selected();
        assert_eq!(calls(&e.take_effects()), 0, "busy: no second call");
        e.completed(ticket, Ok(Reply { rc: 0, body: "{\"ok\":true}".into() }));
        assert!(e.reply.contains("rc = 0"));
        assert!(!e.busy());
    }

    #[test]
    fn stale_completions_never_clear_a_newer_operation() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.call_selected();
        let Some(Effect::Call { ticket, .. }) = e.take_effects().pop() else { panic!() };
        e.completed(ticket + 99, Ok(Reply { rc: 0, body: "late".into() }));
        assert!(e.calling(), "a stale ticket is ignored");
        e.completed(ticket, Ok(Reply { rc: 3, body: "mine".into() }));
        assert!(!e.calling());
        assert_eq!(e.last_reply["rc"], 3);
    }

    #[test]
    fn discovery_keeps_selection_and_partial_failures() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.refresh(None);
        let Some(Effect::Discover { ticket }) = e.take_effects().pop() else { panic!() };
        e.discovered(ticket, snapshot());
        assert_eq!(e.ui.selected, Some(target()));
        assert_eq!(e.snapshot.failures(), 1);
        let tree = e.tree();
        let broken = tree.iter().find(|r| r.key == "service:broken").unwrap();
        assert!(matches!(broken.children[0].kind, RowKind::Error(_)));
        let example = tree.iter().find(|r| r.key == "service:example").unwrap();
        assert!(example.expanded, "the selected verb's service is open");
    }

    #[test]
    fn failed_discovery_keeps_the_last_snapshot_and_events_coalesce() {
        let mut e = engine();
        e.refresh(None);
        let Some(Effect::Discover { ticket }) = e.take_effects().pop() else { panic!() };
        e.delivery(Delivery::Changed);
        e.delivery(Delivery::Changed);
        assert!(e.take_effects().is_empty(), "busy: refetch is deferred");
        e.discovered(ticket, Snapshot { error: Some("noded down".into()), ..Snapshot::default() });
        assert!(e.snapshot.services.contains_key("example"));
        // The coalesced refetch starts at once and owns the status line; the
        // failure stays on the snapshot for `busviewer.info`.
        assert_eq!(e.snapshot.error.as_deref(), Some("noded down"));
        let effects = e.take_effects();
        assert_eq!(effects.iter().filter(|x| matches!(x, Effect::Discover { .. })).count(), 1, "one coalesced refetch");
    }

    #[test]
    fn caller_targets_never_fall_back_silently() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.command(7, "busviewer.call", r#"{"service":"example"}"#);
        let effects = e.take_effects();
        assert!(matches!(&effects[..], [Effect::Reply { id: 7, rc: 10, .. }]));
        assert_eq!(calls(&effects), 0);
        e.command(8, "busviewer.call", r#"{"service":"example","verb":"nope"}"#);
        assert!(matches!(&e.take_effects()[..], [Effect::Reply { id: 8, rc: 10, body }] if body["error_code"] == "ARGUMENT"));
    }

    #[test]
    fn refusals_name_their_reason() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.call_selected();
        e.take_effects();
        e.command(1, "busviewer.refresh", "{}");
        assert!(matches!(&e.take_effects()[..], [Effect::Reply { body, .. }] if body["error_code"] == "BUSY"));
        e.delivery(Delivery::Disconnected);
        e.command(2, "busviewer.call", "{}");
        assert!(matches!(&e.take_effects()[..], [Effect::Reply { body, .. }] if body["error_code"] == "DISCONNECTED"));
        e.command(3, "nope", "{}");
        assert!(matches!(&e.take_effects()[..], [Effect::Reply { body, .. }] if body["error_code"] == "UNKNOWN_VERB"));
        e.command(4, "busviewer.info", "[]");
        assert!(matches!(&e.take_effects()[..], [Effect::Reply { body, .. }] if body["error_code"] == "ARGUMENT"));
    }

    #[test]
    fn quit_waits_for_accepted_work() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.call_selected();
        let Some(Effect::Call { ticket, .. }) = e.take_effects().pop() else { panic!() };
        e.quit();
        assert!(!e.take_effects().contains(&Effect::Exit));
        assert_eq!(e.status, "quitting");
        e.completed(ticket, Ok(Reply { rc: 0, body: "{}".into() }));
        assert!(e.take_effects().contains(&Effect::Exit));
    }

    #[test]
    fn a_lost_reply_is_uncertain_and_never_retried() {
        let mut e = engine();
        e.command(5, "busviewer.call", r#"{"service":"example","verb":"echo","body":"{}"}"#);
        let Some(Effect::Call { ticket, .. }) = e.take_effects().pop() else { panic!() };
        e.completed(ticket, Err(CallError::from("lost response")));
        let effects = e.take_effects();
        assert!(matches!(&effects[..], [Effect::Reply { id: 5, rc: 10, body }]
            if body["outcome_unknown"] == true && body["retried"] == false));
        assert_eq!(calls(&effects), 0);
    }

    #[test]
    fn the_body_is_bounded_and_frozen_during_a_call() {
        let mut e = engine();
        e.set_body("x".repeat(model::BODY_LIMIT + 1));
        assert!(e.ui.body.is_empty());
        assert_eq!(e.status, "body-too-large");
        e.ui.selected = Some(target());
        e.set_body("{}".into());
        e.call_selected();
        e.set_body("changed".into());
        assert_eq!(e.ui.body, "{}", "frozen while calling");
    }

    #[test]
    fn a_dialog_blocks_calls_and_edits() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.open(Dialog::About);
        assert!(!e.callable());
        e.call_selected();
        assert_eq!(calls(&e.take_effects()), 0);
        e.set_filter("x".into());
        assert!(e.ui.filter.is_empty());
        e.close_dialog();
        assert!(e.callable());
    }

    #[test]
    fn commands_and_execute_go_to_the_registry() {
        let mut e = engine();
        e.command(1, "busviewer.commands", "{}");
        e.command(2, "busviewer.execute", r#"{"id":"file.refresh"}"#);
        e.command(3, "busviewer.execute", "{}");
        let effects = e.take_effects();
        assert_eq!(effects[0], Effect::Commands { id: 1 });
        assert_eq!(effects[1], Effect::Execute { id: 2, command: "file.refresh".into() });
        assert!(matches!(&effects[2], Effect::Reply { id: 3, rc: 10, .. }));
    }

    #[test]
    fn the_filter_opens_matches_and_hides_the_rest() {
        let mut e = engine();
        e.set_filter("echo".into());
        let tree = e.tree();
        assert!(tree.iter().any(|r| r.key == "service:example" && r.expanded));
        assert!(!tree.iter().any(|r| r.key == "service:broken"));
    }

    /// Send `verb` with `body` and return the one reply.
    fn ask(e: &mut Engine, verb: &str, body: Value) -> (u8, Value) {
        e.command(9, verb, &body.to_string());
        match &e.take_effects()[..] {
            [Effect::Reply { id: 9, rc, body }] => (*rc, body.clone()),
            other => panic!("{verb}: {other:?}"),
        }
    }

    fn code(reply: (u8, Value)) -> String {
        assert_eq!(reply.0, 10, "{}", reply.1);
        reply.1["error_code"].as_str().unwrap_or_default().to_owned()
    }

    #[test]
    fn every_verb_is_answered_and_the_rest_are_unknown() {
        for verb in model::VERBS {
            let mut e = engine();
            e.command(1, verb, "{}");
            let effects = e.take_effects();
            assert!(
                !effects.iter().any(|x| matches!(x, Effect::Reply { body, .. } if body["error_code"] == "UNKNOWN_VERB")),
                "{verb}: {effects:?}"
            );
        }
        let mut e = engine();
        assert_eq!(code(ask(&mut e, "busviewer.nope", json!({}))), "UNKNOWN_VERB");
        e.command(2, "HELP", "{}");
        e.command(3, "app.describe", "{}");
        assert_eq!(e.take_effects(), [Effect::Describe { id: 2, help: true }, Effect::Describe { id: 3, help: false }]);
    }

    #[test]
    fn window_verbs_go_to_the_drive_layer() {
        let mut e = engine();
        e.command(1, "busviewer.ui.click", r#"{"label":"File"}"#);
        e.command(2, "busviewer.window", r#"{"action":"maximize"}"#);
        e.command(3, "busviewer.window.state", "{}");
        let effects = e.take_effects();
        assert_eq!(effects[0], Effect::Drive { id: 1, verb: "ui.click".into(), args: json!({"label":"File"}) });
        assert!(matches!(&effects[1], Effect::Drive { id: 2, verb, .. } if verb == "window"));
        assert!(matches!(&effects[2], Effect::Drive { id: 3, verb, .. } if verb == "window.state"));
    }

    #[test]
    fn filter_sets_the_filter_and_returns_the_rows() {
        let mut e = engine();
        let (rc, body) = ask(&mut e, "busviewer.filter", json!({"text":"echo"}));
        assert_eq!(rc, 0);
        assert_eq!(e.ui.filter, "echo");
        let keys: Vec<_> = body["rows"].as_array().unwrap().iter().map(|r| r["key"].as_str().unwrap().to_owned()).collect();
        assert_eq!(keys, ["service:example", "verb:example:echo", "mesh", "mesh:empty"]);
        assert_eq!(code(ask(&mut e, "busviewer.filter", json!({"text":5}))), "ARGUMENT");
        e.open(Dialog::About);
        assert_eq!(code(ask(&mut e, "busviewer.filter", json!({"text":"x"}))), "BUSY");
        assert_eq!(e.ui.filter, "echo");
    }

    #[test]
    fn body_respects_the_limit_and_the_call_holding_it() {
        let mut e = engine();
        let (rc, body) = ask(&mut e, "busviewer.body", json!({"text":"{\"a\":1}"}));
        assert_eq!((rc, body["body"].as_str()), (0, Some("{\"a\":1}")));
        assert_eq!(code(ask(&mut e, "busviewer.body", json!({"text":"x".repeat(model::BODY_LIMIT + 1)}))), "ARGUMENT");
        assert_eq!(code(ask(&mut e, "busviewer.body", json!({}))), "ARGUMENT");
        e.ui.selected = Some(target());
        e.call_selected();
        e.take_effects();
        assert_eq!(code(ask(&mut e, "busviewer.body", json!({"text":"{}"}))), "BUSY");
        assert_eq!(e.ui.body, "{\"a\":1}", "frozen while calling");
    }

    #[test]
    fn split_is_bounded() {
        let mut e = engine();
        let (rc, body) = ask(&mut e, "busviewer.split", json!({"value":0.5}));
        assert_eq!((rc, body["split"].as_f64()), (0, Some(0.5)));
        assert_eq!(ask(&mut e, "busviewer.split", json!({"value":0.9})).1["split"].as_f64(), Some(0.65));
        assert_eq!(ask(&mut e, "busviewer.split", json!({"value":0.4})).1["split"].as_f64(), Some(0.4), "rounded, not 0.4000000059604645");
        assert!(e.split_requested, "the shell is told to resize the panel");
        assert_eq!(code(ask(&mut e, "busviewer.split", json!({"value":1.5}))), "ARGUMENT");
        assert_eq!(code(ask(&mut e, "busviewer.split", json!({"value":"half"}))), "ARGUMENT");
    }

    #[test]
    fn tree_lists_the_visible_rows() {
        let mut e = engine();
        let rows = ask(&mut e, "busviewer.tree", json!({})).1["rows"].clone();
        let keys: Vec<_> = rows.as_array().unwrap().iter().map(|r| r["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["service:broken", "service:example", "mesh"], "everything starts closed");
        assert_eq!(rows[1]["kind"], "service");
        assert_eq!(rows[1]["children"], 1);
        assert_eq!(rows[2]["label"], "peers");
        assert_eq!(rows[0]["depth"], 0);
    }

    #[test]
    fn expand_opens_and_closes_rows_with_children() {
        let mut e = engine();
        let (rc, body) = ask(&mut e, "busviewer.expand", json!({"key":"service:example","open":true}));
        assert_eq!((rc, body["expanded"].as_bool()), (0, Some(true)));
        let verb = body["rows"].as_array().unwrap().iter().find(|r| r["key"] == "verb:example:echo").cloned().unwrap();
        assert_eq!((verb["kind"].as_str(), verb["depth"].as_u64()), (Some("verb"), Some(1)));
        assert_eq!(verb["selection"], json!({"service":"example","verb":"echo"}));
        ask(&mut e, "busviewer.expand", json!({"key":"service:example","open":true}));
        assert!(e.ui.expanded.contains("service:example"), "opening twice leaves it open");
        let (_, body) = ask(&mut e, "busviewer.expand", json!({"key":"service:example","open":false}));
        assert_eq!(body["expanded"], false);
        assert_eq!(code(ask(&mut e, "busviewer.expand", json!({"key":"verb:example:echo","open":true}))), "ARGUMENT");
        assert_eq!(code(ask(&mut e, "busviewer.expand", json!({"key":"nope","open":true}))), "ARGUMENT");
        assert_eq!(code(ask(&mut e, "busviewer.expand", json!({"key":"mesh"}))), "ARGUMENT");
    }

    #[test]
    fn select_row_takes_any_row_kind() {
        let mut e = engine();
        let (rc, body) = ask(&mut e, "busviewer.select_row", json!({"key":"verb:example:echo"}));
        assert_eq!(rc, 0);
        assert_eq!(body["selection"], json!({"service":"example","verb":"echo"}));
        assert_eq!(e.ui.selected, Some(target()));
        let (_, body) = ask(&mut e, "busviewer.select_row", json!({"key":"error:broken"}));
        assert_eq!((body["row_key"].as_str(), &body["selection"]), (Some("error:broken"), &Value::Null));
        assert_eq!(e.ui.selected, None, "a non-verb row clears the verb");
        assert_eq!(code(ask(&mut e, "busviewer.select_row", json!({"key":"nope"}))), "ARGUMENT");
        e.open(Dialog::Shortcuts);
        assert_eq!(code(ask(&mut e, "busviewer.select_row", json!({"key":"mesh"}))), "BUSY");
    }

    #[test]
    fn dialog_opens_and_closes() {
        let mut e = engine();
        let (rc, body) = ask(&mut e, "busviewer.dialog", json!({"open":"about"}));
        assert_eq!((rc, body["dialog"].as_str()), (0, Some("about")));
        assert_eq!(e.ui.dialog, Some(Dialog::About));
        assert_eq!(code(ask(&mut e, "busviewer.dialog", json!({"open":"shortcuts"}))), "BUSY", "one dialog at a time");
        let (rc, body) = ask(&mut e, "busviewer.dialog", json!({"open":null}));
        assert_eq!((rc, &body["dialog"]), (0, &Value::Null));
        assert_eq!(e.ui.dialog, None, "a close is never refused as an edit under the dialog");
        let (rc, body) = ask(&mut e, "busviewer.dialog", json!({"open":null}));
        assert_eq!((rc, &body["dialog"], e.ui.dialog), (0, &Value::Null, None), "closing nothing is still closed");
        assert_eq!(code(ask(&mut e, "busviewer.dialog", json!({"open":"settings"}))), "ARGUMENT");
        assert_eq!(code(ask(&mut e, "busviewer.dialog", json!({}))), "ARGUMENT");
        e.quit();
        e.take_effects();
        assert_eq!(code(ask(&mut e, "busviewer.dialog", json!({"open":"about"}))), "BUSY");
    }

    #[test]
    fn reply_reads_the_last_reply() {
        let mut e = engine();
        assert_eq!(ask(&mut e, "busviewer.reply", json!({})).1, json!({"text":"","value":null}));
        e.ui.selected = Some(target());
        e.call_selected();
        let Some(Effect::Call { ticket, .. }) = e.take_effects().pop() else { panic!() };
        e.completed(ticket, Ok(Reply { rc: 0, body: "{\"ok\":true}".into() }));
        let (rc, body) = ask(&mut e, "busviewer.reply", json!({}));
        assert_eq!(rc, 0);
        assert!(body["text"].as_str().unwrap().contains("rc = 0"));
        assert_eq!(body["value"]["rc"], 0);
    }

    #[test]
    fn ui_state_round_trips() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.ui.filter = "ex".into();
        e.toggle("mesh");
        e.set_theme(Some(Scheme::Forest), Some(Mode::Dark));
        let saved = serde_json::to_string(&e.ui).unwrap();
        assert!(saved.contains("\"theme_scheme\":\"forest\"") && saved.contains("\"theme_mode\":\"dark\""), "by name: {saved}");
        let back: UiState = serde_json::from_str(&saved).unwrap();
        assert_eq!(back, e.ui);
        assert_eq!((back.theme_scheme, back.theme_mode), (Some(Scheme::Forest), Some(Mode::Dark)));
    }

    #[test]
    fn the_theme_verb_sets_present_axes_and_answers_the_effective_theme() {
        let mut e = engine();
        e.session = (Scheme::Pro, Mode::Light);
        let (rc, body) = ask(&mut e, "busviewer.theme", json!({"scheme":"forest"}));
        assert_eq!(rc, 0);
        assert_eq!(body, json!({"scheme":"forest","mode":null,"effective":{"scheme":"forest","mode":"light"}}));
        let (_, body) = ask(&mut e, "busviewer.theme", json!({"mode":"dark"}));
        assert_eq!(body["scheme"], "forest", "an absent key leaves its axis");
        assert_eq!(body["effective"], json!({"scheme":"forest","mode":"dark"}));
        let (_, body) = ask(&mut e, "busviewer.theme", json!({"scheme":null}));
        assert_eq!((&body["scheme"], &body["effective"]["scheme"]), (&Value::Null, &json!("pro")), "null follows the session");
        assert_eq!(code(ask(&mut e, "busviewer.theme", json!({"scheme":"neon"}))), "ARGUMENT");
        assert_eq!(code(ask(&mut e, "busviewer.theme", json!({"mode":"dark","scheme":"neon"}))), "ARGUMENT");
        assert_eq!(e.ui.theme_mode, Some(Mode::Dark), "a refused call changes neither axis");
        assert_eq!(code(ask(&mut e, "busviewer.theme", json!({"mode":3}))), "ARGUMENT");
        let (_, body) = ask(&mut e, "busviewer.theme", json!({}));
        assert_eq!(body["mode"], "dark", "nothing given: a read");
    }

    #[test]
    fn the_light_dark_switch_flips_the_mode_shown_and_keeps_the_scheme() {
        let mut e = engine();
        e.session = (Scheme::Pro, Mode::Light);
        e.set_theme(Some(Scheme::Studio), None);
        e.toggle_mode();
        assert_eq!((e.ui.theme_scheme, e.ui.theme_mode), (Some(Scheme::Studio), Some(Mode::Dark)));
        e.toggle_mode();
        assert_eq!(e.effective_theme(), (Scheme::Studio, Mode::Light));
    }

    #[test]
    fn a_filter_focus_request_is_never_saved_and_older_state_still_loads() {
        let mut e = engine();
        e.focus_filter();
        assert!(e.ui.focus_filter);
        let back: UiState = serde_json::from_str(&serde_json::to_string(&e.ui).unwrap()).unwrap();
        assert!(!back.focus_filter, "a one-off request, not state");
        let mut old = serde_json::to_value(UiState::default()).unwrap();
        let fields = old.as_object_mut().unwrap();
        fields.remove("theme_scheme");
        fields.remove("theme_mode");
        // State saved by the build before this one, with its old toggle.
        fields.insert("invert_mode".into(), json!(true));
        let loaded: UiState = serde_json::from_value(old).unwrap();
        assert_eq!((loaded.theme_scheme, loaded.theme_mode), (None, None), "older saved state loads, following the session");
        e.open(Dialog::About);
        e.filter_focused();
        e.focus_filter();
        assert!(!e.ui.focus_filter, "not under a dialog");
    }
}
