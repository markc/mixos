// SPDX-License-Identifier: MIT OR Apache-2.0
//! Named viewports share the root envelope, including in apps which have not
//! adopted root persistence yet. All compositor traffic uses the native Bus.
use super::{Frame, Store};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, sync::{Arc, Mutex}, time::{Duration, Instant}};

#[derive(Default)]
struct Windows {
    path: Option<PathBuf>,
    frames: BTreeMap<String, Frame>,
    workers: BTreeMap<String, std::sync::mpsc::SyncSender<()>>,
    captured: BTreeMap<String, Instant>,
}
type Shared = Arc<Mutex<Windows>>;

fn key(app: &str) -> egui::Id { egui::Id::new(("toolkit.saved.windows", app)) }

pub(super) fn install(ctx: &egui::Context, app: &str, path: Option<PathBuf>, frames: BTreeMap<String, Frame>) {
    ctx.data_mut(|d| d.insert_temp(key(app), Arc::new(Mutex::new(Windows { path, frames, ..Default::default() }))));
}

pub(super) fn collect(ctx: &egui::Context, app: &str) -> Option<BTreeMap<String, Frame>> {
    let state: Shared = ctx.data(|d| d.get_temp(key(app)))?;
    Some(state.lock().unwrap_or_else(|e| e.into_inner()).frames.clone())
}

fn shared(ctx: &egui::Context, app: &str) -> Shared {
    if let Some(state) = ctx.data(|d| d.get_temp::<Shared>(key(app))) { return state; }
    let directory = config::AppDirs::resolve(app).map(|d| d.state());
    let store = Store::<Value>::load(directory, 1);
    install(ctx, app, store.path, store.windows);
    ctx.data(|d| d.get_temp(key(app))).expect("installed secondary windows")
}

/// A distinct Wayland app ID plus PID identifies the editor independently of
/// its changing document title. Embedded viewports have no native geometry.
pub fn builder(ctx: &egui::Context, app: &str, name: &str, builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
    if ctx.embed_viewports() { return builder; }
    let state = shared(ctx, app);
    let app_id = format!("dev.mixos.{app}.{name}");
    let mut windows = state.lock().unwrap_or_else(|e| e.into_inner());
    let frame = windows.frames.entry(name.into()).or_default().clone();
    if !windows.workers.contains_key(name) {
        if let Some(worker) = worker(app, name, &app_id, state.clone()) {
            windows.workers.insert(name.into(), worker);
        }
    }
    frame.viewport(builder.with_app_id(app_id))
}

/// Called inside the child pass, including its closing pass. Portable saves
/// happen synchronously; bounded compositor reads never block the GUI thread.
pub fn capture(ctx: &egui::Context, app: &str, name: &str) {
    if ctx.embed_viewports() { return; }
    let state = shared(ctx, app);
    let mut windows = state.lock().unwrap_or_else(|e| e.into_inner());
    windows.frames.entry(name.into()).or_default().capture(ctx);
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    let now = Instant::now();
    let closing = ctx.input(|i| i.viewport().close_requested());
    if closing || windows.captured.get(name).is_none_or(|last| now.duration_since(*last) >= Duration::from_secs(1)) {
        windows.captured.insert(name.into(), now);
        persist(&windows, name);
        if let Some(worker) = windows.workers.get(name) { let _ = worker.try_send(()); }
    }
}

fn persist(windows: &Windows, name: &str) {
    let Some(path) = &windows.path else { return; };
    let mut store = Store::<Value>::load(path.parent().map(PathBuf::from), 1);
    // This writer owns only one named window. Keep the latest root/app/widget
    // values verbatim, including versions this writer cannot understand.
    if let Ok(raw) = std::fs::read_to_string(path)
        && let Ok(value) = strict::from_str::<Value>(&raw) {
        for section in ["window", "widgets", "app"] {
            if let Some(raw) = value.get(section) { store.preserved.insert(section.into(), raw.clone()); }
        }
    }
    if let Some(frame) = windows.frames.get(name) { store.windows.insert(name.into(), frame.clone()); }
    if let Err(error) = store.flush() { eprintln!("toolkit: saving secondary window: {error}"); }
}

fn worker(app: &str, name: &str, app_id: &str, state: Shared) -> Option<std::sync::mpsc::SyncSender<()>> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let (app, name, app_id) = (app.to_owned(), name.to_owned(), app_id.to_owned());
    let state = Arc::downgrade(&state);
    std::thread::Builder::new().name("saved-viewport".into()).spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return; };
        runtime.block_on(async {
            let service = format!("{app}-saved-{}", std::process::id());
            let Ok(Ok(client)) = tokio::time::timeout(Duration::from_millis(700), ::bus::client_helpers::connect_default(&service)).await else { return; };
            let comp = std::env::var("MIXOS_COMP_SERVICE").unwrap_or_else(|_| "comp".into());
            let call = |command: &'static str, args: Value| {
                let client = &client;
                let comp = &comp;
                async move {
                    let reply = tokio::time::timeout(Duration::from_secs(11), client.call_typed(comp, command, args)).await.ok()?.ok()?;
                    match reply {
                        ::bus::PortReply::Ok { value, .. } => Some(value),
                        ::bus::PortReply::AppError { message, .. } => serde_json::from_str(&message).ok(),
                    }
                }
            };
            // Probe once. A desktop without compd must not generate catalogue
            // requests on every editor repaint or portable save.
            if !call("comp.windows.list", serde_json::json!({})).await.is_some_and(|v| v["windows"].is_array()) { return; }
            let Some(state) = state.upgrade() else { return; };
            let mut frame = state.lock().unwrap_or_else(|e| e.into_inner()).frames.get(&name).cloned().unwrap_or_default();
            frame.restore(&app_id, call).await;
            while rx.recv().is_ok() {
                frame = state.lock().unwrap_or_else(|e| e.into_inner()).frames.get(&name).cloned().unwrap_or_default();
                let _ = tokio::time::timeout(Duration::from_millis(700), frame.capture_exit(&app_id, call)).await;
                let mut windows = state.lock().unwrap_or_else(|e| e.into_inner());
                // Portable capture may have advanced while the Bus replied.
                let latest = windows.frames.entry(name.clone()).or_default();
                latest.placement = frame.placement;
                persist(&windows, &name);
            }
        });
        runtime.shutdown_timeout(Duration::from_millis(100));
    }).ok().map(|_| tx)
}

/// Drop senders at root shutdown so compositor readers cannot outlive the app.
pub fn close(ctx: &egui::Context, app: &str) {
    if let Some(state) = ctx.data(|d| d.get_temp::<Shared>(key(app))) {
        state.lock().unwrap_or_else(|e| e.into_inner()).workers.clear();
    }
}
