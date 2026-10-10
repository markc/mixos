// SPDX-License-Identifier: MIT OR Apache-2.0
//! Prefs' behaviour, with no UI and no I/O: the panels, the Applications
//! panel over the `releases` service (list, check, notes, install, update,
//! roll back, remove), one operation at a time, agent commands over the
//! Bus, and quitting only once accepted work is finished.
//!
//! The shell feeds [`Engine`] events (Bus deliveries, completions, user
//! edits) and runs the [`Effect`]s it returns. Every call carries a ticket,
//! and a completion whose ticket is not the one in flight is ignored, so a
//! late reply can never land on a newer operation.
use crate::model::{self, APP_ID, AppRow, Notes, Panel};
use citizen::{CallError, Delivery, Reply};
use design::{Mode, Scheme};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Localises a catalogue key with named arguments. The engine is headless;
/// the shell supplies it.
pub type Label = fn(&str, &[(&str, &str)]) -> String;

/// Work for the shell. Nothing here has happened yet.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Call `verb` on the releases service and report
    /// [`Engine::released`] with `ticket`.
    Releases {
        ticket: u64,
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
    /// verbs (`HELP`): the engine's and the window's drive verbs.
    Describe { id: u64, help: bool },
    /// Hand the window-level verb `verb` (without the `prefs.` prefix:
    /// `ui.click`, `window`, …) to the toolkit drive layer.
    Drive { id: u64, verb: String, args: Value },
    /// Close: stop the Bus connection and the window.
    Exit,
}

/// A modal dialogue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dialog {
    About,
    Shortcuts,
    /// Confirm removing this app.
    Remove(String),
}

/// One operation on the releases service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "op", content = "app")]
pub enum Op {
    List,
    Check,
    Install(String),
    /// One app, or every installed app.
    Update(Option<String>),
    Rollback(String),
    Remove(String),
}

impl Op {
    fn verb(&self) -> &'static str {
        match self {
            Op::List => "releases.list",
            Op::Check => "releases.check",
            Op::Install(_) => "releases.install",
            Op::Update(_) => "releases.update",
            Op::Rollback(_) => "releases.rollback",
            Op::Remove(_) => "releases.remove",
        }
    }

    fn body(&self) -> Value {
        match self {
            Op::List | Op::Check | Op::Update(None) => json!({}),
            Op::Update(Some(app)) => json!({"apps":[app]}),
            Op::Install(app) | Op::Rollback(app) | Op::Remove(app) => json!({"app":app}),
        }
    }

    /// The status line while it runs.
    fn progress(&self) -> &'static str {
        match self {
            Op::List => "listing",
            Op::Check => "checking",
            Op::Install(_) => "installing",
            Op::Update(None) => "updating-all",
            Op::Update(Some(_)) => "updating",
            Op::Rollback(_) => "rolling-back",
            Op::Remove(_) => "removing",
        }
    }

    fn app(&self) -> &str {
        match self {
            Op::Install(app) | Op::Rollback(app) | Op::Remove(app) | Op::Update(Some(app)) => app,
            _ => "",
        }
    }

    /// It changes installs: the rows are listed again afterwards.
    fn changes(&self) -> bool {
        !matches!(self, Op::List | Op::Check)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiState {
    pub panel: Panel,
    /// The selected app in the Applications panel.
    pub selected: Option<String>,
    pub dialog: Option<Dialog>,
    /// The window's own theme choice (View › Theme, the title bar's
    /// light/dark button, `prefs.theme`): `None` on an axis follows the
    /// session. The session's theme file is never written.
    #[serde(default, with = "by_name::scheme")]
    pub theme_scheme: Option<Scheme>,
    #[serde(default, with = "by_name::mode")]
    pub theme_mode: Option<Mode>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            panel: Panel::Applications,
            selected: None,
            dialog: None,
            theme_scheme: None,
            theme_mode: None,
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
}

#[derive(Clone, Debug)]
struct Job {
    ticket: u64,
    op: Op,
    /// A listing that only refreshes the rows (after a change, or one asked
    /// for while busy): it leaves the status line, which still reports what
    /// just happened, unless it fails.
    quiet: bool,
}

/// The releases service as Prefs last saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Unknown,
    Available,
    /// Not registered on the Bus: the panel says how to start it.
    Missing,
}

/// Prefs' state and rules.
pub struct Engine {
    label: Label,
    pub ui: UiState,
    pub apps: Vec<AppRow>,
    /// The selected app's latest release notes, once loaded.
    pub notes: Option<Notes>,
    /// Why the notes could not be loaded.
    pub notes_error: Option<String>,
    pub status: String,
    /// The status reports a failure (the view colours it).
    pub failed: bool,
    pub connected: bool,
    pub releases: Availability,
    /// A change whose reply was lost (timed out, connection dropped): it may
    /// or may not have happened. Every change is refused until a listing
    /// succeeds and shows the real state; a retry could otherwise undo it
    /// (rolling back twice swaps back).
    pub uncertain: Option<Op>,
    /// Quitting has spent its one settling listing.
    quit_settled: bool,
    pub quitting: bool,
    /// The session theme's scheme and mode, as the shell loaded it.
    pub session: (Scheme, Mode),
    next_ticket: u64,
    job: Option<Job>,
    notes_job: Option<(u64, String)>,
    activations: BTreeSet<u64>,
    relist: bool,
    effects: Vec<Effect>,
}

