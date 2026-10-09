// SPDX-License-Identifier: MIT OR Apache-2.0
//! The drive layer: the same UI-level Bus verbs for every MixOS app, so an
//! agent can do in the window whatever a person can (agentic first).
//!
//! [`install`] registers [`Drive`] as an egui plugin. It turns AccessKit on,
//! keeps each pass's widget tree, and injects queued pointer and keyboard
//! events through egui's input hook: the path a person's input takes, so
//! menus, the title bar, trees and text fields all respond as they would to
//! a hand.
//!
//! The verbs ([`VERBS`]) are namespaced under the app (`<app>.ui.tree`,
//! `<app>.window`, …). The app forwards each one, without its prefix, to
//! [`request`] from inside a frame, sends whatever [`request`] or [`logic`]
//! (once a frame) hands back, and holds later commands while [`pending`];
//! [`finish`] answers everything before it closes. Reads answer at once. Input verbs run one at
//! a time, one step per pass, and answer one pass after their last step with
//! the menu, focus and pointer the input left, so a caller can step the
//! window deterministically.
use crate::menu;
use egui::accesskit::{Node, NodeId, Role, Toggled, TreeUpdate};
use egui::{
    ColorImage, Context, Event, FullOutput, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, TouchPhase,
    Ui, UserData, Vec2, ViewportCommand, pos2, vec2,
};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The drive verbs, after the app's prefix.
pub const VERBS: [&str; 10] = [
    "ui.tree",
    "ui.click",
    "ui.pointer",
    "ui.scroll",
    "ui.key",
    "ui.type",
    "ui.menu",
    "ui.capture",
    "window",
    "window.state",
];

/// Queued input requests beyond this are refused (the Bus holds at most 32
/// unanswered commands per app anyway).
const QUEUE_LIMIT: usize = 32;

/// One Bus answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub id: u64,
    pub rc: u8,
    pub body: Value,
}

fn ok(id: u64, body: Value) -> Answer {
    Answer { id, rc: 0, body }
}

fn refusal(id: u64, code: &str, message: &str) -> Answer {
    Answer { id, rc: 10, body: json!({"error_code":code,"message":message}) }
}

/// Every drive verb under `app`, as `HELP` / `app.describe` list them.
pub fn describe(app: &str) -> Vec<Value> {
    let verbs = [
        ("ui.tree", json!({"label":"optional substring","exact":"optional bool: the label must equal it","role":"optional role name"}),
            "The widget tree from AccessKit, depth first: id (decimal text), role, label, value, placeholder, rect (points), enabled, focused, selected, checked, children", true),
        ("ui.click", json!({"label":"string (or id)","id":"node id as decimal text (or label)","button":"optional primary|secondary|middle","double":"optional bool","trace":"optional bool: also answer passes, each pass's input events and hit test"}),
            "Move to a node's centre, then press and release there in one pass; a label names a control before a tooltip or label that repeats it; answers with the resulting menu and focus", false),
        ("ui.pointer", json!({"x":"number","y":"number","action":"move|press|release","button":"optional primary|secondary|middle"}),
            "One pointer event at a point in window coordinates (points)", false),
        ("ui.scroll", json!({"x":"number","y":"number","dx":"number","dy":"number"}),
            "Move to a point and scroll by dx, dy points", false),
        ("ui.key", json!({"key":"egui key name (Enter, Escape, ArrowDown, F1, A, …)","modifiers":"optional [ctrl|shift|alt|command]"}),
            "Press and release one key with modifiers, one pass each", false),
        ("ui.type", json!({"text":"string"}),
            "Type text into the focused widget; a newline presses Enter", false),
        ("ui.menu", Value::Null, "The open menu and its highlighted path", true),
        ("ui.capture", json!({"name":"optional plain file name ending .png"}),
            "Screenshot the window to a new PNG in $XDG_RUNTIME_DIR/<app>/captures and answer with its path", false),
        ("window", json!({"action":"minimize|maximize|restore|close|focus"}),
            "Send a window command, as the caption buttons do", false),
        ("window.state", Value::Null, "Window size, maximized, minimized, focused and pixels per point", true),
    ];
    verbs
        .into_iter()
        .map(|(verb, args, description, read_only)| {
            let mut entry = json!({"name":format!("{app}.{verb}"),"description":description,"read_only":read_only});
            if !args.is_null() {
                entry["args"] = args;
            }
            entry
        })
        .collect()
}

/// Where captures go and how long one may wait for its screenshot.
#[derive(Clone, Debug)]
pub struct Options {
    /// The runtime directory captures live under, in `<app>/captures`
    /// (default `$XDG_RUNTIME_DIR`; without it, captures are refused).
    pub runtime_dir: Option<PathBuf>,
    pub capture_timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self { runtime_dir: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from), capture_timeout: Duration::from_secs(10) }
    }
}

