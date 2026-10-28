// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Controller: tabs, the global `event_seq`, the op-id generator, `ced.*`
//! commands, `ced.wait` waiters, reattach. UI-toolkit-free — the GUI (`app.rs`)
//! and `--headless` (`headless.rs`) drive the same Controller.
//!
//! Contracts:
//! - Every `edit.changed` event is routed to its buffer's mirror; the global
//!   `last_event_seq` (initialised from the first event after (re)subscribe)
//!   detects gaps → every Live mirror `suspect()`s; so do `resync all` and the
//!   Bus reconnect edge. An epoch change (event/reply epoch, `epoch_mismatch`,
//!   `edit` reappearing in noded `services.registered`) → reattach every tab.
//! - Bus-driven actions carry `Intent::bus(tab, caller_key)`; window input
//!   carries `Intent::ui(tab)`; the intent travels through every async
//!   completion (paste, dialogs, find/replace).
//! - `ced.wait`: evaluated at registration and after EVERY
//!   controller transition; a one-shot deadline timer; cancelled on tab close
//!   / Bus loss. Never a loop.

use std::collections::HashMap;
use std::ops::Range;
use std::time::Instant;

use edit::anchor::Selection;
use edit::pos::{NamedPos, PosSpec};
use edit::text::Text;
use edit::view::MeasureCfg;
use edit::wire;
use editor_model::diag::{Diagnostics, Severity};
use editor_model::highlight::Highlight;
use editor_model::mirror::{DetachReason, LaneArg, Mirror, Outcome, Phase, ServerOp, Step};
use editor_model::model::{EditCfg, EditCommand, EditorModel};
use editor_model::types::{
    DeltaKind, Incoming, Intent, Invoker, Level, Notice, OpIdGen, Outgoing, TabId, UI_ORIGIN,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::actions::ActionId;
use crate::commands::{self, PhaseW, code};
use crate::config::Config;
use crate::editor::{EditorMsg, LayoutReport};
use crate::session::{RECENT_MAX, Session, SessionTab};

mod external;
mod mail;
mod ui;

pub use ui::{MatchQuery, Prompt, RecoveredRow};

/// One tab = one buffer view.
pub struct Tab {
    pub id: TabId,
    /// `None` until `edit.open` answers (or after an open failure).
    pub mirror: Option<Mirror>,
    pub editor: EditorModel,
    pub highlight: Highlight,
    pub diagnostics: Diagnostics,
    /// Path to reopen, updated from the service after Open or Save As.
    pub path: Option<String>,
    /// Detected once after Save As, until fresh metadata arrives.
    saved_language: Option<&'static str>,
    pub mail: Option<crate::mail::MailState>,
    pub document_notice: Option<String>,
    classified: bool,
}

impl Tab {
    /// The saved detection during Save As, then the mirror's fresh metadata.
    pub fn language(&self) -> &str {
        if self.mail.is_some() {
            return "markdown";
        }
        let Some(m) = &self.mirror else {
            return "text";
        };
        self.saved_language.unwrap_or(&m.meta().language)
    }
}

/// A `ced.*` Bus request, as the bus thread hands it over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusCommand {
    /// Correlation for the reply (the bus thread answers `respond(id, …)`).
    pub id: u64,
    pub command: String,
    /// JSON body.
    pub body: String,
    /// Attested caller key: `local:<from>`, `mesh:<service>@<peer>`, `anon`.
    pub caller_key: String,
}

/// What the Controller asks its host (GUI or headless loop) to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Post a window message on the Bus lane, falling back to the status line.
    Notify { text: String, level: Level },
    SaveMail {
        tab: TabId,
        path: String,
        text: String,
        expected: Option<String>,
        send: bool,
        command: Option<(u64, String)>,
    },
    SendMail {
        tab: TabId,
        path: String,
        command: Option<(u64, String)>,
    },
    /// Send a request; `req` correlates the eventual `Incoming::Reply`.
    Send { req: u64, out: Outgoing },
    /// Answer a `ced.*` Bus command.
    Respond { id: u64, rc: u8, body: String },
    /// `ced.execute` ran action `id` directly (the window runs the rest):
    /// the host records it for `<app>.action.executed`.
    Executed { id: String },
    /// Arm a one-shot timer (`Incoming::Timer{id}` after `ms`).
    Timer { id: u64, ms: u64 },
    /// Subscribe a topic (idempotent).
    Subscribe { topic: String },
    /// Show something in the chrome.
    Notice { tab: Option<TabId>, notice: Notice },
    /// Write the clipboard (`primary`: the selection clipboard).
    ClipboardWrite { text: String, primary: bool },
    /// Read the clipboard; the result comes back as `on_paste` with `intent`.
    ClipboardRead { primary: bool, intent: Intent },
    /// Persist the session (debounced by the host).
    SaveSession,
    /// Detach and exit.
    Quit,
    /// Ask the human (a dialog); the answer comes back as an action or a
    /// [`Controller`] method (see [`Prompt`]).
    Prompt(Prompt),
    /// A window-only action (dialogs, find bar, zoom, panels, help, and the
    /// args-taking actions called without their args): the window performs it
    /// and then calls [`Controller::ui_done`] with `token` and the outcome.
    /// A Bus `ced.execute` that caused it is answered only from `ui_done` (or
    /// with a `TIMEOUT` refusal after 10 s) — never before the window acted.
    /// Never emitted headless (those refuse UNAVAILABLE).
    UiAction {
        tab: Option<TabId>,
        action: ActionId,
        args: Option<Value>,
        intent: Intent,
        token: u64,
    },
    /// Run a Mix relex off the UI thread:
    /// `editor_model::highlight::run_mix(&tag.language, &source)`, then
    /// hand the spans to [`Controller::on_relex`] with the same tag.
    Relex {
        tab: TabId,
        tag: editor_model::highlight::ResultTag,
        source: std::sync::Arc<str>,
    },
}

/// `ced.wait` longest deadline.
const WAIT_MAX_MS: u64 = 30_000;
/// `ced.state` refuses to inline more text than this.
const STATE_TEXT_MAX: usize = 4 * 1024 * 1024;
/// Selection publish: 200 ms after the last caret move, at most one per second.
const PUBLISH_DEBOUNCE_MS: u64 = 200;
const PUBLISH_MIN_GAP_MS: u64 = 1_000;
/// Session writes: 1 s after the last change.
const SESSION_DEBOUNCE_MS: u64 = 1_000;
/// Deadlines for the controller's own requests.
const DEADLINE_MS: u64 = 5_000;
const DEADLINE_LONG_MS: u64 = 30_000;
const OPEN_WARN_AFTER: u32 = 3;
/// A Bus `ced.execute` waits this long for its server op's outcome (a save's
/// own deadline, its `edit.list` check and one resend fit inside).
const OP_WAIT_MS: u64 = 90_000;
/// Shown when `edit` is absent.
pub const EDIT_UNAVAILABLE: &str =
    "The edit service is not running; this document will open when it starts.";

/// A Bus `ced.execute` waiting on its server op's outcome.
struct OpWait {
    /// The Bus command to answer, and the action id it named.
    id: u64,
    action: String,
    tab: TabId,
}

/// Controller-private per-tab state.
#[derive(Default)]
struct TabX {
    /// Opaque session-only view hints supplied by the window.
    view_hints: HashMap<&'static str, &'static str>,
    recovery_id: Option<String>,
    /// The spelling used to open the file, retained for mail saves.
    opened_path: Option<String>,
    /// `edit.open` answered (success or failure) at least once.
    opened: bool,
    /// Waiting for a reattach open/list.
    reattaching: bool,
    open_error: Option<String>,
    /// Consecutive open deadlines, including initial opens.
    open_misses: u32,
    /// The caret/selection last published with `edit.select`.
    published: Option<Selection>,
    last_publish: Option<Instant>,
    publish_timer: bool,
    /// Another origin edited since the tab was last focused.
    agent_since_focus: bool,
    layout: Option<LayoutReport>,
    /// Pending `line[:col]` from `path:line:col` or `ced.open {line, col}`.
    goto: Option<(usize, usize)>,
    /// `keep_as_new`: text to insert (by this intent) once the buffer is live.
    fill: Option<(String, Intent)>,
    /// `file.close {save:true}`: close once the save has landed.
    close_after_save: Option<Intent>,
    /// Who asked for the last save (a `disk_modified` refusal prompts them).
    save_intent: Option<Intent>,
    /// A find / replace waiting for the pipeline to go idle.
    find: Option<ui::FindJob>,
    /// An outstanding lint capture and the deltas since.
    lint: Option<ui::LintCapture>,
    relex_timer: bool,
    /// Highlight-all: the query, its matches (view ranges), refresh state.
    match_query: Option<ui::MatchQuery>,
    matches: Vec<Range<usize>>,
    rematch_due: bool,
    rematch_timer: bool,
    /// Stored external sets are due for re-application (a Resync cleared
    /// them), and the live / dirty state they were last checked against.
    ext_due: bool,
    ext_live: bool,
    ext_dirty: bool,
    mail_save: Option<mail::SaveIntent>,
}

/// What a request the controller sent is for.
enum Req {
    Mirror {
        tab: TabId,
        op_id: String,
    },
    Open {
        tab: TabId,
        reattach: bool,
    },
    /// Scratch-tab reattach: find the buffer by `recovery_id`.
    List {
        tab: TabId,
    },
    Info,
    Close {
        tab: TabId,
        cmd: Option<(u64, String)>,
        intent: Intent,
    },
    Select,
    Find {
        tab: TabId,
        job: Box<ui::FindJob>,
    },
    Matches {
        tab: TabId,
        query: ui::MatchQuery,
    },
    /// `edit.list` at start for the recovered-buffers prompt.
    Recovered,
    /// `edit.list` for the tab's save state.
    Saved {
        tab: TabId,
    },
    Discard,
}

enum TimerFor {
    Retry(TabId),
    Reattach(TabId),
    Wait(u64),
    Publish(TabId),
    Session,
    /// Mix relex debounce (150 ms after the last delta).
    Relex(TabId),
    /// Highlight-all refresh debounce (100 ms after the last delta).
    Rematch(TabId),
    /// Change markers clear 2 s after the tab gains focus.
    ClearMarkers(TabId),
    /// A Bus `ced.execute` waiting on the window's `ui_done`.
    UiDeadline(u64),
    /// An `rc 0` reply whose effect has not arrived (the mirror's echo wait).
    Echo(TabId, String),
    /// A Bus `ced.execute` waiting on its server op's outcome (by op id).
    OpWait(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WaitCond {
    Rev(u64),
    Idle,
    Epoch(String),
    Phase(PhaseW),
}

struct Waiter {
    id: u64,
    cmd: u64,
    tab: TabId,
    cond: WaitCond,
    since: Instant,
    timeout_ms: u64,
}

/// A `ced.open` / `ced.new` answered once every tab it opened has an answer.
struct PendingOpen {
    cmd: u64,
    tabs: Vec<TabId>,
    new: bool,
}

#[derive(Default)]
struct Stats {
    keys: u64,
    events: u64,
    history_recoveries: u64,
    snapshot_recoveries: u64,
    conflicts: u64,
    retries: u64,
    uncertain: u64,
}

pub struct Controller {
    config: Config,
    headless: bool,
    ids: OpIdGen,
    tabs: Vec<Tab>,
    x: HashMap<TabId, TabX>,
    active: Option<TabId>,
    next_tab: TabId,
    next_req: u64,
    next_timer: u64,
    next_waiter: u64,
    reqs: HashMap<u64, Req>,
    timers: HashMap<u64, TimerFor>,
    last_event_seq: Option<u64>,
    edit_epoch: Option<String>,
    edit_present: Option<bool>,
    edit_info: commands::EditInfo,
    waiters: Vec<Waiter>,
    opens: Vec<PendingOpen>,
    recent: Vec<String>,
    /// Tabs to reattach at `start` (from session.json).
    restore: Option<Session>,
    session_timer: bool,
    config_path: Option<String>,
    session_path: Option<String>,
    stats: Stats,
    /// `set_layout`: the last frame's geometry (`ced.layout`).
    layout: Option<commands::LayoutReply>,
    frames: ui::Frames,
    /// Recovered buffers offered at start, and their epoch.
    recovered: Vec<wire::BufferSummary>,
    recovered_epoch: String,
    /// `UiAction` tokens: the next one, and the Bus commands awaiting `ui_done`.
    next_ui_token: u64,
    ui_pending: HashMap<u64, (u64, String)>,
    /// Bus `ced.execute`s answered from their server op's outcome, by op id,
    /// and the one being dispatched right now.
    op_waits: HashMap<String, OpWait>,
    op_cmd: Option<(u64, String)>,
    /// `ced.diagnostics` sets per `(path, source)`.
    external: external::Store,
    /// The keys in force per action id, when the window has applied the
    /// person's bindings; `ced.actions` reports the defaults otherwise.
    keys: Option<HashMap<String, Vec<String>>>,
    /// The window's current command enablement, for `ced.actions`.
    action_enabled: HashMap<ActionId, bool>,
}

fn refusal(code: &str, message: impl Into<String>, reason: Option<&str>) -> String {
    serde_json::to_string(&commands::Refusal {
        error_code: code.to_string(),
        message: message.into(),
        reason: reason.map(str::to_string),
    })
    .unwrap_or_default()
}

fn ok_body<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "{}".into())
}

/// `failed`: the tab's open was refused (no mirror) — detached.
fn phase_w(m: Option<&Mirror>, failed: bool) -> PhaseW {
    match m.map(Mirror::phase) {
        Some(Phase::Live) => PhaseW::Live,
        Some(Phase::Bootstrapping { .. }) => PhaseW::Bootstrapping,
        Some(Phase::Recovering { .. }) => PhaseW::Recovering,
        Some(Phase::Detached { .. }) => PhaseW::Detached,
        None if failed => PhaseW::Detached,
        None => PhaseW::Bootstrapping,
    }
}

fn text_string(t: &Text) -> String {
    let mut s = String::with_capacity(t.len());
    t.read(0..t.len(), &mut s);
    s
}

fn disk_str(d: wire::DiskState) -> String {
    serde_json::to_value(d)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn kind_str(k: wire::KindW) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A Bus reply body as the mirror wants it: rc 0 → the JSON body; a
/// refusal → the `{error_code, message, reason, …}` body.
fn reply_result(rc: u8, body: &str) -> Result<Value, wire::Refusal> {
    if rc < 10 {
        return Ok(if body.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(body).unwrap_or(Value::Null)
        });
    }
    let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let mut refusal =
        serde_json::from_str::<wire::Refusal>(body).unwrap_or_else(|_| wire::Refusal {
            error_code: wire::ErrorCode::Internal,
            message: value["message"].as_str().unwrap_or(body).to_string(),
            reason: None,
            buffer: None,
            rev: None,
            context: Default::default(),
        });
    if refusal.message.trim().is_empty() {
        let code = value["error_code"].as_str().unwrap_or("INTERNAL");
        refusal.message = format!("The edit request failed ({code}).");
    }
    Err(refusal)
}

fn unparsed_refusal(v: &Value) -> wire::Refusal {
    wire::Refusal {
        error_code: wire::ErrorCode::Internal,
        message: v.to_string(),
        reason: None,
        buffer: None,
        rev: None,
        context: Default::default(),
    }
}

/// An `edit.open`-shaped reply for a buffer known from `edit.list` (scratch
/// reattach, recovered buffers): the list row carries everything the mirror
/// needs except eol/bom, which a snapshot does not depend on.
fn open_reply_of(epoch: &str, b: &wire::BufferSummary) -> wire::OpenReply {
    wire::OpenReply {
        buffer: b.buffer.clone(),
        epoch: epoch.to_string(),
        path: b.path.clone(),
        opened_as: b.opened_as.clone(),
        name: b.name.clone(),
        language: b.language.clone(),
        rev: b.rev,
        lines: b.lines,
        bytes: b.bytes,
        eol: wire::Eol::Lf,
        bom: false,
        disk: b.disk,
        reopened: true,
        created: false,
        recovery_id: b.recovery_id.clone(),
        recovered: b.recovered,
        recovered_from: None,
    }
}

/// Resolve an editd POS on the view text (byte offset, `{line, col}` with
/// editd's col = scalars incl. `\r`, 1-based, or `start`/`end`).
fn resolve_pos(text: &Text, p: &PosSpec) -> Result<usize, String> {
    match p {
        PosSpec::Offset(o) => {
            if *o > text.len() || !text.is_char_boundary(*o) {
                return Err(format!(
                    "offset {o} is outside the text or not on a char boundary"
                ));
            }
            Ok(*o)
        }
        PosSpec::Named(NamedPos::Start) => Ok(0),
        PosSpec::Named(NamedPos::End) => Ok(text.len()),
        PosSpec::LineCol { line, col } => offset_of_line_col(text, *line, col.unwrap_or(1)),
        PosSpec::Anchor { .. } => Err("anchors are not supported by ced.select".into()),
    }
}

fn offset_of_line_col(text: &Text, line: usize, col: usize) -> Result<usize, String> {
    let r = text
        .line_range(line)
        .ok_or_else(|| format!("line {line} is outside 1..={}", text.line_count()))?;
    if col == 0 {
        return Err("col is 1-based".into());
    }
    let mut s = String::new();
    text.read(r.clone(), &mut s);
    let mut chars = s
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()));
    chars
        .nth(col - 1)
        .map(|i| r.start + i)
        .ok_or_else(|| format!("col {col} is past the end of line {line}"))
}

/// `path:line[:col]` → (path, line, col) when the suffix parses.
fn split_line_col(p: &str) -> (String, Option<(usize, usize)>) {
    let mut parts = p.rsplitn(3, ':');
    let last = parts.next();
    let mid = parts.next();
    let head = parts.next();
    match (head, mid, last) {
        (Some(h), Some(l), Some(c)) if !h.is_empty() => {
            if let (Ok(l), Ok(c)) = (l.parse::<usize>(), c.parse::<usize>()) {
                return (h.to_string(), Some((l, c)));
            }
            if let Ok(line) = c.parse::<usize>() {
                return (format!("{h}:{l}"), Some((line, 1)));
            }
            (p.to_string(), None)
        }
        (None, Some(h), Some(l)) if !h.is_empty() => match l.parse::<usize>() {
            Ok(line) => (h.to_string(), Some((line, 1))),
            Err(_) => (p.to_string(), None),
        },
        _ => (p.to_string(), None),
    }
}

impl Controller {
    /// `run_id`: random per process start (op ids); `headless`: no window.
    pub fn new(config: Config, run_id: u32, headless: bool) -> Self {
        Self {
            config,
            headless,
            ids: OpIdGen::new(run_id),
            tabs: Vec::new(),
            x: HashMap::new(),
            active: None,
            next_tab: 1,
            next_req: 0,
            next_timer: 0,
            next_waiter: 0,
            reqs: HashMap::new(),
            timers: HashMap::new(),
            last_event_seq: None,
            edit_epoch: None,
            edit_present: None,
            edit_info: commands::EditInfo {
                epoch: None,
                version: None,
                volatile: None,
            },
            waiters: Vec::new(),
            opens: Vec::new(),
            recent: Vec::new(),
            restore: None,
            session_timer: false,
            config_path: None,
            session_path: None,
            stats: Stats::default(),
            layout: None,
            frames: ui::Frames::default(),
            recovered: Vec::new(),
            recovered_epoch: String::new(),
            next_ui_token: 0,
            ui_pending: HashMap::new(),
            op_waits: HashMap::new(),
            op_cmd: None,
            external: external::Store::default(),
            keys: None,
            action_enabled: HashMap::new(),
        }
    }

    /// The keys in force per action id, for `ced.actions`.
    pub fn set_keys(&mut self, keys: HashMap<String, Vec<String>>) {
        self.keys = Some(keys);
    }

    /// Echo Markdown view enablement; older actions keep their Bus contract.
    pub fn set_action_enabled(&mut self, action: ActionId, enabled: bool) {
        if matches!(
            action,
            ActionId::ViewMarkdownSource
                | ActionId::ViewMarkdownSplit
                | ActionId::ViewMarkdownPreview
        ) {
            self.action_enabled.insert(action, enabled);
        }
    }

    /// Keep a window's opaque per-tab hint until that tab closes.
    pub fn set_view_hint(&mut self, tab: TabId, key: &'static str, value: &'static str) {
        if let Some(x) = self.x.get_mut(&tab)
            && x.view_hints.get(key).copied() != Some(value)
        {
            x.view_hints.insert(key, value);
        }
    }

    fn view_hint(&self, tab: TabId, key: &str) -> Option<&str> {
        self.x.get(&tab)?.view_hints.get(key).copied()
    }

    /// Tabs to reopen at [`Controller::start`] (the host loads session.json).
    pub fn set_session(&mut self, session: Session) {
        self.recent = session.recent.clone();
        self.restore = Some(session);
    }

    /// Replace the configuration (View › Reload Settings): edits from now on
    /// use its tab size, indent widths and spaces-or-tabs rules.
    pub fn set_config(&mut self, config: Config) {
        self.config = config;
    }

