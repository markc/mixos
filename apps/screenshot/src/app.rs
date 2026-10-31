// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The window's state and rules, with no drawing and no I/O: the document,
//! the one job at a time (a capture, an open, a save, a copy, an
//! activation), the dialogs, and every action, from a menu, a shortcut, a
//! gesture or the Bus. What leaves the process is queued as an [`Effect`]
//! for the shell (or headless mode) to perform; each job carries a ticket,
//! and a completion whose ticket is not the running job's is ignored.
//!
//! Unsaved work is never discarded without asking: an action that would
//! replace or close a changed document opens the Discard dialog (Keep,
//! Save, Discard) in the window; agents' `capture`, `open` and `quit` are
//! refused instead, and headless mode refuses everything that would ask.
//!
//! Ported from Cap's iced reducer (`apps/cap/src/app.rs`, markc/mixos-iced)
//! with the same rules.
use crate::capture::{Captured, Mode, Request, Target, Window};
use crate::commands;
use crate::ink::Ink;
use action::{ExecuteRequest, Origin};
use annotation::library::Saved;
use annotation::{Crop, Document, Kind, Point, Shape};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::{cell::RefCell, rc::Rc};

pub type Commands = toolkit::action::EngineAdapter<App>;
pub type SharedCommands = Rc<RefCell<Commands>>;

/// Localises a catalogue key; the shell supplies it.
pub type Label = fn(&str) -> String;

/// `path` as the status line and tips show it: under the home folder it
/// reads `~/…`.
pub fn shown_path(path: &Path) -> String {
    shown_path_in(path, std::env::var_os("HOME").as_deref().map(Path::new))
}

/// `points` on screen, in image pixels at view `scale` (view points per
/// image pixel), held inside the engine's `limits`.
pub fn to_image(points: f32, scale: f32, limits: &std::ops::RangeInclusive<f32>) -> f32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (points / scale).clamp(*limits.start(), *limits.end())
}

/// [`shown_path`] with `home` given.
pub fn shown_path_in(path: &Path, home: Option<&Path>) -> String {
    match home
        .filter(|h| h.is_absolute() && h.parent().is_some())
        .and_then(|h| path.strip_prefix(h).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// What a press on the picture does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum Tool {
    /// Select, move and delete objects.
    Select,
    /// Drag out the part to keep.
    Crop,
    /// Draw an object of this kind.
    Draw(Kind),
}

impl Tool {
    /// Every tool, in toolbar order.
    pub const ALL: [Tool; 11] = [
        Tool::Select,
        Tool::Crop,
        Tool::Draw(Kind::Arrow),
        Tool::Draw(Kind::Line),
        Tool::Draw(Kind::Rectangle),
        Tool::Draw(Kind::Ellipse),
        Tool::Draw(Kind::Pen),
        Tool::Draw(Kind::Highlighter),
        Tool::Draw(Kind::Redact),
        Tool::Draw(Kind::Text),
        Tool::Draw(Kind::Number),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Crop => "crop",
            Tool::Draw(kind) => kind.name(),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }
}

impl From<Tool> for String {
    fn from(tool: Tool) -> Self {
        tool.name().to_owned()
    }
}

impl TryFrom<String> for Tool {
    type Error = String;
    fn try_from(name: String) -> Result<Self, String> {
        Tool::from_name(&name).ok_or_else(|| format!("unknown tool {name:?}"))
    }
}

/// A modal dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dialog {
    /// Unsaved annotations: Keep editing, Save, Discard.
    Discard,
    /// The path of an image to open.
    Open,
    /// The path to save the PNG at.
    SaveAs,
    Shortcuts,
    About,
}

impl Dialog {
    pub fn name(self) -> &'static str {
        match self {
            Dialog::Discard => "discard",
            Dialog::Open => "open",
            Dialog::SaveAs => "save_as",
            Dialog::Shortcuts => "shortcuts",
            Dialog::About => "about",
        }
    }
}

/// The Discard dialog's answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Keep,
    Save,
    Discard,
}

/// The part of the state a person arranges: readable over the Bus as one
/// value (`screenshot.info`'s `ui`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default)]
    pub macros_open: bool,
    pub tool: Tool,
    /// The ink new objects take, straight sRGB with alpha.
    pub colour: [u8; 4],
    /// Stroke width in image pixels.
    pub width: f32,
    /// The text a Text object takes.
    pub text: String,
    /// Text and marker size in image pixels.
    pub text_size: f32,
    /// Zoom relative to fit, and the pan in view pixels.
    pub zoom: f32,
    pub pan: Point,
    pub mode: Mode,
    pub delay: u32,
    /// Include the pointer in screen and region captures.
    pub cursor: bool,
    pub output: Option<String>,
    pub window: Option<Target>,
    pub dialog: Option<Dialog>,
    /// The Open or Save As dialog's path field.
    pub path: String,
    pub selected: Option<u64>,
}

/// Stroke widths the Annotate menu offers.
pub const WIDTHS: [(&str, &str, f32); 4] = [
    ("thin", "width-thin", 2.0),
    ("medium", "width-medium", 4.0),
    ("thick", "width-thick", 8.0),
    ("heavy", "width-heavy", 16.0),
];

/// Text sizes the Annotate menu offers.
pub const SIZES: [(&str, &str, f32); 4] = [
    ("small", "size-small", 16.0),
    ("medium", "size-medium", 24.0),
    ("large", "size-large", 40.0),
    ("huge", "size-huge", 64.0),
];

/// Delays the Capture menu offers, in seconds.
pub const DELAYS: [u32; 4] = [0, 3, 5, 10];

impl UiState {
    fn new(colour: [u8; 4]) -> Self {
        Self {
            macros_open: false,
            tool: Tool::Draw(Kind::Arrow),
            colour,
            width: WIDTHS[1].2,
            text: String::new(),
            text_size: SIZES[1].2,
            zoom: 1.0,
            pan: Point { x: 0.0, y: 0.0 },
            mode: Mode::Region,
            delay: 0,
            cursor: false,
            output: None,
            window: None,
            dialog: None,
            path: String::new(),
            selected: None,
        }
    }
}

/// An action that may wait on the Discard dialog or a running job.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    Capture(Request),
    /// Show the Open dialog.
    OpenDialog,
    Open(PathBuf),
    Quit,
}

impl Pending {
    /// Whether it replaces or closes the document.
    fn discards(&self) -> bool {
        !matches!(self, Pending::OpenDialog)
    }
}

/// The kinds of job; one runs at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Capture,
    Open,
    Save,
    Copy,
    Show,
}

impl JobKind {
    pub fn name(self) -> &'static str {
        match self {
            JobKind::Capture => "capture",
            JobKind::Open => "open",
            JobKind::Save => "save",
            JobKind::Copy => "copy",
            JobKind::Show => "show",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Job {
    ticket: u64,
    kind: JobKind,
    /// The Bus command waiting on it.
    reply: Option<u64>,
}

/// Where a save goes.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveTo {
    /// The screenshots folder, as `YYYYMMDD_HHMMSS.png`.
    Folder,
    /// Exactly this new file.
    Path(PathBuf),
}

/// Work for the shell. Nothing here has happened yet.
#[derive(Debug)]
pub enum Effect {
    /// Answer Bus command `id`.
    Reply { id: u64, rc: u8, body: Value },
    /// Publish `body` on `<service>.<suffix>`.
    Publish { suffix: &'static str, body: Value },
    /// Run a capture; report [`App::captured`] with `ticket`. With `focus`
    /// the window comes forward afterwards (a person asked). With `hide`
    /// the window is hidden around it.
    Capture {
        ticket: u64,
        request: Request,
        hide: bool,
        focus: bool,
    },
    /// Cancel the running capture.
    Cancel,
    /// Decode `path`; report [`App::opened`].
    Open { ticket: u64, path: PathBuf },
    /// Render and publish `document`; report [`App::saved`].
    Save {
        ticket: u64,
        document: Box<Document>,
        to: SaveTo,
    },
    /// Render `document` onto the clipboard; report [`App::copied`].
    Copy {
        ticket: u64,
        document: Box<Document>,
    },
    /// Restore and focus the window; report [`App::shown`] when `ticket`
    /// is a job, else only do it.
    Show { ticket: Option<u64> },
    /// Read the windows and outputs; report [`App::refreshed`].
    Refresh,
    /// Delete a raw capture the document no longer needs.
    Forget { path: PathBuf },
    /// Answer `id` with the newest `limit` saves.
    Recent { id: u64, limit: usize },
    /// Answer `id` with the command registry's description.
    Commands { id: u64 },
    /// Run registry command `command` and answer `id`.
    Execute {
        id: u64,
        request: ExecuteRequest,
        alias: bool,
    },
    /// Answer `id` with the Bus surface (`HELP`: its commands only).
    Describe { id: u64, help: bool },
    /// Hand window command `command` (`ui.click`, `window`, …) to the drive layer.
    Drive {
        id: u64,
        command: String,
        args: Value,
    },
    /// Stop the Bus connection and close.
    Exit,
}

/// The window's state and rules.
pub struct App {
    pub commands: SharedCommands,
    origin: Origin,
    pub ui: UiState,
    /// The window's own theme choice (View › Theme).
    pub theme: toolkit::theme_menu::Choice,
    pub document: Option<Document>,
    /// The file the document came from: a capture or an opened image.
    pub source: Option<PathBuf>,
    /// The document's last save.
    pub saved: Option<Saved>,
    /// compd's reply for the capture shown.
    pub capture: Value,
    pub status: String,
    pub windows: Vec<Window>,
    pub outputs: Vec<String>,
    /// Why the window and output lists could not be read, if they could not.
    pub windows_error: Option<String>,
    /// Bumps on every change to the document: the preview follows it.
    pub revision: u64,
    /// Bumps when the document is replaced: a render of an older one is
    /// never shown.
    pub document_id: u64,
    /// The raw capture behind the document, deleted once it is saved,
    /// replaced or closed.
    pub capture_file: Option<PathBuf>,
    pub connected: bool,
    pub quitting: bool,
    pub headless: bool,
    /// The compositor this instance captures through (its Bus service).
    pub comp: String,
    pub inks: [Ink; 6],
    /// Command ids Bus commands ran, for `<service>.action.executed`.
    pub ran: Vec<&'static str>,
    /// View points per image pixel as the picture was last laid out (1
    /// until it is): a new object's width and text size are converted
    /// with it.
    pub view_scale: f32,
    job: Option<Job>,
    pending: Option<Pending>,
    effects: Vec<Effect>,
    next_ticket: u64,
    label: Label,
}

/// Refusal codes.
pub mod code {
    pub const ARGUMENT: &str = "ARGUMENT";
    pub const BUSY: &str = "BUSY";
    pub const UNSAVED: &str = "UNSAVED";
    pub const NO_IMAGE: &str = "NO_IMAGE";
    pub const DIALOG: &str = "DIALOG";
    pub const NO_DIALOG: &str = "NO_DIALOG";
    pub const HEADLESS: &str = "HEADLESS";
    pub const FAILED: &str = "FAILED";
    pub const COMPOSITOR: &str = "COMPOSITOR";
    pub const CANCELLED: &str = "CANCELLED";
    pub const UNKNOWN_COMMAND: &str = "UNKNOWN_COMMAND";
}

impl App {
    pub fn new(label: Label, inks: [Ink; 6], headless: bool) -> Self {
        Self {
            commands: Rc::new(RefCell::new(crate::actions::registry())),
            origin: Origin::Ui,
            ui: UiState::new(inks[0].rgba),
            theme: toolkit::theme_menu::Choice::default(),
            document: None,
            source: None,
            saved: None,
            capture: Value::Null,
            status: label("ready"),
            windows: Vec::new(),
            outputs: Vec::new(),
            windows_error: None,
            revision: 0,
            document_id: 0,
            capture_file: None,
            connected: true,
            quitting: false,
            headless,
            comp: "comp".into(),
            inks,
            ran: Vec::new(),
            view_scale: 1.0,
            job: None,
            pending: None,
            effects: Vec::new(),
            next_ticket: 0,
            label,
        }
    }

    /// Whether effects are queued.
    pub fn has_effects(&self) -> bool {
        !self.effects.is_empty()
    }

    /// The effects queued since the last call.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    fn label(&self, key: &str) -> String {
        (self.label)(key)
    }

    fn fail(&mut self, error: impl std::fmt::Display) {
        self.status = format!("{}: {error}", self.label("error"));
    }

    fn next(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }

    // ---- what can run --------------------------------------------------

    /// A job is running.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// The running job.
    pub fn job(&self) -> Option<JobKind> {
        self.job.map(|j| j.kind)
    }

    pub fn capturing(&self) -> bool {
        self.job() == Some(JobKind::Capture)
    }

    pub fn modal(&self) -> bool {
        self.ui.dialog.is_some()
    }

    /// No job, no dialog, not closing.
    pub fn idle(&self) -> bool {
        !self.busy() && !self.modal() && !self.quitting
    }