/// Register the drive layer on `ctx` for the app whose verbs start `app.`.
pub fn install(ctx: &Context, app: &str) {
    install_with(ctx, app, Options::default());
}

/// [`install`] with explicit [`Options`].
pub fn install_with(ctx: &Context, app: &str, options: Options) {
    ctx.add_plugin(Drive { app: app.to_owned(), options, ..Drive::default() });
}

/// `verb` only queues input behind the input before it, so an app may hand
/// it over while earlier input is still running; every other verb reads or
/// changes state now and must wait until [`pending`] is false.
pub fn queues(verb: &str) -> bool {
    matches!(verb, "ui.click" | "ui.pointer" | "ui.scroll" | "ui.key" | "ui.type" | "ui.capture")
}

/// Accepted drive work is still unanswered (queued, running, or answered
/// but not yet taken). An app holds later Bus commands back while it is, so
/// they act on what the input changed.
pub fn pending(ctx: &Context) -> bool {
    ctx.with_plugin(|drive: &mut Drive| drive.running.is_some() || !drive.queue.is_empty() || !drive.answers.is_empty())
        .unwrap_or(false)
}

/// The per-frame step an app runs from its logic, which runs even while
/// the window is hidden: when the window is hidden, settle the running job
/// and cancel the queue (no pass would ever read their input); expire a
/// capture past its deadline; then hand back every finished answer.
pub fn logic(ctx: &Context) -> Vec<Answer> {
    let hidden = ctx.input(|i| i.viewport().visible() == Some(false));
    ctx.with_plugin(|drive: &mut Drive| {
        if hidden {
            drive.settle(ctx, "the window was hidden before this input was read");
        }
        drive.expire();
        std::mem::take(&mut drive.answers)
    })
    .unwrap_or_default()
}

/// Answer every accepted request before the app closes: the running job
/// with what its input did so far (`interrupted` when some of it was never
/// sent), queued jobs as cancelled. Send all of them before the Bus stops.
pub fn finish(ctx: &Context) -> Vec<Answer> {
    ctx.with_plugin(|drive: &mut Drive| {
        if let Some(running) = drive.running.take() {
            let interrupted = running.steps.iter().any(|s| !matches!(s, Step::Idle));
            let answer = match running.answer {
                Some(answer) => answer,
                None if matches!(running.steps.front(), Some(Step::Await { .. })) => {
                    refusal(running.id, "CANCELLED", "the app closed before the screenshot arrived")
                }
                None => {
                    let mut answer = drive.report(ctx, running.id, &running.report, running.extra);
                    answer.body["closing"] = json!(true);
                    answer.body["interrupted"] = json!(interrupted);
                    answer
                }
            };
            drive.answers.push(answer);
        }
        drive.cancel_queue("the app closed before this input was read");
        std::mem::take(&mut drive.answers)
    })
    .unwrap_or_default()
}

/// Answer or queue the drive verb `verb` (without the app prefix). Call it
/// inside a frame. `None` means queued: its answer comes from [`logic`]
/// in a later frame.
pub fn request(ctx: &Context, id: u64, verb: &str, args: &Value) -> Option<Answer> {
    let answer = ctx
        .with_plugin(|drive: &mut Drive| drive.request(ctx, id, verb, args))
        .unwrap_or_else(|| Some(refusal(id, "UNAVAILABLE", "the drive layer is not installed")));
    if answer.is_none() {
        ctx.request_repaint();
    }
    answer
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Id(NodeId),
    Label(String),
}

#[derive(Clone, Debug, PartialEq)]
enum Job {
    Click { target: Target, button: PointerButton, double: bool },
    Pointer { at: Pos2, action: Event },
    Scroll { at: Pos2, delta: Vec2 },
    Key { key: Key, modifiers: Modifiers },
    Type { text: String },
    /// Report the window state once the window has had two passes.
    Window { action: String },
    Capture { path: PathBuf, tag: u64 },
}

enum Step {
    Input(Vec<Event>),
    Viewport(ViewportCommand),
    Idle,
    Await { tag: u64, path: PathBuf, deadline: Instant },
}

enum Report {
    Ui,
    Window,
}

struct Running {
    id: u64,
    steps: VecDeque<Step>,
    report: Report,
    extra: Value,
    answer: Option<Answer>,
    /// Buttons this job pressed and has not released yet.
    down: Vec<PointerButton>,
    /// With `trace`, one entry per pass: every input event egui saw (the
    /// platform's and the injected) and what egui's hit test made of it.
    trace: Option<Vec<Value>>,
    /// This pass's events, kept from the input hook for the output hook.
    pass_events: Vec<String>,
}

/// The tag a capture's screenshot request carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shot(u64);

