// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! One capture job, over compd's commands: hide this window (fenced
//! minimisation, confirmed by the compositor), let the person select a
//! region if asked, capture a fresh frame into the private captures
//! directory, decode it, and restore the window: after success, failure and
//! cancellation alike. Completion is each command's final reply; nothing
//! sleeps or polls (the person's own delay is the only timer).
use annotation::Document;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// What to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// A whole output.
    #[default]
    Screen,
    /// One window, by id and generation.
    Window,
    /// A region the person drags out.
    Region,
}

impl Mode {
    pub const ALL: [Self; 3] = [Self::Screen, Self::Window, Self::Region];

    pub fn name(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Window => "window",
            Self::Region => "region",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// A compd window identity: never guessed, always both halves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: u64,
    pub generation: u64,
}

impl Target {
    /// `ID:GENERATION`, as the command line takes it.
    pub fn parse(text: &str) -> Option<Self> {
        let (id, generation) = text.split_once(':')?;
        Some(Self {
            id: id.parse().ok()?,
            generation: generation.parse().ok()?,
        })
    }
}

/// The longest delay a capture takes, in seconds.
pub const MAX_DELAY: u32 = 10;

/// A rectangle given up front, so a region capture needs no drag:
/// output-local displayed logical pixels, as `comp.region.select` answers
/// and `comp.capture.frame` takes. `output` names the output it is on
/// (else the capture's `output`, else compd's choice: the pointer's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

impl Region {
    /// Checked as compd checks it: non-negative origin, positive extent.
    pub fn validate(&self) -> Result<(), String> {
        if self.x < 0
            || self.y < 0
            || self.width <= 0
            || self.height <= 0
            || self.x.checked_add(self.width).is_none()
            || self.y.checked_add(self.height).is_none()
        {
            return Err(
                "region must have a non-negative origin and a positive width and height".into(),
            );
        }
        if self
            .output
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 256)
        {
            return Err("invalid region output name".into());
        }
        Ok(())
    }

    /// The rectangle alone, as `comp.capture.frame` takes it.
    fn rectangle(&self) -> Value {
        json!({"x": self.x, "y": self.y, "width": self.width, "height": self.height})
    }
}

/// One capture's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub window: Option<Target>,
    /// Include the pointer; off unless asked, everywhere.
    #[serde(default)]
    pub cursor: bool,
    #[serde(default)]
    pub delay: u32,
    /// Region mode only: the rectangle to capture, skipping the drag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            mode: Mode::Screen,
            output: None,
            window: None,
            cursor: false,
            delay: 0,
            region: None,
        }
    }
}

impl Request {
    /// Give the region up front; an output it names replaces the chosen one.
    pub fn set_region(&mut self, region: Option<Region>) {
        if region.as_ref().is_some_and(|r| r.output.is_some()) {
            self.output = None;
        }
        self.region = region;
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.delay > MAX_DELAY {
            return Err(format!("delay must be 0..{MAX_DELAY} seconds"));
        }
        if self
            .output
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 256)
        {
            return Err("invalid output name".into());
        }
        if self.mode == Mode::Window && self.window.is_none() {
            return Err("choose a window first".into());
        }
        if self.mode != Mode::Window && self.window.is_some() {
            return Err("window only applies to window mode".into());
        }
        if let Some(region) = &self.region {
            if self.mode != Mode::Region {
                return Err("region only applies to region mode".into());
            }
            region.validate()?;
            if let (Some(a), Some(b)) = (&region.output, &self.output)
                && a != b
            {
                return Err("region.output and output name different outputs".into());
            }
        }
        Ok(())
    }
}

/// A window compd lists, as the Window menu offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Window {
    pub target: Target,
    pub title: String,
    pub app_id: String,
    pub focused: bool,
    pub minimized: bool,
}

impl Window {
    /// The name a person reads: the title, else the app id.
    pub fn name(&self) -> &str {
        if self.title.is_empty() {
            &self.app_id
        } else {
            &self.title
        }
    }
}

