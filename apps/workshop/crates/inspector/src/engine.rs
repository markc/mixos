// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Workshop's behaviour, with no UI and no I/O: discovery, selection, the
//! body, one call at a time, agent commands over the Bus, and quitting only
//! once accepted work is finished.
//!
//! The shell feeds [`Engine`] events (Bus deliveries, completions, user
//! edits) and runs the [`Effect`]s it returns. Every effect that leaves the
//! process carries a ticket, and a completion whose ticket is not the one in
//! flight is ignored, so a late reply can never land on a newer operation.
//!
//! Ported from the iced reducer (`apps/workshop/src/app.rs`, markc/mixos-iced)
//! with the same rules; only the rendering and the effect plumbing differ.

use crate::bus::{CallError, Delivery, Reply};
use crate::model::{self, APP_ID, Selection, Snapshot};
use design::{CaptionSide, Mode, RequestedDecorations as Decorations, Scheme, Style};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Localises a catalogue key. The engine is headless; the shell supplies it.
pub type Label = fn(&str) -> String;

/// Work for the shell. Nothing here has happened yet.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Workshop {
        ticket: u64,
        operation: String,
        args: Value,
        reply: Option<u64>,
    },
    /// Run discovery and report [`Engine::discovered`] with `ticket`.
    Discover { ticket: u64 },
    /// Send one call and report [`Engine::completed`] with `ticket`.
    Call {
        ticket: u64,
        target: Selection,
        body: String,
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
    /// commands (`HELP`): the engine's and the window's drive commands.
    Describe { id: u64, help: bool },
    /// Hand the window-level command `command` (without the `workshop.` prefix:
    /// `ui.click`, `window`, …) to the toolkit drive layer.
    Drive {
        id: u64,
        command: String,
        args: Value,
    },
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
    NewMacro,
}

/// What the tree shows for one row.
#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Nodes,
    Mesh,
    MacroGroup(String),
    Macro {
        app: String,
        id: String,
        title: String,
    },
    Node(String),
    Service(String),
    Command(Selection),
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

/// The saved placement of the Traffic dock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockSide {
    #[default]
    Auto,
    Bottom,
    Right,
}

// Leave enough room for both the inspector and readable traffic rows.
pub const DOCK_RIGHT_WIDTH: f32 = 1280.0;

impl DockSide {
    pub fn effective(self, width: f32) -> Self {
        match self {
            Self::Auto if width >= DOCK_RIGHT_WIDTH => Self::Right,
            Self::Auto => Self::Bottom,
            side => side,
        }
    }
}

fn dock_width() -> f32 {
    0.3
}
fn dock_height() -> f32 {
    0.28
}

/// The part of the state a person arranges, saved and restored as one
/// value and readable over the Bus (`workshop.info`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default)]
    pub dock_side: DockSide,
    #[serde(default = "dock_width")]
    pub dock_width: f32,
    #[serde(default = "dock_height")]
    pub dock_height: f32,
    #[serde(default)]
    pub macros_open: bool,
    pub selected: Option<Selection>,
    pub row_key: Option<String>,
    pub filter: String,
    pub expanded: BTreeSet<String>,
    pub body: String,
    pub split: f32,
    pub dialog: Option<Dialog>,
    /// The window's own theme choice (View › Theme, the title bar's
    /// light/dark button, `workshop.theme`): `None` on an axis follows the
    /// session. The session's theme file is never written.
    #[serde(default, with = "by_name::scheme")]
    pub theme_scheme: Option<Scheme>,
    #[serde(default, with = "by_name::mode")]
    pub theme_mode: Option<Mode>,
    /// The style: `None` follows the session, `Some(None)` is the scheme's
    /// own (saved as "own"), else a style.
    #[serde(default, with = "by_name::style")]
    pub theme_style: Option<Option<Style>>,
    #[serde(default, with = "by_name::decorations")]
    pub theme_decorations: Option<Decorations>,
    #[serde(default, with = "by_name::captions")]
    pub theme_captions: Option<CaptionSide>,
    /// The view should give the services filter the keyboard (set by
    /// `view.search`, cleared by the view once it has); not saved.
    #[serde(skip)]
    pub focus_filter: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            dock_side: DockSide::Auto,
            dock_width: dock_width(),
            dock_height: dock_height(),
            macros_open: false,
            selected: None,
            row_key: None,
            filter: String::new(),
            expanded: BTreeSet::from(["nodes".into()]),
            body: String::new(),
            split: 0.34,
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

/// The theme choice saved by name ("forest", "dark"): the design's
/// [`Scheme`] and [`Mode`] carry no serde of their own.
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
    named!(decorations, design::RequestedDecorations);
    named!(captions, design::CaptionSide);

    /// The style axis: null follows the session, "own" the scheme's own
    /// style, else a style's name.
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

/// A style axis value's name: a style, or [`OWN_STYLE`].
pub fn style_name(style: Option<Style>) -> &'static str {
    style.map_or(OWN_STYLE, Style::name)
}

/// The style axis value `name` names.
pub fn style_from_name(name: &str) -> Option<Option<Style>> {
    if name == OWN_STYLE {
        Some(None)
    } else {
        Style::from_name(name).map(Some)
    }
}

/// The session theme as the shell loaded it: what an unchosen axis of the
/// window's theme choice shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Session<D = Decorations> {
    pub scheme: Scheme,
    /// `None` is the scheme's own style.
    pub style: Option<Style>,
    pub mode: Mode,
    pub decorations: D,
    pub captions: CaptionSide,
}

#[derive(Clone, Debug)]
struct Call {
    ticket: u64,
    target: Selection,
    body: String,
    reply: Option<u64>,
}

/// The services panel and Bus command share one admissible range.
pub const SERVICES_SPLIT: std::ops::RangeInclusive<f32> = 0.1..=0.8;

/// Workshop's state and rules.
pub struct Engine {
    pub workshop: crate::workshop::Workshop,
    label: Label,
    pub ui: UiState,
    pub snapshot: Snapshot,
    /// The rendered reply panel.
    pub reply: String,
    pub last_reply: Value,
    pub status: String,
    pub connected: bool,
    pub quitting: bool,
    /// `workshop.split` set the split: the shell drops the panel width
    /// egui remembers, so the window takes the new one.
    pub split_requested: bool,
    pub dock_requested: bool,
    pub dock_effective: DockSide,
    pub dock_window_width: f32,
    /// The session theme, as the shell loaded it.
    pub session: Session,
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
            workshop: crate::workshop::Workshop {
                dock_open: true,
                ..Default::default()
            },
            label,
            ui: UiState::default(),
            snapshot: Snapshot::default(),
            reply: String::new(),
            last_reply: Value::Null,
            status: label("connecting"),
            connected: true,
            quitting: false,
            split_requested: false,
            dock_requested: false,
            dock_effective: DockSide::Bottom,
            dock_window_width: 0.0,
            session: Session::default(),
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