/// The plugin: the last widget tree, queued input and finished answers.
#[derive(Default)]
pub struct Drive {
    app: String,
    options: Options,
    tree: Option<TreeUpdate>,
    queue: VecDeque<(u64, Job)>,
    running: Option<Running>,
    outbox: Vec<ViewportCommand>,
    answers: Vec<Answer>,
    shots: u64,
    /// Releases owed by a cancelled job, sent on the next pass.
    cleanup: Vec<Event>,
    /// Queued jobs whose answer carries a per-pass trace.
    traced: Vec<u64>,
}

impl egui::Plugin for Drive {
    fn debug_name(&self) -> &'static str {
        "toolkit::drive"
    }

    fn setup(&mut self, ctx: &Context) {
        ctx.enable_accesskit();
    }

    fn input_hook(&mut self, ctx: &Context, input: &mut RawInput) {
        // The platform may switch AccessKit off when a screen reader leaves.
        ctx.enable_accesskit();
        self.step(input);
        if let Some(running) = &mut self.running
            && running.trace.is_some()
        {
            running.pass_events = input.events.iter().filter(|e| !matches!(e, Event::Screenshot { .. })).map(|e| format!("{e:?}")).collect();
        }
    }

    fn on_begin_pass(&mut self, ui: &mut Ui) {
        for command in self.outbox.drain(..) {
            ui.ctx().send_viewport_cmd(command);
        }
    }

    fn output_hook(&mut self, ctx: &Context, output: &mut FullOutput) {
        if let Some(update) = &output.platform_output.accesskit_update {
            self.tree = Some(update.clone());
        }
        if let Some(running) = &mut self.running
            && let Some(trace) = &mut running.trace
        {
            trace.push(pass_trace(ctx, std::mem::take(&mut running.pass_events)));
        }
        if self.running.as_ref().is_some_and(|r| r.steps.is_empty()) {
            let running = self.running.take().expect("a finished job");
            let answer = match running.answer {
                Some(answer) => answer,
                None => {
                    let mut extra = running.extra;
                    if let Some(trace) = running.trace {
                        extra["passes"] = Value::Array(trace);
                    }
                    self.report(ctx, running.id, &running.report, extra)
                }
            };
            self.answers.push(answer);
        }
        if self.running.is_some() || !self.queue.is_empty() || !self.answers.is_empty() || !self.cleanup.is_empty() {
            ctx.request_repaint();
        }
    }
}

impl Drive {
    fn request(&mut self, ctx: &Context, id: u64, verb: &str, args: &Value) -> Option<Answer> {
        if !args.is_object() {
            return Some(refusal(id, "ARGUMENT", "arguments must be a JSON object"));
        }
        let job = match verb {
            "ui.tree" => return Some(self.tree_answer(id, args)),
            "ui.menu" => return Some(ok(id, json!({"menu":menu_state(ctx)}))),
            "window.state" => return Some(ok(id, window_state(ctx))),
            "window" => return self.window(ctx, id, args),
            "ui.click" => click(args),
            "ui.pointer" => pointer(args),
            "ui.scroll" => scroll(args),
            "ui.key" => key(args),
            "ui.type" => match args.get("text").and_then(Value::as_str) {
                Some(text) => Ok(Job::Type { text: text.to_owned() }),
                None => Err("text must be a string".to_owned()),
            },
            "ui.capture" => self.capture(args),
            _ => return Some(refusal(id, "UNKNOWN_VERB", "unknown drive verb")),
        };
        let job = match job {
            Ok(job) => job,
            Err(message) => return Some(refusal(id, "ARGUMENT", &message)),
        };
        // A hidden window runs no passes, so its input would never be read.
        if ctx.input(|i| i.viewport().visible() == Some(false)) {
            return Some(refusal(id, "BUSY", "the window is hidden: restore it first"));
        }
        if self.queue.len() >= QUEUE_LIMIT {
            return Some(refusal(id, "BUSY", "too many queued inputs"));
        }
        match args.get("trace") {
            None | Some(Value::Null | Value::Bool(false)) => {}
            Some(Value::Bool(true)) => self.traced.push(id),
            Some(_) => return Some(refusal(id, "ARGUMENT", "trace must be a boolean")),
        }
        self.queue.push_back((id, job));
        None
    }