impl Engine {
    /// A fresh engine; the first listing is queued.
    pub fn new(label: Label) -> Self {
        let mut engine = Self {
            label,
            ui: UiState::default(),
            apps: Vec::new(),
            notes: None,
            notes_error: None,
            status: label("connecting", &[]),
            failed: false,
            connected: true,
            releases: Availability::Unknown,
            uncertain: None,
            quit_settled: false,
            quitting: false,
            session: (Scheme::default(), Mode::default()),
            next_ticket: 0,
            job: None,
            notes_job: None,
            activations: BTreeSet::new(),
            relist: false,
            effects: Vec::new(),
        };
        engine.start(Op::List);
        engine
    }

    /// The effects queued since the last call.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    fn label(&self, key: &str, args: &[(&str, &str)]) -> String {
        (self.label)(key, args)
    }

    fn next(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }

    /// An operation, a notes load or a window activation is in flight.
    pub fn busy(&self) -> bool {
        self.job.is_some() || self.notes_job.is_some() || !self.activations.is_empty()
    }

    /// The operation in flight, if any.
    pub fn working(&self) -> Option<&Op> {
        self.job.as_ref().map(|j| &j.op)
    }

    /// Nothing in flight, connected, no dialog, not quitting: a new
    /// operation may start.
    pub fn idle(&self) -> bool {
        self.job.is_none() && self.connected && self.ui.dialog.is_none() && !self.quitting
    }

    /// The selected app's row.
    pub fn selected_row(&self) -> Option<&AppRow> {
        let name = self.ui.selected.as_deref()?;
        self.apps.iter().find(|r| r.app == name)
    }

    // ---- enablement (the command registry asks these) ---------------------

    pub fn can_refresh(&self) -> bool {
        self.idle() && self.ui.panel == Panel::Applications
    }

    pub fn can_check(&self) -> bool {
        self.can_refresh()
            && self.releases != Availability::Missing
            && !self.apps.is_empty()
            && self.uncertain.is_none()
    }

    pub fn can_update_all(&self) -> bool {
        self.can_check()
            && self
                .apps
                .iter()
                .any(|r| r.state() == crate::RowState::Update)
    }

    pub fn can_install(&self) -> bool {
        self.can_check() && self.selected_row().is_some_and(AppRow::installable)
    }

    pub fn can_rollback(&self) -> bool {
        self.can_check() && self.selected_row().is_some_and(AppRow::installed)
    }

    pub fn can_remove(&self) -> bool {
        self.can_rollback()
    }

    // ---- actions (commands dispatch here) --------------------------------

    /// Start `op` if nothing else is in flight. A listing asked for while
    /// busy runs once the current operation finishes.
    fn start(&mut self, op: Op) {
        self.start_with(op, false);
    }

    fn start_with(&mut self, op: Op, quiet: bool) {
        // Quitting refuses new work, except the listing that settles an
        // uncertain change before the window closes.
        // Quitting allows exactly one more listing, to settle an uncertain
        // change; after that attempt (whatever its result) the window closes.
        let settling =
            self.quitting && op == Op::List && self.uncertain.is_some() && !self.quit_settled;
        if self.job.is_some() || !self.connected || (self.quitting && !settling) {
            if op == Op::List && self.connected && (!self.quitting || settling) {
                self.relist = true;
            }
            return;
        }
        if self.quitting {
            self.quit_settled = true;
        }
        let ticket = self.next();
        let (verb, body) = (op.verb(), op.body());
        if !quiet {
            self.status = self.label(op.progress(), &[("app", op.app())]);
            self.failed = false;
        }
        self.job = Some(Job { ticket, op, quiet });
        self.effects.push(Effect::Releases { ticket, verb, body });
    }

    pub fn refresh(&mut self) {
        self.start(Op::List);
    }

    pub fn check(&mut self) {
        self.start(Op::Check);
    }

    pub fn update_all(&mut self) {
        self.start(Op::Update(None));
    }

    /// Install the selected app, or update it when it is installed.
    pub fn install_selected(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        self.start(if row.installed() {
            Op::Update(Some(row.app))
        } else {
            Op::Install(row.app)
        });
    }

    pub fn rollback_selected(&mut self) {
        if let Some(app) = self.ui.selected.clone() {
            self.start(Op::Rollback(app));
        }
    }

    /// Ask before removing the selected app.
    pub fn remove_selected(&mut self) {
        if let Some(app) = self.ui.selected.clone() {
            self.open(Dialog::Remove(app));
        }
    }

    /// The remove dialog's confirmation.
    pub fn confirm_remove(&mut self) {
        if let Some(Dialog::Remove(app)) = self.ui.dialog.clone() {
            self.ui.dialog = None;
            self.start(Op::Remove(app));
        }
    }

    pub fn open(&mut self, dialog: Dialog) {
        if !self.quitting {
            self.ui.dialog = Some(dialog);
        }
    }

    pub fn close_dialog(&mut self) {
        self.ui.dialog = None;
    }

    pub fn set_panel(&mut self, panel: Panel) {
        if self.ui.dialog.is_none() {
            self.ui.panel = panel;
        }
    }

    /// Select `app` and load its release notes. Selection is refused under
    /// a dialog, and while an operation runs (it acts on the selection).
    pub fn select(&mut self, app: &str) {
        if self.ui.dialog.is_some() || self.job.is_some() {
            return;
        }
        if !self.apps.iter().any(|r| r.app == app) {
            return;
        }
        if self.ui.selected.as_deref() == Some(app)
            && (self.notes.is_some() || self.notes_job.is_some())
        {
            return;
        }
        self.ui.selected = Some(app.to_owned());
        self.load_notes(app);
    }

    /// Load `app`'s latest release notes, superseding any load in flight.
    fn load_notes(&mut self, app: &str) {
        self.notes = None;
        self.notes_error = None;
        self.notes_job = None;
        if self.connected && !self.quitting {
            let ticket = self.next();
            self.notes_job = Some((ticket, app.to_owned()));
            self.effects.push(Effect::Releases {
                ticket,
                verb: "releases.notes",
                body: json!({"app":app}),
            });
        }
    }

    /// After a listing: reload the selected app's notes when they are for an
    /// older release than the row now shows, when the row's latest release
    /// moved (`moved`: a load in flight was asked for the old one), or when a
    /// check finished over a load. The reload takes a new ticket, so the old
    /// reply is ignored.
    fn refresh_notes(&mut self, checked: bool, moved: bool) {
        let Some(app) = self.ui.selected.clone() else {
            return;
        };
        let latest = self
            .selected_row()
            .and_then(|r| r.latest.clone())
            .map(|v| v.trim_start_matches('v').to_owned());
        let stale = match (&self.notes, &latest) {
            (Some(notes), Some(latest)) => notes.tag.trim_start_matches('v') != latest,
            _ => false,
        };
        if stale || moved || (checked && self.notes_job.is_some()) {
            self.load_notes(&app);
        }
    }

    /// Close now, or as soon as the work in flight finishes.
    pub fn quit(&mut self) {
        self.quitting = true;
        // An uncertain change is settled first: one listing, then close.
        if self.uncertain.is_some() && self.job.is_none() {
            self.start_with(Op::List, true);
        }
        if self.busy() {
            self.status = self.label("quitting", &[]);
        } else {
            self.effects.push(Effect::Exit);
        }
    }

    /// The scheme and mode the window shows: the choice, each unchosen
    /// axis the session's.
    pub fn effective_theme(&self) -> (Scheme, Mode) {
        (
            self.ui.theme_scheme.unwrap_or(self.session.0),
            self.ui.theme_mode.unwrap_or(self.session.1),
        )
    }

    /// Choose the window's scheme and mode (`None` follows the session).
    pub fn set_theme(&mut self, scheme: Option<Scheme>, mode: Option<Mode>) {
        self.ui.theme_scheme = scheme;
        self.ui.theme_mode = mode;
    }

    /// The title bar's light/dark switch: the mode opposite the one shown.
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

    // ---- events -----------------------------------------------------------

    /// A delivery from the Bus connection.
    pub fn delivery(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Command { id, verb, body } => self.command(id, &verb, &body),
            // A service came or went: the releases service may be one.
            Delivery::Changed => self.refresh(),
            Delivery::Connected => {
                self.connected = true;
                self.refresh();
            }
            Delivery::Disconnected => {
                self.connected = false;
                self.status = self.label("disconnected", &[]);
                self.failed = true;
            }
            // The shell reloads the theme; the engine has nothing to do.
            Delivery::Theme => {}
        }
    }

    /// Releases call `ticket` finished.
    pub fn released(&mut self, ticket: u64, result: Result<Reply, CallError>) {
        if let Some((expected, app)) = self.notes_job.clone()
            && expected == ticket
        {
            self.notes_job = None;
            self.notes_loaded(&app, result);
        } else if let Some(job) = self.job.clone()
            && job.ticket == ticket
        {
            self.job = None;
            self.finished(job.op, job.quiet, result);
            if self.relist {
                self.relist = false;
                self.start_with(Op::List, true);
            }
        }
        if self.quitting && !self.busy() {
            self.effects.push(Effect::Exit);
        }
    }

    /// What an operation acted on, for the status line.
    fn subject(&self, op: &Op) -> String {
        if op.app().is_empty() {
            self.label("all-apps", &[])
        } else {
            op.app().to_owned()
        }
    }

    fn notes_loaded(&mut self, app: &str, result: Result<Reply, CallError>) {
        if self.ui.selected.as_deref() != Some(app) {
            return;
        }
        match decode(result) {
            Ok(value) => match serde_json::from_value::<Notes>(value) {
                Ok(notes) => self.notes = Some(notes),
                Err(error) => self.notes_error = Some(error.to_string()),
            },
            Err(Failure::Missing) => self.releases = Availability::Missing,
            Err(Failure::Refused(message) | Failure::Unknown(message)) => {
                self.notes_error = Some(message)
            }
        }
    }

    fn finished(&mut self, op: Op, quiet: bool, result: Result<Reply, CallError>) {
        let value = match decode(result) {
            Ok(value) => value,
            Err(Failure::Missing) => {
                self.releases = Availability::Missing;
                self.apps.clear();
                self.status = self.label("releases-missing", &[]);
                self.failed = true;
                return;
            }
            Err(Failure::Unknown(message)) if op.changes() => {
                // It may have happened: settle it from a listing before
                // anything else is allowed to change.
                let app = self.subject(&op);
                self.status = self.label("uncertain", &[("app", &app), ("message", &message)]);
                self.failed = true;
                self.uncertain = Some(op);
                self.relist = true;
                return;
            }
            Err(Failure::Refused(message) | Failure::Unknown(message)) => {
                self.status = self.label("failed", &[("message", &message)]);
                self.failed = true;
                if op.changes() {
                    self.relist = true;
                }
                return;
            }
        };
        self.releases = Availability::Available;
        match op {
            Op::List | Op::Check => match model::rows(&value) {
                Ok(rows) => {
                    let before = self.selected_row().and_then(|r| r.latest.clone());
                    self.apps = rows;
                    let moved = self.selected_row().is_some_and(|r| r.latest != before);
                    // A successful listing shows the real state: an
                    // uncertain change is settled, whatever it did.
                    if let Some(settled) = self.uncertain.take() {
                        let app = self.subject(&settled);
                        self.status = self.label("settled", &[("app", &app)]);
                        self.failed = false;
                    }
                    if self
                        .ui
                        .selected
                        .as_ref()
                        .is_some_and(|s| !self.apps.iter().any(|r| &r.app == s))
                    {
                        self.ui.selected = None;
                        self.notes = None;
                    }
                    if !quiet {
                        let count = self.apps.len().to_string();
                        let key = if op == Op::Check { "checked" } else { "listed" };
                        self.status = self.label(key, &[("count", &count)]);
                    }
                    self.refresh_notes(op == Op::Check, moved);
                }
                Err(error) => {
                    self.status = self.label("failed", &[("message", &error)]);
                    self.failed = true;
                }
            },
            _ => {
                self.status = summary(self.label, &value);
                self.failed = results(&value).iter().any(|r| r["action"] == "failed");
                // The new state, as releases now sees it.
                self.relist = true;
                // Notes are for the latest release: unchanged by an install.
            }
        }
    }

    // ---- the Bus surface ---------------------------------------------------

    /// The whole state as `prefs.info` reports it.
    pub fn info(&self) -> Value {
        json!({"schema":"prefs.v1","app_id":APP_ID,"version":env!("CARGO_PKG_VERSION"),"pid":std::process::id(),
            "connected":self.connected,"releases":self.releases,"busy":self.busy(),"working":self.working(),"uncertain":self.uncertain,
            "status":self.status,"failed":self.failed,"apps":self.apps,"notes":self.notes,"notes_error":self.notes_error,
            "ui":self.ui})
    }

    fn apps_view(&self) -> Value {
        json!({"apps":self.apps,"selected":self.ui.selected,"notes":self.notes,"notes_error":self.notes_error,"releases":self.releases})
    }

    fn error(&mut self, id: u64, code: &str, message: &str) {
        self.effects.push(Effect::Reply {
            id,
            rc: 10,
            body: json!({"error_code":code,"message":message}),
        });
    }

    fn ok(&mut self, id: u64, body: Value) {
        self.effects.push(Effect::Reply { id, rc: 0, body });
    }

    /// `prefs.theme`: a present `scheme` or `mode` sets that axis (a name,
    /// or null to follow the session); an absent one leaves it. Both are
    /// checked before either applies.
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
        let mode = axis(args, "mode", Mode::from_name)?;
        if let Some(scheme) = scheme {
            self.ui.theme_scheme = scheme;
        }
        if let Some(mode) = mode {
            self.ui.theme_mode = mode;
        }
        let (scheme, mode) = self.effective_theme();
        Ok(
            json!({"scheme":self.ui.theme_scheme.map(Scheme::name),"mode":self.ui.theme_mode.map(Mode::name),
            "effective":{"scheme":scheme.name(),"mode":mode.name()}}),
        )
    }

    /// A command an agent sent to Prefs over the Bus.
    pub fn command(&mut self, id: u64, verb: &str, body: &str) {
        let args = match serde_json::from_str::<Value>(body) {
            Ok(args) if args.is_object() => args,
            _ => return self.error(id, "ARGUMENT", "arguments must be a JSON object"),
        };
        if let Some(window) = verb
            .strip_prefix("prefs.")
            .filter(|v| v.starts_with("ui.") || v.starts_with("window"))
        {
            return self.effects.push(Effect::Drive {
                id,
                verb: window.to_owned(),
                args,
            });
        }
        if !model::VERBS.contains(&verb) {
            return self.error(id, "UNKNOWN_VERB", "unknown Prefs verb");
        }
        match verb {
            "prefs.ping" => self.ok(
                id,
                json!({"schema":"prefs.v1","version":env!("CARGO_PKG_VERSION")}),
            ),
            "prefs.info" => self.ok(id, self.info()),
            "prefs.apps" => self.ok(id, self.apps_view()),
            "HELP" => self.effects.push(Effect::Describe { id, help: true }),
            "app.describe" => self.effects.push(Effect::Describe { id, help: false }),
            "prefs.commands" => self.effects.push(Effect::Commands { id }),
            "prefs.execute" => match args.get("id").and_then(Value::as_str) {
                Some(command) => self.effects.push(Effect::Execute {
                    id,
                    command: command.to_owned(),
                }),
                None => self.error(id, "ARGUMENT", "id must be a command id string"),
            },
            "prefs.theme" => match self.theme(&args) {
                Ok(body) => self.ok(id, body),
                Err((code, message)) => self.error(id, code, &message),
            },
            "prefs.show" if !self.quitting => self.show(Some(id)),
            "prefs.quit" if self.ui.dialog.is_none() => {
                self.ok(id, json!({"quitting":true}));
                self.quit();
            }
            "prefs.panel" => {
                let panel = args
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(Panel::from_name);
                match panel {
                    None => self.error(id, "ARGUMENT", "name must be a panel name"),
                    Some(_) if self.ui.dialog.is_some() => {
                        let message = self.label("busy", &[]);
                        self.error(id, "BUSY", &message);
                    }
                    Some(panel) => {
                        self.set_panel(panel);
                        self.ok(id, json!({"panel":self.ui.panel}));
                    }
                }
            }
            "prefs.select" => {
                let Some(app) = args.get("app").and_then(Value::as_str) else {
                    return self.error(id, "ARGUMENT", "app must be an app name");
                };
                if !self.apps.iter().any(|r| r.app == app) {
                    return self.error(id, "ARGUMENT", "no app has that name");
                }
                if self.ui.dialog.is_some() || self.job.is_some() {
                    let message = self.label("busy", &[]);
                    return self.error(id, "BUSY", &message);
                }
                self.select(app);
                self.ok(id, self.apps_view());
            }
            "prefs.dialog" => {
                let dialog = match args.get("open") {
                    Some(Value::Null) => None,
                    Some(Value::String(name)) if name == "about" => Some(Dialog::About),
                    Some(Value::String(name)) if name == "shortcuts" => Some(Dialog::Shortcuts),
                    _ => {
                        return self.error(
                            id,
                            "ARGUMENT",
                            "open must be \"about\", \"shortcuts\" or null",
                        );
                    }
                };
                match dialog {
                    None => self.close_dialog(),
                    Some(_) if self.ui.dialog.is_some() || self.quitting => {
                        let message = self.label("busy", &[]);
                        return self.error(id, "BUSY", &message);
                    }
                    Some(dialog) => self.open(dialog),
                }
                self.ok(id, json!({"dialog":self.ui.dialog}));
            }
            "prefs.show" | "prefs.quit" => {
                let message = self.label("busy", &[]);
                self.error(id, "BUSY", &message);
            }
            _ => self.error(id, "UNKNOWN_VERB", "unknown Prefs verb"),
        }
    }

    /// Restore and focus the window; `reply` is the Bus command that asked.
    pub fn show(&mut self, reply: Option<u64>) {
        let ticket = self.next();
        self.activations.insert(ticket);
        self.effects.push(Effect::Show { ticket, reply });
    }

    /// Window activation `ticket` finished.
    pub fn shown(&mut self, ticket: u64, reply: Option<u64>, result: Result<Value, String>) {
        if !self.activations.remove(&ticket) {
            return;
        }
        if let Some(id) = reply {
            match result {
                Ok(body) => self.ok(id, body),
                Err(error) => self.error(id, "SHOW", &error),
            }
        }
        if self.quitting && !self.busy() {
            self.effects.push(Effect::Exit);
        }
    }
}

