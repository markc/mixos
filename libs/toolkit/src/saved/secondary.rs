// SPDX-License-Identifier: MIT OR Apache-2.0
//! Named viewports share the root envelope, including in apps which have not
//! adopted root persistence yet. All compositor traffic uses the native Bus.
use super::{Frame, Store};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Windows {
    path: Option<PathBuf>,
    frames: BTreeMap<String, Frame>,
    workers: BTreeMap<String, CaptureSender>,
    captured: BTreeMap<String, (Instant, Frame, Option<egui::Rect>)>,
    viewports: BTreeMap<String, egui::ViewportId>,
}
type Shared = Arc<Mutex<Windows>>;

fn key(app: &str) -> egui::Id {
    egui::Id::unique(("toolkit.saved.windows", app))
}

pub(super) fn install(
    ctx: &egui::Context,
    app: &str,
    path: Option<PathBuf>,
    frames: BTreeMap<String, Frame>,
) {
    ctx.data_mut(|d| {
        d.insert_temp(
            key(app),
            Arc::new(Mutex::new(Windows {
                path,
                frames,
                ..Default::default()
            })),
        )
    });
}

pub(super) fn collect(ctx: &egui::Context, app: &str) -> Option<BTreeMap<String, Frame>> {
    let state: Shared = ctx.data(|d| d.get_temp(key(app)))?;
    Some(
        state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .frames
            .clone(),
    )
}

fn shared(ctx: &egui::Context, app: &str) -> Shared {
    if let Some(state) = ctx.data(|d| d.get_temp::<Shared>(key(app))) {
        return state;
    }
    #[cfg(not(test))]
    let directory = config::AppDirs::resolve(app).map(|d| d.state());
    #[cfg(test)]
    let directory = None;
    // The child-only writer owns no app value. Version zero is an empty
    // placeholder which a later app-owned version (starting at one) can drop.
    let store = Store::<Value>::load(directory, 0);
    install(ctx, app, store.path, store.windows);
    ctx.data(|d| d.get_temp(key(app)))
        .expect("installed secondary windows")
}

/// A distinct Wayland app ID plus PID identifies the editor independently of
/// its changing document title. Embedded viewports have no native geometry.
pub fn builder(
    ctx: &egui::Context,
    app: &str,
    name: &str,
    builder: egui::ViewportBuilder,
) -> egui::ViewportBuilder {
    if ctx.embed_viewports() {
        return builder;
    }
    let state = shared(ctx, app);
    let app_id = format!("dev.mixos.{app}.{name}");
    let mut windows = state.lock().unwrap_or_else(|e| e.into_inner());
    let frame = windows.frames.entry(name.into()).or_default().clone();
    #[cfg(not(test))]
    if let std::collections::btree_map::Entry::Vacant(entry) = windows.workers.entry(name.into())
        && let Some(worker) = worker(app, name, &app_id, state.clone(), ctx.clone())
    {
        entry.insert(worker);
    }
    frame.viewport(builder.with_app_id(app_id))
}

/// Called inside the child pass, including its closing pass. Portable saves
/// happen synchronously. Normal compositor reads run off the GUI thread;
/// native close gives the final read a bounded 750 ms acknowledgement window.
pub fn capture(ctx: &egui::Context, app: &str, name: &str) {
    if ctx.embed_viewports() {
        return;
    }
    let state = shared(ctx, app);
    let mut windows = state.lock().unwrap_or_else(|e| e.into_inner());
    windows.viewports.insert(name.into(), ctx.viewport_id());
    windows.frames.entry(name.into()).or_default().capture(ctx);
    let now = Instant::now();
    let closing = ctx.input(|i| i.viewport().close_requested());
    let pointer_down = ctx.input(|i| i.pointer.any_down());
    let rect = ctx.input(|i| i.viewport().outer_rect);
    let frame = windows.frames[name].clone();
    let changed = windows
        .captured
        .get(name)
        .is_none_or(|(_, saved, old_rect)| *saved != frame || *old_rect != rect);
    let due = windows
        .captured
        .get(name)
        .is_none_or(|(last, _, _)| now.duration_since(*last) >= Duration::from_secs(1));
    if closing || (changed && due && !pointer_down) {
        ctx.request_repaint_of(egui::ViewportId::ROOT);
        windows.captured.insert(name.into(), (now, frame, rect));
        persist(&windows, name);
        if !closing && let Some(worker) = windows.workers.get(name) {
            let _ = worker.try_send(None);
        }
    } else if changed {
        ctx.request_repaint_after(Duration::from_secs(1));
    }
    drop(windows);
    // The app may cancel the native close for unsaved work. Its accepted-close
    // hook calls `finish`; a close request alone must not retire the worker.
}