    /// Window commands go out at once, from inside the frame: a minimized
    /// window runs no passes, so a queued restore would never be sent.
    fn window(&mut self, ctx: &Context, id: u64, args: &Value) -> Option<Answer> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or_default();
        let commands = match action {
            "minimize" => vec![ViewportCommand::Minimized(true)],
            "maximize" => vec![ViewportCommand::Maximized(true)],
            "restore" => vec![ViewportCommand::Minimized(false), ViewportCommand::Maximized(false)],
            "close" => vec![ViewportCommand::Close],
            "focus" => vec![ViewportCommand::Focus],
            _ => return Some(refusal(id, "ARGUMENT", "action must be minimize, maximize, restore, close or focus")),
        };
        // A minimized window runs no passes: settle what is accepted first.
        if action == "minimize" {
            self.settle(ctx, "the window was minimized before this input was read");
        }
        for command in commands {
            ctx.send_viewport_cmd(command);
        }
        // Minimizing and closing stop the passes: answer with the request,
        // behind whatever was settled before it.
        if matches!(action, "minimize" | "close") {
            let mut body = window_state(ctx);
            body["requested"] = json!(action);
            self.answers.push(ok(id, body));
            return None;
        }
        self.queue.push_back((id, Job::Window { action: action.to_owned() }));
        None
    }

    /// A capture always lands in the app's own `<runtime>/<app>/captures`
    /// (0700), under a caller-chosen plain file name or a generated one,
    /// and never over an existing file or through a link.
    fn capture(&mut self, args: &Value) -> Result<Job, String> {
        self.shots += 1;
        if args.get("path").is_some() {
            return Err("captures go to the app capture directory: give a file name, not a path".into());
        }
        let name = match args.get("name") {
            None | Some(Value::Null) => format!("capture-{}-{}.png", std::process::id(), self.shots),
            Some(Value::String(name)) => {
                let plain = !name.is_empty()
                    && name.len() <= 128
                    && !name.starts_with('.')
                    && !name.contains(['/', '\\', '\0'])
                    && !name.contains("..");
                if !plain || !name.ends_with(".png") {
                    return Err("name must be a plain file name ending .png (no directories, no ..)".into());
                }
                name.clone()
            }
            Some(_) => return Err("name must be a string".into()),
        };
        let dir = self.capture_dir()?;
        let path = dir.join(name);
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(format!("{} already exists", path.display()));
        }
        Ok(Job::Capture { path, tag: self.shots })
    }

    /// `<runtime>/<app>/captures`, created 0700, refusing a link anywhere
    /// in the part the app owns.
    fn capture_dir(&self) -> Result<PathBuf, String> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let runtime = self.options.runtime_dir.clone().ok_or("XDG_RUNTIME_DIR is not set: no capture directory")?;
        let app = if self.app.is_empty() { "app" } else { &self.app };
        let mut dir = runtime;
        for part in [app, "captures"] {
            dir.push(part);
            match std::fs::symlink_metadata(&dir) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => return Err(format!("{} is not a directory", dir.display())),
                Err(_) => std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| format!("{}: {e}", dir.display()))?,
            }
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        Ok(dir)
    }

    /// The answer a finished job gives: what the input left (or the window
    /// state), with the job's own fields.
    fn report(&self, ctx: &Context, id: u64, report: &Report, extra: Value) -> Answer {
        let mut body = match report {
            Report::Ui => self.ui_state(ctx),
            Report::Window => window_state(ctx),
        };
        if let (Some(body), Value::Object(extra)) = (body.as_object_mut(), extra) {
            body.extend(extra);
        }
        ok(id, body)
    }

    /// No pass will run for a while: answer the running job (done when only
    /// its redraw pass is left, else cancelled with the input it never got),
    /// and cancel the queue.
    fn settle(&mut self, ctx: &Context, why: &str) {
        if let Some(running) = self.running.take() {
            let answer = match running.answer {
                Some(answer) => answer,
                None if running.steps.iter().all(|s| matches!(s, Step::Idle)) => {
                    self.report(ctx, running.id, &running.report, running.extra)
                }
                None => {
                    self.release(&running.down);
                    refusal(running.id, "CANCELLED", why)
                }
            };
            self.answers.push(answer);
        }
        self.cancel_queue(why);
    }

    /// Owe a release for each of `buttons`, pressed by a job cancelled
    /// before its own release. egui keeps a button down until it sees one,
    /// so without it the next pointer motion would drag. egui 0.36 makes a
    /// release a click unless the pointer moved past `max_click_dist` since
    /// the press, so in one pass the pointer moves far outside the window,
    /// the buttons are released there and the pointer leaves. A release in
    /// the pass that moved it never starts a drag either.
    fn release(&mut self, buttons: &[PointerButton]) {
        if buttons.is_empty() {
            return;
        }
        let away = pos2(-100_000.0, -100_000.0);
        self.cleanup.push(Event::PointerMoved(away));
        for button in buttons {
            self.cleanup.push(Event::PointerButton { pos: away, button: *button, pressed: false, modifiers: Modifiers::NONE });
        }
        self.cleanup.push(Event::PointerGone);
    }

    fn cancel_queue(&mut self, why: &str) {
        for (id, _) in std::mem::take(&mut self.queue) {
            self.answers.push(refusal(id, "CANCELLED", why));
        }
    }

    /// Give up on a screenshot past its deadline.
    fn expire(&mut self) {
        let late = |r: &Running| matches!(r.steps.front(), Some(Step::Await { deadline, .. }) if Instant::now() >= *deadline);
        if self.running.as_ref().is_some_and(late) {
            let running = self.running.take().expect("a late capture");
            self.answers.push(refusal(running.id, "CAPTURE", "no screenshot arrived in time"));
        }
    }

    /// Start the next queued job, answering any that cannot start.
    fn start(&mut self) {
        while let Some((id, job)) = self.queue.pop_front() {
            match self.plan(job) {
                Ok((steps, report, extra)) => {
                    let trace = self.traced.contains(&id).then(Vec::new);
                    self.traced.retain(|t| *t != id);
                    self.running = Some(Running {
                        id,
                        steps: steps.into(),
                        report,
                        extra,
                        answer: None,
                        down: Vec::new(),
                        trace,
                        pass_events: Vec::new(),
                    });
                    return;
                }
                Err(message) => self.answers.push(refusal(id, "ARGUMENT", &message)),
            }
        }
    }

    /// The steps of `job`, one per pass, ending with a pass that redraws
    /// what the input changed.
    fn plan(&self, job: Job) -> Result<(Vec<Step>, Report, Value), String> {
        let input = Step::Input;
        let press = |pos, button, pressed| Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE };
        Ok(match job {
            Job::Click { target, button: which, double } => {
                let (id, node) = self.find(&target)?;
                let rect = node.bounds().ok_or("that node has no bounds")?;
                let at = pos2(((rect.x0 + rect.x1) / 2.0) as f32, ((rect.y0 + rect.y1) / 2.0) as f32);
                let target = self.node_json(id, node, None);
                // The press and its release share one pass, as a quick
                // hand's do within one frame. Passes can be far apart (a
                // compositor draws an unfocused window once a second), and
                // egui counts a release more than 0.8 s after its press as
                // a long press, not a click.
                let mut clicks = Vec::new();
                for _ in 0..if double { 2 } else { 1 } {
                    clicks.push(press(at, which, true));
                    clicks.push(press(at, which, false));
                }
                let steps = vec![input(vec![Event::PointerMoved(at)]), input(clicks), Step::Idle];
                (steps, Report::Ui, json!({"target":target,"at":[at.x,at.y]}))
            }
            Job::Pointer { at, action } => {
                (vec![input(vec![Event::PointerMoved(at), action]), Step::Idle], Report::Ui, json!({}))
            }
            Job::Scroll { at, delta } => {
                let wheel =
                    Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase: TouchPhase::Move, modifiers: Modifiers::NONE };
                (vec![input(vec![Event::PointerMoved(at)]), input(vec![wheel]), Step::Idle], Report::Ui, json!({}))
            }
            Job::Key { key, modifiers } => {
                // The modifiers go down before the key and up after it, as a
                // keyboard reports them.
                let event = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
                let (down, up) = if modifiers.any() {
                    (vec![Event::ModifiersChanged(modifiers), event(true)], vec![event(false), Event::ModifiersChanged(Modifiers::NONE)])
                } else {
                    (vec![event(true)], vec![event(false)])
                };
                (vec![input(down), input(up), Step::Idle], Report::Ui, json!({}))
            }
            Job::Type { text } => {
                let enter = |pressed| Event::Key { key: Key::Enter, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
                let mut steps = Vec::new();
                for (index, line) in text.split('\n').enumerate() {
                    if index > 0 {
                        steps.push(input(vec![enter(true), enter(false)]));
                    }
                    if !line.is_empty() {
                        steps.push(input(vec![Event::Text(line.to_owned())]));
                    }
                }
                steps.push(Step::Idle);
                (steps, Report::Ui, json!({}))
            }
            Job::Window { action } => (vec![Step::Idle, Step::Idle], Report::Window, json!({"requested":action})),
            Job::Capture { path, tag } => {
                let shot = ViewportCommand::Screenshot(UserData::new(Shot(tag)));
                let deadline = Instant::now() + self.options.capture_timeout;
                (vec![Step::Viewport(shot), Step::Await { tag, path, deadline }], Report::Ui, json!({}))
            }
        })
    }

    /// One pass of the running job: inject its next step.
    fn step(&mut self, input: &mut RawInput) {
        // Owed releases go first, in a pass of their own.
        if !self.cleanup.is_empty() {
            input.events.append(&mut self.cleanup);
            return;
        }
        if self.running.is_none() {
            self.start();
        }
        let Some(running) = &mut self.running else { return };
        if let Some(Step::Await { tag, path, deadline }) = running.steps.front_mut() {
            let image = input.events.iter().find_map(|event| match event {
                Event::Screenshot { user_data, image, .. }
                    if user_data.data.as_ref().and_then(|d| (**d).downcast_ref::<Shot>()) == Some(&Shot(*tag)) =>
                {
                    Some(image.clone())
                }
                _ => None,
            });
            let answer = match image {
                Some(image) => match write_png(path, &image) {
                    Ok(()) => Some(ok(running.id, json!({"path":path,"width":image.size[0],"height":image.size[1]}))),
                    Err(error) => Some(refusal(running.id, "CAPTURE", &error)),
                },
                None if Instant::now() >= *deadline => Some(refusal(running.id, "CAPTURE", "no screenshot arrived in time")),
                None => None,
            };
            if answer.is_some() {
                running.answer = answer;
                running.steps.pop_front();
            }
            return;
        }
        match running.steps.pop_front() {
            Some(Step::Input(events)) => {
                for event in &events {
                    if let Event::PointerButton { button, pressed, .. } = event {
                        running.down.retain(|b| b != button);
                        if *pressed {
                            running.down.push(*button);
                        }
                    }
                }
                input.events.extend(events);
            }
            Some(Step::Viewport(command)) => self.outbox.push(command),
            Some(Step::Idle | Step::Await { .. }) | None => {}
        }
    }

    fn nodes(&self) -> HashMap<NodeId, &Node> {
        self.tree.iter().flat_map(|t| t.nodes.iter().map(|(id, node)| (*id, node))).collect()
    }

    fn focus(&self) -> Option<NodeId> {
        self.tree.as_ref().map(|t| t.focus)
    }

    /// The node `target` names: by id, or the one node with that label (or,
    /// failing any label, that placeholder).
    fn find(&self, target: &Target) -> Result<(NodeId, &Node), String> {
        let nodes = self.nodes();
        if nodes.is_empty() {
            return Err("no frame has been drawn yet".into());
        }
        let found: Vec<_> = match target {
            Target::Id(id) => nodes.get(id).map(|n| (*id, *n)).into_iter().collect(),
            Target::Label(text) => {
                let by = |f: fn(&Node) -> Option<&str>| -> Vec<(NodeId, &Node)> {
                    let mut hits: Vec<_> = nodes.iter().filter(|(_, n)| f(n) == Some(text.as_str())).map(|(id, n)| (*id, *n)).collect();
                    hits.sort_by_key(|(id, _)| id.0);
                    hits
                };
                let labelled = by(label);
                let hits = if labelled.is_empty() { by(Node::placeholder) } else { labelled };
                // A tooltip or a plain label repeats a control's name: when
                // any match is a control, only the controls count.
                let controls: Vec<_> = hits.iter().copied().filter(|(_, n)| actionable(n.role())).collect();
                if controls.is_empty() { hits } else { controls }
            }
        };
        match &found[..] {
            [one] => Ok(*one),
            [] => Err(format!("no node matches {target:?}")),
            many => {
                let ids: Vec<String> = many.iter().map(|(id, _)| node_id(*id)).collect();
                Err(format!("{} nodes match {target:?}; click one by id: {}", many.len(), ids.join(", ")))
            }
        }
    }

    fn node_json(&self, id: NodeId, node: &Node, depth: Option<usize>) -> Value {
        let checked = node.toggled().map(|t| match t {
            Toggled::True => "true",
            Toggled::False => "false",
            Toggled::Mixed => "mixed",
        });
        let rect = node.bounds().map(|r| [r.x0, r.y0, r.x1, r.y1]);
        let children: Vec<String> = node.children().iter().map(|c| node_id(*c)).collect();
        let mut out = json!({"id":node_id(id),"role":format!("{:?}",node.role()),"label":label(node),"value":node.value(),
            "placeholder":node.placeholder(),"rect":rect,"enabled":!node.is_disabled(),"focused":self.focus() == Some(id),
            "selected":node.is_selected(),"checked":checked,"children":children});
        if let Some(depth) = depth {
            out["depth"] = json!(depth);
        }
        out
    }

    /// `ui.tree`: every node depth first from the root, optionally only
    /// those whose label contains `label` or whose role is `role`.
    fn tree_answer(&self, id: u64, args: &Value) -> Answer {
        let Some(tree) = &self.tree else { return refusal(id, "BUSY", "no frame has been drawn yet") };
        let (filter, role) = match (args.get("label"), args.get("role")) {
            (Some(l), _) if !l.is_string() => return refusal(id, "ARGUMENT", "label must be a string"),
            (_, Some(r)) if !r.is_string() => return refusal(id, "ARGUMENT", "role must be a string"),
            (l, r) => (l.and_then(Value::as_str), r.and_then(Value::as_str)),
        };
        let exact = match args.get("exact") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(exact)) => *exact,
            Some(_) => return refusal(id, "ARGUMENT", "exact must be a boolean"),
        };
        // A substring, either case; or with `exact`, the whole label as is.
        let wanted = filter.map(str::to_lowercase);
        let matches_label = |l: &str| match (filter, &wanted) {
            (Some(f), _) if exact => l == f,
            (_, Some(w)) => l.to_lowercase().contains(w.as_str()),
            _ => true,
        };
        let nodes = self.nodes();
        let root = tree.tree.as_ref().map(|t| t.root).or_else(|| tree.nodes.first().map(|(id, _)| *id));
        let mut out = Vec::new();
        let mut stack: Vec<(NodeId, usize)> = root.into_iter().map(|r| (r, 0)).collect();
        while let Some((node_id, depth)) = stack.pop() {
            let Some(node) = nodes.get(&node_id) else { continue };
            let label_matches = filter.is_none() || label(node).is_some_and(matches_label);
            let role_matches = role.is_none_or(|r| format!("{:?}", node.role()) == r);
            if label_matches && role_matches {
                out.push(self.node_json(node_id, node, Some(depth)));
            }
            stack.extend(node.children().iter().rev().map(|c| (*c, depth + 1)));
        }
        ok(id, json!({"root":root.map(node_id),"focus":self.focus().map(node_id),"nodes":out}))
    }

    /// What an input left: the open menu, the focused node and the pointer.
    fn ui_state(&self, ctx: &Context) -> Value {
        let nodes = self.nodes();
        let focused = self.focus().and_then(|f| nodes.get(&f).map(|n| self.node_json(f, n, None)));
        let pointer = ctx.input(|i| i.pointer.latest_pos()).map(|p| [p.x, p.y]);
        json!({"menu":menu_state(ctx),"focused":focused,"pointer":pointer})
    }
}

