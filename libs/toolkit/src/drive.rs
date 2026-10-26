// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The drive layer: the same UI-level Bus commands for every MixOS app, so an
//! agent can do in the window whatever a person can (agentic first).
//!
//! [`install`] registers [`Drive`] as an egui plugin. It turns AccessKit on,
//! keeps each pass's widget tree, and injects queued pointer and keyboard
//! events through egui's input hook: the path a person's input takes, so
//! menus, the title bar, trees and text fields all respond as they would to
//! a hand.
//!
//! The commands ([`COMMANDS`]) are namespaced under the app (`<app>.ui.tree`,
//! `<app>.window`, …). The app forwards each one, without its prefix, to
//! [`request`] from app logic (including logic-only wakes), sends whatever
//! [`request`] or [`logic`] hands back, and holds later commands while [`pending`];
//! [`finish`] answers everything before it closes. Reads answer at once. Input commands run one at
//! a time, one step per pass, and answer one pass after their last step with
//! the menu, focus and pointer the input left, so a caller can step the
//! window deterministically.
use crate::menu;
use egui::accesskit::{Node, NodeId, Role, Toggled, TreeUpdate};
use egui::{
    ColorImage, Context, Event, FullOutput, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2,
    RawInput, TouchPhase, Ui, Vec2, ViewportCommand, pos2, vec2,
};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The drive commands, after the app's prefix.
pub const COMMANDS: [&str; 13] = [
    "ui.tree",
    "ui.click",
    "ui.pointer",
    "ui.scroll",
    "ui.key",
    "ui.type",
    "ui.menu",
    "ui.menu.list",
    "ui.menu.open",
    "ui.menu.close",
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
    Answer {
        id,
        rc: 10,
        body: json!({"error_code":code,"message":message}),
    }
}

fn not_shown(id: u64, quiet: Duration) -> Answer {
    refusal(
        id,
        "NOT_SHOWN",
        &format!(
            "the window is not being drawn (no frames for {} ms): show the window or switch to its VT, then retry the capture",
            quiet.as_millis()
        ),
    )
}

/// Every drive command under `app`, as `HELP` / `app.describe` list them.
pub fn describe(app: &str) -> Vec<Value> {
    let command_names = [
        (
            "ui.tree",
            json!({"label":"optional substring","exact":"optional bool: the label must equal it","role":"optional role name"}),
            "The widget tree from AccessKit, depth first: id (decimal text), role, label, value, placeholder, rect (points), enabled, focused, selected, checked, children",
            true,
        ),
        (
            "ui.click",
            json!({"label":"string (or id)","id":"node id as decimal text (or label)","button":"optional primary|secondary|middle","double":"optional bool","trace":"optional bool: also answer passes, each pass's input events and hit test"}),
            "Move to a node's centre, then press and release there in one pass; a label names a control before a tooltip or label that repeats it; answers with the resulting menu and focus",
            false,
        ),
        (
            "ui.pointer",
            json!({"x":"number","y":"number","action":"move|press|release","button":"optional primary|secondary|middle"}),
            "One pointer event at a point in window coordinates (points)",
            false,
        ),
        (
            "ui.scroll",
            json!({"x":"number","y":"number","dx":"number","dy":"number"}),
            "Move to a point and scroll by dx, dy points",
            false,
        ),
        (
            "ui.key",
            json!({"key":"egui key name (Enter, Escape, ArrowDown, F1, A, …)","modifiers":"optional [ctrl|shift|alt|command]"}),
            "Press and release one key with modifiers, one pass each",
            false,
        ),
        (
            "ui.type",
            json!({"text":"string"}),
            "Type text into the focused widget. A multi-line field takes it verbatim; elsewhere a newline presses Enter",
            false,
        ),
        (
            "ui.menu",
            Value::Null,
            "The open menu and its highlighted path",
            true,
        ),
        (
            "ui.menu.list",
            Value::Null,
            "Every menu: titles, then rows with command id, label, shortcut, enabled, tick and submenu rows",
            true,
        ),
        (
            "ui.menu.open",
            json!({"path":"[menu title, submenu row, …]: opens the menu and each submenu named; the last row named is highlighted","menu":"or a menu title alone"}),
            "Open a menu, and submenus within it, by label; answers the open menu and path",
            false,
        ),
        (
            "ui.menu.close",
            Value::Null,
            "Close every open menu; answers whether one was open",
            false,
        ),
        (
            "ui.capture",
            json!({"name":"optional plain file name ending .png"}),
            "Screenshot the window to a new PNG in $XDG_RUNTIME_DIR/<app>/captures and answer with its path",
            true,
        ),
        (
            "window",
            json!({"action":"minimize|maximize|restore|close|focus"}),
            "Send a window command, as the caption buttons do",
            false,
        ),
        (
            "window.state",
            Value::Null,
            "Window size, maximized, minimized, focused and pixels per point",
            true,
        ),
    ];
    command_names
        .into_iter()
        .map(|(command_name, mut args, description, read_only)| {
            let mut entry = json!({"name":format!("{app}.{command_name}"),"description":description,"read_only":read_only});
            if args.is_null() {
                args = json!({});
            }
            args["viewport"] = json!("optional owned window name: macro-editor; omitted for the app window");
            entry["args"] = args;
            entry
        })
        .collect()
}

/// Where captures go and how long screenshots and child requests may wait.
#[derive(Clone, Debug)]
pub struct Options {
    /// The runtime directory captures live under, in `<app>/captures`
    /// (default `$XDG_RUNTIME_DIR`; without it, captures are refused).
    pub runtime_dir: Option<PathBuf>,
    /// Budget for a screenshot, and for an entire auxiliary viewport request.
    pub capture_timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            capture_timeout: Duration::from_secs(10),
        }
    }
}

/// Register the drive layer on `ctx` for the app whose commands start `app.`.
pub fn install(ctx: &Context, app: &str) {
    install_with(ctx, app, Options::default());
}

/// [`install`] with explicit [`Options`].
pub fn install_with(ctx: &Context, app: &str, options: Options) {
    ctx.add_plugin(Drive {
        app: app.to_owned(),
        options,
        ..Drive::default()
    });
}

/// `command` only queues input behind the input before it, so an app may hand
/// it over while earlier input is still running; every other command reads or
/// changes state now and must wait until [`pending`] is false.
pub fn queues(command_name: &str) -> bool {
    matches!(
        command_name,
        "ui.click" | "ui.pointer" | "ui.scroll" | "ui.key" | "ui.type" | "ui.capture"
    )
}

/// Accepted drive work is still unanswered (queued, running, or answered
/// but not yet taken). An app holds later Bus commands back while it is, so
/// they act on what the input changed.
pub fn pending(ctx: &Context) -> bool {
    ctx.with_plugin(|drive: &mut Drive| drive.has_pending())
        .unwrap_or(false)
}

/// The request whose input was injected into this UI pass, if any. Queued
/// requests and idle/report passes have no input origin. Apps can retain
/// their caller identity until the request is answered and stamp effects
/// triggered during this pass with the actual replayed request id.
pub fn input_request(ctx: &Context) -> Option<u64> {
    ctx.with_plugin(|drive: &mut Drive| {
        if ctx.viewport_id() == egui::ViewportId::ROOT {
            drive.input_request_id
        } else {
            drive
                .children
                .get(&ctx.viewport_id())
                .and_then(|child| child.input_request_id)
        }
    })
    .flatten()
}

/// No UI pass for this long means pending input cannot make progress.
/// Keep brief desktop animations and GPU stalls (150–500 ms) intact, while
/// releasing later Bus commands after a window stops producing frames.
pub const INPUT_QUIET_PERIOD: Duration = Duration::from_secs(1);

/// No UI pass for this long refuses a pending capture. Stay above compd's
/// one-second parked/occluded callback trickle with margin. This is independent
/// of eframe's logic-wake grace period.
pub const CAPTURE_QUIET_PERIOD: Duration = Duration::from_millis(1500);

/// Run from app logic, including logic-only wakes. Settle pending input after
/// [`INPUT_QUIET_PERIOD`] without a UI pass, refuse captures without a UI pass
/// for [`CAPTURE_QUIET_PERIOD`], expire screenshots and return answers.
pub fn logic(ctx: &Context) -> Vec<Answer> {
    logic_at(ctx, Instant::now())
}

fn logic_at(ctx: &Context, now: Instant) -> Vec<Answer> {
    ctx.with_plugin(|drive: &mut Drive| {
        // Wayland can withhold frame callbacks on another virtual desktop
        // without exposing a hidden/minimized flag. A logic-only wake must
        // also settle paint-dependent input, or it holds later Bus commands.
        let logic_only = drive
            .passes
            .get(&std::thread::current().id())
            .is_none_or(Vec::is_empty);
        drive.finish_capture();
        for child in drive.children.values_mut() {
            child.finish_capture();
        }
        let input_quiet_since = drive.input_quiet_since.get_or_insert(now);
        let quiet = now.saturating_duration_since(*input_quiet_since);
        drive.refuse_unshown_captures(ctx, now);
        drive.request_capture_pass(ctx, egui::ViewportId::ROOT);
        for (&viewport, child) in &mut drive.children {
            child.refuse_unshown_captures(ctx, now);
            child.request_capture_pass(ctx, viewport);
        }
        if logic_only && quiet >= INPUT_QUIET_PERIOD {
            drive.settle_input(
                ctx,
                &format!(
                    "the window produced no frames for {} ms before this input was read",
                    quiet.as_millis()
                ),
            );
        } else if logic_only && drive.has_pending() {
            ctx.request_repaint_after(INPUT_QUIET_PERIOD.saturating_sub(quiet));
        }
        drive.expire();
        if let Some(running) = &drive.running
            && let Some(Step::Await { deadline, .. }) = running.steps.front()
        {
            ctx.request_repaint_after(deadline.saturating_duration_since(now));
        }
        let mut answers = std::mem::take(&mut drive.answers);
        for child in drive.children.values_mut() {
            child.expire();
            if let Some(running) = &child.running
                && let Some(Step::Await { deadline, .. }) = running.steps.front()
            {
                ctx.request_repaint_after_for(
                    deadline.saturating_duration_since(now),
                    egui::ViewportId::ROOT,
                );
            }
            answers.append(&mut child.answers);
        }
        answers
    })
    .unwrap_or_default()
}