    /// There is a document and it can be changed now.
    pub fn editable(&self) -> bool {
        self.idle() && self.document.is_some()
    }

    pub fn dirty(&self) -> bool {
        self.document.as_ref().is_some_and(Document::dirty)
    }

    /// The document's export would differ from its last save.
    pub fn unsaved(&self) -> bool {
        self.document.is_some() && (self.dirty() || self.saved.is_none())
    }

    /// The pending action, if any (the Discard dialog's, or after a job).
    pub fn pending(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }

    /// The whole state as `screenshot.info` reports it.
    pub fn info(&self) -> Value {
        json!({
            "schema": commands::SCHEMA, "app_id": crate::APP_ID,
            "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id(),
            "headless": self.headless, "connected": self.connected, "comp": self.comp,
            "busy": self.busy(), "job": self.job().map(JobKind::name),
            "status": self.status,
            "document": self.document.as_ref().map(Document::info),
            "source": self.source, "saved": self.saved, "capture": self.capture,
            "windows": self.windows, "outputs": self.outputs,
            "windows_error": self.windows_error,
            "ui": self.ui,
        })
    }

    // ---- actions ---------------------------------------------------------

    /// The capture the controls describe, in `mode` (else the chosen one).
    pub fn capture_request(&self, mode: Option<Mode>) -> Request {
        let mode = mode.unwrap_or(self.ui.mode);
        Request {
            mode,
            output: (mode != Mode::Window)
                .then(|| self.ui.output.clone())
                .flatten(),
            window: (mode == Mode::Window).then_some(self.ui.window).flatten(),
            cursor: self.ui.cursor,
            delay: self.ui.delay,
        }
    }

    /// Take a screenshot in `mode` (else the chosen one), asking about
    /// unsaved work first.
    pub fn take(&mut self, mode: Option<Mode>) {
        if let Some(mode) = mode {
            self.ui.mode = mode;
        }
        let request = self.capture_request(mode);
        if let Err(error) = request.validate() {
            self.fail(error);
            return;
        }
        self.request(Pending::Capture(request));
    }

    /// Cancel the running capture.
    pub fn cancel(&mut self) {
        if self.capturing() {
            self.effects.push(Effect::Cancel);
            self.status = self.label("cancelling");
        }
    }

    /// Run `action` now, after the Discard dialog, or (quitting) after the
    /// running job.
    pub fn request(&mut self, action: Pending) {
        if self.quitting && action != Pending::Quit {
            return;
        }
        if self.busy() {
            if action == Pending::Quit {
                self.pending = Some(action);
                self.cancel();
            } else {
                self.status = self.label("busy");
            }
            return;
        }
        if self.modal() {
            return;
        }
        if action.discards() && self.dirty() {
            if self.headless {
                self.status = self.label("save-first");
                return;
            }
            self.pending = Some(action);
            self.ui.dialog = Some(Dialog::Discard);
            return;
        }
        self.perform(action);
    }

    fn perform(&mut self, action: Pending) {
        match action {
            Pending::Capture(request) => self.start_capture(request, None),
            Pending::OpenDialog => {
                self.ui.path = self.default_directory();
                self.ui.dialog = Some(Dialog::Open);
            }
            Pending::Open(path) => self.start_open(path, None),
            Pending::Quit => {
                self.quitting = true;
                self.release_capture();
                self.effects.push(Effect::Exit);
            }
        }
    }

    /// New work on the document: an action queued against the old one
    /// never runs on it (only a Quit, which closes whatever is shown,
    /// survives).
    fn supersede(&mut self) {
        if self.pending != Some(Pending::Quit) {
            self.pending = None;
        }
    }

    /// The raw capture behind the document is no longer needed (it was
    /// saved, replaced or closed): delete it.
    fn release_capture(&mut self) {
        if let Some(path) = self.capture_file.take() {
            self.effects.push(Effect::Forget { path });
        }
    }

    fn start(&mut self, kind: JobKind, reply: Option<u64>) -> u64 {
        debug_assert!(self.job.is_none(), "a live job is never replaced");
        let ticket = self.next();
        self.job = Some(Job {
            ticket,
            kind,
            reply,
        });
        ticket
    }

    fn start_capture(&mut self, request: Request, reply: Option<u64>) {
        // The window's controls show what is being taken, whoever asked.
        self.ui.mode = request.mode;
        self.ui.delay = request.delay;
        self.ui.cursor = request.cursor;
        if request.mode == Mode::Window {
            self.ui.window = request.window;
        } else {
            self.ui.output = request.output.clone();
        }
        self.supersede();
        let focus = !self.headless && reply.is_none();
        let ticket = self.start(JobKind::Capture, reply);
        // A region waits on the person: say what to do before the window
        // hides (compd's selection overlay carries no text).
        self.status = self.label(if request.mode == Mode::Region {
            "region-hint"
        } else {
            "capturing"
        });
        self.effects.push(Effect::Capture {
            ticket,
            request,
            hide: !self.headless,
            focus,
        });
    }

    fn start_open(&mut self, path: PathBuf, reply: Option<u64>) {
        let ticket = self.start(JobKind::Open, reply);
        self.status = self.label("working");
        self.effects.push(Effect::Open { ticket, path });
    }

    fn start_save(&mut self, to: SaveTo, reply: Option<u64>) -> bool {
        let Some(document) = self.document.as_ref().map(Document::raster_snapshot) else {
            return false;
        };
        let ticket = self.start(JobKind::Save, reply);
        self.status = self.label("working");
        self.effects.push(Effect::Save {
            ticket,
            document: Box::new(document),
            to,
        });
        true
    }

    /// Save into the screenshots folder (Ctrl+S). A document already saved
    /// there unchanged is not saved twice.
    pub fn save(&mut self) {
        if !self.editable() {
            return;
        }
        if !self.unsaved() {
            let path = self.saved.as_ref().map(|s| shown_path(&s.path));
            self.status =
                crate::label_with("already-saved", &[("path", &path.unwrap_or_default())]);
            return;
        }
        self.start_save(SaveTo::Folder, None);
    }

    /// Open the Save As dialog at the screenshots folder.
    pub fn save_as(&mut self) {
        if !self.editable() || self.headless {
            return;
        }
        let stem = annotation::library::shot_stem(&annotation::library::now_local().0);
        let folder = annotation::library::screenshots_dir().unwrap_or_default();
        self.ui.path = annotation::library::free_name(&folder, &stem)
            .display()
            .to_string();
        self.ui.dialog = Some(Dialog::SaveAs);
    }

    fn default_directory(&self) -> String {
        let folder = annotation::library::screenshots_dir().unwrap_or_default();
        let mut text = folder.display().to_string();
        if !text.ends_with('/') {
            text.push('/');
        }
        text
    }

    /// Copy the annotated image to the clipboard.
    pub fn copy(&mut self) {
        if !self.editable() {
            return;
        }
        let Some(document) = self.document.as_ref().map(Document::raster_snapshot) else {
            return;
        };
        let ticket = self.start(JobKind::Copy, None);
        self.status = self.label("working");
        self.effects.push(Effect::Copy {
            ticket,
            document: Box::new(document),
        });
    }

    pub fn quit(&mut self) {
        self.request(Pending::Quit);
    }

    pub fn open_dialog(&mut self) {
        if self.headless {
            return;
        }
        self.request(Pending::OpenDialog);
    }

    /// Show a help dialog: not under another, while a job runs (the
    /// window may be hidden by a capture) or while closing. A dialog may
    /// still sit on a window the person minimised; Print or
    /// `screenshot.show` brings it back.
    pub fn open(&mut self, dialog: Dialog) {
        if self.idle() && !self.headless {
            self.ui.dialog = Some(dialog);
        }
    }

    /// Close the open dialog as Escape or its close button does: Keep for
    /// the Discard dialog (its pending action is dropped).
    pub fn close_dialog(&mut self) {
        if self.ui.dialog == Some(Dialog::Discard) {
            self.pending = None;
        }
        self.ui.dialog = None;
        // An action that waited behind the dialog (a Quit asked for during
        // a job that has since finished) runs now.
        if !self.busy() {
            self.followup();
        }
    }

    /// The Discard dialog's answer.
    pub fn answer(&mut self, answer: Answer) {
        if self.ui.dialog != Some(Dialog::Discard) {
            return;
        }
        // A job runs: its ticket is never replaced; the dialog stays.
        if self.busy() && answer != Answer::Keep {
            self.status = self.label("busy");
            return;
        }
        self.ui.dialog = None;
        match answer {
            Answer::Keep => self.pending = None,
            Answer::Discard => {
                if let Some(action) = self.pending.take() {
                    self.perform(action);
                }
            }
            // Save into the folder, then carry on.
            Answer::Save => {
                if !self.start_save(SaveTo::Folder, None) {
                    self.pending = None;
                }
            }
        }
    }

    /// The Open or Save As dialog's path, submitted.
    pub fn submit_path(&mut self) {
        let Some(dialog) = self.ui.dialog else { return };
        if self.busy() {
            self.status = self.label("busy");
            return;
        }
        let text = self.ui.path.trim().to_owned();
        let path = match crate::capture::absolute(&expand_home(&text)) {
            Ok(path) if !text.is_empty() && !text.ends_with('/') => path,
            _ => {
                self.status = self.label("choose-file");
                return;
            }
        };
        match dialog {
            Dialog::Open => {
                self.ui.dialog = None;
                self.request(Pending::Open(path));
            }
            Dialog::SaveAs => {
                if path.exists() || path.is_symlink() {
                    self.status = self.label("exists");
                    return;
                }
                self.ui.dialog = None;
                self.start_save(SaveTo::Path(path), None);
            }
            _ => {}
        }
    }

    /// All hand and Bus edits use the same registry and journal. An immutable
    /// adapter borrow permits shell commands to dispatch an engine command.
    pub fn execute(
        &mut self,
        id: &str,
        params: Value,
        origin: Origin,
    ) -> Result<Value, action::Error> {
        let commands = self.commands.clone();
        let previous = std::mem::replace(&mut self.origin, origin);
        let result = commands.borrow().execute(id, self, params, origin);
        self.origin = previous;
        result
    }

    fn edit_command(&mut self, id: &str, params: Value) -> Option<Value> {
        match self.execute(id, params, self.origin) {
            Ok(value) => Some(value),
            Err(error) => {
                match error {
                    action::Error::BadParams { msg, .. } | action::Error::Failed(msg) => {
                        self.fail(msg);
                    }
                    action::Error::Disabled { reason, .. } => self.fail(reason),
                    other => self.fail(other),
                }
                None
            }
        }
    }

    pub fn undo(&mut self) {
        self.edit_command("edit.undo", Value::Null);
    }
    pub fn redo(&mut self) {
        self.edit_command("edit.redo", Value::Null);
    }
    pub fn delete_selected(&mut self) {
        if let Some(id) = self.ui.selected {
            self.edit_command("edit.delete", json!({"id": id}));
        }
    }
    pub fn uncrop(&mut self) {
        self.edit_command("edit.crop", json!({"crop": null}));
    }
    pub fn deselect(&mut self) {
        self.ui.selected = None;
    }

    /// The document changed: the preview follows, a vanished selection goes.
    fn changed(&mut self) {
        self.revision += 1;
        if let (Some(id), Some(doc)) = (self.ui.selected, &self.document)
            && doc.object(id).is_none()
        {
            self.ui.selected = None;
        }
    }