    /// Paths reported by `ced.info`.
    pub fn set_paths(&mut self, config_path: Option<String>, session_path: Option<String>) {
        self.config_path = config_path;
        self.session_path = session_path;
    }

    /// The session to persist now.
    pub fn session(&self) -> Session {
        let tabs: Vec<SessionTab> = self
            .tabs
            .iter()
            .map(|t| SessionTab {
                path: t.path.clone(),
                recovery_id: if t.path.is_none() || t.mail.is_some() {
                    self.x.get(&t.id).and_then(|x| x.recovery_id.clone())
                } else {
                    None
                },
                mail: t.mail.as_ref().map(|mail| crate::session::MailSession {
                    disk: mail.disk.clone(),
                    headers: mail.headers.clone(),
                }),
                caret: t.editor.sel.head,
                first_line: t.editor.scroll.first_line.max(1),
            })
            .collect();
        let active = self
            .active
            .and_then(|a| self.tabs.iter().position(|t| t.id == a));
        Session {
            version: crate::session::VERSION,
            active,
            tabs,
            recent: self.recent.clone(),
        }
    }

    /// The Bus came up: subscribe topics, ping `edit`, reattach the session.
    pub fn start(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        for topic in [wire::TOPIC_CHANGED, "theme.changed", "noded.props.changed"] {
            fx.push(Effect::Subscribe {
                topic: topic.to_string(),
            });
        }
        self.send_info(&mut fx);
        let out = Outgoing {
            command: "edit.list".into(),
            body: "{}".into(),
            op_id: None,
            deadline_ms: DEADLINE_MS,
        };
        self.send(out, Req::Recovered, &mut fx);
        if let Some(s) = self.restore.take() {
            let mut ids = Vec::new();
            for st in &s.tabs {
                let id = self.new_tab(st.path.clone());
                if let Some(x) = self.x.get_mut(&id) {
                    x.recovery_id = st.recovery_id.clone();
                }
                if let Some(t) = self.tab_mut(id) {
                    if let Some(saved) = &st.mail
                        && st.recovery_id.is_some()
                        && let Ok(document) = maildoc::parse(&saved.disk)
                    {
                        let mut mail = crate::mail::MailState::new(document, saved.disk.clone());
                        mail.headers = saved.headers.clone();
                        mail.seeding = false;
                        t.mail = Some(mail);
                        t.classified = true;
                    }
                    t.editor.sel = Selection {
                        anchor: st.caret,
                        head: st.caret,
                    };
                    t.editor.scroll.first_line = st.first_line.max(1);
                }
                ids.push(id);
                match (&st.path, &st.recovery_id) {
                    (Some(_), Some(_)) if self.tab(id).is_some_and(|t| t.mail.is_some()) => {
                        self.send_list(id, &mut fx)
                    }
                    (Some(p), _) => self.send_open(id, Some(p.clone()), false, &mut fx),
                    (None, Some(_)) => self.send_list(id, &mut fx),
                    (None, None) => self.send_open(id, None, false, &mut fx),
                }
            }
            self.active = s
                .active
                .and_then(|i| ids.get(i).copied())
                .or(ids.first().copied());
        }
        fx
    }

    /// A transport delivery (reply, topic, timer, deadline, connection edge).
    pub fn on_incoming(&mut self, incoming: Incoming) -> Vec<Effect> {
        let mut fx = Vec::new();
        match incoming {
            Incoming::Reply { req, rc, body } => self.on_reply(req, rc, &body, &mut fx),
            Incoming::Parsed { req, rc, body } => self.on_parsed(req, rc, body.0, &mut fx),
            Incoming::Deadline { req } => self.on_deadline(req, &mut fx),
            Incoming::Timer { id } => self.on_timer(id, &mut fx),
            Incoming::Topic { topic, body } => self.on_topic(&topic, &body, &mut fx),
            Incoming::Connection { up } => {
                if up {
                    // The reconnect edge: loss may have happened before any
                    // event reached us.
                    self.last_event_seq = None;
                    for id in self.tab_ids() {
                        if let Some(step) = self
                            .tab_mut(id)
                            .and_then(|t| t.mirror.as_mut())
                            .map(Mirror::suspect)
                        {
                            self.drive(id, step, &mut fx);
                        }
                    }
                    self.send_info(&mut fx);
                } else {
                    for tab in self.tab_ids() {
                        self.cancel_mail_save(tab, "the parked mail save was cancelled because the Bus connection was lost", &mut fx);
                    }
                    let all: Vec<u64> = self.waiters.iter().map(|w| w.id).collect();
                    for w in all {
                        self.finish_waiter(
                            w,
                            Err(("cancelled", "the Bus connection was lost".to_string())),
                            &mut fx,
                        );
                    }
                }
            }
        }
        self.eval_waiters(&mut fx);
        fx
    }