    pub(crate) fn text(&self, key: &str) -> String {
        self.label(key)
    }
    pub(crate) fn push_workshop(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    /// Discovery, a call or a window activation is in flight.
    pub fn busy(&self) -> bool {
        self.workshop.pending > 0
            || self.discovery.is_some()
            || self.call.is_some()
            || !self.activations.is_empty()
    }

    pub fn calling(&self) -> bool {
        self.call.is_some()
    }

    /// The selected command is advertised and can be called now.
    pub fn callable(&self) -> bool {
        !self.busy()
            && self.connected
            && self.ui.dialog.is_none()
            && self
                .ui
                .selected
                .as_ref()
                .is_some_and(|t| self.snapshot.command_name(t).is_some())
    }

    fn next(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }

    /// The whole state as `workshop.info` reports it.
    pub fn info(&self) -> Value {
        let mut info = self.state();
        info["ui"]["split"] = json!(rounded(self.ui.split));
        info["ui"]["dock_effective"] = json!(self.dock_effective);
        info["ui"]["dock_size"] = json!(rounded(if self.dock_effective == DockSide::Right {
            self.ui.dock_width
        } else {
            self.ui.dock_height
        }));
        info
    }

    fn state(&self) -> Value {
        json!({"schema":"workshop.v1","app_id":APP_ID,"version":env!("CARGO_PKG_VERSION"),"pid":std::process::id(),
            "connected":self.connected,"busy":self.busy(),"discovering":self.discovery.is_some(),"calling":self.call.is_some(),
            "selection":self.ui.selected,"body":self.ui.body,"reply":self.last_reply,"status":self.status,"snapshot":self.snapshot,
            "ui":self.ui,"workshop":self.workshop})
    }

    /// `workshop.theme`: a present `scheme`, `style`, `mode`, `decorations`
    /// or `caption_side` sets that axis of the window's theme choice (a
    /// name, or null to follow the session; `style` also takes "own", the
    /// scheme's own style); an absent one leaves it. Every axis is checked
    /// before any applies.
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
        if let Some(scheme) = scheme {
            self.ui.theme_scheme = scheme;
        }
        if let Some(style) = style {
            self.ui.theme_style = style;
        }
        if let Some(mode) = mode {
            self.ui.theme_mode = mode;
        }
        if let Some(decorations) = decorations {
            self.ui.theme_decorations = decorations;
        }
        if let Some(captions) = captions {
            self.ui.theme_captions = captions;
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
                "decorations_reason": self.decoration_resolution().reason,
                "caption_side": shown.captions.name(),
            }
        }))
    }

    fn error(&mut self, id: u64, code: &str, message: &str) {
        self.effects.push(Effect::Reply {
            id,
            rc: 10,
            body: json!({"error_code":code,"message":message}),
        });
    }

    fn refusal(&self) -> &'static str {
        if self.connected {
            "BUSY"
        } else {
            "DISCONNECTED"
        }
    }

    // ---- actions (commands dispatch here) --------------------------------

    /// Refetch services; `reply` is the Bus command that asked, if any.
    pub fn refresh(&mut self, reply: Option<u64>) {
        if self.busy() || !self.connected || self.quitting {
            if let Some(id) = reply {
                let key = if self.connected {
                    "busy"
                } else {
                    "disconnected"
                };
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

    /// Call the selected command with the body, as the person would.
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
        } else if self.snapshot.command_name(&target).is_none() {
            Some(self.label("invalid-target"))
        } else if body.len() > model::BODY_LIMIT {
            Some(self.label("body-too-large"))
        } else {
            model::validate_body(&body)
                .err()
                .map(|e| format!("{}: {e}", self.label("invalid-json")))
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
        self.call = Some(Call {
            ticket,
            target: target.clone(),
            body: body.clone(),
            reply,
        });
        self.status = self.label("calling");
        self.effects.push(Effect::Call {
            ticket,
            target,
            body,
        });
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
        let shown = self.effective();
        (shown.scheme, shown.mode)
    }

    /// Every axis the window shows: the choice, each unchosen axis the
    /// session's.
    pub fn decoration_resolution(&self) -> design::decorations::Resolution {
        design::decorations::resolve(self.ui.theme_decorations, self.session.decorations)
    }

    pub fn effective(&self) -> Session<design::Decorations> {
        let ui = &self.ui;
        Session {
            scheme: ui.theme_scheme.unwrap_or(self.session.scheme),
            style: ui.theme_style.unwrap_or(self.session.style),
            mode: ui.theme_mode.unwrap_or(self.session.mode),
            decorations: self.decoration_resolution().mode,
            captions: ui.theme_captions.unwrap_or(self.session.captions),
        }
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
        self.set_mode(if shown == Mode::Dark {
            Mode::Light
        } else {
            Mode::Dark
        });
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

    pub fn set_dock_side(&mut self, side: DockSide) {
        self.ui.dock_side = side;
        self.dock_effective = side.effective(self.dock_window_width);
        self.dock_requested = true;
    }

    pub fn set_dock_size(&mut self, side: DockSide, size: f32) {
        if size.is_finite() {
            // Auto stores both axes, so the request survives the first layout
            // and later crossings of the wide-window threshold alike.
            if side == DockSide::Auto {
                self.ui.dock_width = size.clamp(0.2, 0.5);
                self.ui.dock_height = size.clamp(0.12, 0.5);
                return;
            }
            let side = side.effective(self.dock_window_width);
            if side == DockSide::Right {
                self.ui.dock_width = size.clamp(0.2, 0.5);
            } else {
                self.ui.dock_height = size.clamp(0.12, 0.5);
            }
        }
    }

    pub fn set_split(&mut self, split: f32) {
        if self.ui.dialog.is_none() {
            self.ui.split = split.clamp(*SERVICES_SPLIT.start(), *SERVICES_SPLIT.end());
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

    /// Select the row `key`; a command row also selects that command.
    pub fn select_row(&mut self, row: &Row) {
        if self.ui.dialog.is_some() {
            return;
        }
        self.ui.row_key = Some(row.key.clone());
        if row.kind == RowKind::Mesh {
            self.refresh_mesh();
            self.ensure_observing();
        }
        if let RowKind::Macro { app, id, .. } = &row.kind {
            self.queue_workshop("macros.open", json!({"app":app,"id":id}), None);
        } else {
            self.workshop.selected = None;
        }
        self.ui.selected = match &row.kind {
            RowKind::Command(target) => Some(target.clone()),
            _ => None,
        };
    }

    // ---- the tree ---------------------------------------------------------

    /// The services and mesh trees for the current filter. A filter opens
    /// every match; otherwise rows keep the person's expansion, and the
    /// selected command's service is always open.
    pub fn tree(&self) -> Vec<Row> {
        let mut rows = self.service_tree();
        self.macro_tree(&mut rows);
        // Areas are branches. The Mesh map belongs beside node rows under
        // Nodes; global macros remain a separate root.
        let mut children = vec![Row {
            key: "nodes:mesh".into(),
            kind: RowKind::Mesh,
            expanded: false,
            children: Vec::new(),
        }];
        rows.retain(|row| {
            if row.key == "macros:global" {
                true
            } else {
                children.push(row.clone());
                false
            }
        });
        rows.insert(
            0,
            Row {
                key: "nodes".into(),
                kind: RowKind::Nodes,
                expanded: self.ui.expanded.contains("nodes") || !self.ui.filter.is_empty(),
                children,
            },
        );
        rows
    }

    fn service_tree(&self) -> Vec<Row> {
        if !self.snapshot.nodes.is_empty() {
            return self.node_tree();
        }
        let filter = self.ui.filter.to_lowercase();
        let open = |key: &str| self.ui.expanded.contains(key) || !filter.is_empty();
        let mut rows = Vec::new();
        for (service, result) in &self.snapshot.services {
            let key = format!("service:{service}");
            let service_matches = service.to_lowercase().contains(&filter);
            let command_names: Vec<_> = result
                .as_ref()
                .map(|command_names| {
                    command_names
                        .iter()
                        .filter(|v| {
                            service_matches
                                || format!("{} {} {}", v.name, v.args, v.description)
                                    .to_lowercase()
                                    .contains(&filter)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !service_matches && command_names.is_empty() {
                continue;
            }
            let mut children: Vec<Row> = command_names
                .into_iter()
                .map(|command_name| Row {
                    key: format!("command:{service}:{}", command_name.name),
                    kind: RowKind::Command(Selection {
                        service: service.clone(),
                        command: command_name.name.clone(),
                    }),
                    expanded: false,
                    children: Vec::new(),
                })
                .collect();
            if let Err(error) = result {
                children.push(Row {
                    key: format!("error:{service}"),
                    kind: RowKind::Error(error.clone()),
                    expanded: false,
                    children: Vec::new(),
                });
            }
            let holds_selection = self
                .ui
                .selected
                .as_ref()
                .is_some_and(|s| &s.service == service);
            rows.push(Row {
                expanded: open(&key) || holds_selection,
                key,
                kind: RowKind::Service(service.clone()),
                children,
            });
        }
        let leaf = |key: String, kind| Row {
            key,
            kind,
            expanded: false,
            children: Vec::new(),
        };
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
        rows.push(Row {
            key: "mesh".into(),
            kind: RowKind::Peers,
            expanded: open("mesh"),
            children: mesh,
        });
        rows
    }

    fn node_tree(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(error) = &self.snapshot.peer_error {
            rows.push(Row {
                key: "mesh:error".into(),
                kind: RowKind::Error(format!("noded.peers: {error}")),
                expanded: false,
                children: Vec::new(),
            });
        }
        for node in &self.snapshot.nodes {
            let engine = self.clone_for_node(node);
            // Reuse the service/command filtering and selection rules.
            let mut children = engine.service_tree();
            children.retain(|row| row.key != "mesh");
            for row in &mut children {
                if let RowKind::Service(route) = &row.kind {
                    let name = self
                        .snapshot
                        .identities
                        .get(route)
                        .map(|info| info.name.as_str())
                        .unwrap_or(route);
                    row.key = format!("service:{}:{name}", node.name);
                    row.expanded |= self.ui.expanded.contains(&row.key);
                    for child in &mut row.children {
                        if let RowKind::Command(target) = &child.kind {
                            child.key = format!("command:{}:{name}:{}", node.name, target.command);
                        }
                    }
                }
            }
            if let Some(error) = &node.error {
                children.push(Row {
                    key: format!("node:{}:error", node.name),
                    kind: RowKind::Error(error.clone()),
                    expanded: false,
                    children: Vec::new(),
                });
            }
            let key = format!("node:{}", node.name);
            rows.push(Row {
                expanded: self.ui.expanded.contains(&key) || !self.ui.filter.is_empty(),
                key,
                kind: RowKind::Node(node.name.clone()),
                children,
            });
        }
        rows
    }

    fn clone_for_node(&self, node: &model::Node) -> Self {
        let mut engine = Self::new(self.label);
        engine.ui = self.ui.clone();
        if node
            .name
            .to_lowercase()
            .contains(&engine.ui.filter.to_lowercase())
        {
            engine.ui.filter.clear();
        }
        engine.snapshot.services = self
            .snapshot
            .services
            .iter()
            .filter(|(name, _)| node.services.contains(name))
            .map(|(name, command_names)| (name.clone(), command_names.clone()))
            .collect();
        engine
    }

    pub fn service_label(&self, route: &str) -> String {
        let Some(info) = self.snapshot.identities.get(route) else {
            return route.into();
        };
        let mut label = info.name.clone();
        if let Some(version) = &info.version {
            label.push_str(&format!("  {version}"));
        }
        if let Some(sha) = &info.git_sha {
            label.push_str(&format!(" · {}", sha.chars().take(8).collect::<String>()));
        }
        label
    }

    // ---- events -----------------------------------------------------------

    /// A delivery from the Bus connection.
    pub fn delivery(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Command {
                id,
                command: command_name,
                body,
            } => self.command(id, &command_name, &body),
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
                self.refresh_mesh();
                self.ensure_observing();
            }
            Delivery::Disconnected => {
                self.connected = false;
                self.workshop.subscription = None;
                self.workshop.mesh.observe_ticket = None;
                self.workshop.mesh.nodes_ticket = None;
                self.workshop.mesh.ping_ticket = None;
                self.workshop.mesh.ping_queue.clear();
                let reason = self.label("disconnected");
                self.workshop.mesh.set_observing(false, Some(&reason));
                self.status = self.label("disconnected");
            }
            // The shell reloads the theme; the engine has nothing to do.
            Delivery::Theme => {}
            // Workshop subscribes to no topics of its own.
            Delivery::Topic(_) => {}
        }
    }

    /// Resolve the caller's target: both of service and command, or neither
    /// (the current selection). Never a silent fallback from one to the other.
    fn target(&self, args: &Value) -> Result<Selection, String> {
        match (args.get("service"), args.get("command")) {
            (None, None) => self
                .ui
                .selected
                .clone()
                .ok_or_else(|| self.label("invalid-target")),
            (Some(service), Some(command_name)) => Ok(Selection {
                service: service.as_str().ok_or("service must be a string")?.into(),
                command: command_name
                    .as_str()
                    .ok_or("command must be a string")?
                    .into(),
            }),
            _ => Err("service and command must be supplied together".into()),
        }
    }

    /// A command an agent sent to Workshop over the Bus.
    pub fn command(&mut self, id: u64, command_name: &str, body: &str) {
        if let Some(suffix) = command_name.strip_prefix("workshop.macros.") {
            let limit = if suffix == "put" {
                ::macros::commands::PUT_ENVELOPE_LIMIT
            } else {
                8192
            };
            if body.len() > limit {
                return self.error(
                    id,
                    "INVALID_ARGUMENT",
                    &format!("request exceeds {limit} bytes"),
                );
            }
        }
        let args = match serde_json::from_str::<Value>(body) {
            Ok(args) if args.is_object() => args,
            _ => return self.error(id, "ARGUMENT", "arguments must be a JSON object"),
        };
        if self.workshop_command(id, command_name, &args) {
            return;
        }
        if let Some(window) = command_name
            .strip_prefix("workshop.")
            .filter(|v| v.starts_with("ui.") || v.starts_with("window"))
        {
            return self.effects.push(Effect::Drive {
                id,
                command: window.to_owned(),
                args,
            });
        }
        if !model::COMMANDS.contains(&command_name) {
            return self.error(id, "UNKNOWN_COMMAND", "unknown Workshop command");
        }
        let idle = !self.busy() && self.ui.dialog.is_none() && !self.quitting;
        match command_name {
            "workshop.ping" => self.effects.push(Effect::Reply {
                id,
                rc: 0,
                body: json!({"schema":"workshop.v1","version":env!("CARGO_PKG_VERSION")}),
            }),
            "workshop.info" => self.effects.push(Effect::Reply {
                id,
                rc: 0,
                body: self.info(),
            }),
            "HELP" => self.effects.push(Effect::Describe { id, help: true }),
            "app.describe" => self.effects.push(Effect::Describe { id, help: false }),
            "workshop.tree"
            | "workshop.reply"
            | "workshop.filter"
            | "workshop.body"
            | "workshop.split"
            | "workshop.dock"
            | "workshop.expand"
            | "workshop.select_row"
            | "workshop.dialog" => match self.edit(command_name, &args) {
                Ok(body) => self.effects.push(Effect::Reply { id, rc: 0, body }),
                Err((code, message)) => self.error(id, code, &message),
            },
            "workshop.theme" => match self.theme(&args) {
                Ok(body) => self.effects.push(Effect::Reply { id, rc: 0, body }),
                Err((code, message)) => self.error(id, code, &message),
            },
            "workshop.actions" => self.effects.push(Effect::Commands { id }),
            "workshop.execute" => match args.get("id").and_then(Value::as_str) {
                Some(command) => self.effects.push(Effect::Execute {
                    id,
                    command: command.to_owned(),
                }),
                None => self.error(id, "ARGUMENT", "id must be a command id string"),
            },
            "workshop.show" if !self.quitting => self.show(Some(id)),
            "workshop.refresh" if self.ui.dialog.is_none() => self.refresh(Some(id)),
            "workshop.select" if idle => match self.target(&args) {
                Ok(target) if self.snapshot.command_name(&target).is_some() => {
                    self.ui.row_key = Some(
                        if let Some(node) = self
                            .snapshot
                            .nodes
                            .iter()
                            .find(|node| node.services.contains(&target.service))
                        {
                            let name = self
                                .snapshot
                                .identities
                                .get(&target.service)
                                .map(|info| info.name.as_str())
                                .unwrap_or(&target.service);
                            self.ui.expanded.insert(format!("node:{}", node.name));
                            format!("command:{}:{name}:{}", node.name, target.command)
                        } else {
                            format!("command:{}:{}", target.service, target.command)
                        },
                    );
                    self.ui.selected = Some(target);
                    self.effects.push(Effect::Reply {
                        id,
                        rc: 0,
                        body: self.info(),
                    });
                }
                _ => {
                    let message = self.label("invalid-target");
                    self.error(id, "ARGUMENT", &message);
                }
            },
            "workshop.call" => {
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
            "workshop.quit" if !self.busy() && self.ui.dialog.is_none() => {
                self.effects.push(Effect::Reply {
                    id,
                    rc: 0,
                    body: json!({"quitting":true}),
                });
                self.quit();
            }
            "workshop.show" | "workshop.select" | "workshop.refresh" | "workshop.quit" => {
                let message = self.label("busy");
                self.error(id, "BUSY", &message);
            }
            _ => self.error(id, "UNKNOWN_COMMAND", "unknown Workshop command"),
        }
    }

    /// The state commands: a read, or one edit as the window makes it. Refused
    /// under a dialog (as the window refuses them), and the body while a
    /// call holds it.
    fn edit(&mut self, command_name: &str, args: &Value) -> Result<Value, (&'static str, String)> {
        let text = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| ("ARGUMENT", format!("{key} must be a string")))
        };
        match command_name {
            "workshop.tree" => return Ok(json!({"rows":self.rows()})),
            "workshop.reply" => return Ok(json!({"text":self.reply,"value":self.last_reply})),
            "workshop.dialog" => {
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
                return Ok(json!({"dialog":dialog_name(self.ui.dialog)}));
            }
            _ => {}
        }
        if self.ui.dialog.is_some() {
            return Err(("BUSY", self.label("busy")));
        }
        match command_name {
            "workshop.filter" => {
                self.set_filter(text("text")?);
                Ok(json!({"filter":self.ui.filter,"rows":self.rows()}))
            }
            "workshop.body" => {
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
            "workshop.dock" => {
                let side = args
                    .get("side")
                    .and_then(Value::as_str)
                    .and_then(|side| match side {
                        "auto" => Some(DockSide::Auto),
                        "bottom" => Some(DockSide::Bottom),
                        "right" => Some(DockSide::Right),
                        _ => None,
                    })
                    .ok_or(("ARGUMENT", "side must be auto, bottom or right".to_owned()))?;
                let size = args
                    .get("size")
                    .map(|v| {
                        v.as_f64()
                            .filter(|v| (0.0..=1.0).contains(v))
                            .ok_or(("ARGUMENT", "size must be a number from 0 to 1".to_owned()))
                    })
                    .transpose()?;
                let open = args
                    .get("open")
                    .map(|v| {
                        v.as_bool()
                            .ok_or(("ARGUMENT", "open must be a bool".to_owned()))
                    })
                    .transpose()?;
                self.set_dock_side(side);
                if let Some(size) = size {
                    self.set_dock_size(self.ui.dock_side, size as f32);
                }
                if let Some(open) = open {
                    self.workshop.dock_open = open;
                }
                self.dock_requested = true;
                Ok(
                    json!({"side":self.ui.dock_side,"effective":self.dock_effective,
                    "width":rounded(self.ui.dock_width),"height":rounded(self.ui.dock_height),
                    "open":self.workshop.dock_open}),
                )
            }
            "workshop.split" => {
                let value = args
                    .get("value")
                    .and_then(Value::as_f64)
                    .filter(|v| (0.0..=1.0).contains(v))
                    .ok_or(("ARGUMENT", "value must be a number from 0 to 1".to_owned()))?;
                self.set_split(value as f32);
                self.split_requested = true;
                Ok(json!({"split":rounded(self.ui.split)}))
            }
            "workshop.expand" => {
                let key = text("key")?;
                let open = args
                    .get("open")
                    .and_then(Value::as_bool)
                    .ok_or(("ARGUMENT", "open must be a bool".to_owned()))?;
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
            "workshop.select_row" => {
                let key = text("key")?;
                let row = find(&self.tree(), &key)
                    .cloned()
                    .ok_or(("ARGUMENT", "no row has that key".to_owned()))?;
                self.select_row(&row);
                Ok(json!({"row_key":self.ui.row_key,"selection":self.ui.selected}))
            }
            _ => Err(("UNKNOWN_COMMAND", "unknown Workshop command".into())),
        }
    }

    /// The rows the tree shows, depth first, as `workshop.tree` reports
    /// them: each row's key, kind, label (as the window draws it), depth,
    /// whether it is open and selected, and how many children it has.
    pub fn rows(&self) -> Vec<Value> {
        fn walk(engine: &Engine, rows: &[Row], depth: usize, out: &mut Vec<Value>) {
            for row in rows {
                let (kind, label) = match &row.kind {
                    RowKind::Nodes => ("nodes", engine.label("nodes-root")),
                    RowKind::Mesh => ("mesh", engine.label("mesh-map")),
                    RowKind::MacroGroup(title) => ("macros", title.clone()),
                    RowKind::Macro { title, .. } => ("macro", title.clone()),
                    RowKind::Node(name) => ("node", engine.node_label(name)),
                    RowKind::Service(name) => ("service", engine.service_label(name)),
                    RowKind::Command(target) => ("command", target.command.clone()),
                    RowKind::Error(_) => ("error", engine.label("descriptions-failed")),
                    RowKind::Peers => ("peers", engine.label("peers")),
                    RowKind::Peer(name) => ("peer", name.clone()),
                    RowKind::NoPeers => ("no_peers", engine.label("no-peers")),
                };
                let mut entry = json!({"key":row.key,"kind":kind,"label":label,"depth":depth,"expanded":row.expanded,
                    "children":row.children.len(),"selected":engine.ui.row_key.as_deref() == Some(row.key.as_str())});
                match &row.kind {
                    RowKind::Node(name) => {
                        entry["error"] = json!(
                            engine
                                .snapshot
                                .nodes
                                .iter()
                                .find(|node| node.name == *name)
                                .and_then(|node| node.error.as_ref())
                        );
                    }
                    RowKind::Service(name) => {
                        entry["identity"] = json!(engine.snapshot.identities.get(name))
                    }
                    RowKind::Command(target) => entry["selection"] = json!(target),
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

    pub fn node_label(&self, name: &str) -> String {
        match self
            .snapshot
            .nodes
            .iter()
            .find(|node| node.name == name)
            .and_then(|node| node.error.as_ref())
        {
            Some(error) => format!("{name} — {}: {error}", self.label("unreachable")),
            None => name.to_owned(),
        }
    }

    /// Restore and focus the window; `reply` is the Bus command that asked.
    pub fn show(&mut self, reply: Option<u64>) {
        let ticket = self.next();
        self.activations.insert(ticket);
        self.effects.push(Effect::Show { ticket, reply });
    }

    /// Discovery `ticket` finished.
    pub fn discovered(&mut self, ticket: u64, snapshot: Snapshot) {
        let Some((expected, reply)) = self.discovery else {
            return;
        };
        if expected != ticket {
            return;
        }
        let finished = snapshot
            .progress
            .as_ref()
            .is_none_or(|progress| progress.finished);
        if finished {
            self.discovery = None;
        }
        // A failed discovery keeps the last good snapshot and reports why.
        if snapshot.error.is_some() {
            self.snapshot.error = snapshot.error;
            self.snapshot.peer_error = snapshot.peer_error;
        } else {
            if !self.snapshot.nodes.iter().any(|node| node.local) {
                for node in &snapshot.nodes {
                    if node.local {
                        self.ui.expanded.insert(format!("node:{}", node.name));
                    }
                }
            }
            self.snapshot = snapshot;
        }
        let vanished = finished
            && self.ui.selected.as_ref().is_some_and(|target| {
                self.snapshot.error.is_none()
                    && !matches!(self.snapshot.services.get(&target.service), Some(Err(_)))
                    && self.snapshot.command_name(target).is_none()
            });
        if vanished {
            self.ui.selected = None;
            self.ui.row_key = None;
        }
        self.status = if !self.connected {
            self.label("disconnected")
        } else if let Some(error) = &self.snapshot.error {
            format!("{}: {error}", self.label("discovery-failed"))
        } else if let Some(progress) = &self.snapshot.progress {
            format!(
                "{} {} {} {}{}, {} {}",
                progress.completed,
                self.label("of"),
                progress.total,
                self.label("nodes"),
                if finished { "" } else { "…" },
                self.snapshot
                    .nodes
                    .iter()
                    .filter(|node| node.error.is_some())
                    .count(),
                self.label("unreachable")
            )
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
        if let Some(error) = &self.snapshot.peer_error {
            self.status.push_str(&format!(" · noded.peers: {error}"));
        }
        if !finished {
            return;
        }
        if let Some(id) = reply {
            let rc = if self.snapshot.error.is_some() { 10 } else { 0 };
            self.effects.push(Effect::Reply {
                id,
                rc,
                body: self.info(),
            });
        }
        self.followup();
    }

    /// Call `ticket` finished.
    pub fn completed_at(
        &mut self,
        ticket: u64,
        result: Result<Reply, CallError>,
        at: String,
        ms: u64,
    ) {
        if self.completed_retained(ticket, result) {
            self.workshop.stamp_last(at, ms);
        }
    }

    pub fn completed(&mut self, ticket: u64, result: Result<Reply, CallError>) {
        self.completed_retained(ticket, result);
    }

    fn completed_retained(&mut self, ticket: u64, result: Result<Reply, CallError>) -> bool {
        if self.call.as_ref().is_none_or(|call| call.ticket != ticket) {
            return false;
        }
        let call = self.call.take().expect("matching call");
        let (service, command_name, request) =
            (&call.target.service, &call.target.command, &call.body);
        self.last_reply = match result {
            Ok(reply) => {
                json!({"service":service,"command":command_name,"request_body":request,"rc":reply.rc,"body":reply.body})
            }
            Err(error) => json!({"service":service,"command":command_name,"request_body":request,
                "transport_error":error.message,"outcome_unknown":error.outcome_unknown,"retried":false}),
        };
        let payload = self.last_reply["body"]
            .as_str()
            .and_then(|text| serde_json::from_str::<Value>(text).ok());
        let request_body = serde_json::from_str::<Value>(request).ok();
        let retained = self.workshop.retain(json!({"direction":"local","from":"workshop","service":service,"command":command_name,"rc":self.last_reply["rc"],"payload":payload,"request":request_body}));
        let rendered = if let Some(rc) = self.last_reply["rc"].as_u64() {
            let body = model::pretty(self.last_reply["body"].as_str().unwrap_or_default());
            format!(
                "{service}  {command_name}\n{} = {rc}\n\n{body}",
                self.label("rc")
            )
        } else {
            let error = self.last_reply["transport_error"]
                .as_str()
                .unwrap_or_default();
            format!(
                "{service}  {command_name}\n{}\n{error}",
                self.label("transport-error")
            )
        };
        self.reply = model::bounded(&rendered, &self.label("truncated"));
        self.status = self.label(if self.connected {
            "connected"
        } else {
            "disconnected"
        });
        if let Some(id) = call.reply {
            let rc = if self.last_reply["transport_error"].is_null() {
                0
            } else {
                10
            };
            self.effects.push(Effect::Reply {
                id,
                rc,
                body: self.last_reply.clone(),
            });
        }
        self.followup();
        retained
    }

    /// Window activation `ticket` finished.
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
        self.followup();
    }

    pub fn workshop_settled(&mut self) {
        if !self.busy() {
            self.followup();
        }
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
    rows.iter().find_map(|r| {
        if r.key == key {
            Some(r)
        } else {
            find(&r.children, key)
        }
    })
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
        Dialog::NewMacro => "new-macro",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bus_auto_first_detects_once_with_bounded_reply() {
        use design::decorations::{Resolver, SessionFacts, with_resolver};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use std::time::{Duration, Instant};
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let resolver = Arc::new(Resolver::new(
            move || {
                counted.fetch_add(1, Ordering::Relaxed);
                SessionFacts {
                    wayland: true,
                    desktops: "KDE".into(),
                    decoration_global: Some(true),
                }
            },
            || None,
            |_| {},
        ));
        with_resolver(resolver, || {
            let mut e = Engine::new(label);
            e.take_effects();
            for (id, request, effective, reason) in [
                (1, "csd", "csd", "choice"),
                (2, "auto", "ssd", "rule3"),
                (3, "auto", "ssd", "rule3"),
                (4, "ssd", "ssd", "choice"),
            ] {
                let start = Instant::now();
                e.command(
                    id,
                    "workshop.theme",
                    &json!({"decorations":request}).to_string(),
                );
                assert!(start.elapsed() < Duration::from_millis(250));
                let replies = e.take_effects();
                let body = replies
                    .iter()
                    .find_map(|effect| match effect {
                        Effect::Reply {
                            id: reply,
                            rc: 0,
                            body,
                        } if *reply == id => Some(body),
                        _ => None,
                    })
                    .expect("successful theme reply");
                assert_eq!(body["decorations"], request);
                assert_eq!(body["effective"]["decorations"], effective);
                assert_eq!(body["effective"]["decorations_reason"], reason);
                assert_eq!(calls.load(Ordering::Relaxed), usize::from(id > 1));
            }
            e.command(5, "workshop.theme", r#"{"decorations":"bogus"}"#);
            assert!(e.take_effects().iter().any(|effect| matches!(
                effect, Effect::Reply { id: 5, rc: 10, body } if body["error_code"] == "ARGUMENT"
            )));
            assert_eq!(e.ui.theme_decorations, Some(Decorations::Server));
        });
    }

    #[test]
    fn bus_choice_is_stored_under_environment_override() {
        use design::decorations::{Resolver, SessionFacts, with_resolver};
        use std::sync::Arc;
        let resolver = Arc::new(Resolver::new(
            || SessionFacts {
                wayland: false,
                desktops: String::new(),
                decoration_global: None,
            },
            || Some("csd".into()),
            |_| {},
        ));
        with_resolver(resolver, || {
            let mut e = Engine::new(label);
            let body = e.theme(&json!({"decorations":"ssd"})).unwrap();
            assert_eq!(e.ui.theme_decorations, Some(Decorations::Server));
            assert_eq!(body["effective"]["decorations"], "csd");
            assert_eq!(body["effective"]["decorations_reason"], "override");
        });
    }

    use crate::model::Command;

    fn label(key: &str) -> String {
        key.to_owned()
    }

    fn target() -> Selection {
        Selection {
            service: "example".into(),
            command: "echo".into(),
        }
    }

    fn snapshot() -> Snapshot {
        let mut s = Snapshot::default();
        s.services.insert(
            "example".into(),
            Ok(vec![Command {
                name: "echo".into(),
                args: String::new(),
                description: "Echo".into(),
                read_only: Some(true),
            }]),
        );
        s.services.insert(
            "broken".into(),
            Err("HELP and app.describe unavailable".into()),
        );
        s
    }

    /// An engine past its first discovery, with `snapshot()` loaded.
    fn engine() -> Engine {
        let mut e = Engine::new(label);
        let Some(Effect::Discover { ticket }) = e.take_effects().pop() else {
            panic!("initial discovery")
        };
        e.discovered(ticket, snapshot());
        e.take_effects();
        e
    }

    fn calls(effects: &[Effect]) -> usize {
        effects
            .iter()
            .filter(|e| matches!(e, Effect::Call { .. }))
            .count()
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
        let Effect::Call { ticket, .. } = effects[0] else {
            panic!()
        };
        e.call_selected();
        assert_eq!(calls(&e.take_effects()), 0, "busy: no second call");
        e.completed(
            ticket,
            Ok(Reply {
                rc: 0,
                body: "{\"ok\":true}".into(),
                error: None,
            }),
        );
        assert!(e.reply.contains("rc = 0"));
        assert!(!e.busy());
    }

    #[test]
    fn oversized_completion_never_stamps_an_earlier_row() {
        let mut e = engine();
        e.workshop
            .retain(json!({"command":"earlier","ts":"original","ms":1}));
        e.ui.selected = Some(target());
        e.call_selected();
        let ticket = e.call.as_ref().unwrap().ticket;
        e.completed_at(
            ticket,
            Ok(Reply {
                rc: 0,
                body: json!({"text":"x".repeat(crate::workshop::TRAFFIC_BYTES)}).to_string(),
                error: None,
            }),
            "new".into(),
            99,
        );
        assert_eq!(e.workshop.traffic.len(), 1);
        assert_eq!(e.workshop.traffic[0]["ts"], "original");
        assert_eq!(e.workshop.traffic[0]["ms"], 1);
        assert_eq!(e.workshop.dropped, 1);
    }

    #[test]
    fn stale_completions_never_clear_a_newer_operation() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.call_selected();
        let Some(Effect::Call { ticket, .. }) = e.take_effects().pop() else {
            panic!()
        };
        e.completed(
            ticket + 99,
            Ok(Reply {
                rc: 0,
                body: "late".into(),
                error: None,
            }),
        );
        assert!(e.calling(), "a stale ticket is ignored");
        e.completed(
            ticket,
            Ok(Reply {
                rc: 3,
                body: "mine".into(),
                error: None,
            }),
        );
        assert!(!e.calling());
        assert_eq!(e.last_reply["rc"], 3);
    }

    #[test]
    fn discovery_keeps_selection_and_partial_failures() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.refresh(None);
        let Some(Effect::Discover { ticket }) = e.take_effects().pop() else {
            panic!()
        };
        e.discovered(ticket, snapshot());
        assert_eq!(e.ui.selected, Some(target()));
        assert_eq!(e.snapshot.failures(), 1);
        let tree = e.tree();
        let broken = find(&tree, "service:broken").unwrap();
        assert!(matches!(broken.children[0].kind, RowKind::Error(_)));
        let example = find(&tree, "service:example").unwrap();
        assert!(example.expanded, "the selected command's service is open");
    }

    #[test]
    fn failed_discovery_keeps_the_last_snapshot_and_events_coalesce() {
        let mut e = engine();
        e.refresh(None);
        let Some(Effect::Discover { ticket }) = e.take_effects().pop() else {
            panic!()
        };
        e.delivery(Delivery::Changed);
        e.delivery(Delivery::Changed);
        assert!(e.take_effects().is_empty(), "busy: refetch is deferred");
        e.discovered(
            ticket,
            Snapshot {
                error: Some("noded down".into()),
                ..Snapshot::default()
            },
        );
        assert!(e.snapshot.services.contains_key("example"));
        // The coalesced refetch starts at once and owns the status line; the
        // failure stays on the snapshot for `workshop.info`.
        assert_eq!(e.snapshot.error.as_deref(), Some("noded down"));
        let effects = e.take_effects();
        assert_eq!(
            effects
                .iter()
                .filter(|x| matches!(x, Effect::Discover { .. }))
                .count(),
            1,
            "one coalesced refetch"
        );
    }

    #[test]
    fn simultaneous_membership_and_peer_failures_keep_both_diagnostics() {
        let mut e = engine();
        e.refresh(None);
        let Some(Effect::Discover { ticket }) = e.take_effects().pop() else {
            panic!()
        };
        e.discovered(
            ticket,
            Snapshot {
                error: Some("membership failed".into()),
                peer_error: Some("peers failed".into()),
                ..Snapshot::default()
            },
        );
        assert!(e.snapshot.services.contains_key("example"));
        assert_eq!(e.snapshot.peer_error.as_deref(), Some("peers failed"));
        assert!(e.status.contains("membership failed"));
        assert!(e.status.contains("noded.peers: peers failed"));
    }

    #[test]
    fn caller_targets_never_fall_back_silently() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.command(7, "workshop.call", r#"{"service":"example"}"#);
        let effects = e.take_effects();
        assert!(matches!(
            &effects[..],
            [Effect::Reply { id: 7, rc: 10, .. }]
        ));
        assert_eq!(calls(&effects), 0);
        e.command(
            8,
            "workshop.call",
            r#"{"service":"example","command":"nope"}"#,
        );
        assert!(
            matches!(&e.take_effects()[..], [Effect::Reply { id: 8, rc: 10, body }] if body["error_code"] == "ARGUMENT")
        );
    }

    #[test]
    fn refusals_name_their_reason() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.call_selected();
        e.take_effects();
        e.command(1, "workshop.refresh", "{}");
        assert!(
            matches!(&e.take_effects()[..], [Effect::Reply { body, .. }] if body["error_code"] == "BUSY")
        );
        e.delivery(Delivery::Disconnected);
        e.command(2, "workshop.call", "{}");
        assert!(
            matches!(&e.take_effects()[..], [Effect::Reply { body, .. }] if body["error_code"] == "DISCONNECTED")
        );
        e.command(3, "nope", "{}");
        assert!(
            matches!(&e.take_effects()[..], [Effect::Reply { body, .. }] if body["error_code"] == "UNKNOWN_COMMAND")
        );
        e.command(4, "workshop.info", "[]");
        assert!(
            matches!(&e.take_effects()[..], [Effect::Reply { body, .. }] if body["error_code"] == "ARGUMENT")
        );
    }

    #[test]
    fn quit_waits_for_accepted_work() {
        let mut e = engine();
        e.ui.selected = Some(target());
        e.call_selected();
        let Some(Effect::Call { ticket, .. }) = e.take_effects().pop() else {
            panic!()
        };
        e.quit();
        assert!(!e.take_effects().contains(&Effect::Exit));
        assert_eq!(e.status, "quitting");
        e.completed(
            ticket,
            Ok(Reply {
                rc: 0,
                body: "{}".into(),
                error: None,
            }),
        );
        assert!(e.take_effects().contains(&Effect::Exit));
    }

    #[test]
    fn a_lost_reply_is_uncertain_and_never_retried() {
        let mut e = engine();
        e.command(
            5,
            "workshop.call",
            r#"{"service":"example","command":"echo","body":"{}"}"#,
        );
        let Some(Effect::Call { ticket, .. }) = e.take_effects().pop() else {
            panic!()
        };
        e.completed(ticket, Err(CallError::from("lost response")));
        let effects = e.take_effects();
        assert!(
            matches!(&effects[..], [Effect::Reply { id: 5, rc: 10, body }]
            if body["outcome_unknown"] == true && body["retried"] == false)
        );
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
        e.command(1, "workshop.actions", "{}");
        e.command(2, "workshop.execute", r#"{"id":"file.refresh"}"#);
        e.command(3, "workshop.execute", "{}");
        let effects = e.take_effects();
        assert_eq!(effects[0], Effect::Commands { id: 1 });
        assert_eq!(
            effects[1],
            Effect::Execute {
                id: 2,
                command: "file.refresh".into()
            }
        );
        assert!(matches!(&effects[2], Effect::Reply { id: 3, rc: 10, .. }));
    }

    #[test]
    fn the_filter_opens_matches_and_hides_the_rest() {
        let mut e = engine();
        e.set_filter("echo".into());
        let tree = e.tree();
        assert!(find(&tree, "service:example").is_some_and(|row| row.expanded));
        assert!(find(&tree, "service:broken").is_none());
    }

    /// Send `command` with `body` and return the one reply.
    fn ask(e: &mut Engine, command_name: &str, body: Value) -> (u8, Value) {
        e.command(9, command_name, &body.to_string());
        match &e.take_effects()[..] {
            [Effect::Reply { id: 9, rc, body }] => (*rc, body.clone()),
            other => panic!("{command_name}: {other:?}"),
        }
    }

    fn code(reply: (u8, Value)) -> String {
        assert_eq!(reply.0, 10, "{}", reply.1);
        reply.1["error_code"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    #[test]
    fn every_command_is_answered_and_the_rest_are_unknown() {
        for command_name in model::COMMANDS {
            let mut e = engine();
            e.command(1, command_name, "{}");
            let effects = e.take_effects();
            assert!(
                !effects.iter().any(|x| matches!(x, Effect::Reply { body, .. } if body["error_code"] == "UNKNOWN_COMMAND")),
                "{command_name}: {effects:?}"
            );
        }
        let mut e = engine();
        assert_eq!(
            code(ask(&mut e, "workshop.nope", json!({}))),
            "UNKNOWN_COMMAND"
        );
        e.command(2, "HELP", "{}");
        e.command(3, "app.describe", "{}");
        assert_eq!(
            e.take_effects(),
            [
                Effect::Describe { id: 2, help: true },
                Effect::Describe { id: 3, help: false }
            ]
        );
    }

    #[test]
    fn window_commands_go_to_the_drive_layer() {
        let mut e = engine();
        e.command(1, "workshop.ui.click", r#"{"label":"File"}"#);
        e.command(2, "workshop.window", r#"{"action":"maximize"}"#);
        e.command(3, "workshop.window.state", "{}");
        let effects = e.take_effects();
        assert_eq!(
            effects[0],
            Effect::Drive {
                id: 1,
                command: "ui.click".into(),
                args: json!({"label":"File"})
            }
        );
        assert!(
            matches!(&effects[1], Effect::Drive { id: 2, command: command_name, .. } if command_name == "window")
        );
        assert!(
            matches!(&effects[2], Effect::Drive { id: 3, command: command_name, .. } if command_name == "window.state")
        );
    }

    #[test]
    fn filter_sets_the_filter_and_returns_the_rows() {
        let mut e = engine();
        let (rc, body) = ask(&mut e, "workshop.filter", json!({"text":"echo"}));
        assert_eq!(rc, 0);
        assert_eq!(e.ui.filter, "echo");
        let keys: Vec<_> = body["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["key"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            keys,
            [
                "nodes",
                "nodes:mesh",
                "service:example",
                "command:example:echo",
                "mesh",
                "mesh:empty"
            ]
        );
        assert_eq!(
            code(ask(&mut e, "workshop.filter", json!({"text":5}))),
            "ARGUMENT"
        );
        e.open(Dialog::About);
        assert_eq!(
            code(ask(&mut e, "workshop.filter", json!({"text":"x"}))),
            "BUSY"
        );
        assert_eq!(e.ui.filter, "echo");
    }

    #[test]
    fn body_respects_the_limit_and_the_call_holding_it() {
        let mut e = engine();
        let (rc, body) = ask(&mut e, "workshop.body", json!({"text":"{\"a\":1}"}));
        assert_eq!((rc, body["body"].as_str()), (0, Some("{\"a\":1}")));
        assert_eq!(
            code(ask(
                &mut e,
                "workshop.body",
                json!({"text":"x".repeat(model::BODY_LIMIT + 1)})
            )),
            "ARGUMENT"
        );
        assert_eq!(code(ask(&mut e, "workshop.body", json!({}))), "ARGUMENT");
        e.ui.selected = Some(target());
        e.call_selected();
        e.take_effects();
        assert_eq!(
            code(ask(&mut e, "workshop.body", json!({"text":"{}"}))),
            "BUSY"
        );
        assert_eq!(e.ui.body, "{\"a\":1}", "frozen while calling");
    }

    #[test]
    fn split_is_bounded() {
        let mut e = engine();
        let (rc, body) = ask(&mut e, "workshop.split", json!({"value":0.5}));
        assert_eq!((rc, body["split"].as_f64()), (0, Some(0.5)));
        assert_eq!(
            ask(&mut e, "workshop.split", json!({"value":0.9})).1["split"].as_f64(),
            Some(rounded(*SERVICES_SPLIT.end()))
        );
        assert_eq!(
            ask(&mut e, "workshop.split", json!({"value":0.4})).1["split"].as_f64(),
            Some(0.4),
            "rounded, not 0.4000000059604645"
        );
        assert!(e.split_requested, "the shell is told to resize the panel");
        assert_eq!(
            code(ask(&mut e, "workshop.split", json!({"value":1.5}))),
            "ARGUMENT"
        );
        assert_eq!(
            code(ask(&mut e, "workshop.split", json!({"value":"half"}))),
            "ARGUMENT"
        );
    }

    #[test]
    fn tree_lists_the_visible_rows() {
        let mut e = engine();
        let rows = ask(&mut e, "workshop.tree", json!({})).1["rows"].clone();
        let keys: Vec<_> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["key"].as_str().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "nodes",
                "nodes:mesh",
                "service:broken",
                "service:example",
                "mesh"
            ],
            "Nodes starts open; service branches start closed"
        );
        assert_eq!(rows[3]["kind"], "service");
        assert_eq!(rows[3]["children"], 1);
        assert_eq!(rows[4]["label"], "peers");
        assert_eq!(rows[0]["depth"], 0);
    }

    #[test]
    fn expand_opens_and_closes_rows_with_children() {
        let mut e = engine();
        let (rc, body) = ask(
            &mut e,
            "workshop.expand",
            json!({"key":"service:example","open":true}),
        );
        assert_eq!((rc, body["expanded"].as_bool()), (0, Some(true)));
        let command_name = body["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["key"] == "command:example:echo")
            .cloned()
            .unwrap();
        assert_eq!(
            (
                command_name["kind"].as_str(),
                command_name["depth"].as_u64()
            ),
            (Some("command"), Some(2))
        );
        assert_eq!(
            command_name["selection"],
            json!({"service":"example","command":"echo"})
        );
        ask(
            &mut e,
            "workshop.expand",
            json!({"key":"service:example","open":true}),
        );
        assert!(
            e.ui.expanded.contains("service:example"),
            "opening twice leaves it open"
        );
        let (_, body) = ask(
            &mut e,
            "workshop.expand",
            json!({"key":"service:example","open":false}),
        );
        assert_eq!(body["expanded"], false);
        assert_eq!(
            code(ask(
                &mut e,
                "workshop.expand",
                json!({"key":"command:example:echo","open":true})
            )),
            "ARGUMENT"
        );
        assert_eq!(
            code(ask(
                &mut e,
                "workshop.expand",
                json!({"key":"nope","open":true})
            )),
            "ARGUMENT"
        );
        assert_eq!(
            code(ask(&mut e, "workshop.expand", json!({"key":"mesh"}))),
            "ARGUMENT"
        );
    }

    #[test]
    fn select_row_takes_any_row_kind() {
        let mut e = engine();
        let (rc, body) = ask(
            &mut e,
            "workshop.select_row",
            json!({"key":"command:example:echo"}),
        );
        assert_eq!(rc, 0);
        assert_eq!(
            body["selection"],
            json!({"service":"example","command":"echo"})
        );
        assert_eq!(e.ui.selected, Some(target()));
        let (_, body) = ask(&mut e, "workshop.select_row", json!({"key":"error:broken"}));
        assert_eq!(
            (body["row_key"].as_str(), &body["selection"]),
            (Some("error:broken"), &Value::Null)
        );
        assert_eq!(e.ui.selected, None, "a non-command row clears the command");
        assert_eq!(
            code(ask(&mut e, "workshop.select_row", json!({"key":"nope"}))),
            "ARGUMENT"
        );
        e.open(Dialog::Shortcuts);
        assert_eq!(
            code(ask(&mut e, "workshop.select_row", json!({"key":"mesh"}))),
            "BUSY"
        );
    }

    #[test]
    fn dialog_opens_and_closes() {
        let mut e = engine();
        let (rc, body) = ask(&mut e, "workshop.dialog", json!({"open":"about"}));
        assert_eq!((rc, body["dialog"].as_str()), (0, Some("about")));
        assert_eq!(e.ui.dialog, Some(Dialog::About));
        assert_eq!(
            code(ask(&mut e, "workshop.dialog", json!({"open":"shortcuts"}))),
            "BUSY",
            "one dialog at a time"
        );
        let (rc, body) = ask(&mut e, "workshop.dialog", json!({"open":null}));
        assert_eq!((rc, &body["dialog"]), (0, &Value::Null));
        assert_eq!(
            e.ui.dialog, None,
            "a close is never refused as an edit under the dialog"
        );
        let (rc, body) = ask(&mut e, "workshop.dialog", json!({"open":null}));
        assert_eq!(
            (rc, &body["dialog"], e.ui.dialog),
            (0, &Value::Null, None),
            "closing nothing is still closed"
        );
        assert_eq!(
            code(ask(&mut e, "workshop.dialog", json!({"open":"settings"}))),
            "ARGUMENT"
        );
        assert_eq!(code(ask(&mut e, "workshop.dialog", json!({}))), "ARGUMENT");
        e.quit();
        e.take_effects();
        assert_eq!(
            code(ask(&mut e, "workshop.dialog", json!({"open":"about"}))),
            "BUSY"
        );
    }

    #[test]
    fn reply_reads_the_last_reply() {
        let mut e = engine();
        assert_eq!(
            ask(&mut e, "workshop.reply", json!({})).1,
            json!({"text":"","value":null})
        );
        e.ui.selected = Some(target());
        e.call_selected();
        let Some(Effect::Call { ticket, .. }) = e.take_effects().pop() else {
            panic!()
        };
        e.completed(
            ticket,
            Ok(Reply {
                rc: 0,
                body: "{\"ok\":true}".into(),
                error: None,
            }),
        );
        let (rc, body) = ask(&mut e, "workshop.reply", json!({}));
        assert_eq!(rc, 0);
        assert!(body["text"].as_str().unwrap().contains("rc = 0"));
        assert_eq!(body["value"]["rc"], 0);
    }

    #[test]
    fn ui_state_round_trips() {
        let mut e = engine();
        e.ui.dock_side = DockSide::Right;
        e.ui.dock_width = 0.4;
        e.ui.dock_height = 0.2;
        e.ui.selected = Some(target());
        e.ui.filter = "ex".into();
        e.toggle("mesh");
        e.set_theme(Some(Scheme::Forest), Some(Mode::Dark));
        let saved = serde_json::to_string(&e.ui).unwrap();
        assert!(
            saved.contains("\"theme_scheme\":\"forest\"")
                && saved.contains("\"theme_mode\":\"dark\""),
            "by name: {saved}"
        );
        let back: UiState = serde_json::from_str(&saved).unwrap();
        assert_eq!(back, e.ui);
        assert_eq!(
            (back.theme_scheme, back.theme_mode),
            (Some(Scheme::Forest), Some(Mode::Dark))
        );
    }

    #[test]
    fn old_ui_states_default_to_auto_dock() {
        let mut old = serde_json::to_value(UiState::default()).unwrap();
        for key in ["dock_side", "dock_width", "dock_height"] {
            old.as_object_mut().unwrap().remove(key);
        }
        let loaded: UiState = serde_json::from_value(old).unwrap();
        assert_eq!(loaded.dock_side, DockSide::Auto);
        assert_eq!((loaded.dock_width, loaded.dock_height), (0.3, 0.28));
    }

    #[test]
    fn auto_dock_changes_at_the_wide_window_threshold() {
        assert_eq!(
            DockSide::Auto.effective(DOCK_RIGHT_WIDTH - 1.0),
            DockSide::Bottom
        );
        assert_eq!(DockSide::Auto.effective(DOCK_RIGHT_WIDTH), DockSide::Right);
        assert_eq!(DockSide::Bottom.effective(1920.0), DockSide::Bottom);
        assert_eq!(DockSide::Right.effective(900.0), DockSide::Right);
    }

    #[test]
    fn auto_dock_size_sets_both_axes_before_and_after_layout() {
        let mut e = engine();
        ask(&mut e, "workshop.dock", json!({"side":"auto","size":0.42}));
        assert_eq!((e.ui.dock_width, e.ui.dock_height), (0.42, 0.42));
        e.dock_window_width = 1920.0;
        ask(&mut e, "workshop.dock", json!({"side":"auto","size":0.35}));
        assert_eq!((e.ui.dock_width, e.ui.dock_height), (0.35, 0.35));
    }

    #[test]
    fn services_split_uses_the_same_limits_as_the_panel() {
        let mut e = engine();
        for share in [*SERVICES_SPLIT.start(), 0.15, 0.7, *SERVICES_SPLIT.end()] {
            e.set_split(share);
            assert_eq!(e.ui.split, share);
        }
    }

    #[test]
    fn dock_sets_clamps_and_refuses_invalid_arguments_atomically() {
        let mut e = engine();
        e.dock_window_width = 1920.0;
        let (rc, body) = ask(
            &mut e,
            "workshop.dock",
            json!({"side":"right","size":0.4,"open":false}),
        );
        assert_eq!(rc, 0);
        assert_eq!(body["width"], 0.4);
        assert!(!e.workshop.dock_open);
        assert!(e.dock_requested);
        assert_eq!(
            ask(&mut e, "workshop.dock", json!({"side":"right","size":0.9})).1["width"],
            0.5
        );
        assert_eq!(
            ask(
                &mut e,
                "workshop.dock",
                json!({"side":"bottom","size":0.0,"open":true})
            )
            .1["height"],
            0.12
        );
        assert!(e.workshop.dock_open);
        ask(&mut e, "workshop.dock", json!({"side":"auto","size":0.35}));
        assert_eq!(e.dock_effective, DockSide::Right);
        assert_eq!(e.info()["ui"]["dock_size"], 0.35);
        for args in [
            json!({}),
            json!({"side":"left"}),
            json!({"side":{"right":null}}),
            json!({"side":"right","size":1.1}),
            json!({"side":"right","size":"half"}),
            json!({"side":"right","open":0}),
        ] {
            let before = e.ui.clone();
            assert_eq!(code(ask(&mut e, "workshop.dock", args)), "ARGUMENT");
            assert_eq!(e.ui, before);
        }
    }

    #[test]
    fn the_theme_command_sets_present_axes_and_answers_the_effective_theme() {
        let mut e = engine();
        e.session = Session {
            scheme: Scheme::Pro,
            mode: Mode::Light,
            ..Session::default()
        };
        let (rc, body) = ask(&mut e, "workshop.theme", json!({"scheme":"forest"}));
        assert_eq!(rc, 0);
        assert_eq!(
            body,
            json!({"scheme":"forest","style":null,"mode":null,"decorations":null,"caption_side":null,
                "effective":{"scheme":"forest","style":"own","mode":"light","decorations":"csd","decorations_reason":"rule4","caption_side":"right"}})
        );
        let (_, body) = ask(&mut e, "workshop.theme", json!({"mode":"dark"}));
        assert_eq!(body["scheme"], "forest", "an absent key leaves its axis");
        assert_eq!(
            (&body["effective"]["scheme"], &body["effective"]["mode"]),
            (&json!("forest"), &json!("dark"))
        );
        let (_, body) = ask(&mut e, "workshop.theme", json!({"scheme":null}));
        assert_eq!(
            (&body["scheme"], &body["effective"]["scheme"]),
            (&Value::Null, &json!("pro")),
            "null follows the session"
        );
        assert_eq!(
            code(ask(&mut e, "workshop.theme", json!({"scheme":"neon"}))),
            "ARGUMENT"
        );
        assert_eq!(
            code(ask(
                &mut e,
                "workshop.theme",
                json!({"mode":"dark","scheme":"neon"})
            )),
            "ARGUMENT"
        );
        assert_eq!(
            e.ui.theme_mode,
            Some(Mode::Dark),
            "a refused call changes neither axis"
        );
        assert_eq!(
            code(ask(&mut e, "workshop.theme", json!({"mode":3}))),
            "ARGUMENT"
        );
        let (_, body) = ask(&mut e, "workshop.theme", json!({}));
        assert_eq!(body["mode"], "dark", "nothing given: a read");
    }

    #[test]
    fn the_light_dark_switch_flips_the_mode_shown_and_keeps_the_scheme() {
        let mut e = engine();
        e.session = Session {
            scheme: Scheme::Pro,
            mode: Mode::Light,
            ..Session::default()
        };
        e.set_theme(Some(Scheme::Studio), None);
        e.toggle_mode();
        assert_eq!(
            (e.ui.theme_scheme, e.ui.theme_mode),
            (Some(Scheme::Studio), Some(Mode::Dark))
        );
        e.toggle_mode();
        assert_eq!(e.effective_theme(), (Scheme::Studio, Mode::Light));
    }

    #[test]
    fn the_theme_command_sets_style_decorations_and_caption_side() {
        let mut e = engine();
        let (rc, body) = ask(
            &mut e,
            "workshop.theme",
            json!({"style":"classic","decorations":"ssd","caption_side":"left"}),
        );
        assert_eq!(rc, 0, "{body}");
        assert_eq!(
            (&body["style"], &body["decorations"], &body["caption_side"]),
            (&json!("classic"), &json!("ssd"), &json!("left"))
        );
        assert_eq!(e.ui.theme_style, Some(Some(Style::Classic)));
        let (_, body) = ask(&mut e, "workshop.theme", json!({"style":"own"}));
        assert_eq!(
            (&body["style"], &body["effective"]["style"]),
            (&json!("own"), &json!("own"))
        );
        assert_eq!(e.ui.theme_style, Some(None), "the scheme's own, chosen");
        e.session.style = Some(Style::Pro);
        let (_, body) = ask(
            &mut e,
            "workshop.theme",
            json!({"style":null,"decorations":null}),
        );
        assert_eq!(
            (
                &body["style"],
                &body["effective"]["style"],
                &body["effective"]["decorations"]
            ),
            (&Value::Null, &json!("pro"), &json!("csd")),
            "null follows the session"
        );
        for bad in [
            json!({"style":"neon"}),
            json!({"decorations":"both"}),
            json!({"caption_side":"top"}),
            json!({"style":"pro","caption_side":4}),
        ] {
            assert_eq!(
                code(ask(&mut e, "workshop.theme", bad.clone())),
                "ARGUMENT",
                "{bad}"
            );
        }
        assert_eq!(e.ui.theme_style, None, "a refused call changes no axis");
        let saved = serde_json::to_string(&e.ui).unwrap();
        let back: UiState = serde_json::from_str(&saved).unwrap();
        assert_eq!(back, e.ui, "the axes save by name: {saved}");
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
        fields.remove("theme_style");
        fields.remove("theme_decorations");
        fields.remove("theme_captions");
        // State saved by the build before this one, with its old toggle.
        fields.insert("invert_mode".into(), json!(true));
        let loaded: UiState = serde_json::from_value(old).unwrap();
        assert_eq!(
            (loaded.theme_scheme, loaded.theme_mode, loaded.theme_style),
            (None, None, None),
            "older saved state loads, following the session"
        );
        assert_eq!(
            (loaded.theme_decorations, loaded.theme_captions),
            (None, None)
        );
        e.open(Dialog::About);
        e.filter_focused();
        e.focus_filter();
        assert!(!e.ui.focus_filter, "not under a dialog");
    }
}