    pub fn set_tool(&mut self, tool: Tool) {
        if !self.modal() {
            self.ui.tool = tool;
            if tool != Tool::Select {
                self.ui.selected = None;
            }
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if !self.busy() {
            self.ui.mode = mode;
        }
    }

    pub fn set_delay(&mut self, delay: u32) {
        if !self.busy() && delay <= crate::capture::MAX_DELAY {
            self.ui.delay = delay;
        }
    }

    pub fn toggle_pointer(&mut self) {
        if !self.busy() {
            self.ui.cursor = !self.ui.cursor;
        }
    }

    pub fn choose_window(&mut self, window: Option<Target>) {
        if !self.busy() {
            self.ui.window = window;
        }
    }

    pub fn choose_output(&mut self, output: Option<String>) {
        if !self.busy() {
            self.ui.output = output;
        }
    }

    /// The next output in compd's list (Capture › Output › Next output),
    /// wrapping; from "the pointer's", the first.
    pub fn next_output(&mut self) {
        if self.busy() || self.outputs.is_empty() {
            return;
        }
        let at = self
            .ui
            .output
            .as_ref()
            .and_then(|o| self.outputs.iter().position(|n| n == o))
            .map_or(0, |i| (i + 1) % self.outputs.len());
        self.ui.output = Some(self.outputs[at].clone());
    }

    /// Ask compd for its windows and outputs again.
    pub fn refresh(&mut self) {
        self.effects.push(Effect::Refresh);
    }

    /// The ink `colour` for new objects, and for the selected one (which
    /// keeps its width).
    pub fn set_colour(&mut self, colour: [u8; 4]) {
        if self.modal() {
            return;
        }
        self.ui.colour = colour;
        self.restyle(false);
    }

    /// The stroke width (a preset, in view points) for new objects, and
    /// for the selected one.
    pub fn set_width(&mut self, width: f32) {
        if self.modal() || !(0.5..=128.0).contains(&width) {
            return;
        }
        self.ui.width = width;
        self.restyle(true);
    }

    /// How many view points one image pixel takes on screen now; the view
    /// reports it as it lays the picture out.
    pub fn set_view_scale(&mut self, scale: f32) {
        if scale.is_finite() && scale > 0.0 {
            self.view_scale = scale;
        }
    }

    /// The width a new or restyled object takes, in image pixels: the
    /// chosen width as it shows on screen at the current view scale, so a
    /// stroke looks the same whatever the image's size.
    pub fn stroke_width(&self) -> f32 {
        self.stroke_width_at(self.view_scale)
    }

    /// [`App::stroke_width`] at view `scale` (a gesture passes its own
    /// frame's).
    pub fn stroke_width_at(&self, scale: f32) -> f32 {
        to_image(self.ui.width, scale, &annotation::document::WIDTH)
    }

    /// The text size a new text box or marker takes, in image pixels (as
    /// [`App::stroke_width`]).
    pub fn text_px(&self) -> f32 {
        self.text_px_at(self.view_scale)
    }

    /// [`App::text_px`] at view `scale`.
    pub fn text_px_at(&self, scale: f32) -> f32 {
        to_image(self.ui.text_size, scale, &annotation::document::TEXT_SIZE)
    }

    pub fn set_text_size(&mut self, size: f32) {
        if !self.modal() && (8.0..=256.0).contains(&size) {
            self.ui.text_size = size;
        }
    }

    pub fn set_text(&mut self, text: String) {
        if text.len() <= 4096 {
            self.ui.text = text;
        }
    }

    pub fn set_path(&mut self, path: String) {
        if path.len() <= 4096 {
            self.ui.path = path;
        }
    }

    /// Give the selected object the ink and, with `width`, the chosen
    /// width converted as for a new object; otherwise it keeps its own.
    fn restyle(&mut self, width: bool) {
        if !self.editable() {
            return;
        }
        let colour = self.ui.colour;
        let new_width = width.then(|| self.stroke_width());
        if let (Some(id), Some(doc)) = (self.ui.selected, self.document.as_mut())
            && let Some((had_colour, had_width)) =
                doc.object(id).map(|o| (o.shape.colour, o.shape.width))
        {
            let width = new_width.unwrap_or(had_width);
            if (had_colour != colour || had_width != width)
                && doc.restyle(id, colour, width).is_ok()
            {
                self.changed();
            }
        }
    }

    pub fn zoom_by(&mut self, factor: f32) {
        if self.document.is_some() {
            self.ui.zoom = (self.ui.zoom * factor).clamp(
                *annotation::viewport::ZOOM.start(),
                *annotation::viewport::ZOOM.end(),
            );
        }
    }

    pub fn fit(&mut self) {
        self.ui.zoom = 1.0;
        self.ui.pan = Point { x: 0.0, y: 0.0 };
    }

    // ---- gestures ----------------------------------------------------------

    /// Add `shape` (from a drag): text and markers take the controls' text
    /// and next number, and the converted text size unless the gesture
    /// already set it at its own frame's scale. The new object is selected.
    pub fn add_shape(&mut self, mut shape: Shape) {
        if !self.editable() {
            return;
        }
        let size = shape.size.unwrap_or_else(|| self.text_px());
        match shape.kind {
            Kind::Text => {
                if self.ui.text.trim().is_empty() {
                    self.status = self.label("text-needed");
                    return;
                }
                shape.text = Some(self.ui.text.clone());
                shape.size = Some(size);
            }
            Kind::Number => {
                shape.number = self.document.as_ref().and_then(Document::next_number);
                if shape.number.is_none() {
                    self.status = self.label("marker-limit");
                    return;
                }
                shape.size = Some(size);
            }
            _ => {}
        }
        if let Some(result) = self.edit_command(
            "edit.annotate",
            serde_json::to_value(shape).expect("finite UI shape"),
        ) {
            self.ui.selected = result["id"].as_u64();
        }
    }

    pub fn crop(&mut self, crop: Crop) {
        self.edit_command("edit.crop", json!({"crop": crop}));
    }

    pub fn move_object(&mut self, id: u64, dx: f32, dy: f32) {
        self.edit_command("edit.move", json!({"id": id, "dx": dx, "dy": dy}));
    }
    pub fn select(&mut self, id: Option<u64>) {
        if self.editable() {
            self.ui.selected = id;
        }
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        if self.document.is_some() && !self.modal() {
            self.ui.pan.x += dx;
            self.ui.pan.y += dy;
        }
    }

    // ---- completions -------------------------------------------------------

    /// Finish job `ticket` if it is the running one.
    fn finish(&mut self, ticket: u64) -> Option<Job> {
        if self.job.is_some_and(|j| j.ticket == ticket) {
            self.job.take()
        } else {
            None
        }
    }

    fn reply(&mut self, id: Option<u64>, result: Result<Value, (&str, String)>) {
        let Some(id) = id else { return };
        match result {
            Ok(body) => self.effects.push(Effect::Reply { id, rc: 0, body }),
            Err((code, message)) => self.error(id, code, &message),
        }
    }

    fn error(&mut self, id: u64, code: &str, message: &str) {
        self.effects.push(Effect::Reply {
            id,
            rc: 10,
            body: json!({"error_code": code, "message": message}),
        });
    }

    /// Replace the document (a capture or an open).
    fn replace(&mut self, document: Document, source: PathBuf, capture: Value) {
        self.supersede();
        self.release_capture();
        self.document_id += 1;
        self.document = Some(document);
        self.source = Some(source);
        self.capture = capture;
        self.saved = None;
        self.ui.selected = None;
        self.fit();
        self.changed();
    }

    /// Capture `ticket` finished: whether the window it hid stayed hidden
    /// after every retry (the shot is kept; the person brings the window
    /// back with Print, `screenshot.show` or the taskbar), and whether it
    /// was cancelled (whatever else the error says).
    pub fn captured_job(
        &mut self,
        ticket: u64,
        result: Result<Captured, String>,
        restore_failed: bool,
        cancelled: bool,
    ) {
        let Some(job) = self.finish(ticket) else {
            return;
        };
        let reply = match result {
            Ok(captured) => {
                let path = captured.path.clone();
                self.replace(captured.document, captured.path, captured.metadata);
                self.capture_file = Some(path);
                self.status = self.label(if restore_failed {
                    "restore-failed"
                } else {
                    "capture-complete"
                });
                Ok(self.info())
            }
            Err(error) if cancelled => {
                self.status = self.label("cancelled");
                Err((code::CANCELLED, error))
            }
            Err(error) => {
                // The selection's own endings read as what happened; the
                // Bus answer keeps compd's words.
                match crate::capture::selection_ending(&error) {
                    Some("timeout") => self.status = self.label("nothing-selected"),
                    Some("cancelled") => self.status = self.label("cancelled"),
                    _ => self.fail(&error),
                }
                Err((code::FAILED, error))
            }
        };
        if restore_failed {
            self.status = self.label("restore-failed");
        }
        // Success or failure, the answer says the window stayed hidden.
        if let Some(id) = job.reply {
            let (rc, mut body) = match reply {
                Ok(info) => (0, info),
                Err((code, message)) => (10, json!({"error_code": code, "message": message})),
            };
            if restore_failed {
                body["restore_failed"] = json!(true);
            }
            self.effects.push(Effect::Reply { id, rc, body });
        }
        // Windows come and go: the list is read again after every capture.
        self.refresh();
        self.followup();
    }

    /// Open `ticket` finished.
    pub fn opened(&mut self, ticket: u64, result: Result<(PathBuf, Document), String>) {
        let Some(job) = self.finish(ticket) else {
            return;
        };
        let reply = match result {
            Ok((path, document)) => {
                self.replace(document, path, Value::Null);
                self.status = self.label("ready");
                Ok(self.info())
            }
            Err(error) => {
                self.fail(&error);
                Err((code::FAILED, error))
            }
        };
        self.reply(job.reply, reply);
        self.followup();
    }

    /// Save `ticket` finished: the document is marked saved and the save
    /// published on `<service>.saved`.
    pub fn saved(&mut self, ticket: u64, result: Result<Saved, String>) {
        let Some(job) = self.finish(ticket) else {
            return;
        };
        let reply = match result {
            Ok(saved) => {
                if let Some(doc) = self.document.as_mut() {
                    doc.mark_saved();
                }
                // The pixels are kept in the saved file now.
                self.release_capture();
                // The status line names the file, so the person sees where
                // it went.
                self.status = crate::label_with("saved-to", &[("path", &shown_path(&saved.path))]);
                let body = serde_json::to_value(&saved).unwrap_or(Value::Null);
                self.effects.push(Effect::Publish {
                    suffix: "saved",
                    body: body.clone(),
                });
                self.saved = Some(saved);
                let mut info = self.info();
                info["saved"] = body;
                Ok(info)
            }
            Err(error) => {
                // A failed save leaves whatever waited on it undone.
                self.pending = None;
                self.fail(&error);
                Err((code::FAILED, error))
            }
        };
        self.reply(job.reply, reply);
        self.followup();
    }

    /// Copy `ticket` finished.
    pub fn copied(&mut self, ticket: u64, result: Result<(), String>) {
        if self.finish(ticket).is_none() {
            return;
        }
        match result {
            Ok(()) => self.status = self.label("copied"),
            Err(error) => self.fail(error),
        }
        self.followup();
    }

    /// Activation `ticket` finished.
    pub fn shown(&mut self, ticket: u64, result: Result<Value, String>) {
        let Some(job) = self.finish(ticket) else {
            return;
        };
        let reply = result.map_err(|e| (code::FAILED, e));
        if let Err((_, error)) = &reply {
            let error = error.clone();
            self.fail(error);
        }
        self.reply(job.reply, reply);
        self.followup();
    }

    /// The window was brought forward outside a job (Print over a
    /// dialog). A failure is only reported: the dialog stays, and the next
    /// Print brings it forward again.
    pub fn raised(&mut self, result: Result<Value, String>) {
        if let Err(error) = result {
            self.fail(error);
        }
    }

    /// compd's windows and outputs arrived.
    pub fn refreshed(&mut self, result: Result<(Vec<Window>, Vec<String>), String>) {
        match result {
            Ok((windows, outputs)) => {
                if self
                    .ui
                    .window
                    .is_none_or(|t| !windows.iter().any(|w| w.target == t))
                {
                    self.ui.window = windows
                        .iter()
                        .find(|w| w.focused)
                        .or_else(|| windows.first())
                        .map(|w| w.target);
                }
                if self
                    .ui
                    .output
                    .as_ref()
                    .is_some_and(|o| !outputs.contains(o))
                {
                    self.ui.output = None;
                }
                // One output: nothing to choose.
                if let [only] = outputs.as_slice() {
                    self.ui.output = Some(only.clone());
                }
                self.windows = windows;
                self.outputs = outputs;
                self.windows_error = None;
            }
            // Reported in `screenshot.info`, not the status line: a compd
            // without the window list still captures screens and regions.
            Err(error) => self.windows_error = Some(error),
        }
    }

    /// After a job: the action that waited on it.
    fn followup(&mut self) {
        if self.ui.dialog.is_none()
            && let Some(action) = self.pending.take()
        {
            self.request(action);
        }
    }

    /// The Bus connection came or went.
    pub fn connection(&mut self, connected: bool) {
        self.connected = connected;
        if connected {
            self.refresh();
        } else {
            self.status = self.label("disconnected");
        }
    }

    // ---- the Bus -------------------------------------------------------------

    /// A command an agent sent over the Bus.
    pub fn command(&mut self, id: u64, command_name: &str, body: &str) {
        if body.len() > 1024 * 1024 {
            return self.error(id, code::ARGUMENT, "command exceeds 1 MiB");
        }
        let args =
            match serde_json::from_str::<Value>(if body.trim().is_empty() { "{}" } else { body }) {
                Ok(args) if args.is_object() => args,
                _ => return self.error(id, code::ARGUMENT, "arguments must be a JSON object"),
            };
        if let Some(window) = command_name
            .strip_prefix("screenshot.")
            .filter(|v| v.starts_with("ui.") || v.starts_with("window"))
        {
            if self.headless {
                return self.error(id, code::HEADLESS, "a headless instance has no window");
            }
            return self.effects.push(Effect::Drive {
                id,
                command: window.to_owned(),
                args,
            });
        }
        if !commands::COMMANDS.contains(&command_name) {
            return self.error(id, code::UNKNOWN_COMMAND, "unknown screenshot command");
        }
        if commands::takes_nothing(command_name) && args.as_object().is_some_and(|a| !a.is_empty())
        {
            return self.error(id, code::ARGUMENT, "this command takes an empty object");
        }
        match command_name {
            "HELP" => return self.effects.push(Effect::Describe { id, help: true }),
            "app.describe" => return self.effects.push(Effect::Describe { id, help: false }),
            "screenshot.ping" => {
                return self.effects.push(Effect::Reply {
                    id,
                    rc: 0,
                    body: json!({"schema": commands::SCHEMA, "version": env!("CARGO_PKG_VERSION"), "comp": self.comp, "headless": self.headless}),
                });
            }
            "screenshot.info" => {
                let body = self.info();
                return self.effects.push(Effect::Reply { id, rc: 0, body });
            }
            "screenshot.recent" => {
                return match args.get("limit").map(Value::as_u64) {
                    None => self.effects.push(Effect::Recent { id, limit: 10 }),
                    Some(Some(n @ 1..=100)) => self.effects.push(Effect::Recent {
                        id,
                        limit: n as usize,
                    }),
                    Some(_) => self.error(id, code::ARGUMENT, "limit must be 1..100"),
                };
            }
            "screenshot.actions" => return self.effects.push(Effect::Commands { id }),
            "screenshot.execute" => {
                return match serde_json::from_value::<ExecuteRequest>(args) {
                    Ok(request) => self.effects.push(Effect::Execute {
                        id,
                        request,
                        alias: false,
                    }),
                    Err(error) => self.error(id, code::ARGUMENT, &error.to_string()),
                };
            }
            "screenshot.cancel" => {
                let cancelling = self.capturing();
                self.cancel();
                if cancelling {
                    self.ran.push("capture.cancel");
                }
                return self.effects.push(Effect::Reply {
                    id,
                    rc: 0,
                    body: json!({"cancelling": cancelling}),
                });
            }
            "screenshot.theme" => return self.theme_command(id, &args),
            _ => {}
        }
        if self.quitting {
            let message = self.label("quitting");
            return self.error(id, code::BUSY, &message);
        }
        // Print Screen answers for itself under a dialog or a job.
        if command_name == "screenshot.take" {
            return self.take_command(id, &args);
        }
        // The dialog command answers the dialog itself, so it is never
        // refused because one is open.
        if command_name == "screenshot.dialog" {
            return self.dialog_command(id, &args);
        }
        if self.modal() && command_name != "screenshot.show" {
            let message = self.label("dialog-open");
            return self.error(id, code::DIALOG, &message);
        }
        if self.busy() {
            let message = self.label("busy");
            return self.error(id, code::BUSY, &message);
        }
        if commands::is_edit(command_name) {
            if self.document.is_none() {
                return self.error(id, code::NO_IMAGE, "no image");
            }
            let name = command_name
                .strip_prefix("screenshot.")
                .expect("edit command");
            return self.effects.push(Effect::Execute {
                id,
                request: ExecuteRequest {
                    id: format!("edit.{name}"),
                    params: (!matches!(name, "undo" | "redo")).then_some(args),
                },
                alias: true,
            });
        }
        let unsaved = || {
            (
                code::UNSAVED,
                "save, export or undo the unsaved annotations first".to_owned(),
            )
        };
        match command_name {
            "screenshot.capture" => {
                let request = serde_json::from_value::<Request>(args)
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.validate().map(|()| r));
                match request {
                    Err(error) => self.error(id, code::ARGUMENT, &error),
                    Ok(_) if self.dirty() => self.reply(Some(id), Err(unsaved())),
                    Ok(request) => self.start_capture(request, Some(id)),
                }
            }
            "screenshot.open" => match args.get("path").and_then(Value::as_str) {
                Some(path) if args.as_object().is_some_and(|a| a.len() == 1) => {
                    match crate::capture::absolute(path) {
                        Ok(_) if self.dirty() => self.reply(Some(id), Err(unsaved())),
                        Ok(path) => self.start_open(path, Some(id)),
                        Err(error) => self.error(id, code::ARGUMENT, &error),
                    }
                }
                _ => self.error(id, code::ARGUMENT, "expected {path}"),
            },
            "screenshot.export" => match args.get("path").and_then(Value::as_str) {
                Some(path) if args.as_object().is_some_and(|a| a.len() == 1) => {
                    match crate::capture::absolute(path) {
                        Ok(path) => {
                            if !self.start_save(SaveTo::Path(path), Some(id)) {
                                self.error(id, code::NO_IMAGE, "no image");
                            }
                        }
                        Err(error) => self.error(id, code::ARGUMENT, &error),
                    }
                }
                _ => self.error(id, code::ARGUMENT, "expected {path}"),
            },
            "screenshot.save" => {
                if self.start_save(SaveTo::Folder, Some(id)) {
                    self.ran.push("file.save");
                } else {
                    self.error(id, code::NO_IMAGE, "no image");
                }
            }
            "screenshot.select" => match self.select_command(&args) {
                Ok(()) => {
                    let body = json!({"ui": self.ui});
                    self.effects.push(Effect::Reply { id, rc: 0, body });
                }
                Err(error) => self.error(id, code::ARGUMENT, &error),
            },
            "screenshot.show" => {
                if self.headless {
                    self.error(id, code::HEADLESS, "a headless instance has no window");
                } else {
                    let ticket = self.start(JobKind::Show, Some(id));
                    self.effects.push(Effect::Show {
                        ticket: Some(ticket),
                    });
                }
            }
            "screenshot.quit" => {
                if self.dirty() {
                    self.reply(Some(id), Err(unsaved()));
                } else {
                    self.effects.push(Effect::Reply {
                        id,
                        rc: 0,
                        body: json!({"quitting": true}),
                    });
                    self.ran.push("file.quit");
                    self.quit();
                }
            }
            _ => self.error(id, code::UNKNOWN_COMMAND, "unknown screenshot command"),
        }
    }