/// Answer every accepted request before the app closes: the running job
/// with what its input did so far (`interrupted` when some of it was never
/// sent), queued jobs as cancelled. Send all of them before the Bus stops.
pub fn finish(ctx: &Context) -> Vec<Answer> {
    ctx.with_plugin(|drive: &mut Drive| {
        drive.pending_captures.clear();
        if let Some(running) = drive.running.take() {
            let interrupted = running.steps.iter().any(|s| !matches!(s, Step::Idle));
            let answer = match running.answer {
                Some(answer) => answer,
                None if matches!(running.steps.front(), Some(Step::Await { .. })) => refusal(
                    running.id,
                    "CANCELLED",
                    "the app closed before the screenshot arrived",
                ),
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
        for child in drive.children.values_mut() {
            child.pending_captures.clear();
            child.settle(ctx, "the app closed before this input was read");
            for (id, _, _) in child.requests.drain(..) {
                child
                    .answers
                    .push(refusal(id, "CANCELLED", "the app closed"));
            }
            drive.answers.append(&mut child.answers);
        }
        std::mem::take(&mut drive.answers)
    })
    .unwrap_or_default()
}

/// Answer or queue the drive command `command` (without the app prefix). Call it
/// from app logic, including logic-only wakes. `None` means queued: its
/// answer comes from [`logic`] on a later wake.
pub fn request(ctx: &Context, id: u64, command_name: &str, args: &Value) -> Option<Answer> {
    if let Some(name) = args.get("viewport") {
        let Some(name) = name.as_str() else {
            return Some(refusal(id, "ARGUMENT", "viewport must be a window name"));
        };
        let owner = ctx.viewport_id();
        let target = ctx.data(|d| {
            d.get_temp::<egui::ViewportId>(egui::Id::unique(("drive-viewport", owner, name)))
        });
        let Some(target) = target else {
            return Some(refusal(id, "NOT_FOUND", "viewport is not open"));
        };
        if command_name == "ui.capture"
            && ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("eframe.offscreen_capture")))
                == Some(false)
            && ctx.input(|i| {
                i.raw
                    .viewports
                    .get(&target)
                    .is_some_and(|info| info.visible() == Some(false))
            })
        {
            return Some(refusal(
                id,
                "NOT_SHOWN",
                "this renderer cannot capture a hidden window: show the window first",
            ));
        }
        return ctx
            .with_plugin(|drive: &mut Drive| {
                let app = drive.app.clone();
                let options = drive.options.clone();
                let child = drive.children.entry(target).or_insert_with(|| {
                    Box::new(Drive {
                        app,
                        options,
                        ..Drive::default()
                    })
                });
                if child.requests.len() >= QUEUE_LIMIT {
                    return Some(refusal(id, "BUSY", "viewport request queue is full"));
                }
                if command_name == "ui.capture" {
                    child.pending_captures.insert(id, Instant::now());
                    ctx.send_viewport_cmd_to(target, ViewportCommand::RequestPaintWhileHidden);
                }
                child
                    .requests
                    .push_back((id, command_name.into(), args.clone()));
                child
                    .deadlines
                    .insert(id, Instant::now() + child.options.capture_timeout);
                ctx.request_repaint_of(target);
                // App logic belongs to the root. Wake it too so a parked
                // child capture is checked before the child deadline.
                ctx.request_repaint();
                ctx.request_repaint_after(CAPTURE_QUIET_PERIOD);
                ctx.request_repaint_after(child.options.capture_timeout);
                None
            })
            .unwrap_or_else(|| {
                Some(refusal(
                    id,
                    "UNAVAILABLE",
                    "the drive layer is not installed",
                ))
            });
    }
    let answer = ctx
        .with_plugin(|drive: &mut Drive| drive.request(ctx, id, command_name, args))
        .unwrap_or_else(|| {
            Some(refusal(
                id,
                "UNAVAILABLE",
                "the drive layer is not installed",
            ))
        });
    if answer.is_none() {
        ctx.request_repaint();
    }
    answer
}

/// Name an owned auxiliary viewport for the common drive commands.
pub fn register_viewport(
    ctx: &Context,
    owner: egui::ViewportId,
    name: &str,
    viewport: egui::ViewportId,
) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(("drive-viewport", owner, name)), viewport));
}