/// Why a releases call gave no usable answer.
enum Failure {
    /// The service is not registered: the broker refuses with rc 10, no
    /// body and "Service '…' not found" in its error header. The releases
    /// service itself always answers a refusal with a JSON body.
    Missing,
    /// The call was refused or failed before reaching the service: nothing
    /// happened.
    Refused(String),
    /// The reply was lost after the call may have reached the service
    /// (timed out, connection dropped): what it did is unknown.
    Unknown(String),
}

fn decode(result: Result<Reply, CallError>) -> Result<Value, Failure> {
    let reply = match result {
        Ok(reply) => reply,
        Err(error) if error.outcome_unknown => return Err(Failure::Unknown(error.message)),
        Err(error) => return Err(Failure::Refused(error.message)),
    };
    if reply.rc >= 10 && reply.body.trim().is_empty() {
        // A broker refusal. Only "not found" means the service is gone; an
        // overloaded service (rc 20) is still registered.
        let not_found = reply
            .error
            .as_deref()
            .is_none_or(|e| e.contains("not found"));
        if reply.rc == 10 && not_found {
            return Err(Failure::Missing);
        }
        let message = reply
            .error
            .unwrap_or_else(|| format!("refused by the broker (rc {})", reply.rc));
        return Err(Failure::Refused(message));
    }
    let value: Value = serde_json::from_str(&reply.body).unwrap_or(Value::String(reply.body));
    if reply.rc != 0 {
        let message = match (&value["error_code"], &value["message"]) {
            (Value::String(code), Value::String(message)) => format!("{code}: {message}"),
            _ => value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
        };
        return Err(Failure::Refused(message));
    }
    Ok(value)
}