fn persist(windows: &Windows, name: &str) {
    let Some(path) = &windows.path else {
        return;
    };
    let mut store = Store::<Value>::load(path.parent().map(PathBuf::from), 0);
    // This writer owns only one named window. Keep the latest root/app/widget
    // values verbatim, including versions this writer cannot understand.
    if let Ok(raw) = std::fs::read_to_string(path)
        && let Ok(value) = strict::from_str::<Value>(&raw)
    {
        for section in ["window", "widgets", "app"] {
            if let Some(raw) = value.get(section) {
                store.preserved.insert(section.into(), raw.clone());
            }
        }
    }
    if let Some(frame) = windows.frames.get(name) {
        store.windows.insert(name.into(), frame.clone());
    }
    if let Err(error) = store.flush() {
        eprintln!("toolkit: saving secondary window: {error}");
    }
}

type CaptureSender = tokio::sync::mpsc::Sender<Option<std::sync::mpsc::Sender<()>>>;

fn capture_service(app: &str, name: &str, pid: u32) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hash);
    format!("{app}-saved-{pid}-{:016x}", hash.finish())
}

#[cfg(not(test))]
fn worker(
    app: &str,
    name: &str,
    app_id: &str,
    state: Shared,
    ctx: egui::Context,
) -> Option<CaptureSender> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Option<std::sync::mpsc::Sender<()>>>(2);
    let (app, name, app_id) = (app.to_owned(), name.to_owned(), app_id.to_owned());
    let state = Arc::downgrade(&state);
    std::thread::Builder::new().name("saved-viewport".into()).spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return; };
        runtime.block_on(async {
            let service = capture_service(&app, &name, std::process::id());
            let url = ::bus::client_helpers::resolve_noded_url();
            let options = ::bus::native_client::SupervisedClient::connect_options(&service, &url).fatal_on_registration_rejection(true);
            let Ok(Ok(client)) = tokio::time::timeout(Duration::from_millis(700), options.connect()).await else { return; };
            let comp = std::env::var("MIXOS_COMP_SERVICE").unwrap_or_else(|_| "comp".into());
            let topic = format!("{comp}.props.changed");
            let _ = tokio::time::timeout(Duration::from_millis(700), client.subscribe_topic(&topic)).await;
            let incoming = tokio::sync::Mutex::new(client.incoming());
            let call = |command: &'static str, args: Value| {
                let client = &client;
                let comp = &comp;
                async move {
                    // The typed error lane is a display message and loses
                    // refusal fields. Preserve the raw body for feature probes.
                    let headers = BTreeMap::new();
                    let body = args.to_string();
                    let (rc, body, _) = tokio::time::timeout(Duration::from_secs(11), client.call_with_headers_raw(comp, command, &headers, &body)).await.ok()?.ok()?;
                    reply_value(rc, &body)
                }
            };
            // Probe once. A desktop without compd must not generate catalogue
            // requests on every editor repaint or portable save.
            if !tokio::time::timeout(Duration::from_millis(700), call("comp.windows.list", serde_json::json!({}))).await.ok().flatten().is_some_and(|v| v["windows"].is_array()) { return; }
            let Some(state) = state.upgrade() else { return; };
            let mut frame = state.lock().unwrap_or_else(|e| e.into_inner()).frames.get(&name).cloned().unwrap_or_default();
            frame.restore_with_events(&app_id, call, || async {
                    let mut incoming = incoming.lock().await;
                    let Some(incoming) = incoming.as_mut() else { return false; };
                    while let Some(message) = incoming.recv().await {
                        if message.headers.get("topic") == Some(&topic) { return true; }
                    }
                    false
            }).await;
            let Some(mut incoming) = incoming.into_inner() else { return; };
            let mut own_id = None;
            loop {
                let done = tokio::select! {
                    job = rx.recv() => match job { Some(done) => done, None => break },
                    event = incoming.recv() => {
                        let Some(event) = event else { break; };
                        let mut capture = capture_event(event.headers.get("path").map(String::as_str), own_id);
                        // A native move may change several leaves in one pass.
                        // Drain the burst, including irrelevant windows, before
                        // one catalogue read. Never retain an unread topic queue.
                        while let Ok(event) = incoming.try_recv() {
                            capture |= capture_event(event.headers.get("path").map(String::as_str), own_id);
                        }
                        if !capture { continue; }
                        None
                    }
                };
                frame = state.lock().unwrap_or_else(|e| e.into_inner()).frames.get(&name).cloned().unwrap_or_default();
                let prior_maximised = frame.maximised;
                if let Ok(Some(window)) = tokio::time::timeout(Duration::from_millis(700), frame.capture_identified(&app_id, call)).await {
                    own_id = window["id"].as_u64();
                }
                let mut windows = state.lock().unwrap_or_else(|e| e.into_inner());
                // Portable capture may have advanced while the Bus replied.
                let latest = windows.frames.entry(name.clone()).or_default();
                latest.placement = frame.placement;
                if done.is_some() || latest.maximised == prior_maximised { latest.maximised = frame.maximised; }
                // Normal saves run through the child pass's debounce. A final
                // read must reach disk before acknowledging native destruction.
                if done.is_some() { persist(&windows, &name); }
                if let Some(viewport) = windows.viewports.get(&name) { ctx.request_repaint_of(*viewport); }
                ctx.request_repaint_of(egui::ViewportId::ROOT);
                drop(windows);
                if let Some(done) = done { let _ = done.send(()); }
            }
        });
        runtime.shutdown_timeout(Duration::from_millis(100));
    }).ok().map(|_| tx)
}