    /// `screenshot.take`: as Print Screen. Unsaved work brings the window
    /// forward with the Discard dialog; headless refuses instead.
    fn take_command(&mut self, id: u64, args: &Value) {
        if let Some(key) = args.as_object().and_then(|a| {
            a.keys()
                .find(|k| !matches!(k.as_str(), "mode" | "window" | "comp"))
        }) {
            return self.error(id, code::ARGUMENT, &format!("unknown argument {key}"));
        }
        // A launcher forwarding Print Screen names its compositor: a name
        // shared through a hash collision never captures the wrong one.
        match args.get("comp") {
            None => {}
            Some(Value::String(comp)) if *comp == self.comp => {}
            Some(Value::String(comp)) => {
                let message = format!(
                    "this instance captures through compositor {:?}, not {comp:?}",
                    self.comp
                );
                return self.error(id, code::COMPOSITOR, &message);
            }
            Some(_) => return self.error(id, code::ARGUMENT, "comp must be a string"),
        }
        let mode = match args.get("mode") {
            None => None,
            Some(Value::String(name)) => match Mode::from_name(name) {
                Some(mode) => Some(mode),
                None => {
                    return self.error(id, code::ARGUMENT, "mode must be screen, window or region");
                }
            },
            Some(_) => return self.error(id, code::ARGUMENT, "mode must be a string"),
        };
        let window = match args
            .get("window")
            .map(|w| serde_json::from_value::<Target>(w.clone()))
        {
            None => None,
            Some(Ok(target)) => Some(target),
            Some(Err(error)) => return self.error(id, code::ARGUMENT, &error.to_string()),
        };
        let mode = mode.unwrap_or(self.ui.mode);
        if window.is_some() && mode != Mode::Window {
            return self.error(id, code::ARGUMENT, "window only applies to window mode");
        }
        // Under a dialog the person is being asked something: bring it
        // forward rather than fail the key press.
        if self.modal() {
            self.effects.push(Effect::Show { ticket: None });
            return self.effects.push(Effect::Reply {
                id,
                rc: 0,
                body: json!({"state": "asking"}),
            });
        }
        // A second Print during a capture: say so, in the answer and the
        // window's status line.
        if self.busy() {
            self.status = self.label(if self.capturing() {
                "capture-in-progress"
            } else {
                "busy"
            });
            return self.effects.push(Effect::Reply {
                id,
                rc: 0,
                body: json!({"state": "in_progress", "job": self.job().map(JobKind::name)}),
            });
        }
        if self.quitting {
            let message = self.label("quitting");
            return self.error(id, code::BUSY, &message);
        }
        if self.headless && self.dirty() {
            let message = self.label("save-first");
            return self.error(id, code::UNSAVED, &message);
        }
        // Checked whole before anything changes.
        let mut request = self.capture_request(Some(mode));
        if window.is_some() {
            request.window = window;
        }
        if let Err(error) = request.validate() {
            return self.error(id, code::ARGUMENT, &error);
        }
        if window.is_some() {
            self.ui.window = window;
        }
        self.take(Some(mode));
        let state = if self.capturing() {
            "capturing"
        } else if self.ui.dialog == Some(Dialog::Discard) {
            // The person decides in the window: bring it forward.
            self.effects.push(Effect::Show { ticket: None });
            "asking"
        } else {
            let message = self.status.clone();
            return self.error(id, code::FAILED, &message);
        };
        self.effects.push(Effect::Reply {
            id,
            rc: 0,
            body: json!({"state": state}),
        });
    }

