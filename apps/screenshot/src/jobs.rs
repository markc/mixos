// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The effects that leave the process, shared by the window and headless
//! mode: captures over compd, decoding, rendering and publishing PNGs, the
//! ledger, and the registry-backed Bus commands. Image work runs on blocking
//! threads, never the UI thread or the Bus thread.
use crate::app::{App, Effect, SaveTo};
use crate::capture::{self, Comp, Request, Window};
use action::{Error, Origin};
use annotation::library::{self, Ledger, Saved};
use annotation::{Document, Face};
use image::RgbaImage;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use toolkit::Strings;

/// A finished job, for [`apply`].
#[derive(Debug)]
pub enum Done {
    /// A capture, the window it could not restore and whether it was
    /// cancelled.
    Captured(u64, capture::Taken),
    Opened(u64, Result<(PathBuf, Document), String>),
    Saved(u64, Result<Saved, String>),
    Shown(u64, Result<Value, String>),
    /// The window brought forward outside a job.
    Raised(Result<Value, String>),
    Refreshed(Result<(Vec<Window>, Vec<String>), String>),
}

/// Hand a finished job to the app.
pub fn apply(app: &mut App, done: Done) {
    match done {
        Done::Captured(ticket, taken) => {
            app.captured_job(
                ticket,
                taken.result,
                taken.unrestored.is_some(),
                taken.cancelled,
            );
        }
        Done::Opened(ticket, result) => app.opened(ticket, result),
        Done::Saved(ticket, result) => app.saved(ticket, result),
        Done::Shown(ticket, result) => app.shown(ticket, result),
        Done::Raised(result) => app.raised(result),
        Done::Refreshed(result) => app.refreshed(result),
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

/// One capture into the private captures directory.
pub async fn capture(
    comp: Arc<dyn Comp>,
    ticket: u64,
    request: Request,
    hide: bool,
    focus: bool,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Done {
    let directory = match blocking(library::captures_dir).await {
        Ok(directory) => directory,
        Err(error) => {
            return Done::Captured(
                ticket,
                capture::Taken {
                    result: Err(error),
                    unrestored: None,
                    cancelled: false,
                },
            );
        }
    };
    let taken = capture::take_job(
        comp,
        request,
        hide.then_some(crate::APP_ID),
        focus,
        directory,
        cancel,
    )
    .await;
    Done::Captured(ticket, taken)
}

/// Delete a raw capture the document no longer needs (only ever a file of
/// the private captures directory).
pub fn forget(path: &std::path::Path) {
    if let Ok(dir) = library::captures_dir() {
        library::forget_capture(&dir, path);
    }
}

/// Prune the private captures directory at start (count and age bounds).
pub fn prune() {
    if let Ok(dir) = library::captures_dir() {
        library::prune_captures(&dir);
    }
}

/// Decode the image at `path`.
pub async fn open(ticket: u64, path: PathBuf) -> Done {
    let result = blocking(move || Document::open(&path).map(|d| (path, d))).await;
    Done::Opened(ticket, result)
}

/// Render `document` and publish it, then record it in the ledger.
pub async fn save(ticket: u64, document: Document, to: SaveTo, face: Arc<Face>) -> Done {
    let result = blocking(move || save_now(&document, &to, &face)).await;
    Done::Saved(ticket, result)
}

/// [`save`], here and now.
pub fn save_now(document: &Document, to: &SaveTo, face: &Face) -> Result<Saved, String> {
    let image = document.render(face)?;
    let (now, stamp) = library::now_local();
    let path = match to {
        SaveTo::Folder => library::save_into(
            &library::screenshots_dir()?,
            &library::shot_stem(&now),
            &image,
        )?,
        SaveTo::Path(path) => {
            if !path.is_absolute() {
                return Err("the path must be absolute".into());
            }
            // The proposed folder may not exist yet on first use.
            if let (Some(parent), Ok(folder)) = (path.parent(), library::screenshots_dir())
                && parent == folder
            {
                std::fs::create_dir_all(&folder)
                    .map_err(|e| format!("{}: {e}", folder.display()))?;
            }
            annotation::paint::publish_png(
                &image,
                std::iter::once(path.clone()),
                annotation::paint::PUBLIC,
            )?
        }
    };
    let saved = Saved {
        path,
        width: image.width(),
        height: image.height(),
        annotated: document.annotated(),
        saved_at: stamp,
    };
    // The file is published; a ledger that cannot be written loses only
    // the `screenshot.recent` entry.
    if let Err(error) = library::state_dir().and_then(|s| Ledger::new(&s).append(&saved)) {
        eprintln!("screenshot: the ledger was not written: {error}");
    }
    Ok(saved)
}

/// Render `document` (for the clipboard or the preview).
pub async fn render(document: Document, face: Arc<Face>) -> Result<RgbaImage, String> {
    blocking(move || document.render(&face)).await
}

/// compd's windows (this app's left out) and outputs.
pub async fn refresh(comp: Arc<dyn Comp>) -> Done {
    let limit = Duration::from_secs(5);
    let result = async {
        let windows = comp.call("comp.windows.list", json!({}), limit).await?;
        let outputs = comp
            .call("comp.props.get", json!({"path": "outputs"}), limit)
            .await?;
        Ok((
            capture::windows(&windows, Some(crate::APP_ID)),
            capture::outputs(&outputs),
        ))
    }
    .await;
    Done::Refreshed(result)
}

/// The newest `limit` saves, as `screenshot.recent` answers.
pub fn recent(limit: usize) -> Value {
    let shots = library::state_dir()
        .map(|state| Ledger::new(&state).recent(limit))
        .unwrap_or_default();
    json!({
        "folder": library::screenshots_dir().ok(),
        "shots": shots,
    })
}

/// The whole Bus surface: the app's commands, and with a window the toolkit's
/// drive commands.
pub fn describe(window: bool) -> Value {
    let mut surface = crate::commands::describe();
    if window && let Some(command_names) = surface["commands"].as_array_mut() {
        command_names.extend(toolkit::drive::describe(crate::SERVICE));
    }
    surface
}

/// Resolve the effects that only need the registry, the description or the
/// ledger, in order, and return the rest. A command `screenshot.execute`
/// runs here, before any later delivery, so it acts on the state it arrived
/// to.
pub fn settle(
    app: &mut App,
    commands: &crate::app::Commands,
    strings: &Strings,
    window: bool,
) -> Vec<Effect> {
    let mut out = Vec::new();
    loop {
        let effects = app.take_effects();
        if effects.is_empty() {
            break;
        }
        for effect in effects {
            match effect {
                Effect::Commands { id } => {
                    let body = json!({ "commands": commands.describe(app, strings) });
                    out.push(Effect::Reply { id, rc: 0, body });
                }
                Effect::Execute { id, request, alias } => {
                    let command = request.id;
                    let (rc, body) = match app.execute(
                        &command,
                        request.params.unwrap_or(Value::Null),
                        Origin::Bus,
                    ) {
                        Ok(result) if alias => (0, result),
                        Ok(result) => (
                            0,
                            json!({"executed": command, "result": result, "status": app.status,
                                "dialog": app.ui.dialog, "job": app.job().map(crate::app::JobKind::name)}),
                        ),
                        Err(Error::Denied { .. }) => (
                            10,
                            json!({"error_code":"DENIED","message":crate::label("command-denied")}),
                        ),
                        Err(Error::Unknown(_)) => (
                            10,
                            json!({"error_code":"UNKNOWN_ACTION","message":crate::label("unknown-action")}),
                        ),
                        Err(Error::Disabled { .. })
                            if app.headless && crate::actions::needs_window(&command) =>
                        {
                            (
                                10,
                                json!({"error_code":"HEADLESS","message":crate::label("no-window")}),
                            )
                        }
                        Err(Error::Disabled {
                            reason: "nothing to undo" | "nothing to redo",
                            ..
                        }) if alias => (
                            0,
                            app.document
                                .as_ref()
                                .expect("history enablement requires a document")
                                .info(),
                        ),
                        Err(Error::Disabled { reason, .. }) if alias => (
                            10,
                            json!({"error_code": match reason {
                                "no image" => "NO_IMAGE", "dialog open" => "DIALOG", "busy" => "BUSY", _ => "DISABLED"
                            }, "message": reason}),
                        ),
                        Err(Error::Disabled { .. }) => (
                            10,
                            json!({"error_code":"DISABLED","message":crate::label("command-disabled")}),
                        ),
                        Err(Error::BadParams { msg, .. }) => {
                            (10, json!({"error_code":"ARGUMENT", "message":msg}))
                        }
                        Err(Error::Failed(msg)) => (
                            10,
                            json!({"error_code":if alias { "ARGUMENT" } else { "FAILED" }, "message":msg}),
                        ),
                    };
                    out.push(Effect::Reply { id, rc, body });
                }
                Effect::Describe { id, help } => {
                    let surface = describe(window);
                    let body = if help {
                        surface["commands"].clone()
                    } else {
                        surface
                    };
                    out.push(Effect::Reply { id, rc: 0, body });
                }
                Effect::Recent { id, limit } => out.push(Effect::Reply {
                    id,
                    rc: 0,
                    body: recent(limit),
                }),
                other => out.push(other),
            }
        }
    }
    for id in std::mem::take(&mut app.ran) {
        commands.note(id, Origin::Bus);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_and_commands_are_answered_in_place() {
        let strings = crate::strings();
        let mut a = App::new(crate::label, crate::ink::inks(), false);
        a.document = Some(Document::new(RgbaImage::new(16, 16)).unwrap());
        let commands = a.commands.clone();
        let params = json!({"kind":"arrow", "points":[{"x":1,"y":1},{"x":8,"y":8}], "colour":[255,0,0,255], "width":2});
        a.command(1, "screenshot.actions", "{}");
        a.command(2, "screenshot.execute", r#"{"id":"nope"}"#);
        a.command(
            6,
            "screenshot.execute",
            &json!({"id":"edit.annotate", "params":params}).to_string(),
        );
        a.command(3, "screenshot.execute", r#"{"id":"help.about"}"#);
        a.command(4, "screenshot.execute", r#"{"id":"help.about"}"#);
        a.command(5, "HELP", "{}");
        let effects = settle(&mut a, &commands.borrow(), &strings, true);
        let reply = |id: u64| {
            effects.iter().find_map(|x| match x {
                Effect::Reply { id: i, rc, body } if *i == id => Some((*rc, body.clone())),
                _ => None,
            })
        };
        let (rc, body) = reply(1).unwrap();
        assert_eq!(rc, 0);
        assert!(
            body["commands"]
                .as_array()
                .is_some_and(|c| c.iter().any(|c| c["id"] == "capture.take"))
        );
        assert_eq!(reply(2).unwrap().1["error_code"], "UNKNOWN_ACTION");
        assert_eq!(reply(6).unwrap().0, 0);
        assert_eq!(reply(6).unwrap().1["result"]["id"], 1);
        assert_eq!(reply(3).unwrap().1["dialog"], "about");
        assert_eq!(reply(4).unwrap().1["error_code"], "DISABLED");
        let help = reply(5).unwrap().1;
        let names: Vec<_> = help
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v["name"].as_str())
            .collect();
        assert!(names.contains(&"screenshot.ui.click"));
        assert!(names.contains(&"screenshot.recent"));
        let (runs, _) = commands.borrow().take_executed();
        assert_eq!(runs.len(), 2, "only the accepted executes ran");
        assert_eq!(runs[0].id, "edit.annotate");
        assert_eq!(runs[0].params, params);
        assert_eq!(runs[0].result, reply(6).unwrap().1["result"]);
        assert_eq!(runs[1].params, Value::Null);
        assert_eq!(runs[1].result, Value::Null);
        assert_eq!(reply(3).unwrap().1["result"], Value::Null);
    }

    #[test]
    fn a_folder_save_is_named_by_time_recorded_and_listed() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: this test's process reads these only here.
        unsafe {
            std::env::set_var("MIXOS_SCREENSHOTS_DIR", dir.path().join("shots"));
            std::env::set_var("MIXOS_APP_HOME", dir.path().join("state"));
        }
        let mut document = Document::new(RgbaImage::new(30, 20)).unwrap();
        let first = save_now(&document, &SaveTo::Folder, &crate::face()).unwrap();
        assert!(first.path.starts_with(dir.path().join("shots")));
        let name = first
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(library::is_shot_name(&name), "{name}");
        assert!(!first.annotated);
        document
            .add(annotation::Shape::new(
                annotation::Kind::Line,
                vec![
                    annotation::Point { x: 1.0, y: 1.0 },
                    annotation::Point { x: 9.0, y: 9.0 },
                ],
                [0, 0, 0, 255],
                2.0,
            ))
            .unwrap();
        let second = save_now(&document, &SaveTo::Folder, &crate::face()).unwrap();
        assert_ne!(first.path, second.path, "never the same name");
        assert!(second.annotated);
        let listed = recent(10);
        assert_eq!(listed["shots"][0]["path"], json!(second.path));
        assert_eq!(listed["shots"][1]["path"], json!(first.path));
        assert_eq!(listed["shots"][0]["width"], 30);
        assert!(
            save_now(
                &document,
                &SaveTo::Path("relative.png".into()),
                &crate::face()
            )
            .is_err()
        );
    }
}