    /// A `ced.*` / `app.*` Bus request.
    pub fn on_bus_command(&mut self, cmd: BusCommand) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.bus_command(&cmd, &mut fx);
        self.eval_waiters(&mut fx);
        fx
    }

    /// A menu / key / Bus action on `tab` (default: the active tab).
    pub fn on_action(
        &mut self,
        tab: Option<TabId>,
        action: ActionId,
        intent: Intent,
    ) -> Vec<Effect> {
        let mut fx = Vec::new();
        if let Err((_, msg)) = self.action_args(tab, action, None, intent, &mut fx) {
            fx.push(Effect::Notice {
                tab,
                notice: Notice::Message {
                    level: Level::Warn,
                    text: msg,
                },
            });
        }
        self.eval_waiters(&mut fx);
        fx
    }

    /// A message from a tab's editor widget (window input: `Intent::ui`).
    pub fn on_editor(&mut self, tab: TabId, msg: EditorMsg) -> Vec<Effect> {
        let mut fx = Vec::new();
        match msg {
            EditorMsg::Command(c) => {
                self.stats.keys += 1;
                let started = Instant::now();
                let done = self.command(tab, c, Intent::ui(tab), &mut fx);
                self.record_model(started.elapsed().as_micros() as u64);
                if let Err(e) = done {
                    fx.push(Effect::Notice {
                        tab: Some(tab),
                        notice: Notice::Message {
                            level: Level::Warn,
                            text: e,
                        },
                    });
                }
            }
            EditorMsg::ImeCommit(text) => {
                self.stats.keys += 1;
                if let Some(t) = self.tab_mut(tab) {
                    t.editor.set_preedit(false);
                }
                let started = Instant::now();
                let done = self.command(tab, EditCommand::Insert(text), Intent::ui(tab), &mut fx);
                self.record_model(started.elapsed().as_micros() as u64);
                if let Err(e) = done {
                    fx.push(Effect::Notice {
                        tab: Some(tab),
                        notice: Notice::Message {
                            level: Level::Warn,
                            text: e,
                        },
                    });
                }
            }
            EditorMsg::Scrolled(s) => {
                if let Some(t) = self.tab_mut(tab) {
                    t.editor.scroll = s;
                }
                self.session_changed(&mut fx);
            }
            EditorMsg::Copy | EditorMsg::Cut => {
                if let Some(text) = self.selected_text(tab) {
                    fx.push(Effect::ClipboardWrite {
                        text,
                        primary: false,
                    });
                    if matches!(msg, EditorMsg::Cut)
                        && let Err(e) =
                            self.command(tab, EditCommand::Delete, Intent::ui(tab), &mut fx)
                    {
                        fx.push(Effect::Notice {
                            tab: Some(tab),
                            notice: Notice::Message {
                                level: Level::Warn,
                                text: e,
                            },
                        });
                    }
                }
            }
            EditorMsg::Paste { primary } => fx.push(Effect::ClipboardRead {
                primary,
                intent: Intent::ui(tab),
            }),
            EditorMsg::PrimarySelection(text) => fx.push(Effect::ClipboardWrite {
                text,
                primary: true,
            }),
            EditorMsg::Preedit(s) => {
                if let Some(t) = self.tab_mut(tab) {
                    t.editor.set_preedit(!s.is_empty());
                }
            }
            EditorMsg::Focus(focused) => {
                if focused {
                    self.active = Some(tab);
                    if let Some(x) = self.x.get_mut(&tab) {
                        x.agent_since_focus = false;
                    }
                    self.timer(2_000, TimerFor::ClearMarkers(tab), &mut fx);
                    if self.tab(tab).is_some_and(|t| t.mirror.is_some()) {
                        self.send_saved(tab, &mut fx);
                    }
                }
            }
            EditorMsg::Layout(r) => {
                if let Some(x) = self.x.get_mut(&tab) {
                    x.layout = Some(r);
                }
            }
        }
        self.eval_waiters(&mut fx);
        fx
    }

    /// Insert `text` at `tab`'s caret as window input. `true` when the
    /// document took it; a refusal (the buffer's size limit, a detached
    /// document) is a notice.
    pub fn insert(&mut self, tab: TabId, text: String) -> (bool, Vec<Effect>) {
        let mut fx = Vec::new();
        let done = self.command(tab, EditCommand::Insert(text), Intent::ui(tab), &mut fx);
        let took = match done {
            Ok(_) => true,
            Err(e) => {
                fx.push(Effect::Notice {
                    tab: Some(tab),
                    notice: Notice::Message {
                        level: Level::Warn,
                        text: e,
                    },
                });
                false
            }
        };
        self.eval_waiters(&mut fx);
        (took, fx)
    }

    /// A clipboard read completed for `intent` (applies to `intent.tab`'s
    /// selection as it is now; dropped with a notice if that tab is gone).
    pub fn on_paste(&mut self, intent: Intent, text: Option<String>) -> Vec<Effect> {
        let mut fx = Vec::new();
        let tab = intent.tab;
        let live = self
            .tab(tab)
            .and_then(|t| t.mirror.as_ref())
            .is_some_and(|m| matches!(m.phase(), Phase::Live));
        match text {
            Some(text) if live && !text.is_empty() => {
                if let Err(e) = self.command(tab, EditCommand::Insert(text), intent, &mut fx) {
                    fx.push(Effect::Notice {
                        tab: Some(tab),
                        notice: Notice::Message {
                            level: Level::Warn,
                            text: e,
                        },
                    });
                }
            }
            Some(_) if !live => fx.push(Effect::Notice {
                tab: None,
                notice: Notice::Message {
                    level: Level::Warn,
                    text: "The paste was dropped: its tab is closed or detached".into(),
                },
            }),
            _ => {}
        }
        self.eval_waiters(&mut fx);
        fx
    }

    /// Open paths (argv, `ced.open`, file drop, dialog). `path:line[:col]`
    /// suffixes are honoured when that file does not exist.
    pub fn open_paths(&mut self, paths: &[String], intent: Intent) -> Vec<Effect> {
        let _ = intent;
        let mut fx = Vec::new();
        self.open_many(paths, None, &mut fx);
        fx
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    /// Another origin edited this tab since it was last focused (tab `◆`).
    pub fn agent_since_focus(&self, tab: TabId) -> bool {
        self.x.get(&tab).is_some_and(|x| x.agent_since_focus)
    }

    // ── tabs ─────────────────────────────────────────────────────────────────

    fn tab_ids(&self) -> Vec<TabId> {
        self.tabs.iter().map(|t| t.id).collect()
    }

    fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn new_tab(&mut self, path: Option<String>) -> TabId {
        let id = self.next_tab;
        self.next_tab += 1;
        let highlight = Highlight::for_language("text", path.as_deref().map(std::path::Path::new));
        self.tabs.push(Tab {
            id,
            mirror: None,
            editor: EditorModel::default(),
            highlight,
            diagnostics: Diagnostics::default(),
            saved_language: None,
            mail: None,
            document_notice: None,
            classified: false,
            path,
        });
        self.x.insert(id, TabX::default());
        id
    }

    /// The tab a Bus request names (`tab` or `buffer`), else the active one.
    fn resolve_tab(&self, sel: &commands::TabSel) -> Result<TabId, String> {
        if let Some(t) = sel.tab {
            return self
                .tab(t)
                .map(|t| t.id)
                .ok_or_else(|| format!("no tab {t}"));
        }
        if let Some(b) = &sel.buffer {
            return self
                .tabs
                .iter()
                .find(|t| t.mirror.as_ref().is_some_and(|m| m.buffer() == b))
                .map(|t| t.id)
                .ok_or_else(|| format!("no tab shows buffer {b}"));
        }
        self.active.ok_or_else(|| "no tab is open".to_string())
    }

    fn close_tab(&mut self, id: TabId, fx: &mut Vec<Effect>) {
        self.tabs.retain(|t| t.id != id);
        self.x.remove(&id);
        let waiters: Vec<u64> = self
            .waiters
            .iter()
            .filter(|w| w.tab == id)
            .map(|w| w.id)
            .collect();
        for w in waiters {
            self.finish_waiter(w, Err(("cancelled", "the tab was closed".to_string())), fx);
        }
        // Its server ops die with the mirror: answer their Bus callers now,
        // not with a TIMEOUT that says they may yet complete.
        let ops: Vec<String> = self
            .op_waits
            .iter()
            .filter(|(_, w)| w.tab == id)
            .map(|(op, _)| op.clone())
            .collect();
        for op in ops {
            let msg = "The tab was closed before this completed";
            let r = wire::Refusal {
                error_code: wire::ErrorCode::Conflict,
                message: msg.into(),
                reason: Some("detached".into()),
                buffer: None,
                rev: None,
                context: Default::default(),
            };
            self.answer_op(&op, Outcome::Refused(r), fx);
        }
        if self.active == Some(id) {
            self.active = self.tabs.last().map(|t| t.id);
        }
        self.session_changed(fx);
    }

    // ── requests ─────────────────────────────────────────────────────────────

    fn send(&mut self, out: Outgoing, req: Req, fx: &mut Vec<Effect>) {
        self.next_req += 1;
        self.reqs.insert(self.next_req, req);
        fx.push(Effect::Send {
            req: self.next_req,
            out,
        });
    }

    fn timer(&mut self, ms: u64, what: TimerFor, fx: &mut Vec<Effect>) -> u64 {
        self.next_timer += 1;
        self.timers.insert(self.next_timer, what);
        fx.push(Effect::Timer {
            id: self.next_timer,
            ms,
        });
        self.next_timer
    }

    fn send_info(&mut self, fx: &mut Vec<Effect>) {
        let out = Outgoing {
            command: "edit.info".into(),
            body: "{}".into(),
            op_id: None,
            deadline_ms: DEADLINE_MS,
        };
        self.send(out, Req::Info, fx);
    }

    fn send_open(
        &mut self,
        tab: TabId,
        path: Option<String>,
        reattach: bool,
        fx: &mut Vec<Effect>,
    ) {
        if let Some(x) = self.x.get_mut(&tab) {
            x.open_error = None;
            x.reattaching = reattach;
            if path.is_some() {
                x.opened_path = path.clone();
            }
        }
        let body = match &path {
            Some(p) => json!({"path": p, "origin": UI_ORIGIN}),
            None => match self.tab(tab).and_then(|t| t.mail.as_ref()) {
                Some(mail) => {
                    let text = self
                        .tab(tab)
                        .and_then(|t| t.mirror.as_ref())
                        .map(|m| text_string(m.text()))
                        .unwrap_or_else(|| mail.saved.body.clone());
                    json!({"origin": UI_ORIGIN, "text": text, "language": "markdown"})
                }
                None => json!({"origin": UI_ORIGIN}),
            },
        };
        let out = Outgoing {
            command: "edit.open".into(),
            body: body.to_string(),
            op_id: None,
            deadline_ms: if self.tab(tab).is_some_and(|t| t.mirror.is_some()) {
                DEADLINE_MS
            } else {
                self.open_deadline(tab)
            },
        };
        self.send(out, Req::Open { tab, reattach }, fx);
    }

    fn open_deadline(&self, tab: TabId) -> u64 {
        if self.tab(tab).is_some_and(|t| t.mirror.is_some()) {
            DEADLINE_LONG_MS
        } else {
            match self.x.get(&tab).map_or(0, |x| x.open_misses) {
                0 => 3_000,
                1 => 6_000,
                2 => 12_000,
                _ => 30_000,
            }
        }
    }

    fn send_list(&mut self, tab: TabId, fx: &mut Vec<Effect>) {
        if let Some(x) = self.x.get_mut(&tab) {
            x.open_error = None;
            x.reattaching = true;
        }
        let out = Outgoing {
            command: "edit.list".into(),
            body: "{}".into(),
            op_id: None,
            deadline_ms: self.open_deadline(tab),
        };
        self.send(out, Req::List { tab }, fx);
    }

    /// A reply the bus thread parsed already: a mirror's (snapshot pages,
    /// histories) goes straight in; anything else takes the text path.
    fn on_parsed(&mut self, req: u64, rc: u8, v: Value, fx: &mut Vec<Effect>) {
        if !matches!(self.reqs.get(&req), Some(Req::Mirror { .. })) {
            return self.on_reply(req, rc, &v.to_string(), fx);
        }
        let Some(Req::Mirror { tab, op_id }) = self.reqs.remove(&req) else {
            return;
        };
        let result = if rc < 10 {
            Ok(v)
        } else {
            Err(reply_result(rc, &v.to_string())
                .err()
                .unwrap_or_else(|| unparsed_refusal(&v)))
        };
        self.mirror_reply(tab, &op_id, result, fx);
    }

    fn mirror_reply(
        &mut self,
        tab: TabId,
        op_id: &str,
        result: Result<Value, wire::Refusal>,
        fx: &mut Vec<Effect>,
    ) {
        if let Ok(v) = &result
            && let Some(ep) = v.get("epoch").and_then(Value::as_str)
        {
            self.note_epoch(ep, fx);
        }
        if let Some(step) = self
            .tab_mut(tab)
            .and_then(|t| t.mirror.as_mut())
            .map(|m| m.on_reply(op_id, result))
        {
            self.drive(tab, step, fx);
        }
    }

    fn on_reply(&mut self, req: u64, rc: u8, body: &str, fx: &mut Vec<Effect>) {
        match self.reqs.remove(&req) {
            Some(Req::Mirror { tab, op_id }) => {
                self.mirror_reply(tab, &op_id, reply_result(rc, body), fx)
            }
            Some(Req::Open { tab, reattach }) => self.on_open_reply(tab, reattach, rc, body, fx),
            Some(Req::List { tab }) => self.on_list_reply(tab, rc, body, fx),
            Some(Req::Info) => {
                if rc < 10
                    && let Ok(info) = serde_json::from_str::<wire::InfoReply>(body)
                {
                    self.edit_info = commands::EditInfo {
                        epoch: Some(info.epoch.clone()),
                        version: Some(info.version.clone()),
                        volatile: Some(info.volatile),
                    };
                    self.note_epoch(&info.epoch, fx);
                    self.retry_unattached(fx);
                }
            }
            Some(Req::Close { tab, cmd, intent }) => {
                if rc < 10 {
                    self.close_tab(tab, fx);
                    if let Some((id, action)) = cmd {
                        fx.push(Effect::Respond {
                            id,
                            rc: 0,
                            body: ok_body(&commands::ActionReply {
                                id: action,
                                ok: true,
                                result: None,
                            }),
                        });
                    }
                } else {
                    let r = reply_result(rc, body).err();
                    let message = r
                        .as_ref()
                        .map(|r| r.message.clone())
                        .unwrap_or_else(|| body.to_string());
                    let dirty = r.as_ref().and_then(|r| r.reason.as_deref()) == Some("dirty");
                    match cmd {
                        Some((id, _)) => fx.push(Effect::Respond {
                            id,
                            rc: 10,
                            body: refusal(
                                code::CONFLICT,
                                message,
                                Some(if dirty { "dirty" } else { "refused" }),
                            ),
                        }),
                        None if dirty => {
                            fx.push(Effect::Prompt(Prompt::CloseDirty { tab, intent }))
                        }
                        None => fx.push(Effect::Notice {
                            tab: Some(tab),
                            notice: Notice::Message {
                                level: Level::Warn,
                                text: message,
                            },
                        }),
                    }
                }
            }
            Some(Req::Find { tab, job }) => self.on_find_reply(tab, *job, rc, body, fx),
            Some(Req::Matches { tab, query }) => self.on_matches_reply(tab, query, rc, body, fx),
            Some(Req::Recovered) => self.on_recovered_list(rc, body, fx),
            Some(Req::Saved { tab }) => self.on_saved_reply(tab, rc, body),
            Some(Req::Discard) => {
                if rc >= 10 {
                    let msg = reply_result(rc, body)
                        .err()
                        .map(|r| r.message)
                        .unwrap_or_default();
                    fx.push(Effect::Notice {
                        tab: None,
                        notice: Notice::Message {
                            level: Level::Warn,
                            text: msg,
                        },
                    });
                }
            }
            Some(Req::Select) | None => {}
        }
    }

    fn slow_open(&mut self, tab: TabId, fx: &mut Vec<Effect>) {
        if let Some(x) = self.x.get_mut(&tab) {
            x.open_misses = x.open_misses.saturating_add(1);
            if x.open_misses == 1 {
                fx.push(Effect::Notice {
                    tab: Some(tab),
                    notice: Notice::Message {
                        level: Level::Warn,
                        text: "Still opening; the edit service is slow.".into(),
                    },
                });
            }
        }
    }

    fn on_deadline(&mut self, req: u64, fx: &mut Vec<Effect>) {
        match self.reqs.remove(&req) {
            Some(Req::Mirror { tab, op_id }) => {
                self.stats.uncertain += 1;
                if let Some(step) = self
                    .tab_mut(tab)
                    .and_then(|t| t.mirror.as_mut())
                    .map(|m| m.on_deadline(&op_id))
                {
                    self.drive(tab, step, fx);
                }
            }
            Some(Req::Open { tab, reattach }) => {
                if let Some(x) = self.x.get_mut(&tab) {
                    x.reattaching = false;
                }
                if self.tab(tab).is_some_and(|t| t.mirror.is_some()) {
                    // Retain the established restart recovery for live mirrors.
                    let path = self.tab(tab).and_then(|t| t.path.clone());
                    self.send_open(tab, path, reattach, fx);
                    if let Some(x) = self.x.get_mut(&tab) {
                        x.open_misses += 1;
                        if x.open_misses == OPEN_WARN_AFTER {
                            fx.push(Effect::Notice {
                                tab: Some(tab),
                                notice: Notice::Message {
                                    level: Level::Warn,
                                    text: editor_model::mirror::MSG_SERVICE_SILENT.into(),
                                },
                            });
                        }
                    }
                } else {
                    self.slow_open(tab, fx);
                    if let Some(t) = self.tab(tab) {
                        self.send_open(tab, t.path.clone(), reattach, fx);
                    }
                }
            }
            Some(Req::List { tab }) => {
                if let Some(x) = self.x.get_mut(&tab) {
                    x.reattaching = false;
                    if self.tab(tab).is_some_and(|t| t.mirror.is_none()) {
                        self.slow_open(tab, fx);
                    }
                }
                self.send_list(tab, fx);
            }
            Some(Req::Find { tab, .. }) => fx.push(Effect::Notice {
                tab: Some(tab),
                notice: Notice::Message {
                    level: Level::Warn,
                    text: "The search timed out".into(),
                },
            }),
            Some(Req::Close {
                cmd: Some((id, _)), ..
            }) => {
                fx.push(Effect::Respond {
                    id,
                    rc: 10,
                    body: refusal(code::UNAVAILABLE, "edit.close timed out", Some("timeout")),
                });
            }
            _ => {}
        }
    }

    fn on_timer(&mut self, id: u64, fx: &mut Vec<Effect>) {
        match self.timers.remove(&id) {
            Some(TimerFor::Reattach(tab)) => self.reattach(tab, fx),
            Some(TimerFor::Retry(tab)) => {
                if let Some(step) = self
                    .tab_mut(tab)
                    .and_then(|t| t.mirror.as_mut())
                    .map(Mirror::on_retry)
                {
                    self.drive(tab, step, fx);
                }
            }
            Some(TimerFor::Wait(w)) => {
                let ms = self
                    .waiters
                    .iter()
                    .find(|x| x.id == w)
                    .map(|x| x.timeout_ms)
                    .unwrap_or(0);
                self.finish_waiter(
                    w,
                    Err(("timeout", format!("ced.wait timed out after {ms} ms"))),
                    fx,
                );
            }
            Some(TimerFor::Publish(tab)) => {
                if let Some(x) = self.x.get_mut(&tab) {
                    x.publish_timer = false;
                }
                self.maybe_publish(tab, fx);
            }
            Some(TimerFor::Session) => {
                self.session_timer = false;
                fx.push(Effect::SaveSession);
            }
            Some(TimerFor::Relex(tab)) => self.relex_due(tab, fx),
            Some(TimerFor::Rematch(tab)) => {
                if let Some(x) = self.x.get_mut(&tab) {
                    x.rematch_timer = false;
                }
                self.request_matches(tab, fx);
            }
            Some(TimerFor::UiDeadline(token)) => self.ui_timeout(token, fx),
            Some(TimerFor::Echo(tab, op_id)) => {
                if let Some(step) = self
                    .tab_mut(tab)
                    .and_then(|t| t.mirror.as_mut())
                    .map(|m| m.on_echo_timeout(&op_id))
                {
                    if !step.out.is_empty() {
                        self.stats.history_recoveries += 1;
                    }
                    self.drive(tab, step, fx);
                }
            }
            Some(TimerFor::OpWait(op_id)) => {
                if let Some(OpWait { id, action, .. }) = self.op_waits.remove(&op_id) {
                    let msg = format!(
                        "{action} is still queued or in flight after {} s; it may yet complete",
                        OP_WAIT_MS / 1000
                    );
                    fx.push(Effect::Respond {
                        id,
                        rc: 10,
                        body: refusal("TIMEOUT", msg, Some("timeout")),
                    });
                }
            }
            Some(TimerFor::ClearMarkers(tab)) => {
                if self.active == Some(tab)
                    && let Some(t) = self.tab_mut(tab)
                {
                    t.editor.clear_markers();
                }
            }
            None => {}
        }
    }

    fn on_topic(&mut self, topic: &str, body: &str, fx: &mut Vec<Effect>) {
        match topic {
            wire::TOPIC_CHANGED => {
                let Ok(ev) = serde_json::from_str::<wire::Event>(body) else {
                    return;
                };
                self.on_edit_event(ev, fx);
            }
            // `edit` (re)appearing in services.registered may be a restart:
            // ask it its epoch.
            "noded.props.changed" => {
                let Ok(value) = serde_json::from_str::<Value>(body) else {
                    return;
                };
                if value["path"] != "services.registered" {
                    return;
                }
                let Some(services) = value["new"].as_array() else {
                    return;
                };
                let present = services
                    .iter()
                    .any(|service| service.as_str() == Some("edit"));
                let was_present = self.edit_present.unwrap_or_else(|| {
                    value["old"].as_array().is_some_and(|services| {
                        services
                            .iter()
                            .any(|service| service.as_str() == Some("edit"))
                    })
                });
                let appeared = present && !was_present;
                self.edit_present = Some(present);
                if appeared {
                    self.send_info(fx);
                    self.retry_unattached(fx);
                }
            }
            _ => {}
        }
    }

    fn on_edit_event(&mut self, ev: wire::Event, fx: &mut Vec<Effect>) {
        self.stats.events += 1;
        let (epoch, seq) = match &ev {
            wire::Event::Edit(e) => (e.epoch.clone(), e.event_seq),
            wire::Event::Cursor(e) => (e.epoch.clone(), e.event_seq),
            wire::Event::Anchor(e) => (e.epoch.clone(), e.event_seq),
            wire::Event::Disk(e) => (e.epoch.clone(), e.event_seq),
            wire::Event::Open(e) => (e.epoch.clone(), e.event_seq),
            wire::Event::Close(e) => (e.epoch.clone(), e.event_seq),
            wire::Event::Resync(e) => (e.epoch.clone(), e.event_seq),
        };
        if self.note_epoch(&epoch, fx) {
            self.last_event_seq = Some(seq);
            return;
        }
        // The global event_seq: any gap may have been ours.
        if let Some(last) = self.last_event_seq
            && seq != last + 1
        {
            for id in self.tab_ids() {
                if let Some(step) = self
                    .tab_mut(id)
                    .and_then(|t| t.mirror.as_mut())
                    .map(Mirror::suspect)
                {
                    self.stats.history_recoveries += 1;
                    self.drive(id, step, fx);
                }
            }
        }
        self.last_event_seq = Some(seq);
        let buffer = match &ev {
            wire::Event::Edit(e) => Some(e.buffer.clone()),
            wire::Event::Cursor(e) => Some(e.buffer.clone()),
            wire::Event::Disk(e) => Some(e.buffer.clone()),
            wire::Event::Close(e) => Some(e.buffer.clone()),
            wire::Event::Resync(_) => None,
            _ => return,
        };
        for id in self.tab_ids() {
            let step = match self.tab_mut(id).and_then(|t| t.mirror.as_mut()) {
                Some(m) if buffer.as_deref().is_none_or(|b| b == m.buffer()) => m.on_event(&ev),
                _ => continue,
            };
            if let wire::Event::Edit(e) = &ev
                && !(e.origin == UI_ORIGIN || e.origin.starts_with("agent:ced."))
                && self.active != Some(id)
                && let Some(x) = self.x.get_mut(&id)
            {
                x.agent_since_focus = true;
            }
            self.drive(id, step, fx);
            if matches!(ev, wire::Event::Disk(_)) {
                // A save (by anyone) or reload changed the disk state.
                self.send_saved(id, fx);
            }
        }
    }

    /// Record the daemon's epoch; `true` when it CHANGED (every tab reattaches).
    fn note_epoch(&mut self, epoch: &str, fx: &mut Vec<Effect>) -> bool {
        match &self.edit_epoch {
            Some(e) if e == epoch => false,
            None => {
                self.edit_epoch = Some(epoch.to_string());
                false
            }
            Some(_) => {
                self.edit_epoch = Some(epoch.to_string());
                self.edit_info.epoch = Some(epoch.to_string());
                self.last_event_seq = None;
                for id in self.tab_ids() {
                    let stale = self
                        .tab(id)
                        .and_then(|t| t.mirror.as_ref())
                        .is_some_and(|m| m.epoch() != epoch);
                    if stale {
                        self.reattach(id, fx);
                    }
                }
                true
            }
        }
    }

    /// Retry initial opens after the service becomes available.
    fn retry_unattached(&mut self, fx: &mut Vec<Effect>) {
        for id in self.tab_ids() {
            if self.tab(id).is_some_and(|t| t.mirror.is_none())
                && self.open_error(id) == Some(EDIT_UNAVAILABLE)
            {
                self.reattach(id, fx);
            }
        }
    }

    /// Reopen the tab's buffer in the new daemon session.
    fn reattach(&mut self, tab: TabId, fx: &mut Vec<Effect>) {
        let Some(x) = self.x.get_mut(&tab) else {
            return;
        };
        if x.reattaching {
            return;
        }
        x.reattaching = true;
        x.open_error = None;
        if let Some(step) = self
            .tab_mut(tab)
            .and_then(|t| t.mirror.as_mut())
            .map(Mirror::epoch_changed)
        {
            self.drive(tab, step, fx);
        }
        let path = self
            .tab(tab)
            .filter(|t| t.mail.is_none())
            .and_then(|t| t.path.clone());
        match path {
            Some(p) => self.send_open(tab, Some(p), true, fx),
            None => self.send_list(tab, fx),
        }
    }

    fn on_open_reply(
        &mut self,
        tab: TabId,
        reattach: bool,
        rc: u8,
        body: &str,
        fx: &mut Vec<Effect>,
    ) {
        if self.tab(tab).is_none() {
            return;
        }
        if rc >= 10
            && serde_json::from_str::<Value>(body)
                .ok()
                .is_some_and(|v| v["error_code"] == "UNAVAILABLE")
        {
            self.edit_present = Some(false);
            if self.tab(tab).is_some_and(|t| t.mirror.is_none()) {
                self.open_failed(tab, EDIT_UNAVAILABLE.into(), fx);
            } else {
                if let Some(x) = self.x.get_mut(&tab) {
                    x.reattaching = false;
                }
                self.timer(DEADLINE_LONG_MS, TimerFor::Reattach(tab), fx);
            }
            return;
        }
        match reply_result(rc, body) {
            Ok(v) => match serde_json::from_value::<wire::OpenReply>(v) {
                Ok(open) => self.attach(tab, reattach, &open, fx),
                Err(e) => self.open_failed(tab, format!("bad edit.open reply: {e}"), fx),
            },
            Err(r) => {
                let msg = match r.reason.as_deref() {
                    Some(reason) => format!("{} ({reason})", r.message),
                    None => r.message.clone(),
                };
                self.open_failed(tab, msg, fx);
            }
        }
    }

    /// A scratch tab reattaches by `recovery_id`; with no match it
    /// becomes a fresh scratch buffer and keeps its text as a detached copy.
    fn on_list_reply(&mut self, tab: TabId, rc: u8, body: &str, fx: &mut Vec<Effect>) {
        if rc >= 10
            && serde_json::from_str::<Value>(body)
                .ok()
                .is_some_and(|v| v["error_code"] == "UNAVAILABLE")
        {
            self.on_open_reply(tab, true, rc, body, fx);
            return;
        }
        let reattach = self.x.get(&tab).is_some_and(|x| x.reattaching)
            || self.tab(tab).is_some_and(|t| t.mirror.is_some());
        let rid = self.x.get(&tab).and_then(|x| x.recovery_id.clone());
        let list = reply_result(rc, body)
            .ok()
            .and_then(|v| serde_json::from_value::<wire::ListReply>(v).ok());
        let row = list.as_ref().and_then(|l| {
            l.buffers
                .iter()
                .find(|b| {
                    Some(&b.recovery_id) == rid.as_ref()
                        && (self.tab(tab).is_none_or(|t| t.mail.is_none()) || b.path.is_none())
                })
                .map(|b| (l, b))
        });
        match row {
            Some((l, b)) => {
                let open = open_reply_of(&l.epoch, b);
                self.attach(tab, reattach, &open, fx);
            }
            None => {
                if let Some(x) = self.x.get_mut(&tab) {
                    x.reattaching = false;
                }
                if let Some(t) = self.tab_mut(tab)
                    && t.mirror.is_none()
                    && t.mail.is_some()
                {
                    t.mail = None;
                    t.classified = false;
                }
                let path = self
                    .tab(tab)
                    .filter(|t| t.mail.is_none())
                    .and_then(|t| t.path.clone());
                self.send_open(tab, path, reattach, fx);
            }
        }
    }

    fn attach(&mut self, tab: TabId, reattach: bool, open: &wire::OpenReply, fx: &mut Vec<Effect>) {
        self.note_epoch(&open.epoch, fx);
        let reattaching = reattach && self.tab(tab).is_some_and(|t| t.mirror.is_some());
        let step = if reattaching {
            let Some(m) = self.tab_mut(tab).and_then(|t| t.mirror.as_mut()) else {
                return;
            };
            m.reattach(open)
        } else {
            match Mirror::bootstrap(open) {
                Ok((m, step)) => {
                    let path = open.path.clone();
                    if let Some(t) = self.tab_mut(tab) {
                        t.highlight = Highlight::for_language(
                            &open.language,
                            path.as_deref().map(std::path::Path::new),
                        );
                        t.mirror = Some(m);
                        if t.path.is_none() {
                            t.path = path;
                        }
                    }
                    step
                }
                Err(e) => {
                    self.open_failed(tab, format!("{e:?}"), fx);
                    return;
                }
            }
        };
        if let Some(t) = self.tab_mut(tab) {
            if t.mail.is_none() {
                t.path.clone_from(&open.path);
            }
            t.saved_language = None;
        }
        if let Some(x) = self.x.get_mut(&tab) {
            x.recovery_id = Some(open.recovery_id.clone());
            x.opened = true;
            x.open_misses = 0;
            x.reattaching = false;
            x.open_error = None;
        }
        if let Some(p) = &open.path {
            self.recent.retain(|r| r != p);
            self.recent.insert(0, p.clone());
            self.recent.truncate(RECENT_MAX);
        }
        self.drive(tab, step, fx);
        if self.tab(tab).is_some_and(|t| t.mail.is_none()) {
            self.send_saved(tab, fx);
        }
        self.check_opens(fx);
        self.session_changed(fx);
    }

    fn open_failed(&mut self, tab: TabId, msg: String, fx: &mut Vec<Effect>) {
        if let Some(x) = self.x.get_mut(&tab) {
            x.opened = true;
            x.reattaching = false;
            x.open_error = Some(msg.clone());
        }
        fx.push(Effect::Notice {
            tab: Some(tab),
            notice: Notice::Message {
                level: Level::Error,
                text: msg,
            },
        });
        self.check_opens(fx);
    }

    /// The failure shown instead of an indefinite opening indicator.
    pub fn open_error(&self, tab: TabId) -> Option<&str> {
        self.x.get(&tab)?.open_error.as_deref()
    }

    /// An initial open has exceeded its first deadline and is being retried.
    pub fn open_slow(&self, tab: TabId) -> bool {
        self.x
            .get(&tab)
            .is_some_and(|x| x.open_misses > 0 && x.open_error.is_none())
    }

    /// Answer every `ced.open` / `ced.new` whose tabs have all had an answer.
    fn check_opens(&mut self, fx: &mut Vec<Effect>) {
        let mut i = 0;
        while i < self.opens.len() {
            let done = self.opens[i]
                .tabs
                .iter()
                .all(|t| self.x.get(t).is_none_or(|x| x.opened));
            if !done {
                i += 1;
                continue;
            }
            let p = self.opens.remove(i);
            let row = |c: &Controller, t: TabId| {
                let tab = c.tab(t);
                commands::OpenedTab {
                    tab: t,
                    buffer: tab
                        .and_then(|t| t.mirror.as_ref())
                        .map(|m| m.buffer().to_string()),
                    path: tab.and_then(|t| t.path.clone()),
                }
            };
            let body = if p.new {
                let r = row(self, p.tabs[0]);
                ok_body(&commands::NewReply {
                    tab: r.tab,
                    buffer: r.buffer,
                })
            } else {
                let tabs = p.tabs.iter().map(|&t| row(self, t)).collect();
                ok_body(&commands::OpenReply { tabs })
            };
            fx.push(Effect::Respond {
                id: p.cmd,
                rc: 0,
                body,
            });
        }
    }

    /// Open (or focus) each path; `cmd` answers when all have an answer.
    fn open_many(
        &mut self,
        paths: &[String],
        cmd: Option<(u64, Option<(usize, usize)>)>,
        fx: &mut Vec<Effect>,
    ) -> Vec<TabId> {
        let mut ids = Vec::new();
        for raw in paths {
            let (path, goto) = if std::path::Path::new(raw).exists() {
                (raw.clone(), None)
            } else {
                split_line_col(raw)
            };
            let goto = cmd.and_then(|(_, g)| g).or(goto);
            let opened_path = path.clone();
            let path = std::path::absolute(&path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or(path);
            let existing = self
                .tabs
                .iter()
                .find(|t| {
                    t.path.as_ref().is_some_and(|p| {
                        std::path::absolute(p).ok() == std::path::absolute(&path).ok()
                    })
                })
                .map(|t| t.id);
            let id = match existing {
                Some(id) => id,
                None => {
                    let id = self.new_tab(Some(path.clone()));
                    self.send_open(id, Some(path), false, fx);
                    self.x.get_mut(&id).unwrap().opened_path = Some(opened_path);
                    id
                }
            };
            if let Some(g) = goto {
                self.goto(id, g);
            }
            self.active = Some(id);
            ids.push(id);
        }
        if let Some((cmd, _)) = cmd {
            self.opens.push(PendingOpen {
                cmd,
                tabs: ids.clone(),
                new: false,
            });
            self.check_opens(fx);
        }
        self.session_changed(fx);
        ids
    }

    /// Move the caret to `line:col` now if the text is there, else once it is.
    fn goto(&mut self, tab: TabId, (line, col): (usize, usize)) {
        let Some(t) = self.tab_mut(tab) else { return };
        let at = t
            .mirror
            .as_ref()
            .filter(|m| matches!(m.phase(), Phase::Live))
            .map(|m| offset_of_line_col(m.text(), line, col));
        match at {
            Some(Ok(o)) => {
                t.editor.sel = Selection { anchor: o, head: o };
                t.editor.scroll.first_line = line.saturating_sub(5).max(1);
            }
            Some(Err(_)) => {}
            None => {
                if let Some(x) = self.x.get_mut(&tab) {
                    x.goto = Some((line, col));
                }
            }
        }
    }

    // ── the mirror ↔ editor bridge ───────────────────────────────────────────

    /// Feed a mirror step to the editor side and the transport; drain the
    /// scheduler; react to detaches, drains and pending gotos.
    fn drive(&mut self, tab: TabId, step: Step, fx: &mut Vec<Effect>) {
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) else {
            return;
        };
        let mut resynced = false;
        if let Some(m) = &t.mirror {
            if t.mail.is_none() && m.meta().path != t.path {
                let mut first = String::new();
                if let Some(range) = m.text().line_range(1) {
                    m.text().read(range, &mut first);
                }
                t.saved_language = Some(edit::lang::detect(
                    m.meta().path.as_deref().map(std::path::Path::new),
                    &first,
                ));
                t.path.clone_from(&m.meta().path);
            }
            for d in &step.deltas {
                t.editor.apply_delta(d);
                t.highlight.apply_delta(m.text(), d);
                t.diagnostics.apply_delta(d);
                if d.kind == DeltaKind::Resync {
                    t.editor.clamp(m.text());
                    resynced = true;
                }
            }
        }
        let is_mix = t.highlight.is_mix();
        if self.detect_document(tab, fx) {
            return;
        }
        if !step.deltas.is_empty() {
            self.after_deltas(tab, &step.deltas, is_mix, fx);
        }
        if let Some(c) = self.x.get_mut(&tab).and_then(|x| x.lint.as_mut()) {
            for d in &step.deltas {
                c.record(d);
            }
        }
        if resynced {
            self.stats.snapshot_recoveries += 1;
            if let Some(x) = self.x.get_mut(&tab) {
                x.ext_due = true;
            }
        }
        for n in step.notices {
            if matches!(n, Notice::Conflict(_)) {
                self.stats.conflicts += 1;
            }
            if matches!(&n, Notice::Message { text, .. } if text == editor_model::mirror::MSG_SAVE_DISK_MODIFIED)
            {
                let x = self.x.get_mut(&tab);
                let intent = x.and_then(|x| {
                    x.close_after_save = None;
                    x.save_intent.take()
                });
                let intent = intent.unwrap_or_else(|| Intent::ui(tab));
                // Only the human who pressed Save is asked; a Bus caller gets
                // the refusal as its `ced.execute` reply.
                if matches!(intent.by, Invoker::Ui) {
                    fx.push(Effect::Prompt(Prompt::DiskModified { tab, intent }));
                } else {
                    // No dialog backs "save anyway?": state what happened.
                    let text = "An agent's save was refused: the file changed on disk".to_string();
                    fx.push(Effect::Notice {
                        tab: Some(tab),
                        notice: Notice::Message {
                            level: Level::Warn,
                            text,
                        },
                    });
                }
                continue;
            }
            fx.push(Effect::Notice {
                tab: Some(tab),
                notice: n,
            });
        }
        let (outcomes, echo) = match self.tab_mut(tab).and_then(|t| t.mirror.as_mut()) {
            Some(m) => (m.take_outcomes(), m.take_echo_timer()),
            None => (Vec::new(), None),
        };
        for (op_id, outcome) in outcomes {
            self.answer_op(&op_id, outcome, fx);
        }
        if let Some((op_id, ms)) = echo {
            self.timer(ms, TimerFor::Echo(tab, op_id), fx);
        }
        for out in step.out {
            let op_id = out.op_id.clone().unwrap_or_default();
            self.send(out, Req::Mirror { tab, op_id }, fx);
        }
        let mut outs = Vec::new();
        let mut retry = None;
        let mut detached_epoch = false;
        let mut idle = false;
        let mut live = false;
        if let Some(m) = self.tab_mut(tab).and_then(|t| t.mirror.as_mut()) {
            while let Some(o) = m.next_outgoing() {
                outs.push(o);
            }
            retry = m.take_retry_timer();
            detached_epoch = matches!(
                m.phase(),
                Phase::Detached {
                    reason: DetachReason::EpochChanged
                }
            );
            idle = m.is_idle();
            live = matches!(m.phase(), Phase::Live);
        }
        for out in outs {
            let op_id = out.op_id.clone().unwrap_or_default();
            self.send(out, Req::Mirror { tab, op_id }, fx);
        }
        if let Some(ms) = retry {
            self.stats.retries += 1;
            self.timer(ms, TimerFor::Retry(tab), fx);
        }
        if detached_epoch && !self.x.get(&tab).is_some_and(|x| x.reattaching) {
            // `epoch_mismatch` seen by the mirror itself: learn the new epoch.
            self.send_info(fx);
            self.reattach(tab, fx);
        }
        if live && let Some(g) = self.x.get_mut(&tab).and_then(|x| x.goto.take()) {
            self.goto(tab, g);
        }
        if live && let Some((text, intent)) = self.x.get_mut(&tab).and_then(|x| x.fill.take()) {
            let len = self
                .tab(tab)
                .and_then(|t| t.mirror.as_ref())
                .map_or(0, |m| m.text().len());
            let e = editor_model::types::LocalEdit {
                items: vec![(len..len, text)],
                coalesce: false,
                caret_after: Selection { anchor: 0, head: 0 },
            };
            if let Some(i) = self.tabs.iter().position(|t| t.id == tab)
                && let Some(m) = self.tabs[i].mirror.as_mut()
            {
                match m.local_edit(e, intent, &mut self.ids) {
                    Ok(step) => return self.drive(tab, step, fx),
                    Err(e) => fx.push(Effect::Notice {
                        tab: Some(tab),
                        notice: Notice::Message {
                            level: Level::Error,
                            text: format!("the copy could not be inserted: {e:?}"),
                        },
                    }),
                }
            }
        }
        if !live {
            self.cancel_mail_save(
                tab,
                "the parked mail save was cancelled because the body buffer is reconnecting",
                fx,
            );
        }
        if idle {
            self.drain_mail_save(tab, fx);
            if let Some(job) = self.x.get_mut(&tab).and_then(|x| x.find.take()) {
                self.start_find(tab, job, fx);
            }
            if self
                .x
                .get_mut(&tab)
                .is_some_and(|x| std::mem::take(&mut x.rematch_due))
            {
                self.request_matches(tab, fx);
            }
            let saved = self
                .tab(tab)
                .and_then(|t| t.mirror.as_ref())
                .is_some_and(|m| !m.meta().dirty)
                && self.tab(tab).is_some_and(|t| t.mail.is_none());
            if let Some(intent) = self
                .x
                .get_mut(&tab)
                .and_then(|x| x.close_after_save.take_if(|_| saved))
            {
                self.close(tab, false, None, intent, fx);
                return;
            }
            self.maybe_publish(tab, fx);
        }
    }

    fn edit_cfg(&self, tab: &Tab) -> EditCfg {
        let (language, eol) = tab
            .mirror
            .as_ref()
            .map(|m| (m.meta().language.clone(), m.meta().eol))
            .unwrap_or(("text".into(), wire::Eol::Lf));
        let insert_spaces = self
            .config
            .insert_spaces
            .get(&language)
            .copied()
            .unwrap_or(false);
        // With spaces, one indent = `tab_size` spaces: pass the language's
        // indent width in the editing cfg.
        let tab_size = if insert_spaces {
            self.config.indent_width(&language)
        } else {
            self.config.tab_size
        };
        EditCfg {
            measure: MeasureCfg {
                tab_size: tab_size.clamp(1, 16),
                ambiguous_wide: self.config.ambiguous_wide,
            },
            insert_spaces,
            eol: if eol == wire::Eol::Crlf { "\r\n" } else { "\n" },
            line_comment: editor_model::model::line_comment_for(&language),
        }
    }

    /// Run an editing / motion command on `tab` for `intent`.
    fn command(
        &mut self,
        tab: TabId,
        c: EditCommand,
        intent: Intent,
        fx: &mut Vec<Effect>,
    ) -> Result<(), String> {
        let Some(i) = self.tabs.iter().position(|t| t.id == tab) else {
            return Err(format!("no tab {tab}"));
        };
        if self.tabs[i]
            .mail
            .as_ref()
            .is_some_and(|mail| mail.sending || mail.saving)
            && !self.mail_body_editable(tab)
        {
            return Err("the mail document is busy".into());
        }
        let cfg = self.edit_cfg(&self.tabs[i]);
        let t = &mut self.tabs[i];
        let Some(m) = t.mirror.as_mut() else {
            return Err("the buffer is not open yet".into());
        };
        let before = t.editor.sel;
        let Some(edit) = t.editor.command(m.text(), &cfg, c) else {
            if t.editor.sel != before {
                self.caret_moved(tab, fx);
            }
            return Ok(());
        };
        let caret_after = edit.caret_after;
        let step = m
            .local_edit(edit, intent, &mut self.ids)
            .map_err(|e| match e {
                editor_model::mirror::MirrorError::NotLive => {
                    "The buffer is reconnecting — try again in a moment".to_string()
                }
                editor_model::mirror::MirrorError::TooLarge { items, bytes } => {
                    format!("Too large for one undoable edit ({items} places, {bytes} bytes)")
                }
                editor_model::mirror::MirrorError::Invalid(m) => m,
            })?;
        self.drive(tab, step, fx);
        // The issuing view's selection is the edit's `caret_after`: mapping
        // alone cannot express a moved or re-indented block.
        if let Some(t) = self.tab_mut(tab) {
            t.editor.sel = caret_after;
        }
        self.caret_moved(tab, fx);
        Ok(())
    }

    fn selected_text(&self, tab: TabId) -> Option<String> {
        let t = self.tab(tab)?;
        let m = t.mirror.as_ref()?;
        let s = t.editor.sel;
        let r: Range<usize> = s.anchor.min(s.head)..s.anchor.max(s.head);
        if r.is_empty() || r.end > m.text().len() {
            return None;
        }
        let mut out = String::new();
        m.text().read(r, &mut out);
        Some(out)
    }

    fn server_op(
        &mut self,
        tab: TabId,
        op: ServerOp,
        intent: Intent,
        fx: &mut Vec<Effect>,
    ) -> Result<(), String> {
        if self.tab(tab).is_some_and(|t| t.mail.is_some()) {
            match &op {
                ServerOp::Save { path, .. } => {
                    return self
                        .save_mail(tab, path.clone(), false, fx)
                        .map_err(|e| e.to_string());
                }
                ServerOp::Reload { .. } => {
                    return Err("close and reopen this mail document to reload it".into());
                }
                _ => {}
            }
        }
        let Some(m) = self
            .tabs
            .iter_mut()
            .find(|t| t.id == tab)
            .and_then(|t| t.mirror.as_mut())
        else {
            return Err("the buffer is not open".into());
        };
        if matches!(m.phase(), Phase::Detached { .. }) {
            return Err("the buffer is detached".into());
        }
        // A Bus `ced.execute` being dispatched is answered from this op's
        // outcome, not now.
        let (step, op_id) = m.server_op(op, intent, &mut self.ids);
        if let Some((id, action)) = self.op_cmd.take() {
            self.op_waits
                .insert(op_id.clone(), OpWait { id, action, tab });
            self.timer(OP_WAIT_MS, TimerFor::OpWait(op_id), fx);
        }
        self.drive(tab, step, fx);
        Ok(())
    }

    /// Answer the Bus `ced.execute` waiting on server op `op_id`, if any.
    fn answer_op(&mut self, op_id: &str, outcome: Outcome, fx: &mut Vec<Effect>) {
        let Some(OpWait { id, action, .. }) = self.op_waits.remove(op_id) else {
            return;
        };
        self.timers
            .retain(|_, t| !matches!(t, TimerFor::OpWait(x) if x == op_id));
        let (rc, body) = match outcome {
            Outcome::Done => (
                0,
                ok_body(&commands::ActionReply {
                    id: action,
                    ok: true,
                    result: None,
                }),
            ),
            Outcome::Refused(r) => {
                let code = serde_json::to_value(r.error_code)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                (10, refusal(&code, r.message, r.reason.as_deref()))
            }
        };
        fx.push(Effect::Respond { id, rc, body });
    }

    /// Ask the service for the tab's save state: after an attach,
    /// on a disk-state change and when the tab gains focus — the open reply
    /// carries none, and another holder may save behind ced's back.
    fn send_saved(&mut self, tab: TabId, fx: &mut Vec<Effect>) {
        let out = Outgoing {
            command: "edit.list".into(),
            body: "{}".into(),
            op_id: None,
            deadline_ms: DEADLINE_MS,
        };
        self.send(out, Req::Saved { tab }, fx);
    }

    fn on_saved_reply(&mut self, tab: TabId, rc: u8, body: &str) {
        let Some(list) = reply_result(rc, body)
            .ok()
            .and_then(|v| serde_json::from_value::<wire::ListReply>(v).ok())
        else {
            return;
        };
        let Some(m) = self.tab_mut(tab).and_then(|t| t.mirror.as_mut()) else {
            return;
        };
        if list.epoch != m.epoch() {
            return;
        }
        if let Some(row) = list.buffers.iter().find(|b| b.buffer == m.buffer()) {
            m.note_saved(row.saved_rev, row.recovered);
        }
    }

    // ── selection publishing ─────────────────────────────────────────────────

    fn caret_moved(&mut self, tab: TabId, fx: &mut Vec<Effect>) {
        let arm = self.x.get_mut(&tab).is_some_and(|x| {
            let arm = !x.publish_timer;
            x.publish_timer = true;
            arm
        });
        if arm {
            self.timer(PUBLISH_DEBOUNCE_MS, TimerFor::Publish(tab), fx);
        }
        self.session_changed(fx);
    }

    fn maybe_publish(&mut self, tab: TabId, fx: &mut Vec<Effect>) {
        let Some(t) = self.tab(tab) else { return };
        let Some(m) = t.mirror.as_ref().filter(|m| m.is_idle()) else {
            return;
        };
        let sel = t.editor.sel;
        let buffer = m.buffer().to_string();
        let len = m.text().len();
        let Some(x) = self.x.get_mut(&tab) else {
            return;
        };
        if x.published == Some(sel) || x.publish_timer || sel.anchor.max(sel.head) > len {
            return;
        }
        if let Some(last) = x.last_publish {
            let gap = last.elapsed().as_millis() as u64;
            if gap < PUBLISH_MIN_GAP_MS {
                x.publish_timer = true;
                self.timer(PUBLISH_MIN_GAP_MS - gap, TimerFor::Publish(tab), fx);
                return;
            }
        }
        x.published = Some(sel);
        x.last_publish = Some(Instant::now());
        let body = json!({"buffer": buffer, "ranges": [{"anchor": sel.anchor, "head": sel.head}], "origin": UI_ORIGIN});
        let out = Outgoing {
            command: "edit.select".into(),
            body: body.to_string(),
            op_id: None,
            deadline_ms: DEADLINE_MS,
        };
        self.send(out, Req::Select, fx);
    }

    fn session_changed(&mut self, fx: &mut Vec<Effect>) {
        if !self.session_timer {
            self.session_timer = true;
            self.timer(SESSION_DEBOUNCE_MS, TimerFor::Session, fx);
        }
    }

    // ── actions ──────────────────────────────────────────────────────────────

    /// `Ok(result)` when handled here; `Err((code, message))` otherwise.
    fn action(
        &mut self,
        tab: Option<TabId>,
        action: ActionId,
        intent: Intent,
        fx: &mut Vec<Effect>,
    ) -> Result<Option<Value>, (&'static str, String)> {
        let need = |t: Option<TabId>| t.ok_or((code::NOT_FOUND, "no tab is open".to_string()));
        let tab = tab.or(self.active);
        let edit = |c: &mut Controller,
                    t: TabId,
                    cmd: EditCommand,
                    intent: Intent,
                    fx: &mut Vec<Effect>| {
            c.command(t, cmd, intent, fx)
                .map(|()| None)
                .map_err(|e| (code::CONFLICT, e))
        };
        let conflict = |e: String| (code::CONFLICT, e);
        match action {
            ActionId::FileNew => {
                let id = self.new_tab(None);
                self.send_open(id, None, false, fx);
                self.active = Some(id);
                Ok(Some(json!({"tab": id})))
            }
            ActionId::FileSend => {
                self.save_mail(need(tab)?, None, true, fx)
                    .map_err(|e| (e.code(), e.to_string()))?;
                Ok(None)
            }
            ActionId::MailHeaders => Err((
                code::INVALID_ARGUMENT,
                "mail.headers needs {headers}".into(),
            )),
            ActionId::FileSave => {
                let t = need(tab)?;
                self.save_or_server_op(
                    t,
                    ServerOp::Save {
                        path: None,
                        force: false,
                    },
                    intent,
                    fx,
                )
                .map_err(|e| (e.code(), e.to_string()))?;
                Ok(None)
            }
            ActionId::FileSaveAll => {
                for t in self.tab_ids() {
                    let dirty = self
                        .tab(t)
                        .and_then(|t| t.mirror.as_ref())
                        .is_some_and(|m| m.meta().dirty && m.meta().path.is_some())
                        || self.tab(t).is_some_and(|t| t.mail.is_some() && t.dirty());
                    if dirty {
                        let _ = self.server_op(
                            t,
                            ServerOp::Save {
                                path: None,
                                force: false,
                            },
                            intent.clone(),
                            fx,
                        );
                    }
                }
                Ok(None)
            }
            ActionId::FileReload => {
                let t = need(tab)?;
                self.server_op(t, ServerOp::Reload { force: false }, intent, fx)
                    .map_err(conflict)?;
                Ok(None)
            }
            ActionId::FileClose => {
                let t = need(tab)?;
                self.close(t, false, None, intent, fx);
                Ok(None)
            }
            ActionId::FileExit => {
                self.cancel_parked_mail_saves(fx);
                if let Some(message) = self.mail_exit_refusal() {
                    return Err((code::CONFLICT, message.into()));
                }
                fx.push(Effect::SaveSession);
                self.close_mail_buffers(fx);
                fx.push(Effect::Quit);
                Ok(None)
            }
            ActionId::EditUndo
            | ActionId::EditRedo
            | ActionId::EditUndoAny
            | ActionId::EditUndoOther => {
                let t = need(tab)?;
                if let Some(mail) = self.tab_mut(t).and_then(|t| t.mail.as_mut()) {
                    mail.editing_field = None;
                    if mail.sending || mail.saving {
                        return Err((code::CONFLICT, "the mail document is busy".into()));
                    }
                    let entry = if action == ActionId::EditRedo {
                        mail.redo.pop()
                    } else {
                        mail.undo.pop()
                    };
                    if let Some(headers) = entry {
                        let old = std::mem::replace(&mut mail.headers, headers);
                        if action == ActionId::EditRedo {
                            mail.undo.push(old);
                        } else {
                            mail.redo.push(old);
                        }
                        return Ok(None);
                    }
                }
                let lane = match action {
                    ActionId::EditUndoAny => LaneArg::Any,
                    ActionId::EditUndoOther => {
                        let lane = self
                            .tab(t)
                            .and_then(|t| t.mirror.as_ref())
                            .and_then(|m| m.last_remote().map(|r| r.lane.clone()))
                            .ok_or((
                                code::CONFLICT,
                                "no other origin has edited this buffer".to_string(),
                            ))?;
                        LaneArg::Lane(lane)
                    }
                    _ => LaneArg::Own,
                };
                let op = if action == ActionId::EditRedo {
                    ServerOp::Redo { lane }
                } else {
                    ServerOp::Undo { lane }
                };
                self.server_op(t, op, intent, fx).map_err(conflict)?;
                Ok(None)
            }
            ActionId::EditCopy | ActionId::EditCut => {
                let t = need(tab)?;
                let text = self.selected_text(t);
                if let Some(text) = &text {
                    fx.push(Effect::ClipboardWrite {
                        text: text.clone(),
                        primary: false,
                    });
                    if action == ActionId::EditCut {
                        edit(self, t, EditCommand::Delete, intent, fx)?;
                    }
                }
                Ok(text.map(Value::String))
            }
            ActionId::EditPaste => {
                let t = need(tab)?;
                let intent = Intent { tab: t, ..intent };
                fx.push(Effect::ClipboardRead {
                    primary: false,
                    intent,
                });
                Ok(None)
            }
            ActionId::EditSelectAll => edit(self, need(tab)?, EditCommand::SelectAll, intent, fx),
            ActionId::EditDuplicateLine => {
                edit(self, need(tab)?, EditCommand::DuplicateLine, intent, fx)
            }
            ActionId::EditDeleteLine => edit(self, need(tab)?, EditCommand::DeleteLine, intent, fx),
            ActionId::EditMoveLineUp => edit(self, need(tab)?, EditCommand::MoveLineUp, intent, fx),
            ActionId::EditMoveLineDown => {
                edit(self, need(tab)?, EditCommand::MoveLineDown, intent, fx)
            }
            ActionId::EditToggleComment => {
                edit(self, need(tab)?, EditCommand::ToggleComment, intent, fx)
            }
            ActionId::EditIndent => edit(self, need(tab)?, EditCommand::Tab, intent, fx),
            ActionId::EditOutdent => edit(self, need(tab)?, EditCommand::Outdent, intent, fx),
            ActionId::EditDeleteWordLeft => {
                edit(self, need(tab)?, EditCommand::DeleteWordLeft, intent, fx)
            }
            ActionId::EditDeleteWordRight => {
                edit(self, need(tab)?, EditCommand::DeleteWordRight, intent, fx)
            }
            ActionId::EditOverwrite => {
                let t = need(tab)?;
                let on = self.tab_mut(t).map(|t| {
                    t.editor.overwrite = !t.editor.overwrite;
                    t.editor.overwrite
                });
                Ok(on.map(Value::Bool))
            }
            ActionId::TabsNext | ActionId::TabsPrev => {
                let n = self.tabs.len();
                if n == 0 {
                    return Err((code::NOT_FOUND, "no tab is open".into()));
                }
                let i = self
                    .active
                    .and_then(|a| self.tabs.iter().position(|t| t.id == a))
                    .unwrap_or(0);
                let j = if action == ActionId::TabsNext {
                    (i + 1) % n
                } else {
                    (i + n - 1) % n
                };
                self.active = Some(self.tabs[j].id);
                self.session_changed(fx);
                Ok(Some(json!({"tab": self.tabs[j].id})))
            }
            ActionId::TabsGoto(k) => {
                let t = self
                    .tabs
                    .get(k as usize - 1)
                    .map(|t| t.id)
                    .ok_or((code::NOT_FOUND, format!("no tab {k}")))?;
                self.active = Some(t);
                self.session_changed(fx);
                Ok(Some(json!({"tab": t})))
            }
            // The rest are the chrome's (dialogs, find bar, zoom, panels):
            // the window handles them before they reach the controller.
            _ => Err((
                code::UNAVAILABLE,
                format!("{} is handled by the window", action.id()),
            )),
        }
    }

    /// Close a tab: `edit.close` (refused `dirty` when this is the last
    /// holder of unsaved text); a detached or failed tab closes locally.
    fn close(
        &mut self,
        tab: TabId,
        force: bool,
        cmd: Option<(u64, String)>,
        intent: Intent,
        fx: &mut Vec<Effect>,
    ) {
        self.cancel_mail_save(tab, "the parked mail save was cancelled by Close", fx);
        if self
            .tab(tab)
            .and_then(|t| t.mail.as_ref())
            .is_some_and(|mail| mail.sending || mail.saving)
        {
            let message = "wait for the mail save or Send to finish before closing";
            fx.push(Effect::Notice {
                tab: Some(tab),
                notice: Notice::Message {
                    level: Level::Warn,
                    text: message.into(),
                },
            });
            if let Some((id, _)) = cmd {
                fx.push(Effect::Respond {
                    id,
                    rc: 10,
                    body: refusal(code::CONFLICT, message, None),
                });
            }
            return;
        }
        let is_mail = self.tab(tab).is_some_and(|t| t.mail.is_some());
        if is_mail && !force && self.tab(tab).is_some_and(Tab::dirty) {
            fx.push(Effect::Prompt(Prompt::CloseDirty { tab, intent }));
            if let Some((id, _)) = cmd {
                fx.push(Effect::Respond {
                    id,
                    rc: 10,
                    body: refusal(code::CONFLICT, "mail document has unsaved changes", None),
                });
            }
            return;
        }
        let force = force || is_mail;
        let buffer = self
            .tab(tab)
            .and_then(|t| t.mirror.as_ref())
            .filter(|m| !matches!(m.phase(), Phase::Detached { .. }))
            .map(|m| m.buffer().to_string());
        match buffer {
            Some(b) => {
                let out = Outgoing {
                    command: "edit.close".into(),
                    body: json!({"buffer": b, "force": force, "origin": UI_ORIGIN}).to_string(),
                    op_id: None,
                    deadline_ms: DEADLINE_MS,
                };
                self.send(out, Req::Close { tab, cmd, intent }, fx);
            }
            None => {
                self.close_tab(tab, fx);
                if let Some((id, action)) = cmd {
                    fx.push(Effect::Respond {
                        id,
                        rc: 0,
                        body: ok_body(&commands::ActionReply {
                            id: action,
                            ok: true,
                            result: None,
                        }),
                    });
                }
            }
        }
    }

    // ── the ced.v1 port ──────────────────────────────────────────────────────

    fn bus_command(&mut self, cmd: &BusCommand, fx: &mut Vec<Effect>) {
        // The declared body limit is enforced before the body is parsed at all,
        // not after a full DOM parse of up to the Bus's 16 MiB.
        if cmd.command == "ced.diagnostics" && cmd.body.len() > commands::MAX_DIAGNOSTICS_BODY {
            let msg = format!(
                "the request is {} bytes (at most {})",
                cmd.body.len(),
                commands::MAX_DIAGNOSTICS_BODY
            );
            return fx.push(Effect::Respond {
                id: cmd.id,
                rc: 10,
                body: refusal(code::INVALID_ARGUMENT, msg, Some("too_large")),
            });
        }
        let body: Value = serde_json::from_str(&cmd.body).unwrap_or_else(|_| json!({}));
        macro_rules! parse {
            ($t:ty) => {
                parse!($t, body.clone())
            };
            ($t:ty, $value:expr) => {
                match serde_json::from_value::<$t>($value) {
                    Ok(v) => v,
                    Err(e) => {
                        return fx.push(Effect::Respond {
                            id: cmd.id,
                            rc: 10,
                            body: refusal(
                                code::INVALID_ARGUMENT,
                                format!("{}: {e}", cmd.command),
                                Some("bad_args"),
                            ),
                        });
                    }
                }
            };
        }
        let id = cmd.id;
        let reply =
            |fx: &mut Vec<Effect>, body: String| fx.push(Effect::Respond { id, rc: 0, body });
        let refuse = |fx: &mut Vec<Effect>, code: &str, msg: String, reason: Option<&str>| {
            fx.push(Effect::Respond {
                id,
                rc: 10,
                body: refusal(code, msg, reason),
            })
        };
        match cmd.command.as_str() {
            "ced.ping" => reply(
                fx,
                ok_body(&commands::PingReply {
                    pong: true,
                    service: commands::SERVICE.into(),
                    schema: commands::SCHEMA.into(),
                    pid: std::process::id(),
                    headless: self.headless,
                }),
            ),
            "ced.info" => {
                let info = buildinfo::build_info!();
                reply(
                    fx,
                    ok_body(&commands::InfoReply {
                        version: info.version.into(),
                        git_sha: info.git_sha.into(),
                        build_time: info.build_time.into(),
                        headless: self.headless,
                        tabs: self.tabs.len(),
                        edit: self.edit_info.clone(),
                        config_path: self.config_path.clone(),
                        session_path: self.session_path.clone(),
                    }),
                )
            }
            "ced.open" => {
                let r = parse!(commands::OpenReq);
                if r.paths.is_empty() {
                    return refuse(
                        fx,
                        code::INVALID_ARGUMENT,
                        "ced.open needs at least one path".into(),
                        Some("bad_args"),
                    );
                }
                let goto = r.line.map(|l| (l, r.col.unwrap_or(1)));
                self.open_many(&r.paths, Some((id, goto)), fx);
            }
            "ced.new" => {
                let t = self.new_tab(None);
                self.send_open(t, None, false, fx);
                self.active = Some(t);
                self.opens.push(PendingOpen {
                    cmd: id,
                    tabs: vec![t],
                    new: true,
                });
                self.session_changed(fx);
            }
            "ced.tabs" => {
                let tabs = self.tabs.iter().map(|t| self.tab_row(t)).collect();
                reply(
                    fx,
                    ok_body(&commands::TabsReply {
                        active: self.active,
                        tabs,
                    }),
                );
            }
            "ced.focus" => {
                let sel = parse!(commands::TabSel);
                match self.resolve_tab(&sel) {
                    Ok(t) => {
                        self.active = Some(t);
                        if let Some(x) = self.x.get_mut(&t) {
                            x.agent_since_focus = false;
                        }
                        self.session_changed(fx);
                        reply(fx, ok_body(&commands::FocusReply { tab: t }));
                    }
                    Err(e) => refuse(fx, code::NOT_FOUND, e, None),
                }
            }
            "ced.state" => {
                let r = parse!(commands::StateReq);
                match self
                    .resolve_tab(&r.sel)
                    .map(|t| self.state_reply(t, r.text))
                {
                    Ok(Ok(s)) => reply(fx, ok_body(&s)),
                    Ok(Err((c, m))) => refuse(fx, c, m, None),
                    Err(e) => refuse(fx, code::NOT_FOUND, e, None),
                }
            }
            "ced.type" => {
                let r = parse!(commands::TypeReq);
                let t = match self.resolve_tab(&r.sel) {
                    Ok(t) => t,
                    Err(e) => return refuse(fx, code::NOT_FOUND, e, None),
                };
                match self.command(
                    t,
                    EditCommand::Insert(r.text),
                    Intent::bus(t, &cmd.caller_key),
                    fx,
                ) {
                    Ok(()) => {
                        let pending = self
                            .tab(t)
                            .and_then(|t| t.mirror.as_ref())
                            .map_or(0, Mirror::pending);
                        reply(fx, ok_body(&commands::TypeReply { tab: t, pending }));
                    }
                    Err(e) => refuse(fx, code::CONFLICT, e, None),
                }
            }
            "ced.select" => {
                let r = parse!(commands::SelectReq);
                let t = match self.resolve_tab(&r.sel) {
                    Ok(t) => t,
                    Err(e) => return refuse(fx, code::NOT_FOUND, e, None),
                };
                let res = self
                    .tab(t)
                    .and_then(|t| t.mirror.as_ref())
                    .ok_or("the buffer is not open".to_string())
                    .and_then(|m| {
                        let a = resolve_pos(m.text(), &r.anchor)?;
                        let h = resolve_pos(m.text(), &r.head)?;
                        Ok((a, h, m.text().point(a), m.text().point(h)))
                    });
                match res {
                    Ok((a, h, pa, ph)) => {
                        if let Some(t) = self.tab_mut(t) {
                            t.editor.sel = Selection { anchor: a, head: h };
                            t.editor.preferred_cells = None;
                        }
                        self.caret_moved(t, fx);
                        reply(
                            fx,
                            ok_body(&commands::SelectReply {
                                selection: commands::SelectionP {
                                    anchor: pa,
                                    head: ph,
                                },
                            }),
                        );
                    }
                    Err(e) => refuse(fx, code::INVALID_ARGUMENT, e, Some("bad_position")),
                }
            }
            "ced.execute" => {
                let r = parse!(action::ExecuteRequest);
                // The whole body is checked before the id is looked up, and a
                // misspelt key in `params` is refused rather than ignored
                // (serde's flatten cannot deny unknown fields).
                let raw = r.params.clone().unwrap_or_else(|| json!({}));
                if let Some(key) = raw.as_object().and_then(|m| {
                    m.keys()
                        .find(|k| !matches!(k.as_str(), "tab" | "buffer" | "args"))
                }) {
                    return refuse(
                        fx,
                        code::INVALID_ARGUMENT,
                        format!("unknown params key {key}"),
                        Some("unknown_param"),
                    );
                }
                let params = parse!(commands::ExecuteParams, raw);
                let Some(action) = ActionId::from_id(&r.id) else {
                    return refuse(
                        fx,
                        code::UNKNOWN_ACTION,
                        format!("unknown action {}", r.id),
                        Some("unknown_action"),
                    );
                };
                let tab = match (params.sel.tab, &params.sel.buffer) {
                    (None, None) => self.active,
                    _ => match self.resolve_tab(&params.sel) {
                        Ok(t) => Some(t),
                        Err(e) => return refuse(fx, code::NOT_FOUND, e, None),
                    },
                };
                let intent = Intent::bus(tab.unwrap_or(0), &cmd.caller_key);
                if !self.headless && ui::window_only(action, params.args.as_ref()) {
                    // Full mesh access: the window performs it, and the
                    // reply waits for the window's `ui_done`.
                    return self.ui_dispatch(
                        tab,
                        action,
                        params.args.clone(),
                        intent,
                        Some((id, r.id.clone())),
                        fx,
                    );
                }
                let arg = |k: &str| {
                    params.args
                        .as_ref()
                        .and_then(|a| a.get(k))
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                };
                if action == ActionId::FileClose && !arg("save") {
                    match tab {
                        Some(t) => {
                            fx.push(Effect::Executed { id: action.id() });
                            self.close(t, arg("force"), Some((id, r.id.clone())), intent, fx)
                        }
                        None => refuse(fx, code::NOT_FOUND, "no tab is open".into(), None),
                    }
                    return;
                }
                // One server op each: answered from its outcome.
                // `file.close {save:true}` answers when the save completes;
                // the close follows.
                let on_outcome = matches!(
                    action,
                    ActionId::FileSave
                        | ActionId::FileSaveAs
                        | ActionId::FileSend
                        | ActionId::FileReload
                        | ActionId::FileClose
                        | ActionId::EditUndo
                        | ActionId::EditRedo
                        | ActionId::EditUndoAny
                        | ActionId::EditUndoOther
                );
                self.op_cmd = on_outcome.then(|| (id, r.id.clone()));
                let result = self.action_args(tab, action, params.args.as_ref(), intent, fx);
                if result.is_ok() {
                    fx.push(Effect::Executed { id: action.id() });
                }
                let queued = on_outcome && self.op_cmd.take().is_none();
                match result {
                    Ok(_) if queued => {}
                    Ok(result) => reply(
                        fx,
                        ok_body(&commands::ActionReply {
                            id: r.id,
                            ok: true,
                            result,
                        }),
                    ),
                    Err((c, m)) => {
                        self.op_waits.retain(|_, w| w.id != id);
                        refuse(fx, c, m, None)
                    }
                }
            }
            "ced.actions" => {
                let actions = ActionId::all()
                    .into_iter()
                    .map(|a| commands::ActionRow {
                        id: a.id(),
                        label: a.label(),
                        menu: a.menu().label().to_string(),
                        keys: match self.keys.as_ref().and_then(|k| k.get(&a.id())) {
                            Some(keys) => keys.clone(),
                            None => crate::keymap::chords_for(a)
                                .into_iter()
                                .map(str::to_string)
                                .collect(),
                        },
                        enabled: if a == ActionId::FileSend { self.active.is_some_and(|tab| self.can_send(tab)) } else if a == ActionId::MailHeaders { self.active.and_then(|tab| self.tab(tab)).and_then(|tab| tab.mail.as_ref()).is_some_and(|mail| !mail.sending && !mail.saving) } else { self.action_enabled.get(&a).copied().unwrap_or(!matches!(
                            a,
                            ActionId::ViewMarkdownSource
                                | ActionId::ViewMarkdownSplit
                                | ActionId::ViewMarkdownPreview
                        )) },
                    })
                    .collect();
                reply(fx, ok_body(&commands::ActionsReply { actions }));
            }
            "ced.wait" => {
                let r = parse!(commands::WaitReq);
                self.register_waiter(id, r, fx);
            }
            "ced.layout" => {
                if self.headless {
                    return refuse(
                        fx,
                        code::UNAVAILABLE,
                        "ced.layout needs a window (this instance is --headless)".into(),
                        Some("headless"),
                    );
                }
                let sel = parse!(commands::TabSel);
                let t = match self.resolve_tab(&sel) {
                    Ok(t) => t,
                    Err(e) => return refuse(fx, code::NOT_FOUND, e, None),
                };
                if let Some(l) = self.layout.clone().filter(|l| l.tab == t) {
                    return reply(fx, ok_body(&l));
                }
                match self.x.get(&t).and_then(|x| x.layout) {
                    Some(l) => {
                        let rect = |r: [f32; 4]| commands::Rect {
                            x: r[0],
                            y: r[1],
                            w: r[2],
                            h: r[3],
                        };
                        let zero = commands::Rect {
                            x: 0.0,
                            y: 0.0,
                            w: 0.0,
                            h: 0.0,
                        };
                        reply(
                            fx,
                            ok_body(&commands::LayoutReply {
                                tab: t,
                                window: zero,
                                menubar: zero,
                                tabstrip: zero,
                                editor: rect(l.editor),
                                gutter_w: l.gutter_w,
                                line_height: l.line_height,
                                cell_w: l.cell_w,
                                first_line: l.first_line,
                                visible_rows: l.visible_rows,
                                caret: rect(l.caret),
                                statusbar: zero,
                            }),
                        )
                    }
                    None => refuse(
                        fx,
                        code::UNAVAILABLE,
                        "no frame has been laid out for this tab yet".into(),
                        Some("no_frame"),
                    ),
                }
            }
            "ced.stats" => {
                let s = &self.stats;
                reply(
                    fx,
                    ok_body(&commands::StatsReply {
                        keys: s.keys,
                        frames: self.frames.count,
                        model_us: ui::percentiles(&self.frames.model_us),
                        view_us: ui::percentiles(&self.frames.view_us),
                        next_frame_us: ui::percentiles(&self.frames.next_frame_us),
                        events: s.events,
                        history_recoveries: s.history_recoveries,
                        snapshot_recoveries: s.snapshot_recoveries,
                        conflicts: s.conflicts,
                        retries: s.retries,
                        uncertain: s.uncertain,
                    }),
                )
            }
            "app.describe" => {
                let view = self
                    .active
                    .and_then(|a| self.tab(a))
                    .map(|t| self.tab_name(t))
                    .unwrap_or_default();
                reply(
                    fx,
                    ok_body(&commands::DescribeReply {
                        contract: "ctk-app-control.v0".into(),
                        app: "ced".into(),
                        title: "MixOS Editor".into(),
                        view,
                        engine: "egui".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                        description: "The MixOS Editor: an egui editor over the edit Bus service"
                            .into(),
                        controls: Vec::new(),
                        commands: commands::COMMANDS
                            .iter()
                            .map(|(v, _)| v.to_string())
                            .collect(),
                    }),
                )
            }
            "app.quit" => {
                self.cancel_parked_mail_saves(fx);
                if let Some(message) = self.mail_exit_refusal() {
                    return refuse(fx, code::CONFLICT, message.into(), None);
                }
                reply(fx, ok_body(&commands::QuitReply { quitting: true }));
                fx.push(Effect::SaveSession);
                self.close_mail_buffers(fx);
                fx.push(Effect::Quit);
            }
            "ced.diagnostics" => {
                let r = parse!(commands::DiagnosticsReq);
                if let Err((msg, reason)) = external::check(&r, cmd.body.len()) {
                    return refuse(fx, code::INVALID_ARGUMENT, msg, Some(reason));
                }
                self.external.put(&r);
                let mut out = commands::DiagnosticsReply {
                    path: r.path.clone(),
                    tabs: Vec::new(),
                    shown: 0,
                    stale: false,
                };
                for t in &mut self.tabs {
                    if external::tab_path(t) != Some(r.path.as_str()) {
                        continue;
                    }
                    out.tabs.push(t.id);
                    let applied = external::apply(&self.external, t, Some(&r.source), true);
                    out.shown += applied.shown;
                    out.stale |= applied.stale;
                }
                reply(fx, ok_body(&out));
            }
            "ced.problems" => {
                let sel = parse!(commands::TabSel);
                let t = match self.resolve_tab(&sel) {
                    Ok(t) => t,
                    Err(e) => return refuse(fx, code::NOT_FOUND, e, None),
                };
                let Some(tab) = self.tab(t) else {
                    return refuse(fx, code::NOT_FOUND, format!("no tab {t}"), None);
                };
                let col_of = |offset: usize| {
                    tab.mirror
                        .as_ref()
                        .map_or(1, |m| m.text().point(offset).col)
                };
                let problems = tab
                    .diagnostics
                    .items()
                    .iter()
                    .map(|d| commands::ProblemRow {
                        line: d.line,
                        col: col_of(d.range.start),
                        severity: match d.severity {
                            Severity::Error => commands::DiagSeverity::Error,
                            Severity::Warning => commands::DiagSeverity::Warning,
                            Severity::Note => commands::DiagSeverity::Note,
                        },
                        code: d.code.clone(),
                        message: d.message.clone(),
                        source: d.source.clone(),
                    })
                    .collect();
                let path = external::tab_path(tab).map(str::to_owned);
                reply(
                    fx,
                    ok_body(&commands::ProblemsReply {
                        tab: t,
                        path,
                        problems,
                    }),
                );
            }
            "HELP" => reply(fx, json!(commands::manifest()).to_string()),
            "INFO" => reply(fx, json!({"service":commands::SERVICE,"schema":commands::SCHEMA,"commands":commands::manifest()}).to_string()),
            other => refuse(
                fx,
                code::UNKNOWN_COMMAND,
                format!("unknown command {other}"),
                None,
            ),
        }
    }

    fn tab_name(&self, t: &Tab) -> String {
        if t.mail.is_some()
            && let Some(path) = &t.path
        {
            return path.rsplit('/').next().unwrap_or(path).to_owned();
        }
        t.mirror
            .as_ref()
            .and_then(|m| m.meta().name.clone())
            .or_else(|| {
                t.path
                    .as_ref()
                    .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            })
            .unwrap_or_else(|| format!("untitled-{}", t.id))
    }

    fn tab_row(&self, t: &Tab) -> commands::TabRow {
        let m = t.mirror.as_ref();
        commands::TabRow {
            tab: t.id,
            buffer: m.map(|m| m.buffer().to_string()),
            epoch: m.map(|m| m.epoch().to_string()),
            path: t.path.clone(),
            name: self.tab_name(t),
            language: t.language().into(),
            preview: self.view_hint(t.id, "preview").unwrap_or("source").into(),
            rev: m.map_or(0, Mirror::rev),
            dirty: t.dirty(),
            disk: m.map_or_else(|| "none".into(), |m| disk_str(m.meta().disk)),
            pending: m.map_or(0, Mirror::pending),
            conflicts: m.map_or(0, |m| m.conflicts().len()),
            recovered: m.is_some_and(|m| m.meta().recovered),
            phase: self.phase_of(t.id),
        }
    }

    fn phase_of(&self, tab: TabId) -> PhaseW {
        let failed = self.x.get(&tab).is_some_and(|x| x.open_error.is_some());
        phase_w(self.tab(tab).and_then(|t| t.mirror.as_ref()), failed)
    }

    fn state_reply(
        &self,
        tab: TabId,
        with_text: bool,
    ) -> Result<commands::StateReply, (&'static str, String)> {
        let t = self
            .tab(tab)
            .ok_or((code::NOT_FOUND, format!("no tab {tab}")))?;
        let Some(m) = t.mirror.as_ref() else {
            let msg = self
                .x
                .get(&tab)
                .and_then(|x| x.open_error.clone())
                .unwrap_or_else(|| "the buffer is still opening".into());
            return Err((code::UNAVAILABLE, msg));
        };
        let text = text_string(m.text());
        if with_text && text.len() > STATE_TEXT_MAX {
            return Err((
                code::INVALID_ARGUMENT,
                format!(
                    "the text is {} bytes (ced.state inlines at most {STATE_TEXT_MAX})",
                    text.len()
                ),
            ));
        }
        let len = m.text().len();
        let sel = t.editor.sel;
        Ok(commands::StateReply {
            tab,
            preview: self.view_hint(tab, "preview").unwrap_or("source").into(),
            buffer: Some(m.buffer().to_string()),
            rev: m.rev(),
            view_gen: m.view_gen(),
            phase: self.phase_of(tab),
            pending: m.pending(),
            inflight: m.inflight().is_some(),
            text_hash: blake3::hash(text.as_bytes()).to_hex().to_string(),
            bytes: len,
            lines: m.text().line_count(),
            selection: commands::SelectionP {
                anchor: m.text().point(sel.anchor.min(len)),
                head: m.text().point(sel.head.min(len)),
            },
            first_line: t.editor.scroll.first_line.max(1),
            last_remote: m.last_remote().map(|r| commands::LastRemote {
                origin: r.origin.clone(),
                lane: r.lane.clone(),
                rev: r.rev,
                kind: kind_str(r.kind),
            }),
            conflicts: m
                .conflicts()
                .iter()
                .map(|c| commands::ConflictRow {
                    rev: c.rev,
                    remote_origin: c.remote_origin.clone(),
                    lines: [c.lines.0, c.lines.1],
                    texts: c.texts.clone(),
                })
                .collect(),
            detached_copy: m.detached_copy().is_some(),
            mail: t.mail.as_ref().map(|mail| json!({"headers":mail.headers,"sending":mail.sending,"saving":mail.saving,"notice":mail.notice})),
            document_notice: t.document_notice.clone(),
            text: with_text.then_some(text),
        })
    }

    // ── ced.wait: event-driven, never a loop ─────────────────────────────────

    fn register_waiter(&mut self, cmd: u64, r: commands::WaitReq, fx: &mut Vec<Effect>) {
        let refuse = |fx: &mut Vec<Effect>, code: &str, msg: String| {
            fx.push(Effect::Respond {
                id: cmd,
                rc: 10,
                body: refusal(code, msg, Some("bad_args")),
            })
        };
        let conds = [
            r.rev.map(WaitCond::Rev),
            r.idle.filter(|i| *i).map(|_| WaitCond::Idle),
            r.epoch.clone().map(WaitCond::Epoch),
            r.phase.map(WaitCond::Phase),
        ];
        let mut given = conds.into_iter().flatten();
        let (Some(cond), None) = (given.next(), given.next()) else {
            return refuse(
                fx,
                code::INVALID_ARGUMENT,
                "ced.wait takes exactly one of rev / idle / epoch / phase".into(),
            );
        };
        if r.timeout_ms == 0 || r.timeout_ms > WAIT_MAX_MS {
            return refuse(
                fx,
                code::INVALID_ARGUMENT,
                format!("timeout_ms must be 1..={WAIT_MAX_MS}"),
            );
        }
        let tab = match self.resolve_tab(&r.sel) {
            Ok(t) => t,
            Err(e) => {
                return fx.push(Effect::Respond {
                    id: cmd,
                    rc: 10,
                    body: refusal(code::NOT_FOUND, e, None),
                });
            }
        };
        self.next_waiter += 1;
        let w = Waiter {
            id: self.next_waiter,
            cmd,
            tab,
            cond,
            since: Instant::now(),
            timeout_ms: r.timeout_ms,
        };
        let wid = w.id;
        self.waiters.push(w);
        // Evaluated at registration; replies at once when it already holds.
        self.eval_waiters(fx);
        if self.waiters.iter().any(|w| w.id == wid) {
            self.timer(r.timeout_ms, TimerFor::Wait(wid), fx);
        }
    }

    fn waiter_holds(&self, w: &Waiter) -> bool {
        let m = self.tab(w.tab).and_then(|t| t.mirror.as_ref());
        match &w.cond {
            WaitCond::Rev(r) => {
                m.is_some_and(|m| m.rev() >= *r && matches!(m.phase(), Phase::Live))
            }
            WaitCond::Idle => m.is_some_and(Mirror::is_idle),
            WaitCond::Epoch(e) => m.is_some_and(|m| m.epoch() == e),
            WaitCond::Phase(p) => {
                let opened = self.x.get(&w.tab).is_some_and(|x| x.opened);
                (m.is_some() || opened) && self.phase_of(w.tab) == *p
            }
        }
    }

    /// Re-apply stored external diagnostic sets to every tab that, since the
    /// last transition, went live (opened or reattached), Resynced, or went
    /// clean (`dirty` false: a save, or an undo back to the disk state — the
    /// only time a stale set's digest can start matching again). Checks two
    /// flags per tab; hashes only on those edges, never per keystroke.
    fn sync_external(&mut self) {
        for t in &mut self.tabs {
            let Some(x) = self.x.get_mut(&t.id) else {
                continue;
            };
            let live = t
                .mirror
                .as_ref()
                .is_some_and(|m| matches!(m.phase(), Phase::Live));
            let dirty = t.mirror.as_ref().is_some_and(|m| m.meta().dirty);
            let went_live = live && !x.ext_live;
            let went_clean = x.ext_dirty && !dirty;
            x.ext_live = live;
            x.ext_dirty = dirty;
            if live && (x.ext_due || went_live || went_clean) {
                let undigested = x.ext_due || went_live;
                x.ext_due = false;
                external::apply(&self.external, t, None, undigested);
            }
        }
    }

    /// Re-evaluate every waiter (after every controller transition).
    fn eval_waiters(&mut self, fx: &mut Vec<Effect>) {
        self.sync_external();
        let ready: Vec<u64> = self
            .waiters
            .iter()
            .filter(|w| self.waiter_holds(w))
            .map(|w| w.id)
            .collect();
        for w in ready {
            self.finish_waiter(w, Ok(()), fx);
        }
    }

    fn finish_waiter(
        &mut self,
        wid: u64,
        outcome: Result<(), (&str, String)>,
        fx: &mut Vec<Effect>,
    ) {
        let Some(i) = self.waiters.iter().position(|w| w.id == wid) else {
            return;
        };
        let w = self.waiters.remove(i);
        self.timers
            .retain(|_, t| !matches!(t, TimerFor::Wait(x) if *x == wid));
        let body = match outcome {
            Ok(()) => {
                let m = self.tab(w.tab).and_then(|t| t.mirror.as_ref());
                ok_body(&commands::WaitReply {
                    tab: w.tab,
                    rev: m.map_or(0, Mirror::rev),
                    phase: self.phase_of(w.tab),
                    epoch: m.map(|m| m.epoch().to_string()),
                    waited_ms: w.since.elapsed().as_millis() as u64,
                })
            }
            Err((reason, msg)) => {
                return fx.push(Effect::Respond {
                    id: w.cmd,
                    rc: 10,
                    body: refusal(code::CONFLICT, msg, Some(reason)),
                });
            }
        };
        fx.push(Effect::Respond {
            id: w.cmd,
            rc: 0,
            body,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_edit_fails_fast_and_registration_retries_unattached_tabs() {
        let mut c = ctl();
        let effects = c.open_paths(&["/work/hello.mix".into()], Intent::ui(0));
        let (req, budget) = effects
            .iter()
            .find_map(|fx| match fx {
                Effect::Send { req, out } if out.command == "edit.open" => {
                    Some((*req, out.deadline_ms))
                }
                _ => None,
            })
            .unwrap();
        assert!(budget <= 3000);
        let tab = c.tabs()[0].id;
        let effects = c.on_incoming(Incoming::Reply {
            req,
            rc: 10,
            body: r#"{"error_code":"UNAVAILABLE","message":""}"#.into(),
        });
        assert_eq!(c.open_error(tab), Some(EDIT_UNAVAILABLE));
        assert!(effects.iter().any(|fx| matches!(fx, Effect::Notice { notice: Notice::Message { text, .. }, .. } if text == EDIT_UNAVAILABLE)));
        assert!(
            !effects.iter().any(|fx| matches!(fx, Effect::Send { .. })),
            "wait for registration without polling"
        );
        let effects = c.on_incoming(Incoming::Topic {
            topic: "noded.props.changed".into(),
            body: r#"{"path":"services.registered","old":[],"new":["edit"]}"#.into(),
        });
        assert!(
            effects
                .iter()
                .any(|fx| matches!(fx, Effect::Send { out, .. } if out.command == "edit.open"))
        );
    }

    fn registered(c: &mut Controller, services: Value) -> Vec<Effect> {
        c.on_incoming(Incoming::Topic {
            topic: "noded.props.changed".into(),
            body: json!({"path":"services.registered", "new": services}).to_string(),
        })
    }

    #[test]
    fn scratch_list_absence_backs_off_then_registration_attaches() {
        let mut c = ctl();
        let tab = c.new_tab(None);
        let mut fx = Vec::new();
        c.reattach(tab, &mut fx);
        for (attempt, budget) in [3000, 6000, 12000, 30000, 30000].into_iter().enumerate() {
            let (req, _) = sent(&fx, "edit.list");
            assert!(fx.iter().any(|e| matches!(e, Effect::Send { out, .. }
                if out.command == "edit.list" && out.deadline_ms == budget)));
            fx = c.on_incoming(Incoming::Deadline { req });
            assert!(c.open_slow(tab));
            assert_eq!(c.open_error(tab), None);
            assert_eq!(c.x[&tab].open_misses, attempt as u32 + 1);
            assert_eq!(
                fx.iter()
                    .filter(|e| matches!(e, Effect::Notice { .. }))
                    .count(),
                usize::from(attempt == 0)
            );
        }
        let (req, _) = sent(&fx, "edit.list");
        fx = c.on_incoming(Incoming::Reply {
            req,
            rc: 10,
            body: refusal("UNAVAILABLE", "missing", None),
        });
        assert_eq!(c.open_error(tab), Some(EDIT_UNAVAILABLE));
        assert!(!c.open_slow(tab));
        assert!(!c.x[&tab].reattaching);
        assert!(
            !fx.iter()
                .any(|e| matches!(e, Effect::Send { .. } | Effect::Timer { .. }))
        );
        fx = registered(&mut c, json!(["edit"]));
        let (req, _) = sent(&fx, "edit.list");
        assert_eq!(c.open_error(tab), None);
        fx = reply(&mut c, req, json!({"epoch":"0000e1e1", "buffers":[]}));
        let (req, _) = sent(&fx, "edit.open");
        let mut open = test_open();
        open["path"] = Value::Null;
        reply(&mut c, req, open);
        assert!(c.tab(tab).unwrap().mirror.is_some());
        assert!(c.tab(tab).unwrap().path.is_none());
        assert!(!c.open_slow(tab));
        assert_eq!(c.x[&tab].open_misses, 0);
    }

    fn test_open() -> Value {
        json!({"buffer":"b1_0000e1e1", "epoch":"0000e1e1", "path":"/work/hello.mix", "opened_as":null,
            "name":"hello.mix", "language":"text", "rev":0, "lines":1, "bytes":0, "eol":"lf", "bom":false,
            "disk":"clean", "reopened":false, "created":false, "recovery_id":"5f0c2a9e1b7d4c33",
            "recovered":false, "recovered_from":null})
    }

    #[test]
    fn slow_open_retries_with_capped_budgets_and_one_warning() {
        let mut c = ctl();
        let mut fx = c.open_paths(&["/work/hello.mix".into()], Intent::ui(0));
        let tab = c.tabs()[0].id;
        for (attempt, budget) in [3000, 6000, 12000, 30000, 30000].into_iter().enumerate() {
            let (req, _) = sent(&fx, "edit.open");
            assert!(fx.iter().any(|e| matches!(e, Effect::Send { out, .. } if out.command == "edit.open" && out.deadline_ms == budget)));
            fx = c.on_incoming(Incoming::Deadline { req });
            assert_eq!(c.open_error(tab), None);
            assert!(c.open_slow(tab));
            assert!(!c.x[&tab].reattaching);
            assert_eq!(
                fx.iter()
                    .filter(|e| matches!(e, Effect::Notice { .. }))
                    .count(),
                usize::from(attempt == 0)
            );
        }
        let (req, _) = sent(&fx, "edit.open");
        reply(&mut c, req, test_open());
        assert!(c.tab(tab).unwrap().mirror.is_some());
        assert!(!c.open_slow(tab));
    }

    #[test]
    fn repeated_failure_registration_and_attachment_clear_retry_state() {
        let mut c = ctl();
        let mut fx = c.open_paths(&["/work/hello.mix".into()], Intent::ui(0));
        let tab = c.tabs()[0].id;
        for _ in 0..2 {
            let (req, _) = sent(&fx, "edit.open");
            c.on_incoming(Incoming::Reply {
                req,
                rc: 10,
                body: refusal("UNAVAILABLE", "missing", None),
            });
            assert_eq!(c.open_error(tab), Some(EDIT_UNAVAILABLE));
            assert!(!c.x[&tab].reattaching);
            fx = registered(&mut c, json!(["edit"]));
            assert_eq!(c.open_error(tab), None);
        }
        let (req, _) = sent(&fx, "edit.open");
        reply(&mut c, req, test_open());
        assert!(c.tab(tab).unwrap().mirror.is_some());
        assert!(!c.x[&tab].reattaching);
    }

    #[test]
    fn registrations_only_act_on_edit_appearing() {
        let mut existing = ctl();
        let fx = existing.on_incoming(Incoming::Topic {
            topic: "noded.props.changed".into(),
            body: json!({"path":"services.registered", "old":["edit"], "new":["edit", "other"]})
                .to_string(),
        });
        assert!(
            fx.is_empty(),
            "the first event may be an unrelated registration"
        );
        let mut c = ctl();
        assert!(registered(&mut c, json!(["other"])).is_empty());
        assert!(!registered(&mut c, json!(["edit"])).is_empty());
        assert!(registered(&mut c, json!(["edit", "other"])).is_empty());
        assert!(
            c.on_incoming(Incoming::Topic {
                topic: "noded.props.changed".into(),
                body: r#"{"path":"other","new":["edit"],"note":"services.registered"}"#.into()
            })
            .is_empty()
        );
        assert!(
            c.on_incoming(Incoming::Topic {
                topic: "noded.props.changed".into(),
                body: "invalid services.registered edit".into()
            })
            .is_empty()
        );
        assert!(registered(&mut c, json!([])).is_empty());
        let fx = registered(&mut c, json!(["edit"]));
        sent(&fx, "edit.info");
    }

    #[test]
    fn unavailable_reattach_preserves_mirror_and_retries() {
        let mut c = ctl();
        let fx = c.open_paths(&["/work/hello.mix".into()], Intent::ui(0));
        let (req, _) = sent(&fx, "edit.open");
        reply(&mut c, req, test_open());
        let tab = c.tabs()[0].id;
        let mut fx = Vec::new();
        c.reattach(tab, &mut fx);
        let (req, _) = sent(&fx, "edit.open");
        let fx = c.on_incoming(Incoming::Deadline { req });
        assert!(c.x[&tab].reattaching, "the renewed request is in flight");
        let (req, _) = sent(&fx, "edit.open");
        let fx = c.on_incoming(Incoming::Reply {
            req,
            rc: 10,
            body: refusal("UNAVAILABLE", "missing", None),
        });
        assert!(c.tab(tab).unwrap().mirror.is_some());
        assert_eq!(c.open_error(tab), None);
        assert!(!c.x[&tab].reattaching);
        let timer = fx
            .iter()
            .find_map(|e| match e {
                Effect::Timer { id, .. } => Some(*id),
                _ => None,
            })
            .unwrap();
        let fx = c.on_incoming(Incoming::Timer { id: timer });
        let (req, _) = sent(&fx, "edit.open");
        reply(&mut c, req, test_open());
        assert!(!c.x[&tab].reattaching);
    }

    #[test]
    fn path_line_col_suffixes() {
        assert_eq!(
            split_line_col("/a/b.mix:12:3"),
            ("/a/b.mix".into(), Some((12, 3)))
        );
        assert_eq!(
            split_line_col("/a/b.mix:12"),
            ("/a/b.mix".into(), Some((12, 1)))
        );
        assert_eq!(split_line_col("/a/b.mix"), ("/a/b.mix".into(), None));
        assert_eq!(split_line_col("/a/b:c.mix"), ("/a/b:c.mix".into(), None));
        assert_eq!(
            split_line_col("/a/b:c.mix:7"),
            ("/a/b:c.mix".into(), Some((7, 1)))
        );
    }

    #[test]
    fn line_col_uses_editd_columns() {
        let t = Text::from_text("ab\r\né x\n").unwrap();
        assert_eq!(offset_of_line_col(&t, 1, 1), Ok(0));
        assert_eq!(offset_of_line_col(&t, 1, 4), Ok(3)); // after the \r
        assert!(offset_of_line_col(&t, 1, 5).is_err());
        assert_eq!(offset_of_line_col(&t, 2, 2), Ok(6)); // é is 2 bytes
        assert_eq!(resolve_pos(&t, &PosSpec::Named(NamedPos::End)), Ok(t.len()));
        assert!(resolve_pos(&t, &PosSpec::Offset(5)).is_err()); // inside é
    }

    #[test]
    fn refusals_have_the_fixture_shape() {
        let v: Value = serde_json::from_str(&refusal(
            code::CONFLICT,
            "ced.wait timed out after 5000 ms",
            Some("timeout"),
        ))
        .unwrap();
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/commands/refusal.timeout.json"
        ))
        .unwrap();
        assert_eq!(v, fixture);
    }

    fn ctl() -> Controller {
        Controller::new(Config::default(), 1, true)
    }

    fn cmd(command_name: &str, body: Value) -> BusCommand {
        BusCommand {
            id: 7,
            command: command_name.into(),
            body: body.to_string(),
            caller_key: "local:tester".into(),
        }
    }

    fn response(fx: &[Effect]) -> (u8, Value) {
        fx.iter()
            .find_map(|e| match e {
                Effect::Respond { rc, body, .. } => {
                    Some((*rc, serde_json::from_str(body).unwrap()))
                }
                _ => None,
            })
            .expect("a response")
    }

    #[test]
    fn shared_primary_selection_is_written_by_the_host_without_editing() {
        let mut c = ctl();
        let effects = c.on_editor(1, EditorMsg::PrimarySelection("e\u{301}中".into()));
        assert!(
            matches!(effects.as_slice(), [Effect::ClipboardWrite { text, primary: true }] if text == "e\u{301}中")
        );
        assert_eq!(c.stats.keys, 0);
    }

    #[test]
    fn help_is_a_descriptor_list_of_answered_commands() {
        let (rc, body) = response(&ctl().on_bus_command(cmd("HELP", json!({}))));
        assert_eq!(rc, 0);
        let entries: Vec<bus::CommandDescriptor> = serde_json::from_value(body).unwrap();
        assert_eq!(entries[0].name, "HELP");
        for entry in entries {
            let effects = ctl().on_bus_command(cmd(&entry.name, json!({})));
            assert!(
                !effects.is_empty(),
                "{} was not answered or admitted",
                entry.name
            );
            for effect in effects {
                if let Effect::Respond { body, .. } = effect {
                    let body: Value = serde_json::from_str(&body).unwrap();
                    assert_ne!(body["error_code"], "UNKNOWN_COMMAND", "{}", entry.name);
                }
            }
        }
    }

    #[test]
    fn info_keeps_the_service_schema_and_commands() {
        let (rc, body) = response(&ctl().on_bus_command(cmd("INFO", json!({}))));
        assert_eq!(rc, 0);
        assert_eq!(body["service"], "ced");
        assert_eq!(body["schema"], "ced.v1");
        assert_eq!(body["commands"], json!(commands::manifest()));
        assert!(
            commands::manifest()
                .iter()
                .any(|entry| entry.name == "INFO" && entry.read_only)
        );
    }

    #[test]
    fn help_covers_dispatcher_arms() {
        let entries = commands::manifest();
        let source = include_str!("controller.rs");
        let dispatcher = source.split("match cmd.command.as_str() {").nth(1).unwrap();
        let dispatcher = dispatcher.split("fn tab_name").next().unwrap();
        for line in dispatcher.lines() {
            let line = line.trim();
            if line.starts_with('"') && line.contains("=>") {
                for name in line.split('"').skip(1).step_by(2) {
                    if name.starts_with("ced.")
                        || name.starts_with("app.")
                        || matches!(name, "INFO" | "HELP")
                    {
                        assert!(entries.iter().any(|entry| entry.name == name), "{name}");
                    }
                }
            }
        }
    }

    #[test]
    fn tabless_commands_answer_without_a_tab() {
        let mut c = ctl();
        let (rc, v) = response(&c.on_bus_command(cmd("ced.ping", json!({}))));
        assert_eq!(rc, 0);
        assert_eq!(v["schema"], "ced.v1");
        assert_eq!(v["headless"], true);
        let (rc, v) = response(&c.on_bus_command(cmd("ced.tabs", json!({}))));
        assert_eq!((rc, v["tabs"].as_array().unwrap().len()), (0, 0));
        let (rc, v) = response(&c.on_bus_command(cmd("ced.layout", json!({}))));
        assert_eq!((rc, v["reason"].as_str()), (10, Some("headless")));
        let (rc, v) = response(&c.on_bus_command(cmd("ced.state", json!({}))));
        assert_eq!((rc, v["error_code"].as_str()), (10, Some("NOT_FOUND")));
        let (rc, v) = response(&c.on_bus_command(cmd(
            "ced.wait",
            json!({"idle": true, "rev": 3, "timeout_ms": 10}),
        )));
        assert_eq!((rc, v["reason"].as_str()), (10, Some("bad_args")));
        let (rc, v) = response(&c.on_bus_command(cmd("app.describe", json!({}))));
        assert_eq!(
            (rc, v["contract"].as_str()),
            (0, Some("ctk-app-control.v0"))
        );
        let (rc, v) = response(&c.on_bus_command(cmd("ced.actions", json!({}))));
        assert_eq!(rc, 0);
        assert!(
            v["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["id"] == "edit.undo")
        );
        let (rc, _) = response(&c.on_bus_command(cmd("ced.nope", json!({}))));
        assert_eq!(rc, 10);
        let fx = c.on_bus_command(cmd("app.quit", json!({})));
        assert!(fx.contains(&Effect::Quit));
    }

    #[test]
    fn execute_replaces_the_legacy_command_in_discovery_and_dispatch() {
        let legacy = concat!("ced.", "action");
        let mut c = ctl();
        let (rc, v) = response(&c.on_bus_command(cmd(legacy, json!({"id": "file.new"}))));
        assert_eq!(
            (rc, v["error_code"].as_str()),
            (10, Some("UNKNOWN_COMMAND"))
        );
        assert!(
            c.tabs.is_empty(),
            "the removed command cannot run an action"
        );

        let (_, v) = response(&c.on_bus_command(cmd("app.describe", json!({}))));
        let names = v["commands"].as_array().unwrap();
        assert!(names.contains(&json!("ced.execute")));
        assert!(!names.contains(&json!(legacy)));
    }

    #[test]
    fn execute_refuses_unknown_actions_and_invalid_payloads() {
        for headless in [true, false] {
            let mut c = Controller::new(Config::default(), 1, headless);
            let fx = c.on_bus_command(cmd(
                "ced.execute",
                json!({"id": "no.such.action", "params": {"tab": 999}}),
            ));
            let (rc, v) = response(&fx);
            assert_eq!((rc, v["error_code"].as_str()), (10, Some("UNKNOWN_ACTION")));
            assert_eq!(fx.len(), 1, "an unknown action only produces a refusal");

            for body in [
                json!({}),
                json!({"id": 42}),
                json!({"id": "file.new", "tab": 1}),
                json!({"id": "file.new", "buffer": "b1"}),
                json!({"id": "file.new", "args": {}}),
                json!({"id": "file.new", "params": []}),
                json!({"id": "file.new", "params": {"tab": "bad"}}),
                json!({"id": "file.new", "params": {"buffer": 42}}),
                // The whole body is checked before the id is looked up.
                json!({"id": "no.such.action", "params": {"tab": "bad"}}),
                // A misspelt key is refused, not ignored.
                json!({"id": "file.close", "params": {"arg": {"force": true}}}),
                json!({"id": "file.new", "params": {"tabs": 1}}),
            ] {
                let fx = c.on_bus_command(cmd("ced.execute", body.clone()));
                let (rc, v) = response(&fx);
                assert_eq!(
                    (rc, v["error_code"].as_str()),
                    (10, Some("INVALID_ARGUMENT")),
                    "{body}"
                );
                assert_eq!(fx.len(), 1, "invalid arguments cannot execute: {body}");
            }
        }
    }

    #[test]
    fn execute_runs_without_params_and_refuses_an_unknown_tab() {
        for body in [
            json!({"id": "file.new"}),
            json!({"id": "file.new", "params": null}),
        ] {
            let mut c = ctl();
            let fx = c.on_bus_command(cmd("ced.execute", body.clone()));
            assert_eq!(response(&fx).0, 0, "{body}");
            assert_eq!(c.tabs.len(), 1, "file.new opened a tab: {body}");
        }
        let mut c = ctl();
        let (rc, v) = response(&c.on_bus_command(cmd(
            "ced.execute",
            json!({"id": "file.close", "params": {"tab": 9999}}),
        )));
        assert_eq!((rc, v["error_code"].as_str()), (10, Some("NOT_FOUND")));
    }

    #[test]
    fn execute_params_preserve_selectors_and_ui_arguments() {
        let args = json!({"path": "/work/renamed.md"});
        for by_buffer in [false, true] {
            let mut c = ctl();
            live(&mut c);
            let tab = c.active().unwrap();
            let other = c.new_tab(None);
            c.active = Some(other);
            let selector = if by_buffer {
                json!({"buffer": B})
            } else {
                json!({"tab": tab})
            };
            let mut params = selector;
            params["args"] = args.clone();
            let fx = c.on_bus_command(cmd(
                "ced.execute",
                json!({"id": "file.save_as", "params": params}),
            ));
            let (_, save) = sent(&fx, "edit.save");
            assert_eq!(save["buffer"], B);
            assert_eq!(save["path"], args["path"]);
        }
        let mut c = Controller::new(Config::default(), 1, false);
        live(&mut c);
        let tab = c.active().unwrap();
        let fx = c.on_bus_command(cmd(
            "ced.execute",
            json!({"id": "search.find", "params": {"tab": tab, "args": args}}),
        ));
        assert!(
            fx.iter().any(|effect| matches!(
                effect,
                Effect::UiAction { tab: Some(t), action: ActionId::SearchFind, args: Some(a), .. }
                    if *t == tab && *a == args
            )),
            "the window receives the selected tab and the original args"
        );
    }

    #[test]
    fn start_subscribes_and_asks_edit_its_epoch() {
        let mut c = ctl();
        let fx = c.start();
        for t in ["edit.changed", "theme.changed", "noded.props.changed"] {
            assert!(fx.contains(&Effect::Subscribe { topic: t.into() }), "{t}");
        }
        assert!(
            fx.iter()
                .any(|e| matches!(e, Effect::Send { out, .. } if out.command == "edit.info"))
        );
    }

    fn sent(fx: &[Effect], command_name: &str) -> (u64, Value) {
        fx.iter()
            .find_map(|e| match e {
                Effect::Send { req, out } if out.command == command_name => {
                    Some((*req, serde_json::from_str(&out.body).unwrap()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {command_name} in {fx:?}"))
    }

    fn reply(c: &mut Controller, req: u64, body: Value) -> Vec<Effect> {
        c.on_incoming(Incoming::Reply {
            req,
            rc: 0,
            body: body.to_string(),
        })
    }

    /// Open a tab on a hand-driven `edit`, type over the Bus, ack by the echo.
    #[test]
    fn bus_typing_uses_the_callers_lane_and_acks_by_echo() {
        let mut c = ctl();
        c.start();
        let fx = c.on_bus_command(cmd(
            "ced.open",
            json!({"paths": ["/nonexistent/ced-e1d/x.txt"]}),
        ));
        let (req, _) = sent(&fx, "edit.open");
        let open = json!({"buffer": "b1_0000e1e1", "epoch": "0000e1e1", "path": "/nonexistent/ced-e1d/x.txt", "opened_as": null,
                          "name": "x.txt", "language": "text", "rev": 0, "lines": 2, "bytes": 12, "eol": "lf", "bom": false,
                          "disk": "clean", "reopened": false, "created": false, "recovery_id": "5f0c2a9e1b7d4c33",
                          "recovered": false, "recovered_from": null});
        let fx = reply(&mut c, req, open);
        let (rc, v) = response(&fx);
        assert_eq!(
            (rc, v["tabs"][0]["buffer"].as_str()),
            (0, Some("b1_0000e1e1"))
        );
        let (req, get) = sent(&fx, "edit.get");
        assert_eq!(get["snapshot"], true);
        let p = |o: usize, col: usize| json!({"offset": o, "line": 1, "col": col});
        let page = json!({"buffer": "b1_0000e1e1", "epoch": "0000e1e1", "rev": 0, "text": "hello world\n", "lines": null,
                          "start": p(0, 1), "end": p(12, 13), "bytes_total": 12, "lines_total": 2, "truncated": false,
                          "next": null, "snapshot": "s1"});
        reply(&mut c, req, page);
        let (rc, v) = response(&c.on_bus_command(cmd(
            "ced.wait",
            json!({"phase": "live", "timeout_ms": 1000}),
        )));
        assert_eq!((rc, v["phase"].as_str()), (0, Some("live")));

        let fx = c.on_bus_command(cmd("ced.type", json!({"text": "!"})));
        let (rc, v) = response(&fx);
        assert_eq!((rc, v["pending"].as_u64()), (0, Some(1)));
        let (req, ins) = sent(&fx, "edit.insert");
        assert_eq!(
            ins["origin"], "agent:ced.local_tester",
            "a Bus caller types in its own lane"
        );
        assert_eq!(
            (ins["at"].as_u64(), ins["base_rev"].as_u64()),
            (Some(0), Some(0))
        );
        let op_id = ins["op_id"].as_str().unwrap().to_string();
        let (rc, v) = response(&c.on_bus_command(cmd("ced.state", json!({"text": true}))));
        assert_eq!(
            (rc, v["text"].as_str(), v["inflight"].as_bool()),
            (0, Some("!hello world\n"), Some(true))
        );
        assert_eq!(
            v["selection"]["head"]["offset"].as_u64(),
            Some(1),
            "caret_after of the issuing view"
        );

        reply(
            &mut c,
            req,
            json!({"buffer": "b1_0000e1e1", "epoch": "0000e1e1", "rev": 1, "op_id": op_id}),
        );
        let ev = json!({"event": "edit", "epoch": "0000e1e1", "buffer": "b1_0000e1e1", "rev": 1, "base_rev": 0,
                        "origin": "agent:ced.local_tester", "lane": "agent:ced.local_tester", "kind": "edit", "of": null,
                        "op_id": op_id, "edits": [{"offset": 0, "delete": 0, "insert": "!"}], "event_seq": 1});
        c.on_incoming(Incoming::Topic {
            topic: "edit.changed".into(),
            body: ev.to_string(),
        });
        let (rc, v) =
            response(&c.on_bus_command(cmd("ced.wait", json!({"idle": true, "timeout_ms": 1000}))));
        assert_eq!((rc, v["rev"].as_u64()), (0, Some(1)));
        let (_, v) = response(&c.on_bus_command(cmd("ced.state", json!({}))));
        assert_eq!(
            v["text_hash"].as_str().unwrap(),
            blake3::hash(b"!hello world\n").to_hex().as_str()
        );
        assert_eq!(
            (v["pending"].as_u64(), v["inflight"].as_bool()),
            (Some(0), Some(false))
        );

        // A window-only action over the Bus: headless refuses; with a window
        // the reply waits for ui_done, or times out.
        let (rc, v) =
            response(&c.on_bus_command(cmd("ced.execute", json!({"id": "view.zoom_in"}))));
        assert_eq!((rc, v["error_code"].as_str()), (10, Some("UNAVAILABLE")));
        let mut gui = Controller::new(Config::default(), 2, false);
        let fx = gui.on_bus_command(cmd("ced.execute", json!({"id": "search.find"})));
        let token = fx
            .iter()
            .find_map(|e| match e {
                Effect::UiAction {
                    action: ActionId::SearchFind,
                    token,
                    ..
                } => Some(*token),
                _ => None,
            })
            .expect("a UiAction");
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::Respond { .. })),
            "no reply before the window acts"
        );
        let fx = gui.ui_done(token, Ok(json!({"opened": "find"})));
        let (rc, v) = response(&fx);
        assert_eq!(
            (rc, v["ok"].as_bool(), v["result"]["opened"].as_str()),
            (0, Some(true), Some("find"))
        );
        assert!(
            gui.ui_done(token, Ok(Value::Null)).is_empty(),
            "a token answers once"
        );
        let fx = gui.on_bus_command(cmd("ced.execute", json!({"id": "help.about"})));
        let timer = fx
            .iter()
            .find_map(|e| match e {
                Effect::Timer { id, ms } if *ms == 10_000 => Some(*id),
                _ => None,
            })
            .expect("a deadline");
        let (rc, v) = response(&gui.on_incoming(Incoming::Timer { id: timer }));
        assert_eq!(
            (rc, v["error_code"].as_str(), v["reason"].as_str()),
            (10, Some("TIMEOUT"), Some("timeout"))
        );
    }

    const B: &str = "b1_0000e1e1";

    /// A headless controller with one live tab on "hello world\n" (rev 0);
    /// returns the `edit.list` request the attach sent for the save state.
    fn live(c: &mut Controller) -> u64 {
        c.start();
        let fx = c.on_bus_command(cmd(
            "ced.open",
            json!({"paths": ["/nonexistent/ced-r/x.txt"]}),
        ));
        let (req, _) = sent(&fx, "edit.open");
        let open = json!({"buffer": B, "epoch": "0000e1e1", "path": "/nonexistent/ced-r/x.txt", "opened_as": null,
                          "name": "x.txt", "language": "text", "rev": 0, "lines": 2, "bytes": 12, "eol": "lf", "bom": false,
                          "disk": "clean", "reopened": false, "created": false, "recovery_id": "5f0c2a9e1b7d4c33",
                          "recovered": false, "recovered_from": null});
        let fx = reply(c, req, open);
        let (list, _) = sent(&fx, "edit.list");
        let (req, _) = sent(&fx, "edit.get");
        let p = |o: usize, col: usize| json!({"offset": o, "line": 1, "col": col});
        let page = json!({"buffer": B, "epoch": "0000e1e1", "rev": 0, "text": "hello world\n", "lines": null,
                          "start": p(0, 1), "end": p(12, 13), "bytes_total": 12, "lines_total": 2, "truncated": false,
                          "next": null, "snapshot": "s1"});
        reply(c, req, page);
        list
    }

    #[test]
    fn save_as_keeps_one_detection_until_open_metadata_arrives() {
        let mut c = ctl();
        live(&mut c);
        let tab = c.active().unwrap();
        let fx = c.on_action_args(
            Some(tab),
            ActionId::FileSaveAs,
            Some(json!({"path":"/work/notes"})),
            Intent::ui(tab),
        );
        let (req, _) = sent(&fx, "edit.save");
        reply(
            &mut c,
            req,
            json!({"buffer":B,"epoch":"0000e1e1","path":"/work/notes",
            "rev":0,"saved_rev":0,"file_bytes":12,"disk":"clean","durable":true,"warning":null}),
        );
        assert_eq!(c.tab(tab).unwrap().path.as_deref(), Some("/work/notes"));
        assert_eq!(c.tab(tab).unwrap().saved_language, Some("text"));
        // If language() read the first line again, this shebang would change it.
        c.on_editor(
            tab,
            EditorMsg::Command(EditCommand::Insert("#!/bin/sh\n".into())),
        );
        for _ in 0..2 {
            assert_eq!(c.tab(tab).unwrap().language(), "text");
            c.drive(tab, Step::default(), &mut Vec::new());
        }
        let open = serde_json::from_value(json!({"buffer":B,"epoch":"0000e1e1",
            "path":"/work/fresh-notes","opened_as":null,"name":"notes","language":"markdown",
            "rev":0,"lines":2,"bytes":12,"eol":"lf","bom":false,"disk":"clean",
            "reopened":true,"created":false,"recovery_id":"5f0c2a9e1b7d4c33",
            "recovered":false,"recovered_from":null}))
        .unwrap();
        c.attach(tab, true, &open, &mut Vec::new());
        assert_eq!(c.tab(tab).unwrap().language(), "markdown");
        assert_eq!(c.tab(tab).unwrap().saved_language, None);
    }

    fn refused_reply(c: &mut Controller, req: u64, code: &str, reason: &str) -> Vec<Effect> {
        let body = json!({"error_code": code, "message": format!("refused: {reason}"), "reason": reason, "buffer": B, "rev": 0});
        c.on_incoming(Incoming::Reply {
            req,
            rc: 10,
            body: body.to_string(),
        })
    }

    #[test]
    fn bus_actions_answer_from_the_server_ops_outcome() {
        // Not `ok:true` at enqueue.
        let mut c = ctl();
        live(&mut c);
        let fx = c.on_bus_command(cmd("ced.execute", json!({"id": "edit.undo"})));
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::Respond { .. })),
            "no answer before the service's"
        );
        let (req, _) = sent(&fx, "edit.undo");
        let (rc, v) = response(&refused_reply(&mut c, req, "NOT_FOUND", "nothing_to_undo"));
        assert_eq!(
            (rc, v["error_code"].as_str(), v["reason"].as_str()),
            (10, Some("NOT_FOUND"), Some("nothing_to_undo"))
        );

        let fx = c.on_bus_command(cmd("ced.execute", json!({"id": "file.save"})));
        assert!(!fx.iter().any(|e| matches!(e, Effect::Respond { .. })));
        let (req, _) = sent(&fx, "edit.save");
        // A Bus caller's disk_modified goes to the caller, never a prompt.
        let fx = refused_reply(&mut c, req, "CONFLICT", "disk_modified");
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::Prompt(_))),
            "no prompt for a Bus save: {fx:?}"
        );
        let asks = |e: &Effect| matches!(e, Effect::Notice { notice: Notice::Message { text, .. }, .. } if text == editor_model::mirror::MSG_SAVE_DISK_MODIFIED);
        assert!(
            !fx.iter().any(asks),
            "no unanswerable 'save anyway?' for the human"
        );
        let (rc, v) = response(&fx);
        assert_eq!((rc, v["reason"].as_str()), (10, Some("disk_modified")));

        let fx = c.on_bus_command(cmd("ced.execute", json!({"id": "file.save"})));
        let (req, _) = sent(&fx, "edit.save");
        let saved = json!({"buffer": B, "epoch": "0000e1e1", "path": "/nonexistent/ced-r/x.txt", "rev": 0, "saved_rev": 0,
                           "file_bytes": 12, "disk": "clean", "durable": true, "warning": null});
        let (rc, v) = response(&reply(&mut c, req, saved));
        assert_eq!(
            (rc, v["id"].as_str(), v["ok"].as_bool()),
            (0, Some("file.save"), Some(true))
        );
    }

    #[test]
    fn closing_a_tab_answers_its_waiting_bus_actions_at_once() {
        // Not a 90 s TIMEOUT saying the op may yet complete.
        let mut c = ctl();
        live(&mut c);
        let save = BusCommand {
            id: 41,
            ..cmd("ced.execute", json!({"id": "file.save"}))
        };
        let fx = c.on_bus_command(save);
        sent(&fx, "edit.save");
        let fx = c.on_bus_command(BusCommand {
            id: 42,
            ..cmd(
                "ced.execute",
                json!({"id": "file.close", "params": {"args": {"force": true}}}),
            )
        });
        let (req, _) = sent(&fx, "edit.close");
        let fx = reply(&mut c, req, json!({"closed": true}));
        let answer = |id: u64| {
            fx.iter()
                .find_map(|e| match e {
                    Effect::Respond { id: i, rc, body } if *i == id => {
                        Some((*rc, serde_json::from_str::<Value>(body).unwrap()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no answer to {id} in {fx:?}"))
        };
        let (rc, v) = answer(41);
        assert_eq!(
            (rc, v["error_code"].as_str(), v["reason"].as_str()),
            (10, Some("CONFLICT"), Some("detached"))
        );
        assert_eq!(answer(42).0, 0);
        assert!(
            c.op_waits.is_empty() && !c.timers.values().any(|t| matches!(t, TimerFor::OpWait(_))),
            "timer dropped too"
        );
    }

    #[test]
    fn a_lost_echo_after_the_reply_recovers_from_history() {
        // The mirror's echo timer reaches the host and, when it
        // fires with the op still waiting, starts a history recovery.
        let mut c = ctl();
        live(&mut c);
        let fx = c.on_bus_command(cmd("ced.type", json!({"text": "!"})));
        let (req, ins) = sent(&fx, "edit.insert");
        let fx = reply(
            &mut c,
            req,
            json!({"buffer": B, "epoch": "0000e1e1", "rev": 1, "op_id": ins["op_id"]}),
        );
        let timer = fx
            .iter()
            .find_map(|e| match e {
                Effect::Timer { id, ms: 5_000 } => Some(*id),
                _ => None,
            })
            .expect("an echo timer with the reply");
        // No event ever comes; the timer fires.
        let fx = c.on_incoming(Incoming::Timer { id: timer });
        let (_, h) = sent(&fx, "edit.history");
        assert_eq!(
            (h["buffer"].as_str(), h["since_rev"].as_u64()),
            (Some(B), Some(0))
        );
    }

    #[test]
    fn find_replies_wait_for_pending_local_edits() {
        // A reply at the mirror's rev is still the service's text,
        // not the view, while a local edit is pending.
        let mut c = ctl();
        live(&mut c);
        let fx = c.on_bus_command(cmd(
            "ced.execute",
            json!({"id": "search.find_next", "params": {"args": {"pattern": "world"}}}),
        ));
        // The find job's request (`from`), not highlight-all's.
        let find_req = |fx: &[Effect]| {
            fx.iter().find_map(|e| match e {
                Effect::Send { req, out }
                    if out.command == "edit.find" && out.body.contains("\"from\"") =>
                {
                    Some(*req)
                }
                _ => None,
            })
        };
        let find = find_req(&fx).expect("the find job's edit.find");
        let fx = c.on_bus_command(cmd("ced.type", json!({"text": "!"})));
        let (req, ins) = sent(&fx, "edit.insert");
        let pt = |o: usize| json!({"offset": o, "line": 1, "col": o + 1});
        let m = json!({"start": pt(6), "end": pt(11), "text": "world", "text_truncated": false, "groups": null, "groups_truncated": false});
        let fx = reply(
            &mut c,
            find,
            json!({"buffer": B, "rev": 0, "matches": [m], "truncated": false, "next": null}),
        );
        assert!(find_req(&fx).is_none(), "waits for idle");
        let (_, st) = response(&c.on_bus_command(cmd("ced.state", json!({}))));
        assert_eq!(
            (
                st["selection"]["anchor"]["offset"].as_u64(),
                st["selection"]["head"]["offset"].as_u64()
            ),
            (Some(1), Some(1)),
            "unmapped offsets not applied"
        );
        reply(
            &mut c,
            req,
            json!({"buffer": B, "epoch": "0000e1e1", "rev": 1, "op_id": ins["op_id"]}),
        );
        let ev = json!({"event": "edit", "epoch": "0000e1e1", "buffer": B, "rev": 1, "base_rev": 0, "origin": "agent:ced.local_tester",
                        "lane": "agent:ced.local_tester", "kind": "edit", "of": null, "op_id": ins["op_id"],
                        "edits": [{"offset": 0, "delete": 0, "insert": "!"}], "event_seq": 1});
        let fx = c.on_incoming(Incoming::Topic {
            topic: "edit.changed".into(),
            body: ev.to_string(),
        });
        assert!(find_req(&fx).is_some(), "asked again once idle");
    }

    #[test]
    fn the_save_state_comes_from_the_list_row_and_keys_feed_model_us() {
        // Another holder's unsaved edits show as dirty.
        let mut c = ctl();
        let list = live(&mut c);
        let (_, v) = response(&c.on_bus_command(cmd("ced.tabs", json!({}))));
        assert_eq!(
            v["tabs"][0]["dirty"].as_bool(),
            Some(false),
            "the open-time guess"
        );
        let row = json!({"buffer": B, "path": "/nonexistent/ced-r/x.txt", "opened_as": null, "name": "x.txt", "language": "text",
                         "rev": 0, "saved_rev": null, "dirty": true, "disk": "clean", "lines": 2, "bytes": 12,
                         "holders": ["local:other"], "recovery_id": "5f0c2a9e1b7d4c33", "recovered": false});
        reply(&mut c, list, json!({"epoch": "0000e1e1", "buffers": [row]}));
        let (_, v) = response(&c.on_bus_command(cmd("ced.tabs", json!({}))));
        assert_eq!(v["tabs"][0]["dirty"].as_bool(), Some(true));
        // A disk-state change asks again.
        let ev = json!({"event": "disk", "epoch": "0000e1e1", "buffer": B, "rev": 0, "disk": "modified", "event_seq": 1});
        let fx = c.on_incoming(Incoming::Topic {
            topic: "edit.changed".into(),
            body: ev.to_string(),
        });
        sent(&fx, "edit.list");
        // ced.stats model_us is measured per key.
        let tab = v["tabs"][0]["tab"].as_u64().unwrap() as TabId;
        c.on_editor(tab, EditorMsg::Command(EditCommand::Insert("x".into())));
        assert_eq!(c.frames.model_us.len(), 1);
    }

    /// Perf probe for the nested gate's view_us (run on cbc:
    /// `cargo test --release -p mixos-ced perf_probe -- --ignored --nocapture`).
    /// Times the controller work one `update` does for each stage of the
    /// gate's matrix — the UI-thread cost view_us counts besides `view`.
    #[test]
    #[ignore]
    fn perf_probe() {
        let us = |t: Instant| t.elapsed().as_micros() as u64;
        let pt = |o: usize| json!({"offset": o, "line": 1, "col": 1});
        for (name, path, unit, n) in [
            (
                "64 MB",
                "/p/sixty-four.txt",
                "0123456789abcdef0123456789abcdef0123456\n",
                1_600_000usize,
            ),
            (
                "2 M-line rust",
                "/p/big.rs",
                "fn f() { let x = 1; } // c\n",
                1_999_999,
            ),
            ("2 M lines", "/p/two-million.txt", "abcdefghij\n", 1_999_999),
        ] {
            let text = unit.repeat(n);
            let mut c = ctl();
            c.start();
            let fx = c.on_bus_command(cmd("ced.open", json!({"paths": [path]})));
            let (req, _) = sent(&fx, "edit.open");
            let lang = if path.ends_with(".rs") {
                "rust"
            } else {
                "text"
            };
            let open = json!({"buffer": B, "epoch": "0000e1e1", "path": path, "opened_as": null, "name": "x", "language": lang,
                              "rev": 0, "lines": n + 1, "bytes": text.len(), "eol": "lf", "bom": false, "disk": "clean",
                              "reopened": false, "created": false, "recovery_id": "5f0c2a9e1b7d4c33", "recovered": false,
                              "recovered_from": null});
            let mut fx = reply(&mut c, req, open);
            let mut at = 0;
            let mut times = Vec::new();
            while at < text.len() {
                let (req, get) = sent(&fx, "edit.get");
                // The page editd would cut: the mirror's max_bytes, else its default.
                let page = get["max_bytes"]
                    .as_u64()
                    .map_or(4 * 1024 * 1024 - 4096, |m| m as usize);
                let end = (at + page).min(text.len());
                let next = (end < text.len()).then_some(end);
                let body = json!({"buffer": B, "epoch": "0000e1e1", "rev": 0, "text": &text[at..end], "lines": null,
                                  "start": pt(at), "end": pt(end), "bytes_total": text.len(), "lines_total": n + 1,
                                  "truncated": next.is_some(), "next": next, "snapshot": "s1"})
                .to_string();
                // The bus thread parses big replies (Incoming::Parsed).
                let body = editor_model::types::ParsedBody(serde_json::from_str(&body).unwrap());
                let t = Instant::now();
                fx = c.on_incoming(Incoming::Parsed { req, rc: 0, body });
                times.push(us(t));
                at = end;
            }
            let last = times.pop().unwrap_or(0);
            times.sort_unstable();
            println!(
                "PERF {name}: {} pages, per page p50 {} us max {} us; final page (+ resync) {} us",
                times.len() + 1,
                times.get(times.len() / 2).copied().unwrap_or(0),
                times.last().copied().unwrap_or(0),
                last
            );
            let tab = c.active.unwrap();
            let t = Instant::now();
            let _ = c.on_bus_command(cmd(
                "ced.execute",
                json!({"id": "search.goto_line", "params": {"args": {"line": n}}}),
            ));
            println!("PERF {name}: goto last line {} us", us(t));
            let t = Instant::now();
            let _ = c.on_bus_command(cmd(
                "ced.execute",
                json!({"id": "search.goto_line", "params": {"args": {"line": 10}}}),
            ));
            let goto10 = us(t);
            let t = Instant::now();
            c.on_editor(tab, EditorMsg::Command(EditCommand::Insert("k".into())));
            println!(
                "PERF {name}: goto line 10 {goto10} us; keystroke {} us",
                us(t)
            );
        }
        // The session save (the gate's slow `bus.timer` updates): inline vs
        // handed to the writer thread.
        let dir = std::env::temp_dir().join(format!("ced-perf-session-{}", std::process::id()));
        // Separate dirs: save() names its temp file by pid, so the inline and
        // the writer thread's saves must not share one.
        let path = dir.join("inline/session.json");
        let queued_path = dir.join("queued/session.json");
        let session = crate::session::Session {
            recent: vec!["/p/x".into()],
            ..Default::default()
        };
        let mut inline = Vec::new();
        let mut queued = Vec::new();
        let mut w = crate::session::SessionWriter::spawn();
        for _ in 0..20 {
            let t = Instant::now();
            crate::session::save(&path, &session).unwrap();
            inline.push(us(t));
            let t = Instant::now();
            w.save(queued_path.clone(), session.clone());
            queued.push(us(t));
        }
        w.flush();
        inline.sort_unstable();
        queued.sort_unstable();
        println!(
            "PERF session save: inline p50 {} us max {} us; on the writer thread (UI cost) p50 {} us max {} us",
            inline[10], inline[19], queued[10], queued[19]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovered_buffers_prompt_once_and_frames_feed_stats() {
        let mut c = ctl();
        let fx = c.start();
        let req = fx
            .iter()
            .find_map(|e| match e {
                Effect::Send { req, out } if out.command == "edit.list" => Some(*req),
                _ => None,
            })
            .expect("edit.list at start");
        let row = |b: &str, recovered: bool, holders: Vec<String>| {
            json!({"buffer": b, "path": null, "opened_as": null, "name": null, "language": "text", "rev": 0,
                   "saved_rev": null, "dirty": true, "disk": "none", "lines": 1, "bytes": 5, "holders": holders,
                   "recovery_id": "5f0c2a9e1b7d4c33", "recovered": recovered})
        };
        let body = json!({"epoch": "0000e1e1", "buffers": [row("b1_0000e1e1", true, vec![]), row("b2_0000e1e1", false, vec![]),
                                                            row("b3_0000e1e1", true, vec!["local:x".into()])]});
        let fx = c.on_incoming(Incoming::Reply {
            req,
            rc: 0,
            body: body.to_string(),
        });
        let prompts: Vec<_> = fx
            .iter()
            .filter_map(|e| match e {
                Effect::Prompt(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(prompts.len(), 1);
        let Prompt::Recovered { buffers } = &prompts[0] else {
            panic!("{prompts:?}")
        };
        assert_eq!(
            buffers
                .iter()
                .map(|b| b.buffer.as_str())
                .collect::<Vec<_>>(),
            ["b1_0000e1e1"]
        );
        let fx = c.discard_recovered("b1_0000e1e1");
        assert!(fx.iter().any(|e| matches!(e, Effect::Send { out, .. } if out.command == "edit.close" && out.body.contains("\"force\":true"))));
        for v in 1..=100 {
            c.record_frame(v, Some(v * 10));
        }
        let (rc, v) = response(&c.on_bus_command(cmd("ced.stats", json!({}))));
        assert_eq!(rc, 0);
        assert_eq!(
            (
                v["frames"].as_u64(),
                v["view_us"]["p99"].as_u64(),
                v["next_frame_us"]["max"].as_u64()
            ),
            (Some(100), Some(99), Some(1000))
        );
        assert!(c.edit_info().is_none());
    }

    const P: &str = "/nonexistent/ced-r/x.txt";

    fn sha(text: &str) -> String {
        use sha2::Digest;
        use std::fmt::Write;
        sha2::Sha256::digest(text.as_bytes())
            .iter()
            .fold(String::new(), |mut s, b| {
                let _ = write!(s, "{b:02x}");
                s
            })
    }

    fn diags(c: &mut Controller, digest: Option<String>, lines: &[usize]) -> Value {
        let diagnostics: Vec<Value> =
            lines.iter().map(|l| json!({"line": l, "severity": "error", "code": "binding-policy", "message": "m"})).collect();
        let (rc, v) = response(&c.on_bus_command(cmd(
            "ced.diagnostics",
            json!({"path": P, "source": "scenes", "digest": digest, "diagnostics": diagnostics}),
        )));
        assert_eq!(rc, 0, "{v}");
        v
    }

    /// `(source, line, col)` of every `ced.problems` row.
    fn problems(c: &mut Controller) -> Vec<(String, u64, u64)> {
        let (rc, v) = response(&c.on_bus_command(cmd("ced.problems", json!({}))));
        assert_eq!(rc, 0, "{v}");
        assert_eq!(v["path"].as_str(), Some(P));
        let row = |r: &Value| {
            (
                r["source"].as_str().unwrap().to_string(),
                r["line"].as_u64().unwrap(),
                r["col"].as_u64().unwrap(),
            )
        };
        v["problems"].as_array().unwrap().iter().map(row).collect()
    }

    fn scenes(line: u64, col: u64) -> (String, u64, u64) {
        ("scenes".into(), line, col)
    }

    fn event(seq: u64, kind: &str, edits: Value) -> Incoming {
        let ev = json!({"event": "edit", "epoch": "0000e1e1", "buffer": B, "rev": seq, "base_rev": seq - 1, "origin": "agent:other",
                        "lane": "agent:other", "kind": kind, "of": null, "op_id": null, "edits": edits, "event_seq": seq});
        Incoming::Topic {
            topic: "edit.changed".into(),
            body: ev.to_string(),
        }
    }

    #[test]
    fn external_diagnostics_are_digest_gated_and_cleared_by_an_empty_list() {
        let mut c = ctl();
        live(&mut c);
        let v = diags(&mut c, Some(sha("hello world\n")), &[1, 2]);
        assert_eq!(
            v,
            json!({"path": P, "tabs": [1], "shown": 2, "stale": false})
        );
        assert_eq!(problems(&mut c), [scenes(1, 1), scenes(2, 1)]);

        // A digest of other bytes: nothing shown, and it says so.
        let v = diags(&mut c, Some(sha("other\n")), &[1]);
        assert_eq!(
            (v["shown"].as_u64(), v["stale"].as_bool()),
            (Some(0), Some(true))
        );
        assert!(problems(&mut c).is_empty());

        // No digest: applied as is. An empty list clears the source.
        diags(&mut c, None, &[2]);
        assert_eq!(problems(&mut c), [scenes(2, 1)]);
        let v = diags(&mut c, Some(sha("hello world\n")), &[]);
        assert_eq!(
            (v["shown"].as_u64(), v["stale"].as_bool()),
            (Some(0), Some(false))
        );
        assert!(problems(&mut c).is_empty());
        assert!(c.external.paths().is_empty(), "the stored set is gone too");
    }

    #[test]
    fn external_diagnostics_reach_a_tab_opened_later_and_survive_a_resync() {
        let mut c = ctl();
        let v = diags(&mut c, Some(sha("hello world\n")), &[1]);
        assert_eq!(
            v,
            json!({"path": P, "tabs": [], "shown": 0, "stale": false}),
            "stored with no tab open"
        );
        live(&mut c);
        assert_eq!(
            problems(&mut c),
            [scenes(1, 1)],
            "applied when the tab went live"
        );

        // A Resync clears every set; the stored one comes back with the
        // same transition.
        let tab = c.active.unwrap();
        let view_gen = c
            .tab(tab)
            .and_then(|t| t.mirror.as_ref())
            .unwrap()
            .view_gen()
            + 1;
        let resync = editor_model::types::ViewDelta {
            edits: vec![],
            origin: None,
            kind: DeltaKind::Resync,
            rev: 0,
            view_gen,
        };
        let mut fx = Vec::new();
        c.drive(
            tab,
            Step {
                deltas: vec![resync],
                ..Step::default()
            },
            &mut fx,
        );
        assert!(
            c.tab(tab).unwrap().diagnostics.items().is_empty(),
            "the Resync cleared it"
        );
        c.eval_waiters(&mut fx);
        assert_eq!(
            problems(&mut c),
            [scenes(1, 1)],
            "re-applied after the Resync"
        );
    }

    #[test]
    fn a_stale_set_is_rechecked_when_the_tab_goes_clean_and_edits_invalidate_it() {
        let mut c = ctl();
        live(&mut c);
        // Another origin edits line 1: the text no longer hashes to the
        // digest the loader will send.
        c.on_incoming(event(
            1,
            "edit",
            json!([{"offset": 0, "delete": 0, "insert": "!"}]),
        ));
        let v = diags(&mut c, Some(sha("hello world\n")), &[1]);
        assert_eq!(
            (v["stale"].as_bool(), v["shown"].as_u64()),
            (Some(true), Some(0))
        );
        // Back to the described text, still dirty: not re-checked (no
        // hashing per edit).
        c.on_incoming(event(
            2,
            "edit",
            json!([{"offset": 0, "delete": 1, "insert": ""}]),
        ));
        assert!(problems(&mut c).is_empty());
        // A reload makes it clean: the stored set is checked again, and matches.
        c.on_incoming(event(3, "reload", json!([])));
        let (_, v) = response(&c.on_bus_command(cmd("ced.tabs", json!({}))));
        assert_eq!(v["tabs"][0]["dirty"].as_bool(), Some(false));
        assert_eq!(problems(&mut c), [scenes(1, 1)]);

        // Covered-range invalidation: an edit on line 1 drops it; line 2's
        // diagnostic maps.
        diags(&mut c, Some(sha("hello world\n")), &[1, 2]);
        let fx = c.on_bus_command(cmd("ced.type", json!({"text": "?"})));
        assert_eq!(response(&fx).0, 0);
        assert_eq!(problems(&mut c), [scenes(2, 1)]);
    }

    #[test]
    fn an_undigested_set_dropped_by_an_edit_stays_dropped_when_the_tab_goes_clean() {
        let mut c = ctl();
        live(&mut c);
        diags(&mut c, None, &[1, 2]);
        assert_eq!(problems(&mut c), [scenes(1, 1), scenes(2, 1)]);
        // An edit on line 1 drops that row (covered-range invalidation).
        c.on_incoming(event(
            1,
            "edit",
            json!([{"offset": 0, "delete": 0, "insert": "!"}]),
        ));
        assert_eq!(problems(&mut c), [scenes(2, 1)]);
        // Going clean re-checks digested sets only: nothing says this one
        // still describes the text, so the dropped row must not come back.
        c.on_incoming(event(2, "reload", json!([])));
        let (_, v) = response(&c.on_bus_command(cmd("ced.tabs", json!({}))));
        assert_eq!(v["tabs"][0]["dirty"].as_bool(), Some(false));
        assert_eq!(problems(&mut c), [scenes(2, 1)]);
        // The command itself still applies an undigested set.
        diags(&mut c, None, &[1]);
        assert_eq!(problems(&mut c), [scenes(1, 1)]);
    }

    #[test]
    fn problems_carry_the_lint_and_the_external_sources() {
        let mut c = ctl();
        live(&mut c);
        let tab = c.active.unwrap();
        let (tag, _, _) = c.lint_capture(tab, 5).unwrap();
        let report = r#"{"schema_version":3,"functions":[],"diagnostics":[{"code":"E1","severity":"warning","file":"-","line":1,"column":7,"message":"m","hint":null}]}"#;
        c.on_lint(tab, tag, Ok(report.into()));
        diags(&mut c, Some(sha("hello world\n")), &[1]);
        assert_eq!(problems(&mut c), [("lint".into(), 1, 7), scenes(1, 1)]);
        let (_, v) = response(&c.on_bus_command(cmd("ced.problems", json!({"tab": tab}))));
        assert_eq!(
            v["problems"][0],
            json!({"line": 1, "col": 7, "severity": "warning", "code": "E1", "message": "m", "source": "lint"})
        );
        // A new lint result keeps the external set.
        let (tag, _, _) = c.lint_capture(tab, 5).unwrap();
        c.on_lint(
            tab,
            tag,
            Ok(r#"{"schema_version":3,"functions":[],"diagnostics":[]}"#.into()),
        );
        assert_eq!(problems(&mut c), [scenes(1, 1)]);
        let (rc, v) = response(&c.on_bus_command(cmd("ced.problems", json!({"tab": 99}))));
        assert_eq!((rc, v["error_code"].as_str()), (10, Some("NOT_FOUND")));
    }

    #[test]
    fn diagnostics_requests_are_refused_with_the_fixture_shape() {
        let mut c = ctl();
        let fx = c.on_bus_command(cmd(
            "ced.diagnostics",
            json!({"path": P, "source": "lint", "diagnostics": []}),
        ));
        let (rc, v) = response(&fx);
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/commands/refusal.diagnostics-source.json"
        ))
        .unwrap();
        assert_eq!((rc, v), (10, fixture));
        let reason = |c: &mut Controller, body: Value| {
            response(&c.on_bus_command(cmd("ced.diagnostics", body))).1["reason"].clone()
        };
        assert_eq!(
            reason(
                &mut c,
                json!({"path": "x.txt", "source": "scenes", "diagnostics": []})
            ),
            "bad_path"
        );
        let many: Vec<Value> = (0..=commands::MAX_EXTERNAL_DIAGNOSTICS)
            .map(|_| json!({"line": 1, "severity": "note", "code": "c", "message": ""}))
            .collect();
        assert_eq!(
            reason(
                &mut c,
                json!({"path": P, "source": "scenes", "diagnostics": many})
            ),
            "too_many"
        );
        let big = "x".repeat(commands::MAX_DIAGNOSTICS_BODY);
        assert_eq!(
            reason(
                &mut c,
                json!({"path": P, "source": "scenes", "diagnostics": [{"line": 1, "severity": "note", "code": "c", "message": big}]})
            ),
            "too_large"
        );
        assert_eq!(
            reason(&mut c, json!({"path": P, "source": "scenes"})),
            "bad_args"
        );
        let (rc, _) = response(&c.on_bus_command(cmd("ced.problems", json!({}))));
        assert_eq!(rc, 10, "no tab open");
    }
}