    /// `screenshot.select`: every present key is checked before any applies.
    fn select_command(&mut self, args: &Value) -> Result<(), String> {
        let object = args.as_object().ok_or("expected an object")?;
        if let Some(key) = object
            .keys()
            .find(|k| !matches!(k.as_str(), "tool" | "mode" | "window" | "output"))
        {
            return Err(format!("unknown argument {key}"));
        }
        let tool = match args.get("tool") {
            None => None,
            Some(v) => Some(
                v.as_str()
                    .and_then(Tool::from_name)
                    .ok_or("tool must be select, crop or an annotation kind")?,
            ),
        };
        let mode = match args.get("mode") {
            None => None,
            Some(v) => Some(
                v.as_str()
                    .and_then(Mode::from_name)
                    .ok_or("mode must be screen, window or region")?,
            ),
        };
        let window = match args.get("window") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(v) => Some(Some(
                serde_json::from_value::<Target>(v.clone()).map_err(|e| e.to_string())?,
            )),
        };
        let output = match args.get("output") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(name)) if !name.is_empty() && name.len() <= 256 => {
                Some(Some(name.clone()))
            }
            Some(_) => return Err("output must be a name or null".into()),
        };
        if let Some(tool) = tool {
            self.set_tool(tool);
        }
        if let Some(mode) = mode {
            self.set_mode(mode);
        }
        if let Some(window) = window {
            self.choose_window(window);
        }
        if let Some(output) = output {
            self.choose_output(output);
        }
        Ok(())
    }

    /// The open dialog as `screenshot.dialog` reports it: its kind, title and
    /// buttons (the ids `screenshot.dialog {answer}` takes, Default first),
    /// and for Open and Save As the path field and the status line.
    pub fn dialog_view(&self) -> Value {
        let Some(dialog) = self.ui.dialog else {
            return Value::Null;
        };
        let button = |id: &str, label: &str, role: &str| json!({"id": id, "label": self.label(label), "role": role});
        let (title, buttons) = match dialog {
            Dialog::Discard => (
                "discard-title",
                vec![
                    button("save", "save", "default"),
                    button("discard", "discard", "other"),
                    button("keep", "keep", "cancel"),
                ],
            ),
            Dialog::Open => (
                "open-title",
                vec![
                    button("open", "open-button", "default"),
                    button("cancel", "cancel", "cancel"),
                ],
            ),
            Dialog::SaveAs => (
                "save-title",
                vec![
                    button("save", "save-button", "default"),
                    button("cancel", "cancel", "cancel"),
                ],
            ),
            Dialog::Shortcuts => ("shortcuts", vec![button("done", "done", "default")]),
            Dialog::About => ("about", vec![button("done", "done", "default")]),
        };
        let mut view = json!({"kind": dialog, "title": self.label(title), "buttons": buttons});
        match dialog {
            Dialog::Discard => view["body"] = json!(self.label("discard-body")),
            Dialog::Open | Dialog::SaveAs => {
                view["path"] = json!(self.ui.path);
                view["status"] = json!(self.status);
            }
            _ => {}
        }
        view
    }

    /// `screenshot.dialog`: read the open dialog (`{}`), open a help dialog
    /// or close the open one (`{open: "about" | "shortcuts" | null}`, null
    /// as Escape does), or press one of its buttons by id (`{answer, path?}`;
    /// `path` fills the Open or Save As field first). Answered while a dialog
    /// is open: it is how an agent clears one without driving the window.
    fn dialog_command(&mut self, id: u64, args: &Value) {
        let args = args.as_object().expect("checked object");
        if let Some(key) = args
            .keys()
            .find(|k| !matches!(k.as_str(), "open" | "answer" | "path"))
        {
            let message = format!("unknown field {key:?}");
            return self.error(id, code::ARGUMENT, &message);
        }
        match (args.get("open"), args.get("answer")) {
            (Some(_), Some(_)) => {
                return self.error(id, code::ARGUMENT, "give open or answer, not both");
            }
            (Some(open), None) => {
                if args.contains_key("path") {
                    return self.error(id, code::ARGUMENT, "path goes with answer");
                }
                let dialog = match open {
                    Value::Null => None,
                    Value::String(name) if name == "about" => Some(Dialog::About),
                    Value::String(name) if name == "shortcuts" => Some(Dialog::Shortcuts),
                    _ => {
                        return self.error(
                            id,
                            code::ARGUMENT,
                            "open must be \"about\", \"shortcuts\" or null",
                        );
                    }
                };
                match dialog {
                    None => {
                        if self.modal() {
                            self.close_dialog();
                        }
                    }
                    Some(_) if self.headless => {
                        return self.error(id, code::HEADLESS, "a headless instance has no window");
                    }
                    Some(_) if self.modal() => {
                        let message = self.label("dialog-open");
                        return self.error(id, code::DIALOG, &message);
                    }
                    Some(_) if !self.idle() => {
                        let message = self.label("busy");
                        return self.error(id, code::BUSY, &message);
                    }
                    Some(dialog) => self.open(dialog),
                }
            }
            (None, Some(answer)) => {
                let Some(dialog) = self.ui.dialog else {
                    let message = self.label("no-dialog");
                    return self.error(id, code::NO_DIALOG, &message);
                };
                let Some(answer) = answer.as_str() else {
                    return self.error(id, code::ARGUMENT, "answer must be a button id");
                };
                let path = match args.get("path") {
                    None => None,
                    Some(Value::String(path))
                        if matches!(dialog, Dialog::Open | Dialog::SaveAs) =>
                    {
                        Some(path.clone())
                    }
                    Some(Value::String(_)) => {
                        return self.error(id, code::ARGUMENT, "only Open and Save As take a path");
                    }
                    Some(_) => return self.error(id, code::ARGUMENT, "path must be a string"),
                };
                match (dialog, answer) {
                    (Dialog::Discard, "save" | "discard" | "keep") => {
                        self.answer(match answer {
                            "save" => Answer::Save,
                            "discard" => Answer::Discard,
                            _ => Answer::Keep,
                        });
                        if self.ui.dialog == Some(Dialog::Discard) {
                            let message = self.label("busy");
                            return self.error(id, code::BUSY, &message);
                        }
                    }
                    (Dialog::Open, "open") | (Dialog::SaveAs, "save") => {
                        if let Some(path) = path {
                            self.set_path(path);
                        }
                        self.submit_path();
                        if self.ui.dialog == Some(dialog) {
                            // Refused as the window refuses it: the dialog
                            // stays, with the reason on its status line.
                            let message = self.status.clone();
                            return self.error(id, code::ARGUMENT, &message);
                        }
                    }
                    (Dialog::Open | Dialog::SaveAs, "cancel")
                    | (Dialog::Shortcuts | Dialog::About, "done") => self.close_dialog(),
                    _ => {
                        let ids: Vec<String> = self.dialog_view()["buttons"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|b| b["id"].as_str().map(str::to_owned))
                            .collect();
                        let message = format!(
                            "the {} dialog has no button {answer:?}; it has {}",
                            dialog.name(),
                            ids.join(", ")
                        );
                        return self.error(id, code::ARGUMENT, &message);
                    }
                }
            }
            (None, None) => {
                if args.contains_key("path") {
                    return self.error(id, code::ARGUMENT, "path goes with answer");
                }
            }
        }
        let body = json!({"dialog": self.dialog_view()});
        self.effects.push(Effect::Reply { id, rc: 0, body });
    }

    /// `screenshot.theme`: the window's scheme and mode by name (null
    /// follows the session).
    fn theme_command(&mut self, id: u64, args: &Value) {
        let scheme = match args.get("scheme") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(name)) => match design::Scheme::from_name(name) {
                Some(s) => Some(Some(s)),
                None => return self.error(id, code::ARGUMENT, &format!("unknown scheme {name:?}")),
            },
            Some(_) => return self.error(id, code::ARGUMENT, "scheme must be a name or null"),
        };
        let mode = match args.get("mode") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(name)) => match design::Mode::from_name(name) {
                Some(m) => Some(Some(m)),
                None => return self.error(id, code::ARGUMENT, &format!("unknown mode {name:?}")),
            },
            Some(_) => return self.error(id, code::ARGUMENT, "mode must be a name or null"),
        };
        if let Some(scheme) = scheme {
            self.theme.scheme = scheme;
        }
        if let Some(mode) = mode {
            self.theme.mode = mode;
        }
        let body = json!({
            "scheme": self.theme.scheme.map(design::Scheme::name),
            "mode": self.theme.mode.map(design::Mode::name),
        });
        self.effects.push(Effect::Reply { id, rc: 0, body });
    }
}

/// `~/x` as the home directory's `x`.
fn expand_home(text: &str) -> String {
    match (text.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => text.to_owned(),
    }
}