/// The results of a change: one (install, rollback, remove) or a list (update).
fn results(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(list) => list.clone(),
        other => vec![other.clone()],
    }
}

/// One status line for a change's results.
fn summary(label: Label, value: &Value) -> String {
    let results = results(value);
    if results.is_empty() {
        return label("nothing-to-update", &[]);
    }
    if let [one] = results.as_slice() {
        let text = |key: &str| one[key].as_str().unwrap_or_default().to_owned();
        if one["action"] == "failed" {
            return label(
                "failed",
                &[("message", &format!("{}: {}", text("app"), text("error")))],
            );
        }
        return label(
            "result",
            &[
                ("app", &text("app")),
                ("action", &text("action")),
                ("version", &text("version")),
            ],
        );
    }
    let failed = results.iter().filter(|r| r["action"] == "failed").count();
    let changed = results
        .iter()
        .filter(|r| r["action"] == "installed")
        .count();
    label(
        "updated-all",
        &[
            ("changed", &changed.to_string()),
            ("failed", &failed.to_string()),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(key: &str, args: &[(&str, &str)]) -> String {
        let mut out = key.to_owned();
        for (k, v) in args {
            out.push_str(&format!(" {k}={v}"));
        }
        out
    }

    fn ok(body: Value) -> Result<Reply, CallError> {
        Ok(Reply {
            rc: 0,
            body: body.to_string(),
            error: None,
        })
    }

    fn rows() -> Value {
        json!([
            {"app":"demo","repo":"o/demo","installed":"1.0","latest":"2.0","published":"2026-10-10","checked":"x","status":"update"},
            {"app":"new","repo":"o/new","installed":null,"latest":"1.0","published":null,"checked":null,"status":"not installed"}
        ])
    }

    fn releases(effects: &[Effect]) -> Vec<(u64, &'static str, Value)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Releases { ticket, verb, body } => Some((*ticket, *verb, body.clone())),
                _ => None,
            })
            .collect()
    }

    /// A fresh engine with its first listing answered.
    fn listed() -> Engine {
        let mut e = Engine::new(label);
        let calls = releases(&e.take_effects());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "releases.list");
        e.released(calls[0].0, ok(rows()));
        e.take_effects();
        e
    }

    #[test]
    fn the_first_listing_fills_the_panel() {
        let e = listed();
        assert_eq!(e.apps.len(), 2);
        assert_eq!(e.releases, Availability::Available);
        assert_eq!(e.status, "listed count=2");
        assert!(!e.busy());
    }

    #[test]
    fn an_unregistered_releases_service_reads_as_missing() {
        let mut e = Engine::new(label);
        let calls = releases(&e.take_effects());
        e.released(
            calls[0].0,
            Ok(Reply {
                rc: 10,
                body: String::new(),
                error: None,
            }),
        );
        assert_eq!(e.releases, Availability::Missing);
        assert!(e.failed && e.apps.is_empty());
        assert!(!e.can_check());
    }

    #[test]
    fn selecting_loads_notes_and_a_late_reply_for_another_app_is_ignored() {
        let mut e = listed();
        e.select("demo");
        let first = releases(&e.take_effects());
        assert_eq!(first[0].1, "releases.notes");
        e.select("new");
        let second = releases(&e.take_effects());
        e.released(
            first[0].0,
            ok(json!({"app":"demo","tag":"v2.0","notes":"old"})),
        );
        assert!(e.notes.is_none(), "a superseded notes reply must not land");
        e.released(
            second[0].0,
            ok(json!({"app":"new","tag":"v1.0","notes":"hello"})),
        );
        assert_eq!(e.notes.as_ref().map(|n| n.notes.as_str()), Some("hello"));
    }

    #[test]
    fn install_updates_an_installed_app_and_relists_after() {
        let mut e = listed();
        e.select("demo");
        let notes = releases(&e.take_effects());
        e.released(
            notes[0].0,
            ok(json!({"app":"demo","tag":"v2.0","notes":""})),
        );
        assert!(e.can_install() && e.can_rollback());
        e.install_selected();
        let call = releases(&e.take_effects());
        assert_eq!(
            (call[0].1, call[0].2.clone()),
            ("releases.update", json!({"apps":["demo"]}))
        );
        assert!(e.working().is_some() && !e.can_check());
        e.select("new");
        assert_eq!(
            e.ui.selected.as_deref(),
            Some("demo"),
            "no selection change mid-operation"
        );
        e.released(
            call[0].0,
            ok(json!([{"app":"demo","action":"installed","version":"2.0","previous":"1.0"}])),
        );
        assert_eq!(e.status, "result app=demo action=installed version=2.0");
        let relist = releases(&e.take_effects());
        assert_eq!(
            relist[0].1, "releases.list",
            "a change is followed by a fresh listing"
        );
        e.released(relist[0].0, ok(rows()));
        assert_eq!(
            e.status, "result app=demo action=installed version=2.0",
            "the quiet listing leaves the result on the status line"
        );
    }

    #[test]
    fn a_new_app_is_installed_not_updated() {
        let mut e = listed();
        e.select("new");
        e.take_effects();
        e.install_selected();
        let call = releases(&e.take_effects());
        assert_eq!(
            (call[0].1, call[0].2.clone()),
            ("releases.install", json!({"app":"new"}))
        );
    }

    #[test]
    fn remove_asks_first_and_only_the_confirmation_removes() {
        let mut e = listed();
        e.select("demo");
        e.take_effects();
        e.remove_selected();
        assert_eq!(e.ui.dialog, Some(Dialog::Remove("demo".into())));
        assert!(releases(&e.take_effects()).is_empty());
        e.close_dialog();
        e.confirm_remove();
        assert!(
            releases(&e.take_effects()).is_empty(),
            "a closed dialog confirms nothing"
        );
        e.remove_selected();
        e.confirm_remove();
        let call = releases(&e.take_effects());
        assert_eq!(
            (call[0].1, call[0].2.clone()),
            ("releases.remove", json!({"app":"demo"}))
        );
    }

    #[test]
    fn a_refusal_is_reported_with_its_code_and_the_rows_relisted() {
        let mut e = listed();
        e.select("demo");
        e.take_effects();
        e.rollback_selected();
        let call = releases(&e.take_effects());
        e.released(
            call[0].0,
            Ok(Reply {
                rc: 15,
                body: json!({"error_code":"NO_PREVIOUS","message":"demo has no previous version"})
                    .to_string(),
                error: None,
            }),
        );
        assert!(e.failed);
        assert_eq!(
            e.status,
            "failed message=NO_PREVIOUS: demo has no previous version"
        );
        assert_eq!(releases(&e.take_effects())[0].1, "releases.list");
    }

    #[test]
    fn a_stale_ticket_never_lands() {
        let mut e = listed();
        e.check();
        let call = releases(&e.take_effects());
        e.released(call[0].0 + 100, ok(json!([])));
        assert_eq!(e.apps.len(), 2);
        assert!(e.working().is_some());
    }

    #[test]
    fn a_listing_asked_for_while_busy_runs_after() {
        let mut e = listed();
        e.check();
        let call = releases(&e.take_effects());
        e.delivery(Delivery::Changed);
        assert!(releases(&e.take_effects()).is_empty());
        e.released(call[0].0, ok(rows()));
        assert_eq!(releases(&e.take_effects())[0].1, "releases.list");
    }

    #[test]
    fn quit_waits_for_the_operation_in_flight() {
        let mut e = listed();
        e.update_all();
        let call = releases(&e.take_effects());
        e.quit();
        assert!(!e.take_effects().contains(&Effect::Exit));
        e.released(call[0].0, ok(json!([])));
        assert!(e.take_effects().contains(&Effect::Exit));
    }

    #[test]
    fn verbs_answer_in_place_and_refuse_what_the_window_refuses() {
        let mut e = listed();
        e.command(1, "prefs.select", r#"{"app":"nope"}"#);
        e.command(2, "prefs.select", r#"{"app":"demo"}"#);
        e.command(3, "prefs.panel", r#"{"name":"applications"}"#);
        e.command(4, "prefs.dialog", r#"{"open":"about"}"#);
        e.command(5, "prefs.select", r#"{"app":"new"}"#);
        e.command(6, "prefs.ui.click", r#"{"label":"Check"}"#);
        e.command(7, "prefs.nope", "{}");
        e.command(8, "prefs.info", "[]");
        let effects = e.take_effects();
        let reply = |id: u64| {
            effects.iter().find_map(|x| match x {
                Effect::Reply { id: i, rc, body } if *i == id => Some((*rc, body.clone())),
                _ => None,
            })
        };
        assert_eq!(reply(1).unwrap().1["error_code"], "ARGUMENT");
        assert_eq!(reply(2).unwrap().0, 0);
        assert_eq!(reply(3).unwrap().1["panel"], "applications");
        assert_eq!(reply(4).unwrap().1["dialog"], "about");
        assert_eq!(
            reply(5).unwrap().1["error_code"],
            "BUSY",
            "a dialog refuses selection"
        );
        assert!(
            effects
                .iter()
                .any(|x| matches!(x, Effect::Drive { id: 6, verb, .. } if verb == "ui.click"))
        );
        assert_eq!(reply(7).unwrap().1["error_code"], "UNKNOWN_VERB");
        assert_eq!(reply(8).unwrap().1["error_code"], "ARGUMENT");
    }

    fn lost(message: &str) -> Result<Reply, CallError> {
        Err(CallError {
            message: message.into(),
            outcome_unknown: true,
        })
    }

    fn select_with_notes(e: &mut Engine, app: &str, tag: &str) {
        e.select(app);
        let notes = releases(&e.take_effects());
        e.released(notes[0].0, ok(json!({"app":app,"tag":tag,"notes":""})));
        e.take_effects();
    }

    /// sol (prefs review): a rollback whose reply is lost may have happened;
    /// a second rollback would swap back. Changes stay refused until a
    /// listing shows the real state.
    #[test]
    fn a_lost_rollback_reply_blocks_changes_until_a_listing_settles_it() {
        let mut e = listed();
        select_with_notes(&mut e, "demo", "v2.0");
        e.rollback_selected();
        let call = releases(&e.take_effects());
        e.released(call[0].0, lost("Bus request timed out"));
        assert_eq!(e.uncertain, Some(Op::Rollback("demo".into())));
        assert!(e.failed && e.status.starts_with("uncertain app=demo"));
        assert!(!e.can_rollback() && !e.can_install() && !e.can_check());
        // The reconciling listing went out at once.
        let relist = releases(&e.take_effects());
        assert_eq!(relist[0].1, "releases.list");
        e.released(relist[0].0, ok(rows()));
        assert_eq!(e.uncertain, None);
        assert_eq!(e.status, "settled app=demo");
        assert!(e.can_rollback());
    }

    #[test]
    fn a_failed_reconciliation_keeps_changes_refused_but_refresh_open() {
        let mut e = listed();
        select_with_notes(&mut e, "demo", "v2.0");
        e.install_selected();
        let call = releases(&e.take_effects());
        e.released(call[0].0, lost("connection reset"));
        let relist = releases(&e.take_effects());
        e.released(relist[0].0, lost("connection reset"));
        assert!(e.uncertain.is_some());
        assert!(!e.can_install() && !e.can_rollback() && !e.can_update_all());
        assert!(e.can_refresh(), "the person can try to settle it again");
        e.refresh();
        let retry = releases(&e.take_effects());
        e.released(retry[0].0, ok(rows()));
        assert!(e.uncertain.is_none() && e.can_install());
    }

    #[test]
    fn quitting_while_uncertain_settles_it_first() {
        let mut e = listed();
        select_with_notes(&mut e, "demo", "v2.0");
        e.rollback_selected();
        let call = releases(&e.take_effects());
        e.released(call[0].0, lost("timed out"));
        let relist = releases(&e.take_effects());
        // The reconciling listing fails; quitting tries once more, then closes.
        e.released(relist[0].0, lost("timed out"));
        e.take_effects();
        e.quit();
        let effects = e.take_effects();
        let settle = releases(&effects);
        assert_eq!(settle[0].1, "releases.list");
        assert!(!effects.contains(&Effect::Exit));
        e.released(settle[0].0, ok(rows()));
        assert!(e.take_effects().contains(&Effect::Exit));
        assert!(e.uncertain.is_none());
    }

    /// sol (prefs review 2): quitting gets one settling listing; service
    /// churn while it fails must not queue more, or the window never closes.
    #[test]
    fn churn_during_a_failed_quit_settlement_does_not_keep_the_window_open() {
        let mut e = listed();
        select_with_notes(&mut e, "demo", "v2.0");
        e.rollback_selected();
        let call = releases(&e.take_effects());
        e.released(call[0].0, lost("timed out"));
        let relist = releases(&e.take_effects());
        e.released(relist[0].0, lost("timed out"));
        e.take_effects();
        e.quit();
        let settle = releases(&e.take_effects());
        assert_eq!(settle.len(), 1);
        e.delivery(Delivery::Changed);
        e.delivery(Delivery::Changed);
        assert!(
            releases(&e.take_effects()).is_empty(),
            "no listing queued while quitting"
        );
        e.released(settle[0].0, lost("timed out"));
        let effects = e.take_effects();
        assert!(
            releases(&effects).is_empty(),
            "nothing more after the one attempt"
        );
        assert!(effects.contains(&Effect::Exit));
    }

    /// sol (prefs review 2): notes asked for the old release, still in
    /// flight when a listing moves the release, must not land.
    #[test]
    fn notes_in_flight_for_an_old_release_are_superseded() {
        let mut e = listed();
        e.select("demo");
        let old = releases(&e.take_effects());
        e.refresh();
        let list = releases(&e.take_effects());
        e.released(
            list[0].0,
            ok(json!([
                {"app":"demo","repo":"o/demo","installed":"1.0","latest":"3.0","published":"2026-10-11","status":"update"}
            ])),
        );
        let reload = releases(&e.take_effects());
        assert_eq!(reload[0].1, "releases.notes");
        e.released(
            old[0].0,
            ok(json!({"app":"demo","tag":"v2.0","notes":"old"})),
        );
        assert!(e.notes.is_none(), "the old release's notes are refused");
        e.released(
            reload[0].0,
            ok(json!({"app":"demo","tag":"v3.0","notes":"new"})),
        );
        assert_eq!(e.notes.as_ref().map(|n| n.tag.as_str()), Some("v3.0"));
    }

    #[test]
    fn a_lost_listing_is_an_ordinary_failure() {
        let mut e = listed();
        e.check();
        let call = releases(&e.take_effects());
        e.released(call[0].0, lost("timed out"));
        assert!(e.uncertain.is_none() && e.failed);
        assert_eq!(e.apps.len(), 2, "rows kept");
    }

    /// sol (prefs review): an overloaded service answers rc 20 with no body
    /// and is still registered; only "not found" means it is gone.
    #[test]
    fn an_overloaded_broker_is_not_a_missing_service() {
        let mut e = listed();
        e.refresh();
        let call = releases(&e.take_effects());
        e.released(
            call[0].0,
            Ok(Reply {
                rc: 20,
                body: String::new(),
                error: Some("Service 'releases' is overloaded".into()),
            }),
        );
        assert_eq!(e.releases, Availability::Available);
        assert_eq!(e.apps.len(), 2, "rows kept");
        assert_eq!(e.status, "failed message=Service 'releases' is overloaded");
        e.refresh();
        let call = releases(&e.take_effects());
        e.released(
            call[0].0,
            Ok(Reply {
                rc: 10,
                body: String::new(),
                error: Some("Service 'releases' not found".into()),
            }),
        );
        assert_eq!(e.releases, Availability::Missing);
    }

    /// sol (prefs review): a check that finds a newer release replaces the
    /// selected app's notes rather than showing the old release's.
    #[test]
    fn a_check_that_advances_the_release_reloads_the_notes() {
        let mut e = listed();
        select_with_notes(&mut e, "demo", "v2.0");
        e.check();
        let call = releases(&e.take_effects());
        e.released(
            call[0].0,
            ok(json!([
                {"app":"demo","repo":"o/demo","installed":"1.0","latest":"3.0","published":"2026-10-11","status":"update"}
            ])),
        );
        assert!(e.notes.is_none(), "the v2.0 notes are gone");
        let reload = releases(&e.take_effects());
        assert_eq!(reload[0].1, "releases.notes");
        e.released(
            reload[0].0,
            ok(json!({"app":"demo","tag":"v3.0","notes":"new"})),
        );
        assert_eq!(e.notes.as_ref().map(|n| n.tag.as_str()), Some("v3.0"));
    }

    #[test]
    fn unchanged_notes_are_not_reloaded() {
        let mut e = listed();
        select_with_notes(&mut e, "demo", "v2.0");
        e.refresh();
        let call = releases(&e.take_effects());
        e.released(call[0].0, ok(rows()));
        assert!(releases(&e.take_effects()).is_empty());
        assert!(e.notes.is_some());
    }

    #[test]
    fn update_all_summarises_several_results() {
        let s = summary(
            label,
            &json!([{"app":"a","action":"installed"},{"app":"b","action":"already current"},{"app":"c","action":"failed","error":"x"}]),
        );
        assert_eq!(s, "updated-all changed=1 failed=1");
        assert_eq!(summary(label, &json!([])), "nothing-to-update");
    }
}