/// The windows in a `comp.windows.list` reply, except `own` (this app's,
/// matched by app id and process). A row without both id halves is
/// dropped, never guessed.
pub fn windows(value: &Value, own: Option<&str>) -> Vec<Window> {
    let pid = u64::from(std::process::id());
    value["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| !(own.is_some_and(|id| row["app_id"] == id) && row["pid"] == pid))
        .filter_map(|row| {
            Some(Window {
                target: Target {
                    id: row["id"].as_u64()?,
                    generation: row["generation"].as_u64()?,
                },
                title: row["title"].as_str().unwrap_or("").into(),
                app_id: row["app_id"].as_str().unwrap_or("").into(),
                focused: row["focused"].as_bool().unwrap_or(false),
                minimized: row["minimized"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// The output names in a `comp.props.get {path: "outputs"}` reply.
pub fn outputs(value: &Value) -> Vec<String> {
    let outputs = value.get("value").unwrap_or(value);
    outputs
        .as_object()
        .into_iter()
        .flat_map(|map| map.values())
        .filter_map(|row| row["name"].as_str().map(str::to_owned))
        .collect()
}

/// The compositor, as a capture needs it: one call at a time with a time
/// limit, answered with the reply's JSON or a refusal.
pub trait Comp: Send + Sync {
    fn call(
        &self,
        command_name: &'static str,
        args: Value,
        limit: Duration,
    ) -> BoxFuture<'static, Result<Value, String>>;
}

/// compd on the Bus, through the app's own connection.
pub struct CompBus {
    pub handle: citizen::Handle,
    pub service: String,
}

impl Comp for CompBus {
    fn call(
        &self,
        command_name: &'static str,
        args: Value,
        limit: Duration,
    ) -> BoxFuture<'static, Result<Value, String>> {
        let (handle, service) = (self.handle.clone(), self.service.clone());
        Box::pin(async move {
            let reply = handle
                .raw_within(&service, command_name, args.to_string(), limit)
                .await
                .map_err(|e| format!("{command_name}: {e}"))?;
            if reply.rc != 0 {
                let why = if reply.body.trim().is_empty() {
                    reply.error.unwrap_or_default()
                } else {
                    reply.body
                };
                return Err(format!("{command_name} refused (rc {}): {why}", reply.rc));
            }
            if reply.body.trim().is_empty() {
                return Ok(Value::Null);
            }
            serde_json::from_str(&reply.body).map_err(|e| format!("{command_name}: {e}"))
        })
    }
}

const STEP: Duration = Duration::from_secs(5);
/// A region selection waits on the person; compd bounds it itself.
const SELECTION_LIMIT: Duration = Duration::from_secs(70);
/// How a selection that ended without a region begins its error:
/// `selection <compd's status>`.
const SELECTION: &str = "selection ";

/// compd's status for a selection that ended without a region (`timeout`,
/// `cancelled`, …), read from the capture's error; `None` for any other
/// failure.
pub fn selection_ending(error: &str) -> Option<&str> {
    let rest = error.strip_prefix(SELECTION)?;
    Some(rest.split(';').next().unwrap_or(rest).trim())
}
const FRAME: Duration = Duration::from_secs(8);

/// This process's window, once it is mapped: `comp.window.wait` until it
/// is, then its identity from `comp.windows.list` (app id and pid).
pub async fn own_window(comp: &dyn Comp, app_id: &str) -> Result<Target, String> {
    comp.call(
        "comp.window.wait",
        json!({"match":{"app_id":app_id},"until":"mapped","timeout_ms":10_000}),
        Duration::from_secs(12),
    )
    .await?;
    let rows = comp.call("comp.windows.list", json!({}), STEP).await?;
    let pid = u64::from(std::process::id());
    rows["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["app_id"] == app_id && row["pid"] == pid)
        .and_then(|row| {
            Some(Target {
                id: row["id"].as_u64()?,
                generation: row["generation"].as_u64()?,
            })
        })
        .ok_or_else(|| "this window is not yet known to compd".into())
}

/// A finished capture.
#[derive(Debug, Clone)]
pub struct Captured {
    pub document: Document,
    /// The raw frame compd wrote, in the private captures directory.
    pub path: PathBuf,
    /// compd's reply (output, scale, source, window).
    pub metadata: Value,
}

fn cancelled(cancel: &tokio::sync::watch::Receiver<bool>) -> bool {
    *cancel.borrow()
}

/// How often a restore is tried before the capture gives up and reports
/// `restore_failed` (the person brings the window back), and the first
/// pause between tries. The five pauses
/// double from 0.25 s to 4 s: 7.75 s of waiting, plus each call's own
/// 5 s limit when compd does not answer.
const RESTORE_TRIES: u32 = 6;
const RESTORE_PAUSE: Duration = Duration::from_millis(250);

/// How a restore ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restored {
    /// compd confirmed the window is shown again.
    Shown,
    /// The window is gone (closed, or replaced by a new generation):
    /// nothing is left to restore.
    Gone,
}

/// Restore `target`, retrying with backoff until compd confirms it or says
/// the window is gone. With `activate` false compd only unminimises it,
/// leaving the focus and the workspace alone (an agent's capture). An
/// error means it is still hidden as far as anyone knows: the capture says
/// so, and the person brings the window back.
pub async fn restore(comp: &dyn Comp, target: Target, activate: bool) -> Result<Restored, String> {
    let args = json!({"id": target.id, "generation": target.generation, "activate": activate});
    let mut pause = RESTORE_PAUSE;
    let mut last = String::new();
    for attempt in 0..RESTORE_TRIES {
        if attempt > 0 {
            tokio::time::sleep(pause).await;
            pause *= 2;
        }
        match comp.call("comp.window.restore", args.clone(), STEP).await {
            Ok(reply) if reply["minimized"].as_bool() == Some(false) => {
                return Ok(Restored::Shown);
            }
            Ok(reply) => last = format!("not restored: {reply}"),
            Err(error) => last = error,
        }
        // A listing without the window means there is nothing to restore;
        // a listing that fails (the Bus is down) means try again.
        if let Ok(rows) = comp.call("comp.windows.list", json!({}), STEP).await
            && !rows["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|row| row["id"] == target.id && row["generation"] == target.generation)
        {
            return Ok(Restored::Gone);
        }
    }
    Err(last)
}

/// A capture job's outcome, and the window it left hidden if restoring it
/// failed (the app reports it; the person brings the window back).
#[derive(Debug)]
pub struct Taken {
    pub result: Result<Captured, String>,
    pub unrestored: Option<Target>,
    /// The capture was cancelled (whatever else the error says).
    pub cancelled: bool,
}

/// Run `request`. With `hide` (this app's id), the app's own window is
/// minimised first and restored afterwards whatever happened, retrying
/// with backoff; a restore that still fails is reported with the capture's
/// own outcome. With `focus` the window is restored activated and raised,
/// whether the capture succeeded or not (a person asked for it); without
/// it the restore leaves the focus where it is (an agent's capture). The raw frame is made private (0600), and deleted again
/// when the capture does not complete.
pub async fn take(
    comp: Arc<dyn Comp>,
    request: Request,
    hide: Option<&'static str>,
    focus: bool,
    directory: PathBuf,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<Captured, String> {
    take_job(comp, request, hide, focus, directory, cancel)
        .await
        .result
}

/// [`take`], also reporting a window it could not restore.
pub async fn take_job(
    comp: Arc<dyn Comp>,
    request: Request,
    hide: Option<&'static str>,
    focus: bool,
    directory: PathBuf,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> Taken {
    let refused = |error: String| Taken {
        cancelled: error == "cancelled",
        result: Err(error),
        unrestored: None,
    };
    if let Err(error) = request.validate() {
        return refused(error);
    }
    if cancelled(&cancel) {
        return refused("cancelled".into());
    }
    let mut hidden: Option<Target> = None;
    let mut written: Option<PathBuf> = None;
    let mut result = async {
        if request.delay > 0 {
            let timer = tokio::time::sleep(Duration::from_secs(u64::from(request.delay)));
            tokio::select! {
                () = timer => {}
                _ = cancel.changed() => return Err("cancelled".into()),
            }
        }
        if cancelled(&cancel) {
            return Err("cancelled".into());
        }
        if let Some(app_id) = hide {
            let target = own_window(comp.as_ref(), app_id).await?;
            // Restore even when the reply to the minimise is lost.
            hidden = Some(target);
            let reply = comp
                .call("comp.window.minimize", json!(target), STEP)
                .await?;
            if reply["minimized"].as_bool() != Some(true) {
                return Err("the compositor did not confirm the window was hidden".into());
            }
        }
        if cancelled(&cancel) {
            return Err("cancelled".into());
        }
        let path = directory.join(format!("capture-{}.png", uuid::Uuid::now_v7()));
        let mut args = json!({"path": path, "cursor": request.cursor});
        if let Some(target) = &request.window {
            args["window"] = json!(target);
        } else if let Some(output) = &request.output {
            args["output"] = json!(output);
        }
        if let Some(region) = request
            .region
            .as_ref()
            .filter(|_| request.mode == Mode::Region)
        {
            // Given up front (an agent's capture): no drag, the seat stays
            // with whoever has it.
            if let Some(output) = region.output.as_ref().or(request.output.as_ref()) {
                args["output"] = json!(output);
            }
            args["region"] = region.rectangle();
        } else if request.mode == Mode::Region {
            let selection = comp
                .call(
                    "comp.region.select",
                    request
                        .output
                        .as_ref()
                        .map_or(json!({}), |o| json!({"output": o})),
                    SELECTION_LIMIT,
                )
                .await?;
            if selection["status"] != "selected" {
                return Err(format!(
                    "{SELECTION}{}",
                    selection["status"].as_str().unwrap_or("failed")
                ));
            }
            args["output"] = selection["output"].clone();
            args["region"] = selection["region"].clone();
            args["output_generation"] = selection["output_generation"].clone();
        }
        if cancelled(&cancel) {
            return Err("cancelled".into());
        }
        written = Some(path.clone());
        let metadata = comp.call("comp.capture.frame", args, FRAME).await?;
        if cancelled(&cancel) {
            return Err("cancelled".into());
        }
        let source = path.clone();
        let document = tokio::task::spawn_blocking(move || {
            annotation::library::secure_capture(&source)?;
            Document::open(&source)
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(Captured {
            document,
            path,
            metadata,
        })
    }
    .await;
    if result.is_ok() && cancelled(&cancel) {
        result = Err("cancelled".into());
    }
    let was_cancelled = cancelled(&cancel) || result.as_ref().is_err_and(|e| e == "cancelled");
    // A frame that does not become the document is not kept.
    if result.is_err()
        && let Some(path) = written
    {
        annotation::library::forget_capture(&directory, &path);
    }
    let mut unrestored = None;
    if let Some(target) = hidden {
        match restore(comp.as_ref(), target, focus).await {
            Ok(Restored::Shown) => {
                // A person's capture ends with the window on top and
                // focused, whatever happened (a timed-out or cancelled
                // selection too): the restore activates it, and this raise
                // covers a window compd no longer had minimised.
                if focus {
                    // Best effort: compd may refuse focus (an exclusive layer).
                    let raise =
                        json!({"id": target.id, "generation": target.generation, "raise": true});
                    let _ = comp.call("comp.window.focus", raise, STEP).await;
                }
            }
            Ok(Restored::Gone) => {}
            // The capture stands; the app reports the failure.
            Err(error) => {
                unrestored = Some(target);
                if let Err(failed) = &mut result {
                    *failed = format!("{failed}; restoring the window failed: {error}");
                }
            }
        }
    }
    Taken {
        result,
        unrestored,
        cancelled: was_cancelled,
    }
}

/// `path` made absolute against this process's working directory.
pub fn absolute(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if p.is_absolute() {
        Ok(p.into())
    } else {
        std::env::current_dir()
            .map(|d| d.join(p))
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A scripted compositor: each call is logged and answered from the
    /// script in order; `on` runs as a call arrives (to cancel mid-way).
    #[derive(Default)]
    pub(crate) struct Script {
        pub calls: Mutex<Vec<(String, Value)>>,
        pub answers: Mutex<std::collections::VecDeque<Result<Value, String>>>,
        #[allow(clippy::type_complexity)]
        pub on: Mutex<Option<Box<dyn FnMut(&str) + Send>>>,
    }

    impl Script {
        pub(crate) fn new(answers: Vec<Result<Value, String>>) -> Arc<Self> {
            Arc::new(Self {
                answers: Mutex::new(answers.into()),
                ..Self::default()
            })
        }
        pub(crate) fn command_names(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|c| c.0.clone())
                .collect()
        }
    }

    impl Comp for Script {
        fn call(
            &self,
            command_name: &'static str,
            args: Value,
            _limit: Duration,
        ) -> BoxFuture<'static, Result<Value, String>> {
            self.calls
                .lock()
                .unwrap()
                .push((command_name.into(), args.clone()));
            if let Some(on) = self.on.lock().unwrap().as_mut() {
                on(command_name);
            }
            // The frame command writes the file it was asked for.
            if command_name == "comp.capture.frame"
                && let Some(path) = args["path"].as_str()
            {
                image::RgbaImage::from_pixel(40, 30, image::Rgba([10, 20, 30, 255]))
                    .save(path)
                    .unwrap();
            }
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(format!("unscripted {command_name}")));
            Box::pin(async move { answer })
        }
    }

    fn own() -> Vec<Result<Value, String>> {
        let pid = std::process::id();
        vec![
            Ok(json!({"matched":true})),
            Ok(
                json!({"windows":[{"id":4,"generation":9,"app_id":crate::APP_ID,"pid":pid},{"id":5,"generation":1,"app_id":"other","pid":1}]}),
            ),
        ]
    }

    fn channel() -> (
        tokio::sync::watch::Sender<bool>,
        tokio::sync::watch::Receiver<bool>,
    ) {
        tokio::sync::watch::channel(false)
    }

    #[tokio::test]
    async fn a_region_capture_hides_selects_captures_and_restores() {
        let dir = tempfile::tempdir().unwrap();
        let mut answers = own();
        answers.extend([
            Ok(json!({"minimized":true})),
            Ok(
                json!({"status":"selected","output":"DP-1","output_generation":3,"region":{"x":1,"y":2,"width":30,"height":20}}),
            ),
            Ok(json!({"width":40,"height":30,"output":"DP-1","source":"kms"})),
            Ok(json!({"minimized":false})),
            Ok(json!({"focused":true})),
        ]);
        let script = Script::new(answers);
        let (_tx, rx) = channel();
        let request = Request {
            mode: Mode::Region,
            cursor: false,
            ..Request::default()
        };
        let captured = take(
            script.clone(),
            request,
            Some(crate::APP_ID),
            true,
            dir.path().into(),
            rx,
        )
        .await
        .unwrap();
        assert_eq!(captured.document.dimensions(), (40, 30));
        assert!(captured.path.starts_with(dir.path()));
        assert_eq!(
            script.command_names(),
            [
                "comp.window.wait",
                "comp.windows.list",
                "comp.window.minimize",
                "comp.region.select",
                "comp.capture.frame",
                "comp.window.restore",
                "comp.window.focus"
            ]
        );
        let calls = script.calls.lock().unwrap();
        assert_eq!(calls[2].1, json!({"id":4,"generation":9}));
        let frame = &calls[4].1;
        assert_eq!(frame["cursor"], false);
        assert_eq!(frame["output"], "DP-1");
        assert_eq!(frame["output_generation"], 3);
        assert_eq!(frame["region"]["width"], 30);
        // A person's capture: restored activated, then raised and focused.
        assert_eq!(calls[5].1, json!({"id":4,"generation":9,"activate":true}));
        assert_eq!(calls[6].1, json!({"id":4,"generation":9,"raise":true}));
    }

    #[tokio::test]
    async fn a_given_region_skips_the_selection_and_captures_it() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new(vec![Ok(
            json!({"width":30,"height":20,"output":"DP-2","source":"kms"}),
        )]);
        let (_tx, rx) = channel();
        let request: Request = serde_json::from_value(json!({
            "mode": "region",
            "region": {"x": 5, "y": 6, "width": 30, "height": 20, "output": "DP-2"},
        }))
        .unwrap();
        take(script.clone(), request, None, false, dir.path().into(), rx)
            .await
            .unwrap();
        assert_eq!(script.command_names(), ["comp.capture.frame"]);
        let calls = script.calls.lock().unwrap();
        let frame = &calls[0].1;
        assert_eq!(frame["output"], "DP-2");
        assert_eq!(frame["region"], json!({"x":5,"y":6,"width":30,"height":20}));
        assert!(frame.get("output_generation").is_none());
    }

    #[tokio::test]
    async fn a_given_region_takes_the_requests_output_when_it_names_none() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new(vec![Ok(json!({"width":30,"height":20}))]);
        let (_tx, rx) = channel();
        let request = Request {
            mode: Mode::Region,
            output: Some("HDMI-A-1".into()),
            region: Some(Region {
                x: 0,
                y: 0,
                width: 30,
                height: 20,
                output: None,
            }),
            ..Request::default()
        };
        take(script.clone(), request, None, false, dir.path().into(), rx)
            .await
            .unwrap();
        let calls = script.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1["output"], "HDMI-A-1");
    }

    #[test]
    fn region_arguments_are_checked_before_anything_runs() {
        let parse = |value: Value| {
            serde_json::from_value::<Request>(value)
                .map_err(|e| e.to_string())
                .and_then(|r| r.validate())
        };
        let rect = json!({"x":0,"y":0,"width":10,"height":10});
        assert!(parse(json!({"mode":"region","region":rect})).is_ok());
        // Region mode only.
        assert!(parse(json!({"region":rect})).is_err());
        assert!(parse(json!({"mode":"screen","region":rect})).is_err());
        // Positive extent, non-negative origin, i32 that does not overflow.
        for bad in [
            json!({"x":-1,"y":0,"width":10,"height":10}),
            json!({"x":0,"y":-1,"width":10,"height":10}),
            json!({"x":0,"y":0,"width":0,"height":10}),
            json!({"x":0,"y":0,"width":10,"height":-5}),
            json!({"x":i32::MAX,"y":0,"width":10,"height":10}),
            json!({"x":0,"y":0,"width":10}),
            json!({"x":0,"y":0,"width":10,"height":10,"colour":1}),
            json!({"x":0.5,"y":0,"width":10,"height":10}),
            json!({"x":0,"y":0,"width":10,"height":10,"output":""}),
        ] {
            assert!(
                parse(json!({"mode":"region","region":bad})).is_err(),
                "{bad}"
            );
        }
        // Two different outputs are refused; the same one twice is fine.
        assert!(
            parse(json!({"mode":"region","output":"DP-1",
                "region":{"x":0,"y":0,"width":1,"height":1,"output":"DP-2"}}))
            .is_err()
        );
        assert!(
            parse(json!({"mode":"region","output":"DP-1",
                "region":{"x":0,"y":0,"width":1,"height":1,"output":"DP-1"}}))
            .is_ok()
        );
    }

    #[tokio::test]
    async fn a_timed_out_selection_still_brings_a_persons_window_forward() {
        let mut answers = own();
        answers.extend([
            Ok(json!({"minimized":true})),
            Ok(json!({"version":1,"status":"timeout"})),
            Ok(json!({"minimized":false})),
            Ok(json!({"focused":true})),
        ]);
        let script = Script::new(answers);
        let (_tx, rx) = channel();
        let taken = take_job(
            script.clone(),
            Request {
                mode: Mode::Region,
                ..Request::default()
            },
            Some(crate::APP_ID),
            true,
            "/tmp".into(),
            rx,
        )
        .await;
        let error = taken.result.unwrap_err();
        assert_eq!(selection_ending(&error), Some("timeout"));
        assert!(!taken.cancelled && taken.unrestored.is_none());
        let calls = script.calls.lock().unwrap();
        let tail: Vec<_> = calls[calls.len() - 2..].to_vec();
        assert_eq!(
            tail,
            [
                (
                    "comp.window.restore".to_owned(),
                    json!({"id":4,"generation":9,"activate":true})
                ),
                (
                    "comp.window.focus".to_owned(),
                    json!({"id":4,"generation":9,"raise":true})
                ),
            ]
        );
    }

    #[test]
    fn selection_endings_are_read_from_the_error() {
        assert_eq!(selection_ending("selection timeout"), Some("timeout"));
        assert_eq!(
            selection_ending("selection cancelled; restoring the window failed: Bus down"),
            Some("cancelled")
        );
        assert_eq!(selection_ending("cancelled"), None);
        assert_eq!(selection_ending("Bus down"), None);
    }

    #[tokio::test]
    async fn a_lost_minimise_reply_still_restores_the_window() {
        let mut answers = own();
        answers.extend([
            Err("acknowledgement lost".into()),
            Ok(json!({"minimized":false})),
        ]);
        let script = Script::new(answers);
        let (_tx, rx) = channel();
        let error = take(
            script.clone(),
            Request::default(),
            Some(crate::APP_ID),
            false,
            "/tmp".into(),
            rx,
        )
        .await
        .unwrap_err();
        assert_eq!(error, "acknowledgement lost");
        assert_eq!(
            script.command_names().last().unwrap(),
            "comp.window.restore"
        );
    }

    #[tokio::test]
    async fn cancelling_during_the_delay_never_hides_the_window() {
        let script = Script::new(vec![]);
        let (tx, rx) = channel();
        let job = tokio::spawn(take(
            script.clone(),
            Request {
                delay: MAX_DELAY,
                ..Request::default()
            },
            Some(crate::APP_ID),
            false,
            "/tmp".into(),
            rx,
        ));
        tx.send(true).unwrap();
        assert_eq!(job.await.unwrap().unwrap_err(), "cancelled");
        assert!(script.command_names().is_empty());
    }

    #[tokio::test]
    async fn cancelling_while_hiding_never_starts_a_selection_and_restores() {
        let mut answers = own();
        answers.extend([
            Ok(json!({"minimized":true})),
            Ok(json!({"minimized":false})),
        ]);
        let script = Script::new(answers);
        let (tx, rx) = channel();
        *script.on.lock().unwrap() = Some(Box::new(move |command_name| {
            if command_name == "comp.window.minimize" {
                tx.send(true).unwrap();
            }
        }));
        let error = take(
            script.clone(),
            Request {
                mode: Mode::Region,
                ..Request::default()
            },
            Some(crate::APP_ID),
            false,
            "/tmp".into(),
            rx,
        )
        .await
        .unwrap_err();
        assert_eq!(error, "cancelled");
        assert_eq!(
            script.command_names(),
            [
                "comp.window.wait",
                "comp.windows.list",
                "comp.window.minimize",
                "comp.window.restore"
            ]
        );
    }

    #[tokio::test]
    async fn a_cancelled_selection_restores_and_reports_it() {
        let mut answers = own();
        answers.extend([
            Ok(json!({"minimized":true})),
            Ok(json!({"status":"cancelled","reason":"escape"})),
            Ok(json!({"minimized":false})),
        ]);
        let script = Script::new(answers);
        let (_tx, rx) = channel();
        let error = take(
            script.clone(),
            Request {
                mode: Mode::Region,
                ..Request::default()
            },
            Some(crate::APP_ID),
            false,
            "/tmp".into(),
            rx,
        )
        .await
        .unwrap_err();
        assert_eq!(error, "selection cancelled");
        assert_eq!(
            script.command_names().last().unwrap(),
            "comp.window.restore"
        );
    }

    #[tokio::test]
    async fn a_headless_window_capture_names_its_target_and_hides_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new(vec![Ok(json!({"width":40,"height":30}))]);
        let (_tx, rx) = channel();
        let target = Target {
            id: 7,
            generation: 3,
        };
        take(
            script.clone(),
            Request {
                mode: Mode::Window,
                window: Some(target),
                ..Request::default()
            },
            None,
            false,
            dir.path().into(),
            rx,
        )
        .await
        .unwrap();
        let calls = script.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1["window"], json!({"id":7,"generation":3}));
        assert!(calls[0].1.get("output").is_none());
    }

    #[tokio::test]
    async fn a_failing_restore_is_retried_until_shown_or_gone() {
        let listed = || Ok(json!({"windows":[{"id":4,"generation":9}]}));
        let target = Target {
            id: 4,
            generation: 9,
        };
        let script = Script::new(vec![
            Err("Bus down".into()),
            Err("Bus down".into()),
            Ok(json!({"minimized":true})),
            listed(),
            Ok(json!({"minimized":false})),
        ]);
        assert_eq!(
            restore(script.as_ref(), target, true).await,
            Ok(Restored::Shown)
        );
        assert_eq!(
            script.command_names(),
            [
                "comp.window.restore",
                "comp.windows.list",
                "comp.window.restore",
                "comp.windows.list",
                "comp.window.restore"
            ]
        );
        // The window closed meanwhile: nothing left to restore.
        let gone = Script::new(vec![
            Err("no such window".into()),
            Ok(json!({"windows":[{"id":4,"generation":10}]})),
        ]);
        assert_eq!(
            restore(gone.as_ref(), target, true).await,
            Ok(Restored::Gone)
        );
    }

    #[tokio::test]
    async fn a_window_never_restored_is_reported_and_the_capture_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut answers = own();
        answers.extend([
            Ok(json!({"minimized":true})),
            Ok(json!({"width":40,"height":30})),
        ]);
        // Every restore and listing fails: the Bus is gone.
        answers.extend((0..2 * RESTORE_TRIES).map(|_| Err("Bus down".to_owned())));
        let script = Script::new(answers);
        let (_tx, rx) = channel();
        let taken = take_job(
            script.clone(),
            Request::default(),
            Some(crate::APP_ID),
            false,
            dir.path().into(),
            rx,
        )
        .await;
        assert!(taken.result.is_ok(), "the shot itself succeeded");
        assert_eq!(
            taken.unrestored,
            Some(Target {
                id: 4,
                generation: 9
            })
        );
    }

    #[tokio::test]
    async fn a_frame_cancelled_after_capture_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new(vec![Ok(json!({"width":40,"height":30}))]);
        let (tx, rx) = channel();
        *script.on.lock().unwrap() = Some(Box::new(move |command_name| {
            if command_name == "comp.capture.frame" {
                tx.send(true).unwrap();
            }
        }));
        let error = take(
            script.clone(),
            Request::default(),
            None,
            false,
            dir.path().into(),
            rx,
        )
        .await
        .unwrap_err();
        assert_eq!(error, "cancelled");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn an_agent_capture_restores_without_taking_the_focus() {
        let dir = tempfile::tempdir().unwrap();
        let mut answers = own();
        answers.extend([
            Ok(json!({"minimized":true})),
            Ok(json!({"width":40,"height":30})),
            Ok(json!({"minimized":false})),
        ]);
        let script = Script::new(answers);
        let (_tx, rx) = channel();
        let taken = take_job(
            script.clone(),
            Request::default(),
            Some(crate::APP_ID),
            false,
            dir.path().into(),
            rx,
        )
        .await;
        assert!(taken.result.is_ok() && !taken.cancelled);
        let calls = script.calls.lock().unwrap();
        let restore = calls.last().unwrap();
        assert_eq!(restore.0, "comp.window.restore", "no focus call follows");
        assert_eq!(restore.1["activate"], false);
    }

    #[tokio::test]
    async fn a_cancelled_capture_says_so_even_when_the_restore_fails() {
        let mut answers = own();
        answers.extend([Ok(json!({"minimized":true}))]);
        answers.extend((0..2 * RESTORE_TRIES).map(|_| Err("Bus down".to_owned())));
        let script = Script::new(answers);
        let (tx, rx) = channel();
        *script.on.lock().unwrap() = Some(Box::new(move |command_name| {
            if command_name == "comp.window.minimize" {
                tx.send(true).unwrap();
            }
        }));
        let taken = take_job(
            script.clone(),
            Request::default(),
            Some(crate::APP_ID),
            true,
            "/tmp".into(),
            rx,
        )
        .await;
        assert!(taken.cancelled);
        assert!(
            taken
                .result
                .unwrap_err()
                .contains("restoring the window failed")
        );
        assert!(taken.unrestored.is_some());
    }

    #[test]
    fn invalid_requests_are_refused() {
        assert!(
            Request {
                delay: MAX_DELAY + 1,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Request {
                mode: Mode::Window,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(serde_json::from_value::<Request>(json!({"mode":"screen","extra":true})).is_err());
        assert!(
            serde_json::from_value::<Request>(
                json!({"mode":"window","window":{"id":1,"generation":2,"extra":true}})
            )
            .is_err()
        );
    }

    #[test]
    fn window_identity_is_never_guessed_and_own_is_left_out() {
        let pid = std::process::id();
        let rows = windows(
            &json!({"windows":[{"id":1},{"id":3,"generation":9,"title":"Editor","focused":true},{"id":4,"generation":1,"app_id":crate::APP_ID,"pid":pid}]}),
            Some(crate::APP_ID),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].target,
            Target {
                id: 3,
                generation: 9
            }
        );
        assert_eq!(rows[0].name(), "Editor");
        assert_eq!(
            outputs(&json!({"value":{"o_dp_1":{"name":"DP-1"},"o_hdmi":{"name":"HDMI-A-1"}}})),
            ["DP-1", "HDMI-A-1"]
        );
        assert_eq!(
            Target::parse("12:3"),
            Some(Target {
                id: 12,
                generation: 3
            })
        );
        assert_eq!(Target::parse("12"), None);
    }
}