fn capture_event(path: Option<&str>, own_id: Option<u64>) -> bool {
    let Some(path) = path else {
        return false;
    };
    if path == "outputs" || path.starts_with("outputs.") || path == "windows" {
        return true;
    }
    match own_id {
        Some(id) => {
            let prefix = format!("windows.s{id}");
            path == prefix || path.starts_with(&(prefix + "."))
        }
        None => path.starts_with("windows."),
    }
}

fn reply_value(rc: u8, body: &str) -> Option<Value> {
    let mut value: Value = serde_json::from_str(body).ok()?;
    if rc >= 10 && value.get("error").is_none() {
        value = serde_json::json!({"error":"refused"});
    }
    Some(value)
}

/// Drop senders at root shutdown so compositor readers cannot outlive the app.
pub fn close(ctx: &egui::Context, app: &str) {
    if let Some(windows) = collect(ctx, app) {
        for name in windows.keys() {
            finish(ctx, app, name);
        }
    }
}

/// Finish before the native viewport is destroyed, also for Escape or a
/// save-and-close dialog. Never create a store for an unopened editor.
pub fn finish(ctx: &egui::Context, app: &str, name: &str) {
    let Some(state) = ctx.data(|d| d.get_temp::<Shared>(key(app))) else {
        return;
    };
    let mut windows = state.lock().unwrap_or_else(|e| e.into_inner());
    if windows.captured.remove(name).is_none() && !windows.workers.contains_key(name) {
        return;
    }
    persist(&windows, name);
    let worker = windows.workers.remove(name);
    let (done, finished) = std::sync::mpsc::channel();
    let waiting = worker.is_some_and(|worker| worker.try_send(Some(done)).is_ok());
    drop(windows);
    if waiting {
        let _ = finished.recv_timeout(Duration::from_millis(750));
    }
    ctx.request_repaint_of(egui::ViewportId::ROOT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_workers_have_distinct_stable_window_identities() {
        assert_eq!(capture_service("email", "compose-1", 7), capture_service("email", "compose-1", 7));
        assert_ne!(capture_service("email", "compose-1", 7), capture_service("email", "compose-2", 7));
        assert_ne!(capture_service("email", "compose-1", 7), capture_service("email", "compose-1", 8));
    }

    #[test]
    fn close_request_keeps_the_worker_until_the_editor_accepts_closing() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        install(&ctx, "example", None, BTreeMap::new());
        let shared = shared(&ctx, "example");
        let (worker, mut calls) = tokio::sync::mpsc::channel(2);
        shared
            .lock()
            .unwrap()
            .workers
            .insert("macro-editor".into(), worker);
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        let mut output = ctx.run_ui(input, |ui| capture(ui.ctx(), "example", "macro-editor"));
        output.textures_delta.clear();
        assert!(shared.lock().unwrap().workers.contains_key("macro-editor"));
        assert!(
            calls.try_recv().is_err(),
            "the accepted-close hook owns final compositor capture"
        );
    }

    #[test]
    fn secondary_events_follow_only_its_window_and_output_geometry() {
        for path in [
            "windows.s7.window_x",
            "windows.s7.workspace",
            "windows.s7.maximized",
            "outputs.o_main.x",
            "windows.s7",
        ] {
            assert!(capture_event(Some(path), Some(7)), "{path}");
        }
        for path in [
            "windows.s70.window_x",
            "windows.s8.workspace",
            "decoration.style",
        ] {
            assert!(!capture_event(Some(path), Some(7)), "{path}");
        }
        assert!(!capture_event(None, Some(7)));
        assert!(capture_event(Some("windows.s8.app_id"), None));
    }

    #[test]
    fn secondary_transport_retains_feature_refusal_fields() {
        let body = r#"{"error":"invalid_args","field":"output","allowed":["id","generation"]}"#;
        assert_eq!(reply_value(10, body).unwrap()["field"], "output");
        assert_eq!(reply_value(10, "{}").unwrap()["error"], "refused");
        assert!(reply_value(20, "unparseable transport error").is_none());
    }

    #[test]
    fn closing_flushes_the_latest_size_before_the_debounce_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let frame = Frame {
            size: Some([840.0, 620.0]),
            ..Default::default()
        };
        install(
            &ctx,
            "example",
            Some(dir.path().join("window.conf.mix")),
            [("macro-editor".into(), frame.clone())].into(),
        );
        let shared = shared(&ctx, "example");
        shared.lock().unwrap().captured.insert(
            "macro-editor".into(),
            (Instant::now(), Frame::default(), None),
        );
        finish(&ctx, "example", "macro-editor");
        let back = Store::<Value>::load(Some(dir.path().into()), 1);
        assert_eq!(back.windows["macro-editor"], frame);
        assert!(shared.lock().unwrap().captured.is_empty());
    }

    #[test]
    fn macro_editor_size_and_maximised_state_round_trip_as_a_named_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("window.conf.mix");
        let mut root = Store::<Value>::load(Some(dir.path().into()), 1);
        root.app = serde_json::json!({"keep":"root"});
        root.window.size = Some([1200.0, 800.0]);
        root.flush().unwrap();
        let frame = Frame {
            size: Some([700.0, 480.0]),
            maximised: true,
            ..Default::default()
        };
        let windows = Windows {
            path: Some(path),
            frames: [("macro-editor".into(), frame.clone())].into(),
            ..Default::default()
        };
        persist(&windows, "macro-editor");
        let back = Store::<Value>::load(Some(dir.path().into()), 1);
        assert_eq!(back.windows["macro-editor"], frame);
        assert_eq!(back.window.size, Some([1200.0, 800.0]));
        assert_eq!(back.app["keep"], "root");
    }

    #[test]
    fn embedded_editor_creates_no_store_or_compositor_worker() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(true);
        let original = egui::ViewportBuilder::default().with_inner_size([760.0, 560.0]);
        let built = builder(&ctx, "example", "macro-editor", original);
        capture(&ctx, "example", "macro-editor");
        assert_eq!(built.inner_size, Some(egui::vec2(760.0, 560.0)));
        assert!(collect(&ctx, "example").is_none());
    }

    #[test]
    fn secondary_comp_restore_uses_its_own_app_id_and_fenced_bodies() {
        let frame = Frame {
            placement: Some(super::super::frame::Placement {
                output: "o_main".into(),
                workspace: 3,
                x: 10.0,
                y: 20.0,
                width: 700,
                height: 480,
            }),
            ..Default::default()
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut calls = Vec::new();
        runtime.block_on(frame.restore("dev.mixos.example.macro-editor", |command, args| {
            calls.push((command, args));
            std::future::ready(Some(if command == "comp.windows.list" {
                serde_json::json!({"windows":[
                    {"app_id":"dev.mixos.example","pid":std::process::id(),"id":1,"generation":1},
                    {"app_id":"dev.mixos.example.macro-editor","pid":std::process::id(),"id":2,"generation":4}
                ]})
            } else { serde_json::json!({}) }))
        }));
        assert_eq!(
            calls[3].1,
            serde_json::json!({"id":2,"generation":4,"index":3,"follow":false})
        );
        assert_eq!(
            calls[4].1,
            serde_json::json!({"id":2,"generation":4,"output":"o_main","x":10.0,"y":20.0,"width":700,"height":480})
        );
        calls.clear();
        runtime.block_on(
            frame.restore("dev.mixos.example.macro-editor", |command, args| {
                calls.push((command, args));
                std::future::ready(None)
            }),
        );
        assert_eq!(
            calls.len(),
            1,
            "absent compd allows only the catalogue probe"
        );
    }
}