/// One traced pass: the input events egui read, then what its hit test and
/// pointer state made of them (widget ids as decimal text, as node ids).
fn pass_trace(ctx: &Context, events: Vec<String>) -> Value {
    let text = |id: egui::Id| id.value().to_string();
    let (clicked, drag_started, dragged, drag_stopped, hovered, contains) = ctx.interaction_snapshot(|s| {
        let set = |ids: &egui::IdSet| ids.iter().map(|id| text(*id)).collect::<Vec<_>>();
        (s.clicked.map(text), s.drag_started.map(text), s.dragged.map(text), s.drag_stopped.map(text), set(&s.hovered), set(&s.contains_pointer))
    });
    let (latest, down, moved_too_much, time) = ctx.input(|i| {
        (i.pointer.latest_pos().map(|p| [p.x, p.y]), i.pointer.any_down(), !i.pointer.could_any_button_be_click(), i.time)
    });
    json!({"time":time,"events":events,"clicked":clicked,"drag_started":drag_started,"dragged":dragged,"drag_stopped":drag_stopped,
        "hovered":hovered,"contains_pointer":contains,"pointer":latest,"down":down,"no_longer_a_click":moved_too_much})
}

/// Whether a node of `role` is a control a click acts on, as opposed to
/// text that names one (a tooltip, a label).
fn actionable(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::DefaultButton
            | Role::Link
            | Role::CheckBox
            | Role::Switch
            | Role::RadioButton
            | Role::ComboBox
            | Role::Slider
            | Role::SpinButton
            | Role::TextInput
            | Role::MultilineTextInput
            | Role::SearchInput
            | Role::MenuItem
            | Role::MenuItemCheckBox
            | Role::MenuItemRadio
            | Role::Tab
            | Role::ListBoxOption
    )
}