/// Engine access to the shell's document and preview invalidation.
impl annotation::HasDocument for App {
    fn document(&self) -> Option<&Document> {
        self.document.as_ref()
    }
    fn document_mut(&mut self) -> Option<&mut Document> {
        self.document.as_mut()
    }
    fn document_changed(&mut self) {
        self.changed();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    fn app() -> App {
        App::new(crate::label, crate::ink::inks(), false)
    }

    fn with_doc() -> App {
        let mut a = app();
        a.document = Some(Document::new(RgbaImage::new(100, 80)).unwrap());
        a
    }

    fn rect() -> Shape {
        Shape::new(
            Kind::Rectangle,
            vec![Point { x: 2.0, y: 2.0 }, Point { x: 20.0, y: 20.0 }],
            [255, 0, 0, 255],
            2.0,
        )
    }

    fn replies(effects: &[Effect]) -> Vec<(u64, u8, Value)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Reply { id, rc, body } => Some((*id, *rc, body.clone())),
                _ => None,
            })
            .collect()
    }

    fn ticket_of(effects: &[Effect]) -> u64 {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::Capture { ticket, .. }
                | Effect::Open { ticket, .. }
                | Effect::Save { ticket, .. }
                | Effect::Copy { ticket, .. } => Some(*ticket),
                Effect::Show { ticket } => *ticket,
                _ => None,
            })
            .expect("a job")
    }

    fn captured(width: u32) -> Captured {
        Captured {
            document: Document::new(RgbaImage::new(width, 50)).unwrap(),
            path: "/tmp/capture.png".into(),
            metadata: json!({"output":"DP-1"}),
        }
    }

    #[test]
    fn taking_with_unsaved_work_asks_and_discarding_carries_on() {
        let mut a = with_doc();
        a.add_shape(rect());
        assert!(a.dirty());
        a.take(Some(Mode::Screen));
        assert_eq!(a.ui.dialog, Some(Dialog::Discard));
        assert!(!a.busy(), "nothing starts before the answer");
        a.answer(Answer::Keep);
        assert!(a.pending().is_none());
        assert_eq!(a.document.as_ref().unwrap().objects().len(), 1);
        a.take(Some(Mode::Screen));
        a.answer(Answer::Discard);
        let effects = a.take_effects();
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Capture { request, hide: true, .. } if request.mode == Mode::Screen)),
            "{effects:?}"
        );
        assert!(a.capturing());
    }

    #[test]
    fn save_from_the_discard_dialog_then_captures() {
        let mut a = with_doc();
        a.add_shape(rect());
        a.take(Some(Mode::Region));
        a.answer(Answer::Save);
        let effects = a.take_effects();
        let ticket = ticket_of(&effects);
        assert!(matches!(
            effects.last(),
            Some(Effect::Save {
                to: SaveTo::Folder,
                ..
            })
        ));
        a.saved(
            ticket,
            Ok(Saved {
                path: "/home/user/Pictures/Screenshots/20261012_140322.png".into(),
                width: 100,
                height: 80,
                annotated: true,
                saved_at: "2026-10-12T14:03:22+10:00".into(),
            }),
        );
        let effects = a.take_effects();
        assert!(effects.iter().any(
            |e| matches!(e, Effect::Publish { suffix: "saved", body } if body["annotated"] == true)
        ));
        assert!(
            effects.iter().any(|e| matches!(e, Effect::Capture { .. })),
            "the capture waited on the save: {effects:?}"
        );
        assert!(!a.dirty());
    }

    #[test]
    fn a_stale_completion_changes_nothing() {
        let mut a = app();
        a.take(Some(Mode::Screen));
        let ticket = ticket_of(&a.take_effects());
        a.captured_job(ticket + 7, Ok(captured(10)), false, false);
        assert!(a.document.is_none());
        assert!(a.capturing());
        a.captured_job(ticket, Ok(captured(40)), false, false);
        assert_eq!(a.document.as_ref().unwrap().dimensions(), (40, 50));
        assert_eq!(a.capture["output"], "DP-1");
        assert!(!a.busy());
    }

    #[test]
    fn agents_are_refused_rather_than_discarding_work() {
        let mut a = with_doc();
        a.add_shape(rect());
        for (command_name, body) in [
            ("screenshot.capture", "{}"),
            ("screenshot.open", r#"{"path":"/tmp/x.png"}"#),
            ("screenshot.quit", "{}"),
        ] {
            a.command(1, command_name, body);
            let r = replies(&a.take_effects());
            assert_eq!(r[0].2["error_code"], code::UNSAVED, "{command_name}");
        }
        assert!(!a.quitting);
        assert_eq!(a.document.as_ref().unwrap().objects().len(), 1);
    }

    #[test]
    fn take_over_the_bus_asks_in_the_window_and_brings_it_forward() {
        let mut a = with_doc();
        a.add_shape(rect());
        a.command(
            3,
            "screenshot.take",
            r#"{"mode":"window","window":{"id":7,"generation":2}}"#,
        );
        let effects = a.take_effects();
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Show { ticket: None }))
        );
        assert_eq!(replies(&effects)[0].2["state"], "asking");
        assert_eq!(
            a.ui.window,
            Some(Target {
                id: 7,
                generation: 2
            })
        );
        assert_eq!(a.ui.mode, Mode::Window);
        // Headless never asks.
        let mut h = App::new(crate::label, crate::ink::inks(), true);
        h.document = Some(Document::new(RgbaImage::new(10, 10)).unwrap());
        h.add_shape(rect());
        h.command(4, "screenshot.take", "{}");
        assert_eq!(replies(&h.take_effects())[0].2["error_code"], code::UNSAVED);
        assert!(h.ui.dialog.is_none());
    }

    #[test]
    fn take_without_unsaved_work_starts_at_once() {
        let mut a = app();
        a.command(1, "screenshot.take", r#"{"mode":"region"}"#);
        let effects = a.take_effects();
        assert_eq!(replies(&effects)[0].2["state"], "capturing");
        assert!(
            effects.iter().any(
                |e| matches!(e, Effect::Capture { request, .. } if request.mode == Mode::Region)
            )
        );
        // One capture at a time: a second Print hears so, and the window
        // says so.
        a.command(2, "screenshot.take", "{}");
        let second = replies(&a.take_effects());
        assert_eq!(second[0].1, 0);
        assert_eq!(second[0].2["state"], "in_progress");
        assert_eq!(second[0].2["job"], "capture");
        assert_eq!(a.status, crate::label("capture-in-progress"));
        // A window argument goes with window mode only, and a refused take
        // changes nothing.
        let mut c = app();
        c.command(
            4,
            "screenshot.take",
            r#"{"mode":"screen","window":{"id":7,"generation":2}}"#,
        );
        assert_eq!(
            replies(&c.take_effects())[0].2["error_code"],
            code::ARGUMENT
        );
        assert_eq!(c.ui.window, None);
        let mut b = app();
        b.command(3, "screenshot.take", r#"{"mode":"nope"}"#);
        assert_eq!(
            replies(&b.take_effects())[0].2["error_code"],
            code::ARGUMENT
        );
    }

    #[test]
    fn print_screen_over_an_open_dialog_brings_it_forward() {
        let mut a = with_doc();
        a.open(Dialog::About);
        a.command(1, "screenshot.take", r#"{"mode":"region"}"#);
        let effects = a.take_effects();
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Show { ticket: None }))
        );
        assert_eq!(replies(&effects)[0].2["state"], "asking");
        assert!(!a.busy(), "nothing starts under the dialog");
        assert_eq!(a.ui.dialog, Some(Dialog::About));
    }

    #[test]
    fn alias_undo_with_empty_history_returns_document_without_an_event() {
        empty_history_alias("screenshot.undo", "edit.undo");
    }

    #[test]
    fn alias_redo_with_empty_history_returns_document_without_an_event() {
        empty_history_alias("screenshot.redo", "edit.redo");
    }

    fn empty_history_alias(command_name: &str, command: &str) {
        let mut a = with_doc();
        let info = a.document.as_ref().unwrap().info();
        let commands = a.commands.clone();
        let strings = crate::strings();
        a.command(1, command_name, "{}");
        let r = replies(&crate::jobs::settle(
            &mut a,
            &commands.borrow(),
            &strings,
            false,
        ));
        assert_eq!(r, vec![(1, 0, info)]);
        assert!(commands.borrow().take_executed().0.is_empty());
        a.command(2, "screenshot.execute", &json!({"id":command}).to_string());
        let r = replies(&crate::jobs::settle(
            &mut a,
            &commands.borrow(),
            &strings,
            false,
        ));
        assert_eq!(r[0].1, 10);
        assert_eq!(r[0].2["error_code"], "DISABLED");
        assert!(commands.borrow().take_executed().0.is_empty());
    }

    #[test]
    fn annotation_limit_preserves_alias_argument_and_execute_failed() {
        let mut a = with_doc();
        let commands = a.commands.clone();
        let strings = crate::strings();
        let params = serde_json::to_value(rect()).unwrap();
        for _ in 0..annotation::MAX_OBJECTS {
            a.command(1, "screenshot.annotate", &params.to_string());
            let r = replies(&crate::jobs::settle(
                &mut a,
                &commands.borrow(),
                &strings,
                false,
            ));
            assert_eq!(r[0].1, 0);
            assert_eq!(commands.borrow().take_executed().0.len(), 1);
        }
        let info = a.document.as_ref().unwrap().info();
        for (command_name, body, code) in [
            ("screenshot.annotate", params.clone(), "ARGUMENT"),
            (
                "screenshot.execute",
                json!({"id":"edit.annotate", "params":params}),
                "FAILED",
            ),
        ] {
            a.command(2, command_name, &body.to_string());
            let r = replies(&crate::jobs::settle(
                &mut a,
                &commands.borrow(),
                &strings,
                false,
            ));
            assert_eq!(r[0].1, 10);
            assert_eq!(r[0].2["error_code"], code);
            assert_eq!(a.document.as_ref().unwrap().info(), info);
            assert!(commands.borrow().take_executed().0.is_empty());
        }
        a.add_shape(rect());
        assert_eq!(
            a.status,
            format!("{}: annotation limit reached", a.label("error"))
        );
    }

    #[test]
    fn hand_edit_errors_show_only_the_inner_message() {
        let mut a = with_doc();
        a.undo();
        assert_eq!(a.status, format!("{}: nothing to undo", a.label("error")));
        let mut shape = rect();
        shape.width = 0.0;
        let message = a.document.as_mut().unwrap().add(shape.clone()).unwrap_err();
        a.add_shape(shape);
        assert_eq!(a.status, format!("{}: {message}", a.label("error")));
    }

    #[test]
    fn aliases_and_execute_record_identical_edits_and_ui_uses_the_same_journal() {
        let mut a = with_doc();
        let initial = a.document.clone();
        let commands = a.commands.clone();
        let strings = crate::strings();
        let params = serde_json::to_value(rect()).unwrap();
        a.command(1, "screenshot.annotate", &params.to_string());
        let alias = replies(&crate::jobs::settle(
            &mut a,
            &commands.borrow(),
            &strings,
            false,
        ))[0]
            .2
            .clone();
        let mut alias_event = commands.borrow().take_executed().0.remove(0);
        a.document = initial.clone();
        a.command(
            2,
            "screenshot.execute",
            &json!({"id":"edit.annotate", "params":params}).to_string(),
        );
        let execute = replies(&crate::jobs::settle(
            &mut a,
            &commands.borrow(),
            &strings,
            false,
        ))[0]
            .2
            .clone();
        let mut execute_event = commands.borrow().take_executed().0.remove(0);
        assert_eq!(alias, execute["result"]);
        alias_event.seq = 0;
        alias_event.at_ms = 0;
        execute_event.seq = 0;
        execute_event.at_ms = 0;
        assert_eq!(alias_event, execute_event);
        a.document = initial;
        crate::view::apply(
            &mut a,
            &commands.borrow(),
            vec![crate::view::UiEvent::Add(rect())],
        );
        let ui = commands.borrow().take_executed().0.remove(0);
        assert_eq!(ui.id, "edit.annotate");
        assert_eq!(ui.params, params);
        assert_eq!(ui.result, alias);
        assert_eq!(ui.origin, Origin::Ui);
        println!("UI_EXECUTED {}", ui.body());
        let id = a.ui.selected.unwrap();
        crate::view::apply(
            &mut a,
            &commands.borrow(),
            vec![crate::view::UiEvent::Move(id, 4.0, 0.0)],
        );
        let moved = commands.borrow().take_executed().0;
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].params, json!({"id":id, "dx":4.0, "dy":0.0}));
        a.execute("edit.undo", Value::Null, Origin::Shortcut)
            .unwrap();
        assert_eq!(
            a.document.as_ref().unwrap().object(id).unwrap().shape,
            rect()
        );
        a.execute("edit.delete_selected", Value::Null, Origin::Shortcut)
            .unwrap();
        assert!(a.document.as_ref().unwrap().objects().is_empty());
        let runs = commands.borrow().take_executed().0;
        assert_eq!(
            runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["edit.undo", "edit.delete", "edit.delete_selected"]
        );
        assert!(runs.iter().all(|r| r.origin == Origin::Shortcut));
        assert_eq!(runs[1].params, json!({"id":id}));
    }

    #[test]
    fn edits_over_the_bus_share_the_document_and_history() {
        let mut a = with_doc();
        let commands = a.commands.clone();
        let strings = crate::strings();
        a.command(
            1,
            "screenshot.annotate",
            &json!({"kind":"arrow","points":[{"x":1,"y":1},{"x":30,"y":20}],"colour":[0,0,255,255],"width":3}).to_string(),
        );
        let r = replies(&crate::jobs::settle(
            &mut a,
            &commands.borrow(),
            &strings,
            false,
        ));
        assert_eq!(r[0].1, 0);
        let id = r[0].2["id"].as_u64().unwrap();
        a.command(
            2,
            "screenshot.move",
            &json!({"id":id,"dx":5,"dy":0}).to_string(),
        );
        a.command(
            3,
            "screenshot.crop",
            r#"{"crop":{"x":0,"y":0,"width":50,"height":40}}"#,
        );
        a.command(4, "screenshot.undo", "{}");
        let r = replies(&crate::jobs::settle(
            &mut a,
            &commands.borrow(),
            &strings,
            false,
        ));
        assert!(r.iter().all(|(_, rc, _)| *rc == 0), "{r:?}");
        assert!(a.document.as_ref().unwrap().crop().is_none());
        let runs = commands.borrow().take_executed().0;
        assert_eq!(
            runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["edit.annotate", "edit.move", "edit.crop", "edit.undo"]
        );
        assert_eq!(runs[0].params["kind"], "arrow");
        assert_eq!(runs[0].result["id"], id);
        assert_eq!(runs[1].params, json!({"id":id, "dx":5, "dy":0}));
        assert_eq!(runs[1].result["objects"][0]["shape"]["points"][0]["x"], 6.0);
        assert_eq!(
            runs[2].params,
            json!({"crop":{"x":0,"y":0,"width":50,"height":40}})
        );
        assert_eq!(runs[2].result["width"], 50);
        assert_eq!(runs[2].result["height"], 40);
        assert_eq!(runs[3].params, Value::Null);
        assert!(runs[3].result["crop"].is_null());
        a.command(5, "screenshot.delete", r#"{"id":999}"#);
        assert_eq!(
            replies(&crate::jobs::settle(
                &mut a,
                &commands.borrow(),
                &strings,
                false
            ))[0]
                .2["error_code"],
            code::ARGUMENT
        );
        a.command(6, "screenshot.undo", r#"{"x":1}"#);
        assert_eq!(
            replies(&crate::jobs::settle(
                &mut a,
                &commands.borrow(),
                &strings,
                false
            ))[0]
                .2["error_code"],
            code::ARGUMENT
        );
    }

    #[test]
    fn a_dialog_or_a_job_refuses_agents_but_reads_still_answer() {
        let mut a = with_doc();
        a.open(Dialog::About);
        a.command(1, "screenshot.undo", "{}");
        a.command(2, "screenshot.info", "{}");
        let r = replies(&a.take_effects());
        assert_eq!(r[0].2["error_code"], code::DIALOG);
        assert_eq!(r[1].1, 0);
        assert_eq!(r[1].2["ui"]["dialog"], "about");
        a.close_dialog();
        a.copy();
        a.take_effects();
        a.command(3, "screenshot.save", "{}");
        assert_eq!(replies(&a.take_effects())[0].2["error_code"], code::BUSY);
    }

    #[test]
    fn quitting_during_a_capture_cancels_it_then_asks() {
        let mut a = with_doc();
        a.add_shape(rect());
        a.document.as_mut().unwrap().mark_saved();
        a.take(Some(Mode::Screen));
        let ticket = ticket_of(&a.take_effects());
        a.document = Some({
            let mut d = Document::new(RgbaImage::new(10, 10)).unwrap();
            d.add(rect()).unwrap();
            d
        });
        a.quit();
        assert!(a.take_effects().iter().any(|e| matches!(e, Effect::Cancel)));
        a.captured_job(ticket, Err("cancelled".into()), false, true);
        assert_eq!(a.ui.dialog, Some(Dialog::Discard));
        assert_eq!(a.pending(), Some(&Pending::Quit));
        assert!(!a.quitting);
        a.answer(Answer::Discard);
        assert!(a.quitting);
        assert!(a.take_effects().iter().any(|e| matches!(e, Effect::Exit)));
    }

    #[test]
    fn quick_save_goes_to_the_folder_once_per_change() {
        let mut a = with_doc();
        a.save();
        let effects = a.take_effects();
        let ticket = ticket_of(&effects);
        let saved = Saved {
            path: "/tmp/a.png".into(),
            width: 100,
            height: 80,
            annotated: false,
            saved_at: String::new(),
        };
        a.saved(ticket, Ok(saved));
        a.take_effects();
        a.save();
        assert!(a.take_effects().is_empty(), "unchanged since its save");
        assert!(a.status.contains("/tmp/a.png"));
        a.add_shape(rect());
        a.save();
        assert!(matches!(
            a.take_effects().last(),
            Some(Effect::Save {
                to: SaveTo::Folder,
                ..
            })
        ));
    }

    #[test]
    fn save_as_refuses_an_existing_file_and_takes_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join("taken.png");
        std::fs::write(&taken, b"x").unwrap();
        let mut a = with_doc();
        a.save_as();
        assert_eq!(a.ui.dialog, Some(Dialog::SaveAs));
        // Proposed as the folder save would name it: YYYYMMDD_HHMMSS.png.
        let proposed = Path::new(&a.ui.path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(annotation::library::is_shot_name(&proposed), "{proposed}");
        a.set_path(taken.display().to_string());
        a.submit_path();
        assert_eq!(a.ui.dialog, Some(Dialog::SaveAs), "kept open");
        let fresh = dir.path().join("fresh.png");
        a.set_path(fresh.display().to_string());
        a.submit_path();
        assert!(a.ui.dialog.is_none());
        assert!(matches!(
            a.take_effects().last(),
            Some(Effect::Save { to: SaveTo::Path(p), .. }) if *p == fresh
        ));
    }

    #[test]
    fn text_needs_text_and_markers_count_up() {
        let mut a = with_doc();
        let mut text = rect();
        text.kind = Kind::Text;
        a.add_shape(text.clone());
        assert!(a.document.as_ref().unwrap().objects().is_empty());
        a.set_text("Look here".into());
        a.add_shape(text);
        let mut marker = rect();
        marker.kind = Kind::Number;
        a.add_shape(marker.clone());
        a.add_shape(marker);
        let objects = a.document.as_ref().unwrap().objects();
        assert_eq!(objects[0].shape.text.as_deref(), Some("Look here"));
        assert_eq!(objects[1].shape.number, Some(1));
        assert_eq!(objects[2].shape.number, Some(2));
        assert_eq!(a.ui.selected, Some(objects[2].id));
    }

    #[test]
    fn ink_and_width_restyle_the_selection_as_one_step() {
        let mut a = with_doc();
        a.add_shape(rect());
        let id = a.ui.selected.unwrap();
        let blue = a.inks[3].rgba;
        a.set_colour(blue);
        a.set_width(8.0);
        let shape = &a.document.as_ref().unwrap().object(id).unwrap().shape;
        assert_eq!((shape.colour, shape.width), (blue, 8.0));
        a.undo();
        assert_eq!(
            a.document
                .as_ref()
                .unwrap()
                .object(id)
                .unwrap()
                .shape
                .colour,
            blue,
            "width undone first"
        );
    }

    #[test]
    fn a_width_looks_the_same_on_screen_whatever_the_image_size() {
        // A 4K shot and a small region in the same canvas: the chosen width
        // and text size show at the same view-point size on both.
        for (w, h) in [(3840, 2160), (1513, 1023)] {
            let mut a = app();
            a.document = Some(Document::new(RgbaImage::new(w, h)).unwrap());
            let vp = annotation::Viewport::fit((w, h), (1040.0, 590.0), 1.0, a.ui.pan);
            a.set_view_scale(vp.scale);
            for (_, _, width) in WIDTHS {
                a.set_width(width);
                assert!(
                    (a.stroke_width() * vp.scale - width).abs() < 1e-3,
                    "{w}x{h}"
                );
            }
            for (_, _, size) in SIZES {
                a.set_text_size(size);
                assert!((a.text_px() * vp.scale - size).abs() < 1e-3, "{w}x{h}");
            }
        }
        // The 4K image needs more than the old 128 / 256 pixel limits.
        let mut a = app();
        a.set_view_scale(0.25);
        a.set_width(WIDTHS[3].2);
        a.set_text_size(SIZES[3].2);
        assert_eq!((a.stroke_width(), a.text_px()), (64.0, 256.0));
        a.set_view_scale(0.1);
        assert_eq!((a.stroke_width(), a.text_px()), (160.0, 640.0));
        // Past the engine's limits the value is held inside them.
        a.set_view_scale(0.001);
        assert_eq!((a.stroke_width(), a.text_px()), (512.0, 1024.0));
        a.set_view_scale(100.0);
        assert_eq!((a.stroke_width(), a.text_px()), (0.5, 8.0));
        // A nonsense scale is ignored.
        a.set_view_scale(f32::NAN);
        a.set_view_scale(0.0);
        assert_eq!(a.view_scale, 100.0);
    }

    #[test]
    fn new_objects_take_the_converted_width_and_old_ones_keep_theirs() {
        let mut a = with_doc();
        a.set_view_scale(0.5);
        a.add_shape(Shape::new(
            Kind::Rectangle,
            vec![Point { x: 2.0, y: 2.0 }, Point { x: 20.0, y: 20.0 }],
            a.ui.colour,
            a.stroke_width(),
        ));
        let first = a.ui.selected.unwrap();
        let width = |a: &App, id| a.document.as_ref().unwrap().object(id).unwrap().shape.width;
        assert_eq!(width(&a, first), 8.0, "medium (4) at half scale");
        // Text takes the converted size.
        a.set_text("Note".into());
        let mut text = rect();
        text.kind = Kind::Text;
        a.add_shape(text);
        let note = a.ui.selected.unwrap();
        let size = a
            .document
            .as_ref()
            .unwrap()
            .object(note)
            .unwrap()
            .shape
            .size;
        assert_eq!(size, Some(48.0), "medium text (24) at half scale");
        // Zooming in changes nothing already drawn, and a new ink keeps
        // the selection's width.
        a.set_view_scale(2.0);
        a.select(Some(first));
        a.set_colour(a.inks[2].rgba);
        assert_eq!(width(&a, first), 8.0);
        // A new width restyles the selection converted at today's scale.
        a.set_width(WIDTHS[3].2);
        assert_eq!(width(&a, first), 8.0, "heavy (16) at double scale");
        a.set_width(WIDTHS[0].2);
        assert_eq!(width(&a, first), 1.0, "thin (2) at double scale");
    }

    #[test]
    fn the_ui_state_round_trips_and_tools_go_by_name() {
        let a = app();
        let text = serde_json::to_string(&a.ui).unwrap();
        assert!(text.contains("\"tool\":\"arrow\""));
        let back: UiState = serde_json::from_str(&text).unwrap();
        assert_eq!(back, a.ui);
        for tool in Tool::ALL {
            assert_eq!(Tool::from_name(tool.name()), Some(tool));
        }
    }

    #[test]
    fn select_and_theme_commands_check_every_key_first() {
        let mut a = app();
        a.command(
            1,
            "screenshot.select",
            r#"{"tool":"pen","mode":"screen","output":"DP-1"}"#,
        );
        a.command(2, "screenshot.select", r#"{"tool":"pen","bogus":1}"#);
        a.command(
            3,
            "screenshot.select",
            r#"{"tool":"crayon","mode":"region"}"#,
        );
        a.command(
            4,
            "screenshot.theme",
            r#"{"scheme":"forest","mode":"dark"}"#,
        );
        a.command(5, "screenshot.theme", r#"{"scheme":"plaid"}"#);
        let r = replies(&a.take_effects());
        assert_eq!(r[0].1, 0);
        assert_eq!(r[1].2["error_code"], code::ARGUMENT);
        assert_eq!(r[2].2["error_code"], code::ARGUMENT);
        assert_eq!(a.ui.mode, Mode::Screen, "a refused select changes nothing");
        assert_eq!(a.ui.tool, Tool::Draw(Kind::Pen));
        assert_eq!(a.ui.output.as_deref(), Some("DP-1"));
        assert_eq!(r[3].2["scheme"], "forest");
        assert_eq!(r[4].2["error_code"], code::ARGUMENT);
        assert_eq!(a.theme.mode, Some(design::Mode::Dark));
    }

    #[test]
    fn refreshed_lists_keep_a_live_choice_and_drop_a_gone_one() {
        let mut a = app();
        let window = |id, focused| Window {
            target: Target { id, generation: 1 },
            title: format!("w{id}"),
            app_id: String::new(),
            focused,
            minimized: false,
        };
        a.refreshed(Ok((
            vec![window(1, false), window(2, true)],
            vec!["DP-1".into()],
        )));
        assert_eq!(a.ui.window.map(|t| t.id), Some(2), "the focused one");
        a.choose_window(Some(Target {
            id: 1,
            generation: 1,
        }));
        a.refreshed(Ok((vec![window(1, false), window(3, true)], vec![])));
        assert_eq!(a.ui.window.map(|t| t.id), Some(1), "kept");
        a.refreshed(Ok((vec![window(3, true)], vec![])));
        assert_eq!(a.ui.window.map(|t| t.id), Some(3), "replaced");
    }

    fn forgotten(effects: &[Effect]) -> Vec<PathBuf> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Forget { path } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_raw_capture_goes_once_saved_replaced_or_closed() {
        let mut a = app();
        a.take(Some(Mode::Screen));
        let ticket = ticket_of(&a.take_effects());
        let first_id = a.document_id;
        a.captured_job(ticket, Ok(captured(40)), false, false);
        assert_eq!(
            a.capture_file.as_deref(),
            Some(std::path::Path::new("/tmp/capture.png"))
        );
        assert_ne!(a.document_id, first_id, "a new document, a new id");
        let effects = a.take_effects();
        assert!(
            effects.iter().any(|e| matches!(e, Effect::Refresh)),
            "windows re-read"
        );
        // Saved: the raw frame is not needed any more.
        a.save();
        let ticket = ticket_of(&a.take_effects());
        a.saved(
            ticket,
            Ok(Saved {
                path: "/tmp/saved.png".into(),
                width: 40,
                height: 50,
                annotated: false,
                saved_at: String::new(),
            }),
        );
        assert_eq!(
            forgotten(&a.take_effects()),
            [PathBuf::from("/tmp/capture.png")]
        );
        assert!(a.capture_file.is_none());
        // Replaced unsaved: the old frame goes with it.
        a.take(Some(Mode::Screen));
        let ticket = ticket_of(&a.take_effects());
        a.captured_job(ticket, Ok(captured(30)), false, false);
        a.take_effects();
        a.take(Some(Mode::Screen));
        let ticket = ticket_of(&a.take_effects());
        a.captured_job(ticket, Ok(captured(20)), false, false);
        assert_eq!(
            forgotten(&a.take_effects()),
            [PathBuf::from("/tmp/capture.png")]
        );
        // Closed: the last one goes too.
        a.quit();
        assert_eq!(
            forgotten(&a.take_effects()),
            [PathBuf::from("/tmp/capture.png")]
        );
    }

    #[test]
    fn a_capture_that_cannot_bring_the_window_back_says_so() {
        // The shot is kept; the answer and the status line say so. No
        // restore is tried later: the person brings the window back.
        let mut a = app();
        a.command(1, "screenshot.capture", "{}");
        let capture = ticket_of(&a.take_effects());
        a.captured_job(capture, Ok(captured(10)), true, false);
        let effects = a.take_effects();
        let r = replies(&effects);
        assert_eq!(r[0].1, 0);
        assert_eq!(r[0].2["restore_failed"], true);
        assert!(a.document.is_some(), "the shot is kept");
        assert_eq!(a.status, crate::label("restore-failed"));
        assert!(!a.busy());
        // A capture that brought the window back says nothing of it.
        let mut b = app();
        b.command(1, "screenshot.capture", "{}");
        let capture = ticket_of(&b.take_effects());
        b.captured_job(capture, Ok(captured(10)), false, false);
        assert!(
            replies(&b.take_effects())[0]
                .2
                .get("restore_failed")
                .is_none()
        );
        // A failed capture whose restore also failed still says FAILED,
        // and a cancelled one CANCELLED.
        let mut c = app();
        c.command(1, "screenshot.capture", "{}");
        let capture = ticket_of(&c.take_effects());
        c.captured_job(
            capture,
            Err("x; restoring the window failed: down".into()),
            true,
            false,
        );
        let r = replies(&c.take_effects());
        assert_eq!(r[0].2["error_code"], code::FAILED);
        assert_eq!(r[0].2["restore_failed"], true);
        assert_eq!(c.status, crate::label("restore-failed"));
        let mut d = app();
        d.command(1, "screenshot.capture", "{}");
        let capture = ticket_of(&d.take_effects());
        d.captured_job(
            capture,
            Err("cancelled; restoring the window failed: down".into()),
            true,
            true,
        );
        let r = replies(&d.take_effects());
        assert_eq!(r[0].2["error_code"], code::CANCELLED);
        assert_eq!(r[0].2["restore_failed"], true);
    }

    #[test]
    fn print_shows_the_window_and_asks_at_once_and_again_after_a_failed_show() {
        let mut a = with_doc();
        a.add_shape(rect());
        a.command(1, "screenshot.take", "{}");
        let effects = a.take_effects();
        assert_eq!(a.ui.dialog, Some(Dialog::Discard), "asked at once");
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Show { ticket: None }))
        );
        assert_eq!(replies(&effects)[0].2, json!({"state": "asking"}));
        // The show failed: the dialog stays, and the next Print shows again.
        a.raised(Err("refused".into()));
        assert_eq!(a.ui.dialog, Some(Dialog::Discard));
        a.command(2, "screenshot.take", "{}");
        let effects = a.take_effects();
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Show { ticket: None }))
        );
        assert_eq!(replies(&effects)[0].2, json!({"state": "asking"}));
        assert!(matches!(a.pending(), Some(Pending::Capture(_))));
    }

    #[test]
    fn dialogs_never_open_beside_a_job_and_answers_keep_their_checks() {
        // A job running: no help dialog, by hand or by command.
        let mut j = with_doc();
        j.copy();
        j.open(Dialog::About);
        assert_eq!(j.ui.dialog, None);
        let r = crate::actions::registry();
        for id in ["help.about", "help.shortcuts", "file.save_as", "file.open"] {
            assert!(
                r.execute(id, &mut j, Value::Null, toolkit::action::Origin::Bus)
                    .is_err(),
                "{id} waits for the job"
            );
        }
        // Idle, they open.
        let mut h = app();
        assert!(
            r.execute(
                "help.about",
                &mut h,
                Value::Null,
                toolkit::action::Origin::Bus
            )
            .is_ok()
        );
        assert_eq!(h.ui.dialog, Some(Dialog::About));
        // A Discard or Save answered while a job runs is refused; the job
        // keeps its ticket and the dialog stays.
        let mut b = with_doc();
        b.add_shape(rect());
        b.copy();
        let copy = ticket_of(&b.take_effects());
        b.ui.dialog = Some(Dialog::Discard);
        b.answer(Answer::Discard);
        b.answer(Answer::Save);
        assert_eq!(b.ui.dialog, Some(Dialog::Discard));
        assert!(b.take_effects().is_empty());
        assert_eq!(b.job(), Some(JobKind::Copy));
        b.copied(copy, Err("no clipboard".into()));
    }

    #[test]
    fn new_work_drops_an_action_queued_against_the_old_document() {
        // A new capture supersedes it…
        let mut a = with_doc();
        a.pending = Some(Pending::OpenDialog);
        a.command(1, "screenshot.capture", "{}");
        assert!(a.capturing());
        assert_eq!(a.pending(), None);
        // …and so does a replaced document.
        let capture = ticket_of(&a.take_effects());
        a.pending = Some(Pending::Open("/tmp/old.png".into()));
        a.captured_job(capture, Ok(captured(10)), false, false);
        assert_eq!(a.pending(), None);
        assert!(
            !a.take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Open { .. }))
        );
        // A Quit closes whatever is shown: it survives.
        let mut q = app();
        q.command(1, "screenshot.capture", "{}");
        let capture = ticket_of(&q.take_effects());
        q.quit();
        assert_eq!(q.pending(), Some(&Pending::Quit));
        q.captured_job(capture, Ok(captured(10)), false, false);
        assert!(q.quitting);
    }

    #[test]
    fn take_refuses_another_compositor() {
        let mut a = app();
        a.comp = "comp-nested".into();
        a.command(1, "screenshot.take", r#"{"comp":"comp"}"#);
        let r = replies(&a.take_effects());
        assert_eq!(r[0].2["error_code"], code::COMPOSITOR);
        assert!(r[0].2["message"].as_str().unwrap().contains("comp-nested"));
        assert!(!a.busy());
        a.command(2, "screenshot.take", r#"{"comp":"comp-nested"}"#);
        assert!(a.capturing());
    }

    #[test]
    fn ping_and_info_name_the_compositor() {
        let mut a = app();
        a.comp = "comp-nested".into();
        a.command(1, "screenshot.ping", "{}");
        a.command(2, "screenshot.info", "{}");
        let r = replies(&a.take_effects());
        assert_eq!(r[0].2["comp"], "comp-nested");
        assert_eq!(r[1].2["comp"], "comp-nested");
    }

    #[test]
    fn a_quit_held_behind_a_help_dialog_runs_when_it_closes() {
        let mut a = with_doc();
        a.copy();
        let ticket = ticket_of(&a.take_effects());
        a.quit();
        a.ui.dialog = Some(Dialog::Shortcuts);
        a.copied(ticket, Ok(()));
        assert!(!a.quitting, "the dialog is up");
        a.close_dialog();
        assert!(a.quitting);
        assert!(a.take_effects().iter().any(|e| matches!(e, Effect::Exit)));
    }

    #[test]
    fn one_output_is_chosen_and_markers_stop_at_the_cap() {
        let mut a = with_doc();
        a.refreshed(Ok((vec![], vec!["DP-1".into()])));
        assert_eq!(a.ui.output.as_deref(), Some("DP-1"));
        a.refreshed(Ok((vec![], vec!["DP-1".into(), "HDMI-A-1".into()])));
        assert_eq!(a.ui.output.as_deref(), Some("DP-1"), "a live choice stays");
        let mut top = rect();
        top.kind = Kind::Number;
        top.number = Some(9999);
        top.size = Some(20.0);
        a.document.as_mut().unwrap().add(top).unwrap();
        let mut marker = rect();
        marker.kind = Kind::Number;
        a.add_shape(marker);
        assert_eq!(a.document.as_ref().unwrap().objects().len(), 1);
        assert_eq!(a.status, crate::label("marker-limit"));
    }

    #[test]
    fn an_agent_capture_shows_in_the_controls() {
        let mut a = app();
        a.command(
            1,
            "screenshot.capture",
            r#"{"mode":"screen","output":"HDMI-A-1","delay":3,"cursor":true}"#,
        );
        assert_eq!(a.ui.mode, Mode::Screen);
        assert_eq!(a.ui.output.as_deref(), Some("HDMI-A-1"));
        assert_eq!((a.ui.delay, a.ui.cursor), (3, true));
        assert!(
            a.take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Capture { focus: false, .. })),
            "an agent's capture leaves focus alone"
        );
        assert!(
            !Request::default().cursor,
            "the pointer is off unless asked"
        );
    }

    #[test]
    fn a_persons_capture_comes_back_focused_by_every_route() {
        let focused = |effects: Vec<Effect>| {
            effects.iter().any(|e| {
                matches!(
                    e,
                    Effect::Capture {
                        focus: true,
                        hide: true,
                        ..
                    }
                )
            })
        };
        // The Take button and Ctrl+N.
        let commands = crate::actions::registry();
        let mut a = app();
        commands
            .execute(
                "capture.take",
                &mut a,
                Value::Null,
                toolkit::action::Origin::Ui,
            )
            .unwrap();
        assert!(focused(a.take_effects()), "the Take button");
        // Print Screen forwarded to a running window.
        let mut a = app();
        a.command(1, "screenshot.take", r#"{"mode":"region"}"#);
        assert!(focused(a.take_effects()), "Print Screen");
        // `--capture` at launch.
        let mut a = app();
        a.take(Some(Mode::Screen));
        assert!(focused(a.take_effects()), "--capture");
    }

    #[test]
    fn a_region_capture_says_what_to_do_and_a_timeout_says_nothing_was_selected() {
        let mut a = app();
        a.take(Some(Mode::Region));
        let ticket = ticket_of(&a.take_effects());
        assert_eq!(a.status, crate::label("region-hint"));
        a.captured_job(ticket, Err("selection timeout".into()), false, false);
        assert_eq!(a.status, crate::label("nothing-selected"));
        a.take(Some(Mode::Region));
        let ticket = ticket_of(&a.take_effects());
        a.captured_job(ticket, Err("selection cancelled".into()), false, false);
        assert_eq!(a.status, crate::label("cancelled"));
        // Any other failure still says what went wrong.
        a.take(Some(Mode::Screen));
        let ticket = ticket_of(&a.take_effects());
        assert_eq!(a.status, crate::label("capturing"));
        a.captured_job(ticket, Err("Bus down".into()), false, false);
        assert!(a.status.ends_with("Bus down"), "{}", a.status);
    }

    #[test]
    fn a_save_names_the_file_it_wrote() {
        let mut a = with_doc();
        a.save();
        let ticket = ticket_of(&a.take_effects());
        let path = PathBuf::from("/home/user/Pictures/Screenshots/20261013_090000.png");
        a.saved(
            ticket,
            Ok(Saved {
                path: path.clone(),
                width: 100,
                height: 80,
                annotated: false,
                saved_at: "2026-10-13T09:00:00+10:00".into(),
            }),
        );
        assert_eq!(
            a.status,
            crate::label_with("saved-to", &[("path", &shown_path(&path))])
        );
        assert!(a.status.ends_with("20261013_090000.png"));
        assert_eq!(
            shown_path_in(&path, Some(Path::new("/home/user"))),
            "~/Pictures/Screenshots/20261013_090000.png"
        );
        assert_eq!(
            shown_path_in(&path, Some(Path::new("/home/other"))),
            path.display().to_string()
        );
        assert_eq!(
            shown_path_in(&path, Some(Path::new("/"))),
            path.display().to_string()
        );
        assert_eq!(shown_path_in(&path, None), path.display().to_string());
    }

    #[test]
    fn unknown_commands_and_window_commands_headless_are_refused() {
        let mut h = App::new(crate::label, crate::ink::inks(), true);
        h.command(1, "screenshot.nope", "{}");
        h.command(2, "screenshot.ui.click", r#"{"label":"File"}"#);
        h.command(3, "screenshot.show", "{}");
        h.command(4, "screenshot.recent", r#"{"limit":0}"#);
        let r = replies(&h.take_effects());
        assert_eq!(r[0].2["error_code"], code::UNKNOWN_COMMAND);
        assert_eq!(r[1].2["error_code"], code::HEADLESS);
        assert_eq!(r[2].2["error_code"], code::HEADLESS);
        assert_eq!(r[3].2["error_code"], code::ARGUMENT);
        // No dialog ever opens headless: the commands that would open one
        // are refused as HEADLESS, by hand too; a capture still runs.
        let registry = crate::actions::registry();
        let strings = crate::strings();
        for (n, id) in ["help.about", "help.shortcuts", "file.open", "file.save_as"]
            .into_iter()
            .enumerate()
        {
            h.document = Some(Document::new(RgbaImage::new(10, 10)).unwrap());
            h.command(
                10 + n as u64,
                "screenshot.execute",
                &json!({ "id": id }).to_string(),
            );
            let effects = crate::jobs::settle(&mut h, &registry, &strings, false);
            assert_eq!(replies(&effects)[0].2["error_code"], "HEADLESS", "{id}");
        }
        h.open(Dialog::About);
        h.open_dialog();
        h.save_as();
        assert_eq!(h.ui.dialog, None);
        h.document = None;
        h.command(20, "screenshot.capture", "{}");
        assert!(h.capturing());
        let mut a = app();
        a.command(5, "screenshot.ui.click", r#"{"label":"File"}"#);
        assert!(matches!(
            a.take_effects()[..],
            [Effect::Drive { id: 5, .. }]
        ));
    }

    fn dialog(a: &mut App, id: u64, body: Value) -> (u8, Value) {
        a.command(id, "screenshot.dialog", &body.to_string());
        let r = replies(&a.take_effects());
        let (_, rc, body) = r.into_iter().find(|r| r.0 == id).expect("a reply");
        (rc, body)
    }

    #[test]
    fn dialog_command_reads_and_answers_the_discard_dialog() {
        let mut a = with_doc();
        a.add_shape(rect());
        a.take(Some(Mode::Screen));
        assert_eq!(a.ui.dialog, Some(Dialog::Discard));
        // Every other command is still refused while it is up.
        a.command(1, "screenshot.open", r#"{"path":"/tmp/x.png"}"#);
        assert_eq!(replies(&a.take_effects())[0].2["error_code"], code::DIALOG);
        let (rc, body) = dialog(&mut a, 2, json!({}));
        assert_eq!(rc, 0);
        assert_eq!(body["dialog"]["kind"], "discard");
        assert_eq!(body["dialog"]["title"], crate::label("discard-title"));
        let ids: Vec<_> = body["dialog"]["buttons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["save", "discard", "keep"]);
        let (rc, body) = dialog(&mut a, 3, json!({"answer":"open"}));
        assert_eq!((rc, &body["error_code"]), (10, &json!(code::ARGUMENT)));
        assert_eq!(a.ui.dialog, Some(Dialog::Discard));
        let (rc, body) = dialog(&mut a, 4, json!({"answer":"keep"}));
        assert_eq!((rc, &body["dialog"]), (0, &Value::Null));
        assert!(a.pending().is_none());
        assert_eq!(a.document.as_ref().unwrap().objects().len(), 1);
        a.take(Some(Mode::Screen));
        let (rc, _) = dialog(&mut a, 5, json!({"answer":"discard"}));
        assert_eq!(rc, 0);
        assert_eq!(a.ui.dialog, None);
        assert!(a.capturing(), "discarding carries on with the capture");
    }

    #[test]
    fn dialog_command_opens_closes_and_submits_paths() {
        let mut a = app();
        assert_eq!(dialog(&mut a, 1, json!({})), (0, json!({"dialog":null})));
        assert_eq!(
            dialog(&mut a, 2, json!({"answer":"done"})).1["error_code"],
            code::NO_DIALOG
        );
        assert_eq!(
            dialog(&mut a, 3, json!({"open":"about"})).1["dialog"]["kind"],
            "about"
        );
        assert_eq!(
            dialog(&mut a, 4, json!({"open":"shortcuts"})).1["error_code"],
            code::DIALOG
        );
        assert_eq!(
            dialog(&mut a, 5, json!({"answer":"done"})),
            (0, json!({"dialog":null}))
        );
        for bad in [
            json!({"open":"settings"}),
            json!({"open":null,"answer":"done"}),
            json!({"path":"/tmp"}),
            json!({"bogus":1}),
        ] {
            assert_eq!(
                dialog(&mut a, 6, bad.clone()).1["error_code"],
                code::ARGUMENT,
                "{bad}"
            );
        }
        assert_eq!(dialog(&mut a, 7, json!({"open":null})).0, 0);

        a.open_dialog();
        assert_eq!(a.ui.dialog, Some(Dialog::Open));
        let view = dialog(&mut a, 8, json!({})).1;
        assert_eq!(view["dialog"]["kind"], "open");
        assert!(view["dialog"]["path"].is_string());
        // An empty path is refused as the window refuses it; the dialog stays.
        let (rc, body) = dialog(&mut a, 9, json!({"answer":"open","path":""}));
        assert_eq!((rc, &body["error_code"]), (10, &json!(code::ARGUMENT)));
        assert_eq!(a.ui.dialog, Some(Dialog::Open));
        a.command(
            10,
            "screenshot.dialog",
            r#"{"answer":"open","path":"/tmp/shot.png"}"#,
        );
        let effects = a.take_effects();
        assert_eq!(replies(&effects)[0].2, json!({"dialog":null}));
        assert!(effects.iter().any(|e| matches!(e, Effect::Open { .. })));

        let mut s = with_doc();
        s.save_as();
        assert_eq!(s.ui.dialog, Some(Dialog::SaveAs));
        let existing = std::env::current_exe().unwrap().display().to_string();
        let (rc, body) = dialog(&mut s, 11, json!({"answer":"save","path":existing}));
        assert_eq!(
            (rc, body["message"].as_str()),
            (10, Some(crate::label("exists").as_str()))
        );
        assert_eq!(s.ui.dialog, Some(Dialog::SaveAs));
        assert_eq!(
            dialog(&mut s, 12, json!({"answer":"cancel"})),
            (0, json!({"dialog":null}))
        );
    }

    #[test]
    fn dialog_command_in_a_headless_instance_reads_and_refuses_to_open() {
        let mut h = App::new(crate::label, crate::ink::inks(), true);
        assert_eq!(dialog(&mut h, 1, json!({})), (0, json!({"dialog":null})));
        assert_eq!(
            dialog(&mut h, 2, json!({"open":"about"})).1["error_code"],
            code::HEADLESS
        );
        assert_eq!(dialog(&mut h, 3, json!({"open":null})).0, 0);
    }
}