pub fn unregister_viewport(
    ctx: &Context,
    owner: egui::ViewportId,
    name: &str,
    viewport: egui::ViewportId,
) {
    let target = ctx.data_mut(|d| {
        let key = egui::Id::unique(("drive-viewport", owner, name));
        if d.get_temp::<egui::ViewportId>(key) == Some(viewport) {
            d.remove_temp::<egui::ViewportId>(key)
        } else {
            None
        }
    });
    if let Some(target) = target {
        ctx.with_plugin(|drive: &mut Drive| {
            if let Some(mut child) = drive.children.remove(&target) {
                child.settle(ctx, "the viewport closed");
                for (id, _, _) in child.requests.drain(..) {
                    child
                        .answers
                        .push(refusal(id, "CANCELLED", "the viewport closed"));
                }
                drive.answers.append(&mut child.answers);
            }
        });
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Id(NodeId),
    Label(String),
}

#[derive(Clone, Debug, PartialEq)]
enum Job {
    Click {
        target: Target,
        button: PointerButton,
        double: bool,
    },
    Pointer {
        at: Pos2,
        action: Event,
    },
    Scroll {
        at: Pos2,
        delta: Vec2,
    },
    Key {
        key: Key,
        modifiers: Modifiers,
    },
    Type {
        text: String,
    },
    /// Report the window state once the window has had two passes.
    Window {
        action: String,
    },
    Capture {
        path: PathBuf,
        tag: u64,
    },
}

enum Step {
    Input(Vec<Event>),
    Viewport(ViewportCommand),
    Idle,
    Await {
        tag: u64,
        path: PathBuf,
        deadline: Instant,
    },
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

/// The plugin: the last widget tree, queued input and finished answers.
#[derive(Default)]
pub struct Drive {
    /// Only the input injected in this pass, cleared before every step.
    input_request_id: Option<u64>,
    /// Pending captures, tracked until completion, cancellation or shutdown.
    pending_captures: HashMap<u64, Instant>,
    last_ui_pass: Option<Instant>,
    /// Quiet time starts when input first waits, and restarts on root UI passes.
    /// Idle time before a request must not shorten its tolerance for a stall.
    input_quiet_since: Option<Instant>,
    children: HashMap<egui::ViewportId, Box<Drive>>,
    requests: VecDeque<(u64, String, Value)>,
    passes: HashMap<std::thread::ThreadId, Vec<egui::ViewportId>>,
    deadlines: HashMap<u64, Instant>,
    screenshots: std::sync::Arc<std::sync::Mutex<Vec<(u64, std::sync::Arc<ColorImage>)>>>,
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
        self.input_quiet_since = Some(Instant::now());
        ctx.enable_accesskit();
    }

    fn input_hook(&mut self, ctx: &Context, input: &mut RawInput) {
        if input.viewport_id == egui::ViewportId::ROOT {
            self.input_quiet_since = Some(Instant::now());
            self.last_ui_pass = Some(Instant::now());
        }
        self.passes
            .entry(std::thread::current().id())
            .or_default()
            .push(input.viewport_id);
        // The platform may switch AccessKit off when a screen reader leaves.
        ctx.enable_accesskit();
        if input.viewport_id != egui::ViewportId::ROOT {
            let app = self.app.clone();
            let options = self.options.clone();
            let child = self.children.entry(input.viewport_id).or_insert_with(|| {
                Box::new(Drive {
                    app,
                    options,
                    ..Drive::default()
                })
            });
            child.last_ui_pass = Some(Instant::now());
            child.step(ctx, input);
            if let Some(running) = &mut child.running
                && running.trace.is_some()
            {
                running.pass_events = input.events.iter().map(|e| format!("{e:?}")).collect();
            }
            return;
        }
        self.step(ctx, input);
        if let Some(running) = &mut self.running
            && running.trace.is_some()
        {
            running.pass_events = input.events.iter().map(|e| format!("{e:?}")).collect();
        }
    }

    fn on_begin_pass(&mut self, ui: &mut Ui) {
        if ui.ctx().viewport_id() != egui::ViewportId::ROOT {
            if let Some(child) = self.children.get_mut(&ui.ctx().viewport_id()) {
                while let Some((id, command_name, args)) = child.requests.pop_front() {
                    if let Some(answer) = child.request(ui.ctx(), id, &command_name, &args) {
                        child.answers.push(answer);
                    }
                }
                for command in child.outbox.drain(..) {
                    ui.ctx().send_viewport_cmd(command);
                }
            }
            return;
        }
        for command in self.outbox.drain(..) {
            ui.ctx().send_viewport_cmd(command);
        }
    }

    fn on_end_pass(&mut self, ui: &mut Ui) {
        if ui.ctx().viewport_id() != egui::ViewportId::ROOT
            && let Some(child) = self.children.get_mut(&ui.ctx().viewport_id())
        {
            // egui pops the viewport before output_hook. Finish reports while
            // the editor's input, menu and window state are still current.
            child.output(ui.ctx(), &mut FullOutput::default());
        }
    }

    fn output_hook(&mut self, ctx: &Context, output: &mut FullOutput) {
        // Deferred viewports can overlap on different render threads. Only
        // passes nested on this thread share a stack.
        let thread = std::thread::current().id();
        let viewport = self
            .passes
            .get_mut(&thread)
            .and_then(Vec::pop)
            // A plugin installed during the app's first frame has not seen
            // input_hook yet. Its first output still belongs to the root.
            .unwrap_or(egui::ViewportId::ROOT);
        if self.passes.get(&thread).is_some_and(Vec::is_empty) {
            self.passes.remove(&thread);
        }
        if viewport != egui::ViewportId::ROOT {
            if let Some(child) = self.children.get_mut(&viewport) {
                if let Some(update) = &output.platform_output.accesskit_update {
                    child.tree = Some(update.clone());
                }
                if !child.answers.is_empty() {
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
            return;
        }
        self.input_quiet_since = Some(Instant::now());
        self.output(ctx, output);
    }
}

impl Drive {
    fn request_capture_pass(&self, ctx: &Context, viewport: egui::ViewportId) {
        // A capture behind input must advance through the same passes as a
        // visible window. Stop asking for paint once only GPU readback remains.
        if self
            .running
            .as_ref()
            .is_some_and(|running| matches!(running.steps.front(), Some(Step::Await { .. })))
        {
            return;
        }
        if !self.pending_captures.is_empty()
            && (!self.queue.is_empty()
                || !self.requests.is_empty()
                || self.running.as_ref().is_some_and(|running| {
                    running
                        .steps
                        .front()
                        .is_some_and(|step| !matches!(step, Step::Await { .. }))
                }))
        {
            ctx.send_viewport_cmd_to(viewport, ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint_of(viewport);
        }
    }

    /// A capture that has painted still gets its full screenshot budget.
    /// The input quiet period only settles input, not GPU readback.
    fn settle_input(&mut self, ctx: &Context, why: &str) {
        // Protection follows step shape (Screenshot/Await), not the job's identity.
        let capture = self.running.as_ref().is_some_and(|running| {
            running.steps.iter().any(|step| {
                matches!(
                    step,
                    Step::Viewport(ViewportCommand::Screenshot(_)) | Step::Await { .. }
                )
            })
        });
        let running = if capture { self.running.take() } else { None };
        let mut captures = VecDeque::new();
        let mut inputs = VecDeque::new();
        for job in std::mem::take(&mut self.queue) {
            if self.pending_captures.contains_key(&job.0) {
                captures.push_back(job);
            } else {
                inputs.push_back(job);
            }
        }
        self.queue = inputs;
        self.settle(ctx, why);
        self.queue = captures;
        self.running = running;
    }

    fn refuse_unshown_captures(&mut self, ctx: &Context, now: Instant) {
        for (id, accepted) in std::mem::take(&mut self.pending_captures) {
            if self.answers.iter().any(|answer| answer.id == id) {
                continue;
            }
            if let Some(running) = &mut self.running
                && running.id == id
                && let Some(answer) = running.answer.take()
            {
                self.answers.push(answer);
                self.running = None;
                self.deadlines.remove(&id);
                continue;
            }
            // Await means the screenshot command has been emitted by a UI pass.
            // Only its capture deadline may expire GPU work already in flight.
            if self.running.as_ref().is_some_and(|running| {
                running.id == id && matches!(running.steps.front(), Some(Step::Await { .. }))
            }) {
                self.pending_captures.insert(id, accepted);
                continue;
            }
            let since = self
                .last_ui_pass
                .map_or(accepted, |pass| pass.max(accepted));
            let quiet = now.saturating_duration_since(since);
            if quiet < CAPTURE_QUIET_PERIOD {
                self.pending_captures.insert(id, accepted);
                ctx.request_repaint_after(CAPTURE_QUIET_PERIOD - quiet);
                continue;
            }
            self.requests.retain(|(queued, _, _)| *queued != id);
            self.queue.retain(|(queued, _)| *queued != id);
            self.traced.retain(|queued| *queued != id);
            self.deadlines.remove(&id);
            if self
                .running
                .as_ref()
                .is_some_and(|running| running.id == id)
            {
                self.running = None;
            }
            self.answers.push(not_shown(id, quiet));
        }
    }

    fn has_pending(&self) -> bool {
        self.running.is_some()
            || !self.queue.is_empty()
            || !self.answers.is_empty()
            || !self.requests.is_empty()
            || self.children.values().any(|child| child.has_pending())
    }

    fn output(&mut self, ctx: &Context, output: &mut FullOutput) {
        self.last_ui_pass = Some(Instant::now());
        if !self.pending_captures.is_empty() {
            ctx.request_repaint_after_for(CAPTURE_QUIET_PERIOD, egui::ViewportId::ROOT);
        }
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
        if self.running.is_some()
            || !self.queue.is_empty()
            || !self.answers.is_empty()
            || !self.cleanup.is_empty()
        {
            ctx.request_repaint();
        }
    }
}

impl Drive {
    fn request(
        &mut self,
        ctx: &Context,
        id: u64,
        command_name: &str,
        args: &Value,
    ) -> Option<Answer> {
        if !args.is_object() {
            return Some(refusal(id, "ARGUMENT", "arguments must be a JSON object"));
        }
        let job = match command_name {
            "ui.tree" => return Some(self.tree_answer(id, args)),
            "ui.menu" => return Some(ok(id, json!({"menu":menu_state(ctx)}))),
            "ui.menu.list" => {
                let menus: Vec<Value> = menu::model(ctx).iter().map(menu_json).collect();
                return Some(ok(id, json!({"menus":menus})));
            }
            "ui.menu.open" => {
                let path: Vec<String> = match (args.get("path"), args.get("menu")) {
                    // Every element a label: a path with anything else is refused,
                    // never opened as a shorter path.
                    (Some(Value::Array(path)), _) => match path
                        .iter()
                        .map(|v| v.as_str().map(str::to_owned))
                        .collect::<Option<Vec<String>>>()
                    {
                        Some(path) => path,
                        None => {
                            return Some(refusal(
                                id,
                                "ARGUMENT",
                                "path: every element is a label (a string)",
                            ));
                        }
                    },
                    (None, Some(Value::String(menu))) => vec![menu.clone()],
                    _ => Vec::new(),
                };
                if path.is_empty() {
                    return Some(refusal(id, "ARGUMENT", "give path: [menu, row, …] or menu"));
                }
                return Some(match menu::open_path(ctx, &path) {
                    Ok(_) => ok(id, json!({"menu":menu_state(ctx)})),
                    Err(message) => refusal(id, "ARGUMENT", &message),
                });
            }
            "ui.menu.close" => return Some(ok(id, json!({"closed":menu::close(ctx)}))),
            "window.state" => return Some(ok(id, window_state(ctx))),
            "window" => return self.window(ctx, id, args),
            "ui.click" => click(args),
            "ui.pointer" => pointer(args),
            "ui.scroll" => scroll(args),
            "ui.key" => key(args),
            "ui.type" => match args.get("text").and_then(Value::as_str) {
                Some(text) => Ok(Job::Type {
                    text: text.to_owned(),
                }),
                None => Err("text must be a string".to_owned()),
            },
            "ui.capture" => self.capture(args),
            _ => return Some(refusal(id, "UNKNOWN_COMMAND", "unknown drive command")),
        };
        let job = match job {
            Ok(job) => job,
            Err(message) => return Some(refusal(id, "ARGUMENT", &message)),
        };
        if command_name == "ui.capture"
            && ctx.input(|i| i.viewport().visible() == Some(false))
            && ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("eframe.offscreen_capture")))
                == Some(false)
        {
            return Some(refusal(
                id,
                "NOT_SHOWN",
                "this renderer cannot capture a hidden window: show the window first",
            ));
        }
        // A hidden window runs no passes, so its input would never be read.
        if command_name != "ui.capture" && ctx.input(|i| i.viewport().visible() == Some(false)) {
            return Some(refusal(
                id,
                "BUSY",
                "the window is hidden: restore it first",
            ));
        }
        if self.queue.len() >= QUEUE_LIMIT {
            return Some(refusal(id, "BUSY", "too many queued inputs"));
        }
        match args.get("trace") {
            None | Some(Value::Null | Value::Bool(false)) => {}
            Some(Value::Bool(true)) => self.traced.push(id),
            Some(_) => return Some(refusal(id, "ARGUMENT", "trace must be a boolean")),
        }
        if self.running.is_none() && self.queue.is_empty() {
            self.input_quiet_since = Some(Instant::now());
        }
        self.queue.push_back((id, job));
        if command_name == "ui.capture" {
            self.pending_captures.insert(id, Instant::now());
            // Bootstrap a full pass even when the request arrived in logic-only
            // processing. The screenshot itself stays behind all earlier input.
            ctx.send_viewport_cmd(ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint_after_for(CAPTURE_QUIET_PERIOD, egui::ViewportId::ROOT);
        }
        None
    }

    /// Window commands go out at once, from inside the frame: a minimized
    /// window runs no passes, so a queued restore would never be sent.
    fn window(&mut self, ctx: &Context, id: u64, args: &Value) -> Option<Answer> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let commands = match action {
            "minimize" => vec![ViewportCommand::Minimized(true)],
            "maximize" => vec![ViewportCommand::Maximized(true)],
            "restore" => vec![
                ViewportCommand::Minimized(false),
                ViewportCommand::Maximized(false),
            ],
            "close" => vec![ViewportCommand::Close],
            "focus" => vec![ViewportCommand::Focus],
            _ => {
                return Some(refusal(
                    id,
                    "ARGUMENT",
                    "action must be minimize, maximize, restore, close or focus",
                ));
            }
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
        self.queue.push_back((
            id,
            Job::Window {
                action: action.to_owned(),
            },
        ));
        None
    }

    /// A capture always lands in the app's own `<runtime>/<app>/captures`
    /// (0700), under a caller-chosen plain file name or a generated one,
    /// and never over an existing file or through a link.
    fn capture(&mut self, args: &Value) -> Result<Job, String> {
        self.shots += 1;
        if args.get("path").is_some() {
            return Err(
                "captures go to the app capture directory: give a file name, not a path".into(),
            );
        }
        let name = match args.get("name") {
            None | Some(Value::Null) => {
                format!("capture-{}-{}.png", std::process::id(), self.shots)
            }
            Some(Value::String(name)) => {
                let plain = !name.is_empty()
                    && name.len() <= 128
                    && !name.starts_with('.')
                    && !name.contains(['/', '\\', '\0'])
                    && !name.contains("..");
                if !plain || !name.ends_with(".png") {
                    return Err(
                        "name must be a plain file name ending .png (no directories, no ..)".into(),
                    );
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
        Ok(Job::Capture {
            path,
            tag: self.shots,
        })
    }

    /// `<runtime>/<app>/captures`, created 0700, refusing a link anywhere
    /// in the part the app owns.
    fn capture_dir(&self) -> Result<PathBuf, String> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let runtime = self
            .options
            .runtime_dir
            .clone()
            .ok_or("XDG_RUNTIME_DIR is not set: no capture directory")?;
        let app = if self.app.is_empty() {
            "app"
        } else {
            &self.app
        };
        let mut dir = runtime;
        for part in [app, "captures"] {
            dir.push(part);
            match std::fs::symlink_metadata(&dir) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => return Err(format!("{} is not a directory", dir.display())),
                Err(_) => std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&dir)
                    .map_err(|e| format!("{}: {e}", dir.display()))?,
            }
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("{}: {e}", dir.display()))?;
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
            self.pending_captures.remove(&running.id);
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
            self.cleanup.push(Event::PointerButton {
                pos: away,
                button: *button,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
        }
        self.cleanup.push(Event::PointerGone);
    }

    fn cancel_queue(&mut self, why: &str) {
        for (id, _) in std::mem::take(&mut self.queue) {
            self.pending_captures.remove(&id);
            self.answers.push(refusal(id, "CANCELLED", why));
        }
    }

    /// Give up on a screenshot past its deadline.
    fn expire(&mut self) {
        for answer in &self.answers {
            self.deadlines.remove(&answer.id);
            self.pending_captures.remove(&answer.id);
        }
        let now = Instant::now();
        let expired: Vec<_> = self
            .deadlines
            .iter()
            .filter_map(|(&id, &deadline)| (now >= deadline).then_some(id))
            .collect();
        for id in expired {
            self.deadlines.remove(&id);
            self.requests.retain(|(queued, _, _)| *queued != id);
            self.queue.retain(|(queued, _)| *queued != id);
            self.traced.retain(|queued| *queued != id);
            if self
                .running
                .as_ref()
                .is_some_and(|running| running.id == id)
            {
                let running = self.running.take().expect("expired input");
                self.release(&running.down);
            }
            self.answers.push(refusal(
                id,
                "TIMEOUT",
                "the viewport did not finish the request in time",
            ));
        }
        let late = |r: &Running| matches!(r.steps.front(), Some(Step::Await { deadline, .. }) if Instant::now() >= *deadline);
        if self.running.as_ref().is_some_and(late) {
            let running = self.running.take().expect("a late capture");
            self.answers.push(refusal(
                running.id,
                "CAPTURE",
                "no screenshot arrived in time",
            ));
        }
        for answer in &self.answers {
            self.pending_captures.remove(&answer.id);
        }
    }

    /// Start the next queued job, answering any that cannot start.
    fn start(&mut self, ctx: &Context) {
        while let Some((id, job)) = self.queue.pop_front() {
            match self.plan(ctx, job) {
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
    fn plan(&self, ctx: &Context, job: Job) -> Result<(Vec<Step>, Report, Value), String> {
        let input = Step::Input;
        let press = |pos, button, pressed| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        };
        Ok(match job {
            Job::Click {
                target,
                button: which,
                double,
            } => {
                let (id, node) = self.find(&target)?;
                let rect = node.bounds().ok_or("that node has no bounds")?;
                let at = pos2(
                    ((rect.x0 + rect.x1) / 2.0) as f32,
                    ((rect.y0 + rect.y1) / 2.0) as f32,
                );
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
                let steps = vec![
                    input(vec![Event::PointerMoved(at)]),
                    input(clicks),
                    Step::Idle,
                ];
                (steps, Report::Ui, json!({"target":target,"at":[at.x,at.y]}))
            }
            Job::Pointer { at, action } => (
                vec![input(vec![Event::PointerMoved(at), action]), Step::Idle],
                Report::Ui,
                json!({}),
            ),
            Job::Scroll { at, delta } => {
                let wheel = Event::MouseWheel {
                    source: egui::MouseWheelSource::Unknown,
                    unit: MouseWheelUnit::Point,
                    delta,
                    phase: TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                };
                (
                    vec![
                        input(vec![Event::PointerMoved(at)]),
                        input(vec![wheel]),
                        Step::Idle,
                    ],
                    Report::Ui,
                    json!({}),
                )
            }
            Job::Key { key, modifiers } => {
                // The modifiers go down before the key and up after it, as a
                // keyboard reports them.
                let event = |pressed| Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                };
                let (down, up) = if modifiers.any() {
                    (
                        vec![Event::ModifiersChanged(modifiers), event(true)],
                        vec![event(false), Event::ModifiersChanged(Modifiers::NONE)],
                    )
                } else {
                    (vec![event(true)], vec![event(false)])
                };
                (
                    vec![input(down), input(up), Step::Idle],
                    Report::Ui,
                    json!({}),
                )
            }
            // A multi-line field takes the whole text at once, line breaks
            // and all, as a paste would: pressing Enter there auto-indents,
            // and the text's own indent would stack on top of it.
            Job::Type { text }
                if !text.is_empty() && self.focused_role() == Some(Role::MultilineTextInput) =>
            {
                (
                    vec![input(vec![Event::Text(text)]), Step::Idle],
                    Report::Ui,
                    json!({}),
                )
            }
            Job::Type { text } => {
                let enter = |pressed| Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                };
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
            Job::Window { action } => (
                vec![Step::Idle, Step::Idle],
                Report::Window,
                json!({"requested":action}),
            ),
            Job::Capture { path, tag } => {
                let completed = self.screenshots.clone();
                let ctx = ctx.clone();
                let shot =
                    ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |image| {
                        completed.lock().unwrap().push((tag, image));
                        // App::logic runs on the root even for child captures.
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                    }));
                let deadline = Instant::now() + self.options.capture_timeout;
                (
                    vec![
                        Step::Viewport(shot),
                        Step::Await {
                            tag,
                            path,
                            deadline,
                        },
                    ],
                    Report::Ui,
                    json!({}),
                )
            }
        })
    }

    fn finish_capture(&mut self) {
        self.receive_capture();
        if self
            .running
            .as_ref()
            .is_some_and(|r| r.steps.is_empty() && r.answer.is_some())
        {
            let running = self.running.take().unwrap();
            self.pending_captures.remove(&running.id);
            self.answers.push(running.answer.unwrap());
        }
    }

    /// GPU readback can finish even while the next UI frame is withheld.
    fn receive_capture(&mut self) {
        let Some(running) = &mut self.running else {
            self.screenshots.lock().unwrap().clear();
            return;
        };
        if let Some(Step::Await { tag, path, .. }) = running.steps.front_mut() {
            let image = self
                .screenshots
                .lock()
                .unwrap()
                .drain(..)
                .find_map(|(completed_tag, image)| (completed_tag == *tag).then_some(image));
            let answer = match image {
                Some(image) => match write_png(path, &image) {
                    Ok(()) => Some(ok(
                        running.id,
                        json!({"path":path,"width":image.size[0],"height":image.size[1]}),
                    )),
                    Err(error) => Some(refusal(running.id, "CAPTURE", &error)),
                },
                None => None,
            };
            if answer.is_some() {
                running.answer = answer;
                running.steps.pop_front();
            }
        }
    }

    /// One pass of the running job: inject its next step.
    fn step(&mut self, ctx: &Context, input: &mut RawInput) {
        self.input_request_id = None;
        // Owed releases go first, in a pass of their own.
        if !self.cleanup.is_empty() {
            input.events.append(&mut self.cleanup);
            return;
        }
        if self.running.is_none() {
            self.start(ctx);
        }
        let Some(running) = &mut self.running else {
            return;
        };
        if matches!(running.steps.front(), Some(Step::Await { .. })) {
            self.receive_capture();
            return;
        }
        match running.steps.pop_front() {
            Some(Step::Input(events)) => {
                self.input_request_id = Some(running.id);
                for event in &events {
                    if let Event::PointerButton {
                        button, pressed, ..
                    } = event
                    {
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
        self.tree
            .iter()
            .flat_map(|t| t.nodes.iter().map(|(id, node)| (*id, node)))
            .collect()
    }

    fn focus(&self) -> Option<NodeId> {
        self.tree.as_ref().map(|t| t.focus)
    }

    fn focused_role(&self) -> Option<Role> {
        let focus = self.focus()?;
        self.nodes().get(&focus).map(|node| node.role())
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
                    let mut hits: Vec<_> = nodes
                        .iter()
                        .filter(|(_, n)| f(n) == Some(text.as_str()))
                        .map(|(id, n)| (*id, *n))
                        .collect();
                    hits.sort_by_key(|(id, _)| id.0);
                    hits
                };
                let labelled = by(label);
                let hits = if labelled.is_empty() {
                    by(Node::placeholder)
                } else {
                    labelled
                };
                // A tooltip or a plain label repeats a control's name: when
                // any match is a control, only the controls count.
                let controls: Vec<_> = hits
                    .iter()
                    .copied()
                    .filter(|(_, n)| actionable(n.role()))
                    .collect();
                if controls.is_empty() { hits } else { controls }
            }
        };
        match &found[..] {
            [one] => Ok(*one),
            [] => Err(format!("no node matches {target:?}")),
            many => {
                let ids: Vec<String> = many.iter().map(|(id, _)| node_id(*id)).collect();
                Err(format!(
                    "{} nodes match {target:?}; click one by id: {}",
                    many.len(),
                    ids.join(", ")
                ))
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
        let Some(tree) = &self.tree else {
            return refusal(id, "BUSY", "no frame has been drawn yet");
        };
        let (filter, role) = match (args.get("label"), args.get("role")) {
            (Some(l), _) if !l.is_string() => {
                return refusal(id, "ARGUMENT", "label must be a string");
            }
            (_, Some(r)) if !r.is_string() => {
                return refusal(id, "ARGUMENT", "role must be a string");
            }
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
        let root = tree
            .tree
            .as_ref()
            .map(|t| t.root)
            .or_else(|| tree.nodes.first().map(|(id, _)| *id));
        let mut out = Vec::new();
        let mut stack: Vec<(NodeId, usize)> = root.into_iter().map(|r| (r, 0)).collect();
        while let Some((node_id, depth)) = stack.pop() {
            let Some(node) = nodes.get(&node_id) else {
                continue;
            };
            let label_matches = filter.is_none() || label(node).is_some_and(matches_label);
            let role_matches = role.is_none_or(|r| format!("{:?}", node.role()) == r);
            if label_matches && role_matches {
                out.push(self.node_json(node_id, node, Some(depth)));
            }
            stack.extend(node.children().iter().rev().map(|c| (*c, depth + 1)));
        }
        ok(
            id,
            json!({"root":root.map(node_id),"focus":self.focus().map(node_id),"nodes":out}),
        )
    }

    /// What an input left: the open menu, the focused node and the pointer.
    fn ui_state(&self, ctx: &Context) -> Value {
        let nodes = self.nodes();
        let focused = self
            .focus()
            .and_then(|f| nodes.get(&f).map(|n| self.node_json(f, n, None)));
        let pointer = ctx.input(|i| i.pointer.latest_pos()).map(|p| [p.x, p.y]);
        json!({"menu":menu_state(ctx),"focused":focused,"pointer":pointer})
    }
}

/// One traced pass: the input events egui read, then what its hit test and
/// pointer state made of them (widget ids as decimal text, as node ids).
fn pass_trace(ctx: &Context, events: Vec<String>) -> Value {
    let text = |id: egui::Id| id.value().to_string();
    let (clicked, drag_started, dragged, drag_stopped, hovered, contains) = ctx
        .interaction_snapshot(|s| {
            let set = |ids: &egui::IdSet| ids.iter().map(|id| text(*id)).collect::<Vec<_>>();
            (
                s.clicked.map(text),
                s.drag_started.map(text),
                s.dragged.map(text),
                s.drag_stopped.map(text),
                set(&s.hovered),
                set(&s.contains_pointer),
            )
        });
    let (latest, down, moved_too_much, time) = ctx.input(|i| {
        (
            i.pointer.latest_pos().map(|p| [p.x, p.y]),
            i.pointer.any_down(),
            !i.pointer.could_any_button_be_click(),
            i.time,
        )
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
            | Role::Row
    )
}

/// The text a person reads on `node`: egui puts a plain label's text in its
/// value.
fn label(node: &Node) -> Option<&str> {
    node.label().or_else(|| {
        if node.role() == Role::Label {
            node.value()
        } else {
            None
        }
    })
}

/// One menu as `ui.menu.list` reports it.
fn menu_json(menu: &menu::Menu) -> Value {
    json!({"title":menu.title,"rows":entries_json(&menu.entries)})
}

fn entries_json(entries: &[menu::Entry]) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| match entry {
            menu::Entry::Separator => json!({"separator":true}),
            menu::Entry::Row(row) => {
                let mut out = json!({"label":row.label,"id":row.id,"shortcut":row.shortcut,
                    "enabled":row.enabled,"checked":row.checked});
                if row.is_submenu() {
                    out["submenu"] = Value::Array(entries_json(&row.children));
                }
                out
            }
        })
        .collect()
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
    parsed
        .map(NodeId)
        .ok_or_else(|| "id must be a node id (decimal text)".to_owned())
}

fn click(args: &Value) -> Result<Job, String> {
    let target = match (args.get("id"), args.get("label")) {
        (Some(id), None) => Target::Id(parse_id(id)?),
        (None, Some(label)) => {
            Target::Label(label.as_str().ok_or("label must be a string")?.to_owned())
        }
        _ => return Err("give exactly one of label and id".into()),
    };
    let double = match args.get("double") {
        None => false,
        Some(double) => double.as_bool().ok_or("double must be a bool")?,
    };
    Ok(Job::Click {
        target,
        button: button(args)?,
        double,
    })
}

fn pointer(args: &Value) -> Result<Job, String> {
    let at = pos2(number(args, "x")?, number(args, "y")?);
    let action = match args.get("action").and_then(Value::as_str) {
        Some("move") => Event::PointerMoved(at),
        Some(action @ ("press" | "release")) => Event::PointerButton {
            pos: at,
            button: button(args)?,
            pressed: action == "press",
            modifiers: Modifiers::NONE,
        },
        _ => return Err("action must be move, press or release".into()),
    };
    Ok(Job::Pointer { at, action })
}

fn scroll(args: &Value) -> Result<Job, String> {
    Ok(Job::Scroll {
        at: pos2(number(args, "x")?, number(args, "y")?),
        delta: vec2(number(args, "dx")?, number(args, "dy")?),
    })
}

fn key(args: &Value) -> Result<Job, String> {
    let name = args
        .get("key")
        .and_then(Value::as_str)
        .ok_or("key must be a key name")?;
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
            Some("ctrl" | "command") => {
                modifiers = modifiers.plus(Modifiers::CTRL).plus(Modifiers::COMMAND)
            }
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
    // Interactive captures favour latency over the smallest possible file.
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    let data: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|p| p.to_srgba_unmultiplied())
        .collect();
    writer.write_image_data(&data).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_origin_tracks_replayed_steps_and_clears_on_idle_passes() {
        let ctx = Context::default();
        install(&ctx, "demo");
        assert!(request(&ctx, 42, "ui.key", &json!({"key":"Enter"})).is_none());
        assert_eq!(input_request(&ctx), None, "accepting a job injects no input");
        assert!(request(&ctx, 43, "ui.key", &json!({"key":"Escape"})).is_none());
        let mut seen = Vec::new();
        for _ in 0..10 {
            ctx.run_ui(RawInput::default(), |ui| {
                seen.push(input_request(ui.ctx()));
            }).drop_without_applying_deltas();
            let _answers = logic(&ctx);
        }
        assert_eq!(seen.first(), Some(&Some(42)), "{seen:?}");
        assert!(seen.contains(&Some(43)), "the later job must get its own origin: {seen:?}");
        assert_eq!(seen.last(), Some(&None), "idle passes must not keep a caller: {seen:?}");
    }

    fn capture_context() -> (Context, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let ctx = Context::default();
        install_with(
            &ctx,
            "demo",
            Options {
                runtime_dir: Some(temp.path().into()),
                ..Options::default()
            },
        );
        (ctx, temp)
    }

    #[test]
    fn capture_refuses_only_at_its_quiet_deadline() {
        for running in [false, true] {
            let (ctx, _temp) = capture_context();
            assert!(request(&ctx, 1, "ui.type", &json!({"text":"waiting"})).is_none());
            assert!(request(&ctx, 2, "ui.capture", &json!({})).is_none());
            if running {
                ctx.with_plugin(|drive: &mut Drive| {
                    drive.queue.pop_front();
                    drive.start(&ctx);
                });
            }
            let now = Instant::now();
            assert!(logic_at(&ctx, now + Duration::from_millis(1)).is_empty());
            assert!(
                logic_at(&ctx, now + INPUT_QUIET_PERIOD)
                    .iter()
                    .all(|a| a.id != 2)
            );
            let answers = logic_at(&ctx, now + CAPTURE_QUIET_PERIOD);
            let answer = answers.iter().find(|a| a.id == 2).unwrap();
            assert_eq!(answer.rc, 10);
            assert_eq!(answer.body["error_code"], "NOT_SHOWN");
            assert!(logic_at(&ctx, now + CAPTURE_QUIET_PERIOD).is_empty());
        }
    }

    #[test]
    fn minimized_and_occluded_captures_wait_for_a_pass_or_the_quiet_deadline() {
        for minimized in [false, true] {
            let (ctx, _temp) = capture_context();
            let mut input = RawInput::default();
            let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
            viewport.minimized = Some(minimized);
            viewport.occluded = Some(!minimized);
            let _output = ctx.run_logic(&input, |ctx| {
                assert!(request(ctx, 1, "ui.capture", &json!({})).is_none());
                let now = Instant::now();
                assert!(logic_at(ctx, now + Duration::from_millis(250)).is_empty());
                assert!(pending(ctx));
                let answers = logic_at(ctx, now + CAPTURE_QUIET_PERIOD);
                assert_eq!(answers[0].body["error_code"], "NOT_SHOWN");
                assert!(!pending(ctx));
            });
        }
    }

    #[test]
    fn hidden_capture_completes_after_an_offscreen_ui_pass() {
        for child in [false, true] {
            let (ctx, temp) = capture_context();
            let target = if child {
                let target = egui::ViewportId::from_hash_of("hidden-child");
                register_viewport(&ctx, egui::ViewportId::ROOT, "child", target);
                target
            } else {
                egui::ViewportId::ROOT
            };
            let mut input = RawInput::default();
            input.viewports.entry(target).or_default().occluded = Some(true);
            let bootstrap = ctx.run_logic(&input, |ctx| {
                let args = if child {
                    json!({"viewport":"child", "name":"hidden.png"})
                } else {
                    json!({"name":"hidden.png"})
                };
                assert!(request(ctx, 7, "ui.capture", &args).is_none());
                assert!(logic(ctx).is_empty());
            });
            assert!(
                bootstrap
                    .viewport_commands
                    .get(&target)
                    .is_some_and(|commands| {
                        commands.iter().any(|command| {
                            matches!(command, ViewportCommand::RequestPaintWhileHidden)
                        })
                    })
            );
            input.viewport_id = target;
            let mut screenshot = None;
            for _ in 0..4 {
                let output = ctx.run_ui(input.clone(), |_| {});
                screenshot = output
                    .viewport_output
                    .values()
                    .flat_map(|v| &v.commands)
                    .find_map(|command| match command {
                        ViewportCommand::Screenshot(callback) => Some(callback.clone()),
                        _ => None,
                    });
                output.drop_without_applying_deltas();
                if screenshot.is_some() {
                    break;
                }
            }
            let callback = screenshot.expect("hidden pass requests screenshot");
            let waiting = ctx.run_logic(&RawInput::default(), |ctx| {
                assert!(logic(ctx).is_empty());
            });
            assert!(
                waiting.viewport_commands.values().flatten().all(|command| {
                    !matches!(command, ViewportCommand::RequestPaintWhileHidden)
                }),
                "readback alone does not keep painting"
            );
            callback.complete(std::sync::Arc::new(ColorImage::filled(
                [67, 31],
                egui::Color32::RED,
            )));
            let answers = logic(&ctx);
            assert_eq!(answers.len(), 1);
            assert_eq!((answers[0].id, answers[0].rc), (7, 0));
            assert!(temp.path().join("demo/captures/hidden.png").is_file());
            assert!(!pending(&ctx));
        }
    }

    #[test]
    fn hidden_capture_preserves_input_order_and_stops_painting_during_readback() {
        let (ctx, _temp) = capture_context();
        assert!(request(&ctx, 1, "ui.type", &json!({"text":"before"})).is_none());
        assert!(request(&ctx, 2, "ui.capture", &json!({})).is_none());
        assert!(request(&ctx, 3, "ui.type", &json!({"text":"after"})).is_none());
        let mut input = RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .minimized = Some(true);
        let mut seen = Vec::new();
        let mut screenshot = None;
        for _ in 0..12 {
            let bootstrap = ctx.run_logic(&input, |ctx| {
                let _ = logic(ctx);
            });
            assert!(
                bootstrap
                    .viewport_commands
                    .values()
                    .flatten()
                    .any(|command| { matches!(command, ViewportCommand::RequestPaintWhileHidden) })
            );
            let output = ctx.run_ui(input.clone(), |ui| {
                ui.input(|i| {
                    seen.extend(i.events.iter().filter_map(|event| match event {
                        Event::Text(text) => Some(text.clone()),
                        _ => None,
                    }));
                });
            });
            screenshot = output
                .viewport_output
                .values()
                .flat_map(|v| &v.commands)
                .find_map(|command| match command {
                    ViewportCommand::Screenshot(callback) => Some(callback.clone()),
                    _ => None,
                });
            output.drop_without_applying_deltas();
            if screenshot.is_some() {
                break;
            }
        }
        assert_eq!(seen, ["before"]);
        let callback = screenshot.expect("capture followed earlier input");
        let waiting = ctx.run_logic(&input, |ctx| {
            let _ = logic(ctx);
        });
        assert!(
            waiting
                .viewport_commands
                .values()
                .flatten()
                .all(|command| { !matches!(command, ViewportCommand::RequestPaintWhileHidden) }),
            "later input stays queued without a paint loop"
        );
        callback.complete(std::sync::Arc::new(ColorImage::filled(
            [4, 3],
            egui::Color32::WHITE,
        )));
        let answers = logic(&ctx);
        assert!(
            answers
                .iter()
                .any(|answer| answer.id == 2 && answer.rc == 0)
        );
        assert!(!answers.iter().any(|answer| answer.id == 3));
        for _ in 0..4 {
            ctx.run_ui(input.clone(), |ui| {
                ui.input(|i| {
                    seen.extend(i.events.iter().filter_map(|event| match event {
                        Event::Text(text) => Some(text.clone()),
                        _ => None,
                    }));
                });
            })
            .drop_without_applying_deltas();
        }
        assert_eq!(seen, ["before", "after"]);
    }

    #[test]
    fn capture_quiet_clock_uses_later_of_acceptance_and_last_pass() {
        for pass_after_acceptance in [false, true] {
            let (ctx, _temp) = capture_context();
            assert!(request(&ctx, 1, "ui.capture", &json!({})).is_none());
            // Still queued: emitted screenshots use capture_timeout instead.
            let start = Instant::now();
            let later = start + Duration::from_secs(2);
            ctx.with_plugin(|drive: &mut Drive| {
                drive
                    .pending_captures
                    .insert(1, if pass_after_acceptance { start } else { later });
                drive.last_ui_pass = Some(if pass_after_acceptance { later } else { start });
            });
            assert!(
                logic_at(
                    &ctx,
                    later + CAPTURE_QUIET_PERIOD - Duration::from_millis(1)
                )
                .is_empty()
            );
            let answers = logic_at(&ctx, later + CAPTURE_QUIET_PERIOD);
            assert_eq!(answers.len(), 1);
            assert_eq!(answers[0].rc, 10);
            assert_eq!(answers[0].body["error_code"], "NOT_SHOWN");
            assert!(!pending(&ctx));
        }
    }

    #[test]
    fn root_ui_pass_does_not_reset_child_capture_clock() {
        let (ctx, _temp) = capture_context();
        let child = egui::ViewportId::from_hash_of("child");
        register_viewport(&ctx, egui::ViewportId::ROOT, "child", child);
        assert!(request(&ctx, 3, "ui.capture", &json!({"viewport":"child"})).is_none());
        ctx.run_ui(RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let now = Instant::now();
        ctx.with_plugin(|drive: &mut Drive| drive.last_ui_pass = Some(now + CAPTURE_QUIET_PERIOD));
        let answers = logic_at(&ctx, now + CAPTURE_QUIET_PERIOD);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].id, 3);
        assert_eq!(answers[0].rc, 10);
        assert_eq!(answers[0].body["error_code"], "NOT_SHOWN");
    }

    fn requested_screenshot(ctx: &Context) -> egui::ScreenshotCallback {
        let output = ctx.run_ui(RawInput::default(), |_| {});
        let callback = output
            .viewport_output
            .values()
            .flat_map(|v| &v.commands)
            .find_map(|c| match c {
                ViewportCommand::Screenshot(callback) => Some(callback.clone()),
                _ => None,
            })
            .expect("capture requested a screenshot");
        output.drop_without_applying_deltas();
        callback
    }

    #[test]
    fn child_passes_do_not_reset_root_capture_clock() {
        let (ctx, _temp) = capture_context();
        ctx.set_embed_viewports(false);
        let child = egui::ViewportId::from_hash_of("clock-child");
        assert!(request(&ctx, 1, "ui.capture", &json!({})).is_none());
        let start = Instant::now();
        ctx.with_plugin(|drive: &mut Drive| {
            drive.pending_captures.insert(1, start);
            drive.last_ui_pass = Some(start);
        });
        for _ in 0..3 {
            let mut input = RawInput {
                viewport_id: child,
                ..Default::default()
            };
            input.viewports.insert(child, Default::default());
            ctx.run_ui(input, |_| {}).drop_without_applying_deltas();
            ctx.with_plugin(|drive: &mut Drive| assert_eq!(drive.last_ui_pass, Some(start)));
        }
        let answers = logic_at(&ctx, start + CAPTURE_QUIET_PERIOD);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].rc, 10);
        assert_eq!(answers[0].body["error_code"], "NOT_SHOWN");
    }

    #[test]
    fn completed_screenshot_survives_hidden_refusal_check() {
        let (ctx, temp) = capture_context();
        assert!(request(&ctx, 1, "ui.capture", &json!({"name":"finished.png"})).is_none());
        let callback = requested_screenshot(&ctx);
        callback.complete(std::sync::Arc::new(ColorImage::filled(
            [4, 3],
            egui::Color32::WHITE,
        )));
        let mut hidden = RawInput::default();
        hidden
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .minimized = Some(true);
        let mut answers = Vec::new();
        let _output = ctx.run_logic(&hidden, |ctx| {
            answers = logic_at(ctx, Instant::now() + CAPTURE_QUIET_PERIOD);
        });
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].rc, 0);
        assert!(temp.path().join("demo/captures/finished.png").is_file());
        assert!(logic(&ctx).is_empty());
    }

    #[test]
    fn child_screenshot_await_schedules_root_backstop_wake() {
        let (ctx, _temp) = capture_context();
        // Consume initial repaint requests so the callback below observes only logic.
        for _ in 0..3 {
            ctx.run_ui(RawInput::default(), |_| {})
                .drop_without_applying_deltas();
        }
        let delays = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = delays.clone();
        ctx.set_request_repaint_callback(move |request| {
            if request.viewport_id == egui::ViewportId::ROOT {
                recorded.lock().unwrap().push(request.delay);
            }
        });
        let child = egui::ViewportId::from_hash_of("await-child");
        let now = Instant::now();
        let budget = Duration::from_millis(200);
        ctx.with_plugin(|drive: &mut Drive| {
            drive.children.insert(
                child,
                Box::new(Drive {
                    running: Some(Running {
                        id: 7,
                        steps: VecDeque::from([Step::Await {
                            tag: 7,
                            path: PathBuf::from("unused.png"),
                            deadline: now + budget,
                        }]),
                        report: Report::Ui,
                        extra: json!({}),
                        answer: None,
                        down: Vec::new(),
                        trace: None,
                        pass_events: Vec::new(),
                    }),
                    ..Drive::default()
                }),
            );
        });
        assert!(logic_at(&ctx, now).is_empty());
        let delays = delays.lock().unwrap();
        assert!(
            delays
                .iter()
                .any(|delay| !delay.is_zero() && *delay <= budget),
            "child deadline must schedule a root wake within its budget: {delays:?}"
        );
    }

    #[test]
    fn ui_pass_before_logic_only_preserves_capture_until_screenshot() {
        let (ctx, temp) = capture_context();
        assert!(request(&ctx, 1, "ui.capture", &json!({"name":"shown.png"})).is_none());
        let callback = requested_screenshot(&ctx);
        assert!(logic_at(&ctx, Instant::now() + INPUT_QUIET_PERIOD).is_empty());
        let duplicate = callback.clone();
        callback.complete(std::sync::Arc::new(ColorImage::filled(
            [4, 3],
            egui::Color32::WHITE,
        )));
        duplicate.complete(std::sync::Arc::new(ColorImage::filled(
            [8, 9],
            egui::Color32::BLACK,
        )));
        let pass = ctx.cumulative_pass_nr();
        let answers = logic(&ctx);
        assert_eq!(
            ctx.cumulative_pass_nr(),
            pass,
            "completion needs no UI pass"
        );
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].rc, 0);
        assert_eq!(answers[0].body["width"], 4);
        assert!(logic(&ctx).is_empty(), "a callback answers only once");
        assert!(temp.path().join("demo/captures/shown.png").is_file());
    }

    #[test]
    fn one_hz_ui_passes_keep_capture_alive_until_screenshot() {
        let (ctx, temp) = capture_context();
        assert!(request(&ctx, 1, "ui.capture", &json!({"name":"trickle.png"})).is_none());
        let callback = requested_screenshot(&ctx);
        let start = Instant::now();
        for seconds in 1..=5 {
            ctx.run_ui(RawInput::default(), |_| {})
                .drop_without_applying_deltas();
            let pass = start + Duration::from_secs(seconds);
            ctx.with_plugin(|drive: &mut Drive| drive.last_ui_pass = Some(pass));
            assert!(logic_at(&ctx, pass + Duration::from_millis(999)).is_empty());
        }
        callback.complete(std::sync::Arc::new(ColorImage::filled(
            [4, 3],
            egui::Color32::WHITE,
        )));
        assert_eq!(logic(&ctx)[0].rc, 0);
        assert!(temp.path().join("demo/captures/trickle.png").is_file());
    }

    #[test]
    fn readback_survives_quiet_period_without_another_ui_pass() {
        let (ctx, _temp) = capture_context();
        assert!(request(&ctx, 1, "ui.capture", &json!({})).is_none());
        let callback = requested_screenshot(&ctx);
        assert!(logic_at(&ctx, Instant::now() + CAPTURE_QUIET_PERIOD * 2).is_empty());
        callback.complete(std::sync::Arc::new(ColorImage::filled(
            [4, 3],
            egui::Color32::WHITE,
        )));
        assert_eq!(logic(&ctx)[0].rc, 0);
    }

    #[test]
    fn hidden_glow_capture_refuses_immediately() {
        let (ctx, _temp) = capture_context();
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("eframe.offscreen_capture"), false));
        // Renderer capability must survive many frames without being read.
        for _ in 0..1_000 {
            ctx.run_ui(RawInput::default(), |_| {})
                .drop_without_applying_deltas();
        }
        let mut input = RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .minimized = Some(true);
        ctx.run_ui(input, |_| {}).drop_without_applying_deltas();
        let answer = request(&ctx, 1, "ui.capture", &json!({})).unwrap();
        assert_eq!(answer.body["error_code"], "NOT_SHOWN");
        assert!(answer.body.to_string().contains("show the window"));
        assert!(!pending(&ctx));
    }

    #[test]
    fn glow_capture_guard_reads_the_target_viewport() {
        for hidden_child in [false, true] {
            let (ctx, _temp) = capture_context();
            let child = egui::ViewportId::from_hash_of("glow-child");
            register_viewport(&ctx, egui::ViewportId::ROOT, "child", child);
            ctx.data_mut(|data| {
                data.insert_temp(egui::Id::unique("eframe.offscreen_capture"), false);
            });
            let mut input = RawInput::default();
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .minimized = Some(!hidden_child);
            input.viewports.entry(child).or_default().minimized = Some(hidden_child);
            ctx.run_ui(input, |_| {}).drop_without_applying_deltas();
            let answer = request(&ctx, 1, "ui.capture", &json!({"viewport":"child"}));
            if hidden_child {
                assert_eq!(answer.unwrap().body["error_code"], "NOT_SHOWN");
            } else {
                assert!(answer.is_none());
            }
        }
    }

    #[test]
    fn painted_capture_uses_gpu_timeout_instead_of_not_shown() {
        let (ctx, _temp) = capture_context();
        assert!(request(&ctx, 1, "ui.capture", &json!({})).is_none());
        let _callback = requested_screenshot(&ctx);
        let now = Instant::now();
        ctx.with_plugin(|drive: &mut Drive| {
            let stalled = now - CAPTURE_QUIET_PERIOD;
            drive.last_ui_pass = Some(stalled);
            drive.pending_captures.insert(1, stalled);
            if let Some(Step::Await { deadline, .. }) =
                drive.running.as_mut().unwrap().steps.front_mut()
            {
                *deadline = now;
            } else {
                panic!("capture awaits readback");
            }
        });
        assert_eq!(logic_at(&ctx, now)[0].body["error_code"], "CAPTURE");
    }

    #[test]
    fn child_capture_refuses_without_waiting_for_child_ui_or_deadline() {
        let (ctx, _temp) = capture_context();
        let child = egui::ViewportId::from_hash_of("child");
        register_viewport(&ctx, egui::ViewportId::ROOT, "child", child);
        assert!(request(&ctx, 3, "ui.capture", &json!({"viewport":"child"})).is_none());
        assert!(logic_at(&ctx, Instant::now() + Duration::from_millis(1)).is_empty());
        let answers = logic_at(&ctx, Instant::now() + CAPTURE_QUIET_PERIOD);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].rc, 10);
        assert_eq!(answers[0].body["error_code"], "NOT_SHOWN");
        assert!(!pending(&ctx));
        assert!(logic(&ctx).is_empty());
    }

    #[test]
    fn painted_capture_without_screenshot_still_expires() {
        let (ctx, _temp) = capture_context();
        assert!(request(&ctx, 1, "ui.capture", &json!({})).is_none());
        ctx.run_ui(RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        ctx.with_plugin(|drive: &mut Drive| {
            let running = drive.running.as_mut().unwrap();
            let Some(Step::Await { deadline, .. }) = running.steps.front_mut() else {
                panic!("capture must await its screenshot");
            };
            *deadline = Instant::now() - Duration::from_millis(1);
        });
        let answers = logic(&ctx);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].body["error_code"], "CAPTURE");
        assert!(!pending(&ctx));
    }

    #[test]
    fn capture_timeout_with_continuing_ui_passes_releases_queued_work() {
        let (ctx, temp) = capture_context();
        let budget = Duration::from_millis(100);
        ctx.with_plugin(|drive: &mut Drive| drive.options.capture_timeout = budget);
        assert!(request(&ctx, 1, "ui.capture", &json!({"name":"missing.png"})).is_none());
        let _uncompleted = requested_screenshot(&ctx);
        let deadline = ctx
            .with_plugin(|drive: &mut Drive| {
                let Some(Step::Await { deadline, .. }) =
                    drive.running.as_ref().unwrap().steps.front()
                else {
                    panic!("capture awaits readback");
                };
                *deadline
            })
            .unwrap();
        assert!(request(&ctx, 2, "ui.type", &json!({"text":"later"})).is_none());
        let mut answers = Vec::new();
        while Instant::now() < deadline + Duration::from_secs(1) {
            ctx.run_ui(RawInput::default(), |_| {})
                .drop_without_applying_deltas();
            answers.extend(logic(&ctx));
            if answers.iter().any(|answer| answer.id == 1) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let capture = answers
            .iter()
            .find(|answer| answer.id == 1)
            .expect("capture expired");
        assert!(Instant::now() >= deadline);
        assert_eq!(capture.body["error_code"], "CAPTURE");
        assert_eq!(capture.body["message"], "no screenshot arrived in time");
        assert!(!temp.path().join("demo/captures/missing.png").exists());
        for _ in 0..4 {
            ctx.run_ui(RawInput::default(), |_| {})
                .drop_without_applying_deltas();
            answers.extend(logic(&ctx));
        }
        assert!(
            answers
                .iter()
                .any(|answer| answer.id == 2 && answer.rc == 0),
            "queued typing completes after capture refusal: {answers:?}"
        );
        assert!(!pending(&ctx));
    }

    #[test]
    fn capture_ignores_an_unrelated_callback_until_its_own_completes() {
        let (ctx, temp) = capture_context();
        assert!(request(&ctx, 1, "ui.capture", &json!({"name":"mine.png"})).is_none());
        let callback = requested_screenshot(&ctx);
        egui::ScreenshotCallback::new(|_| {}).complete(std::sync::Arc::new(ColorImage::filled(
            [8, 9],
            egui::Color32::BLACK,
        )));
        assert!(logic(&ctx).is_empty());
        assert!(pending(&ctx));
        assert!(!temp.path().join("demo/captures/mine.png").exists());
        callback.complete(std::sync::Arc::new(ColorImage::filled(
            [4, 3],
            egui::Color32::WHITE,
        )));
        let answers = logic(&ctx);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].rc, 0);
        assert_eq!(answers[0].body["width"], 4);
        assert_eq!(answers[0].body["height"], 3);
        assert!(temp.path().join("demo/captures/mine.png").is_file());
    }

    #[test]
    fn pending_input_survives_until_exactly_the_quiet_deadline() {
        let ctx = Context::default();
        install(&ctx, "demo");
        let now = Instant::now();
        ctx.with_plugin(|drive: &mut Drive| {
            drive.input_quiet_since = Some(now);
            drive.queue.push_back((
                99,
                Job::Type {
                    text: "hello".into(),
                },
            ));
        });
        let deadline = now + INPUT_QUIET_PERIOD;
        let before = deadline.checked_sub(Duration::from_millis(1)).unwrap();
        assert!(logic_at(&ctx, before).is_empty());
        assert!(pending(&ctx));
        let answers = logic_at(&ctx, now + INPUT_QUIET_PERIOD);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].id, 99);
        assert_eq!(answers[0].body["error_code"], "CANCELLED");
        assert!(
            answers[0]
                .body
                .to_string()
                .contains("no frames for 1000 ms")
        );
        assert!(!pending(&ctx));
    }

    #[test]
    fn idle_time_before_input_does_not_shorten_the_quiet_period() {
        let ctx = Context::default();
        install(&ctx, "demo");
        let now = Instant::now();
        ctx.with_plugin(|drive: &mut Drive| {
            drive.input_quiet_since = now.checked_sub(Duration::from_secs(30));
        });
        assert!(request(&ctx, 100, "ui.type", &json!({"text":"hello"})).is_none());
        assert!(logic_at(&ctx, now + Duration::from_millis(500)).is_empty());
        assert!(pending(&ctx));
    }

    #[test]
    fn overlapping_render_threads_keep_their_pass_attribution() {
        use egui::Plugin;
        use std::sync::{Arc, Mutex, mpsc};
        let ctx = Context::default();
        let drive = Arc::new(Mutex::new(Drive::default()));
        let child = egui::ViewportId::from_hash_of("overlapping-child");
        let update = |id| TreeUpdate {
            nodes: vec![],
            tree: None,
            tree_id: egui::accesskit::TreeId::ROOT,
            focus: NodeId(id),
        };
        drive
            .lock()
            .unwrap()
            .input_hook(&ctx, &mut RawInput::default());
        let (begun_tx, begun_rx) = mpsc::channel();
        let (end_tx, end_rx) = mpsc::channel();
        let child_drive = drive.clone();
        let child_ctx = ctx.clone();
        let handle = std::thread::spawn(move || {
            child_drive.lock().unwrap().input_hook(
                &child_ctx,
                &mut RawInput {
                    viewport_id: child,
                    ..Default::default()
                },
            );
            begun_tx.send(()).unwrap();
            end_rx.recv().unwrap();
            let mut output = FullOutput::default();
            output.platform_output.accesskit_update = Some(update(22));
            child_drive
                .lock()
                .unwrap()
                .output_hook(&child_ctx, &mut output);
        });
        begun_rx.recv().unwrap();
        let mut output = FullOutput::default();
        output.platform_output.accesskit_update = Some(update(11));
        drive.lock().unwrap().output_hook(&ctx, &mut output);
        end_tx.send(()).unwrap();
        handle.join().unwrap();
        let drive = drive.lock().unwrap();
        assert_eq!(drive.tree.as_ref().unwrap().focus, NodeId(11));
        assert_eq!(
            drive.children[&child].tree.as_ref().unwrap().focus,
            NodeId(22)
        );
        assert!(drive.passes.is_empty());
    }

    #[test]
    fn idle_child_requests_expire_and_release_the_bus_queue() {
        let ctx = Context::default();
        install(&ctx, "demo");
        let child = egui::ViewportId::from_hash_of("idle-child");
        register_viewport(&ctx, egui::ViewportId::ROOT, "macro-editor", child);
        assert!(
            request(
                &ctx,
                70,
                "ui.type",
                &json!({"viewport":"macro-editor", "text":"hello"})
            )
            .is_none()
        );
        assert!(pending(&ctx));
        ctx.with_plugin(|drive: &mut Drive| {
            drive
                .children
                .get_mut(&child)
                .unwrap()
                .deadlines
                .insert(70, Instant::now());
        });
        let answers = logic(&ctx);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].body["error_code"], "TIMEOUT");
        assert!(!pending(&ctx));
    }

    #[test]
    fn viewport_names_are_window_local_and_removal_compares_the_target() {
        let ctx = Context::default();
        install(&ctx, "demo");
        let a = egui::ViewportId::ROOT;
        let b = egui::ViewportId::from_hash_of("window-b");
        let editor_a = egui::ViewportId::from_hash_of("editor-a");
        let editor_b = egui::ViewportId::from_hash_of("editor-b");
        register_viewport(&ctx, a, "macro-editor", editor_a);
        register_viewport(&ctx, b, "macro-editor", editor_b);
        assert!(request(&ctx, 8, "ui.tree", &json!({"viewport":"macro-editor"})).is_none());
        unregister_viewport(&ctx, b, "macro-editor", editor_b);
        unregister_viewport(&ctx, a, "macro-editor", editor_b);
        assert!(pending(&ctx));
        ctx.with_plugin(|drive: &mut Drive| {
            assert_eq!(drive.children[&editor_a].requests.front().unwrap().0, 8);
        });
        unregister_viewport(&ctx, a, "macro-editor", editor_a);
        assert_eq!(logic(&ctx)[0].body["error_code"], "CANCELLED");
        assert!(!pending(&ctx));
    }

    #[test]
    fn gpu_capture_completion_reaches_the_editor() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("editor.png");
        let editor = egui::ViewportId::from_hash_of("captured-editor");
        let mut drive = Drive::default();
        drive.children.insert(
            editor,
            Box::new(Drive {
                running: Some(Running {
                    id: 7,
                    steps: VecDeque::from([Step::Await {
                        tag: 7,
                        path: path.clone(),
                        deadline: Instant::now() + Duration::from_secs(10),
                    }]),
                    report: Report::Ui,
                    extra: json!({}),
                    answer: None,
                    down: Vec::new(),
                    trace: None,
                    pass_events: Vec::new(),
                }),
                ..Drive::default()
            }),
        );
        let ctx = Context::default();
        let (steps, _, _) = drive.children[&editor]
            .plan(
                &ctx,
                Job::Capture {
                    path: path.clone(),
                    tag: 7,
                },
            )
            .unwrap();
        let callback = steps
            .into_iter()
            .find_map(|step| match step {
                Step::Viewport(ViewportCommand::Screenshot(callback)) => Some(callback),
                _ => None,
            })
            .unwrap();
        ctx.add_plugin(drive);
        callback.complete(std::sync::Arc::new(ColorImage::filled(
            [4, 3],
            egui::Color32::WHITE,
        )));
        let answers = logic(&ctx);
        assert_eq!(answers.len(), 1);
        let answer = &answers[0];
        assert_eq!(answer.rc, 0);
        assert_eq!(
            (
                answer.body["width"].as_u64(),
                answer.body["height"].as_u64()
            ),
            (Some(4), Some(3))
        );
        assert!(path.is_file());
    }

    #[test]
    fn auxiliary_viewport_has_its_own_tree_input_and_close() {
        let ctx = Context::default();
        install(&ctx, "demo");
        ctx.set_embed_viewports(false);
        let editor = egui::ViewportId::from_hash_of("test-editor");
        register_viewport(&ctx, egui::ViewportId::ROOT, "macro-editor", editor);
        let mut root_clicks = 0;
        let mut editor_clicks = 0;
        {
            let mut frame = |viewport| {
                let mut input = RawInput {
                    viewport_id: viewport,
                    ..Default::default()
                };
                input.viewports.insert(editor, Default::default());
                let mut output = ctx.run_ui(input, |ui| {
                    if viewport == egui::ViewportId::ROOT {
                        ui.ctx().show_viewport_deferred(
                            editor,
                            egui::ViewportBuilder::default(),
                            |_, _| {},
                        );
                    }
                    if ui
                        .button(if viewport == editor { "Save" } else { "Root" })
                        .clicked()
                    {
                        if viewport == editor {
                            editor_clicks += 1;
                        } else {
                            root_clicks += 1;
                        }
                    }
                });
                output.textures_delta.clear();
            };
            for _ in 0..3 {
                frame(egui::ViewportId::ROOT);
                frame(editor);
            }
            assert!(request(&ctx, 1, "ui.tree", &json!({"viewport":"macro-editor"})).is_none());
            frame(editor);
            let answers = logic(&ctx);
            assert!(answers[0].body.to_string().contains("Save"));
            assert!(!answers[0].body.to_string().contains("Root"));
            assert!(
                request(
                    &ctx,
                    2,
                    "ui.click",
                    &json!({"viewport":"macro-editor", "label":"Save"})
                )
                .is_none()
            );
            for _ in 0..8 {
                frame(egui::ViewportId::ROOT);
                frame(editor);
            }
        }
        assert_eq!((root_clicks, editor_clicks), (0, 1));
        assert_eq!(logic(&ctx)[0].rc, 0);
        unregister_viewport(&ctx, egui::ViewportId::ROOT, "macro-editor", editor);
        assert_eq!(
            request(&ctx, 3, "ui.tree", &json!({"viewport":"macro-editor"}))
                .unwrap()
                .body["error_code"],
            "NOT_FOUND"
        );
    }

    #[test]
    fn every_command_is_described_once() {
        let names: Vec<String> = describe("demo")
            .iter()
            .filter_map(|v| v["name"].as_str().map(str::to_owned))
            .collect();
        let expected: Vec<String> = COMMANDS.iter().map(|v| format!("demo.{v}")).collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn arguments_are_validated() {
        let ctx = Context::default();
        install(&ctx, "demo");
        let code = |command_name: &str, args: Value| {
            request(&ctx, 1, command_name, &args)
                .map(|a| a.body["error_code"].as_str().unwrap_or("").to_owned())
        };
        assert_eq!(
            code("ui.nope", json!({})).as_deref(),
            Some("UNKNOWN_COMMAND")
        );
        assert_eq!(code("ui.tree", json!([])).as_deref(), Some("ARGUMENT"));
        assert_eq!(code("ui.click", json!({})).as_deref(), Some("ARGUMENT"));
        assert_eq!(
            code("ui.click", json!({"label":"a","id":1})).as_deref(),
            Some("ARGUMENT")
        );
        assert_eq!(
            code("ui.click", json!({"label":"a","button":"thumb"})).as_deref(),
            Some("ARGUMENT")
        );
        assert_eq!(
            code("ui.pointer", json!({"x":1,"y":2,"action":"wave"})).as_deref(),
            Some("ARGUMENT")
        );
        assert_eq!(
            code("ui.scroll", json!({"x":1,"y":2,"dx":"far"})).as_deref(),
            Some("ARGUMENT")
        );
        assert_eq!(
            code("ui.key", json!({"key":"NoSuchKey"})).as_deref(),
            Some("ARGUMENT")
        );
        assert_eq!(
            code("ui.key", json!({"key":"A","modifiers":["hyper"]})).as_deref(),
            Some("ARGUMENT")
        );
        assert_eq!(
            code("ui.type", json!({"text":5})).as_deref(),
            Some("ARGUMENT")
        );
        assert_eq!(
            code("ui.capture", json!({"path":"/tmp/x.png"})).as_deref(),
            Some("ARGUMENT"),
            "no caller paths"
        );
        assert_eq!(
            code("ui.click", json!({"id":"12x"})).as_deref(),
            Some("ARGUMENT")
        );
        assert_eq!(
            code("window", json!({"action":"spin"})).as_deref(),
            Some("ARGUMENT")
        );
        assert_eq!(
            code("ui.tree", json!({})).as_deref(),
            Some("BUSY"),
            "no frame yet"
        );
        assert_eq!(
            code("ui.click", json!({"label":"a"})),
            None,
            "valid input is queued"
        );
    }

    #[test]
    fn without_the_plugin_every_command_is_unavailable() {
        let ctx = Context::default();
        let answer = request(&ctx, 3, "ui.tree", &json!({})).unwrap();
        assert_eq!(answer.body["error_code"], "UNAVAILABLE");
    }

    /// Typing into a multi-line field sends the text whole, so an editor's
    /// Enter auto-indent never stacks on the text's own indent; a single-line
    /// field still gets Enter for each line break.
    #[test]
    fn typing_into_a_multi_line_field_keeps_the_text_verbatim() {
        let focused = |role| Drive {
            tree: Some(TreeUpdate {
                nodes: vec![(NodeId(5), Node::new(role))],
                tree: None,
                tree_id: egui::accesskit::TreeId::ROOT,
                focus: NodeId(5),
            }),
            ..Drive::default()
        };
        let ctx = Context::default();
        let text = "if $a then\n  print(1)\nend";
        let events = |drive: &Drive| -> Vec<Vec<Event>> {
            let (steps, _, _) = drive.plan(&ctx, Job::Type { text: text.into() }).unwrap();
            steps
                .into_iter()
                .filter_map(|step| match step {
                    Step::Input(events) => Some(events),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            events(&focused(Role::MultilineTextInput)),
            vec![vec![Event::Text(text.into())]]
        );
        let single = events(&focused(Role::TextInput));
        assert_eq!(single.len(), 5, "three lines and two Enter presses");
        assert_eq!(single[0], vec![Event::Text("if $a then".into())]);
        assert!(matches!(
            single[1][0],
            Event::Key {
                key: Key::Enter,
                pressed: true,
                ..
            }
        ));
    }
}