/// The text a person reads on `node`: egui puts a plain label's text in its
/// value.
fn label(node: &Node) -> Option<&str> {
    node.label().or_else(|| if node.role() == Role::Label { node.value() } else { None })
}

fn menu_state(ctx: &Context) -> Value {
    match menu::current(ctx) {
        Some(c) => json!({"menu":c.menu,"path":c.path,"depth":c.depth}),
        None => Value::Null,
    }
}

fn window_state(ctx: &Context) -> Value {
    let content = ctx.viewport_rect();
    ctx.input(|i| {
        let v = i.viewport();
        let size = |r: Option<egui::Rect>| r.map(|r| [r.width(), r.height()]);
        // Without a platform window (tests), the size is the content's.
        let inner = size(v.inner_rect).unwrap_or([content.width(), content.height()]);
        json!({"size":inner,"outer_size":size(v.outer_rect),"maximized":v.maximized,"minimized":v.minimized,
            "fullscreen":v.fullscreen,"focused":v.focused,"pixels_per_point":i.pixels_per_point()})
    })
}

fn number(args: &Value, key: &str) -> Result<f32, String> {
    args.get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .map(|v| v as f32)
        .ok_or_else(|| format!("{key} must be a number"))
}

fn button(args: &Value) -> Result<PointerButton, String> {
    match args.get("button").map(|b| b.as_str()) {
        None | Some(Some("primary")) => Ok(PointerButton::Primary),
        Some(Some("secondary")) => Ok(PointerButton::Secondary),
        Some(Some("middle")) => Ok(PointerButton::Middle),
        _ => Err("button must be primary, secondary or middle".into()),
    }
}

/// A node id on the Bus: decimal text, because AccessKit ids use all 64
/// bits and a JSON number loses them past 2^53 in most callers.
fn node_id(id: NodeId) -> String {
    id.0.to_string()
}

/// A node id from a caller: its decimal text, or an exact integer.
fn parse_id(id: &Value) -> Result<NodeId, String> {
    let parsed = match id {
        Value::String(text) => text.parse::<u64>().ok(),
        Value::Number(number) => number.as_u64(),
        _ => None,
    };
    parsed.map(NodeId).ok_or_else(|| "id must be a node id (decimal text)".to_owned())
}

fn click(args: &Value) -> Result<Job, String> {
    let target = match (args.get("id"), args.get("label")) {
        (Some(id), None) => Target::Id(parse_id(id)?),
        (None, Some(label)) => Target::Label(label.as_str().ok_or("label must be a string")?.to_owned()),
        _ => return Err("give exactly one of label and id".into()),
    };
    let double = match args.get("double") {
        None => false,
        Some(double) => double.as_bool().ok_or("double must be a bool")?,
    };
    Ok(Job::Click { target, button: button(args)?, double })
}

fn pointer(args: &Value) -> Result<Job, String> {
    let at = pos2(number(args, "x")?, number(args, "y")?);
    let action = match args.get("action").and_then(Value::as_str) {
        Some("move") => Event::PointerMoved(at),
        Some(action @ ("press" | "release")) => {
            Event::PointerButton { pos: at, button: button(args)?, pressed: action == "press", modifiers: Modifiers::NONE }
        }
        _ => return Err("action must be move, press or release".into()),
    };
    Ok(Job::Pointer { at, action })
}

fn scroll(args: &Value) -> Result<Job, String> {
    Ok(Job::Scroll { at: pos2(number(args, "x")?, number(args, "y")?), delta: vec2(number(args, "dx")?, number(args, "dy")?) })
}

fn key(args: &Value) -> Result<Job, String> {
    let name = args.get("key").and_then(Value::as_str).ok_or("key must be a key name")?;
    let key = Key::from_name(name).ok_or_else(|| format!("unknown key {name:?}"))?;
    let mut modifiers = Modifiers::NONE;
    let names = match args.get("modifiers") {
        None => &[][..],
        Some(Value::Array(names)) => names.as_slice(),
        Some(_) => return Err("modifiers must be a list".into()),
    };
    for name in names {
        match name.as_str() {
            // Ctrl is the command key off macOS, as egui-winit reports it.
            Some("ctrl" | "command") => modifiers = modifiers.plus(Modifiers::CTRL).plus(Modifiers::COMMAND),
            Some("shift") => modifiers = modifiers.plus(Modifiers::SHIFT),
            Some("alt") => modifiers = modifiers.plus(Modifiers::ALT),
            _ => return Err("modifiers are ctrl, shift, alt or command".into()),
        }
    }
    Ok(Job::Key { key, modifiers })
}

fn write_png(path: &Path, image: &ColorImage) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    // O_CREAT|O_EXCL: never an existing file, and never through a link.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let (width, height) = (image.size[0] as u32, image.size[1] as u32);
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    let data: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
    writer.write_image_data(&data).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verb_is_described_once() {
        let names: Vec<String> = describe("demo").iter().filter_map(|v| v["name"].as_str().map(str::to_owned)).collect();
        let expected: Vec<String> = VERBS.iter().map(|v| format!("demo.{v}")).collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn arguments_are_validated() {
        let ctx = Context::default();
        install(&ctx, "demo");
        let code = |verb: &str, args: Value| {
            request(&ctx, 1, verb, &args).map(|a| a.body["error_code"].as_str().unwrap_or("").to_owned())
        };
        assert_eq!(code("ui.nope", json!({})).as_deref(), Some("UNKNOWN_VERB"));
        assert_eq!(code("ui.tree", json!([])).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.click", json!({})).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.click", json!({"label":"a","id":1})).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.click", json!({"label":"a","button":"thumb"})).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.pointer", json!({"x":1,"y":2,"action":"wave"})).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.scroll", json!({"x":1,"y":2,"dx":"far"})).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.key", json!({"key":"NoSuchKey"})).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.key", json!({"key":"A","modifiers":["hyper"]})).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.type", json!({"text":5})).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.capture", json!({"path":"/tmp/x.png"})).as_deref(), Some("ARGUMENT"), "no caller paths");
        assert_eq!(code("ui.click", json!({"id":"12x"})).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("window", json!({"action":"spin"})).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.tree", json!({})).as_deref(), Some("BUSY"), "no frame yet");
        assert_eq!(code("ui.click", json!({"label":"a"})), None, "valid input is queued");
    }

    #[test]
    fn without_the_plugin_every_verb_is_unavailable() {
        let ctx = Context::default();
        let answer = request(&ctx, 3, "ui.tree", &json!({})).unwrap();
        assert_eq!(answer.body["error_code"], "UNAVAILABLE");
    }
}
