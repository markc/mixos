// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The window's own Bus commands over a real broker (libs/test-broker): the
//! lane registers `term`, serves its terminal commands itself and hands the
//! window `app.describe`, `term.actions`, `term.execute` and the drive commands,
//! which the window answers between its frames. Every command run is
//! published on `term.action.executed`.

use ::bus::native_client::{BoundedIncomingEvent, NodedClient, SupervisedClient};
use egui_kittest::Harness;
use egui_kittest::wgpu::{WgpuTestRenderer, create_render_state, default_wgpu_setup};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use term::app::App;
use term::bus::WindowBus;
use term::clipboard::Clipboard;
use term::shell::Shell;
use term_core::config::{Config, Cursor, Settings};
use term_core::font::FontSize;
use term_core::raster::Raster;
use term_core::tabs::{Cleanup, TabSet};

const EXECUTED: &str = "term.action.executed";

struct Holder {
    start: Option<(App, WindowBus)>,
    shell: Option<Shell>,
}

fn mix_available() -> bool {
    let available = &::paths::root(::paths::Root::Prefix).join("bin/mix").is_file();
    if !available {
        eprintln!("skipping: the PTY tests need Mix at /opt/mixos/bin/mix");
    }
    available
}

#[test]
fn the_window_answers_its_commands_over_a_real_broker() {
    if !mix_available() {
        return;
    }
    let broker = test_broker::Broker::start();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    // A subscriber first, so no run is published before it listens.
    let (watcher, mut executed) = runtime.block_on(async {
        let watcher = SupervisedClient::connect_options("watcher", &broker.url)
            .bounded_incoming(64)
            .connect()
            .await
            .unwrap();
        let incoming = watcher.incoming_bounded().unwrap();
        watcher.subscribe_topic(EXECUTED).await.unwrap();
        (watcher, incoming)
    });

    let settings = Settings {
        config: Config::default(),
        term: "xterm-256color".into(),
    };
    let tabs = Arc::new(Mutex::new(
        TabSet::with_session(settings, None).expect("a Mix PTY"),
    ));
    let woken = Arc::new(AtomicBool::new(true));
    let flag = woken.clone();
    tabs.lock()
        .unwrap()
        .set_wake(Arc::new(move || flag.store(true, Ordering::Release)));
    let (cleanup, reaper) = Cleanup::start().expect("cleanup worker");
    let (events_tx, events_rx) = std::sync::mpsc::sync_channel(64);
    let outbox = Arc::new(citizen::outbox::Outbox::default());
    let link = Arc::new(OnceLock::new());
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
    let (_notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel();
    let lane = term_core::bus::start_with_window_at(
        "term",
        tabs.clone(),
        cleanup.clone(),
        notify_rx,
        broker.url.clone(),
        term_core::bus::Window {
            answers: term::bus::answers,
            events: events_tx,
            wake: wake.clone(),
            outbox: outbox.clone(),
            link: link.clone(),
            reply_timeout: Duration::from_secs(2),
        },
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while link.get().is_none() {
        assert!(Instant::now() < deadline, "the lane never registered");
        std::thread::sleep(Duration::from_millis(20));
    }

    let raster = Raster::for_test(1.0, 13.0, Cursor::Underline).expect("the test font");
    let app = App::new(
        tabs.clone(),
        cleanup.clone(),
        None,
        woken,
        raster,
        FontSize::new(13.0),
    );
    let window_bus = WindowBus::new(link, events_rx, outbox, wake);
    let state = create_render_state(
        default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    term_render::install(&state);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(800.0, 400.0))
        .renderer(WgpuTestRenderer::from_render_state(state))
        .build_ui_state(
            |ui, h: &mut Holder| {
                if let Some((app, bus)) = h.start.take() {
                    h.shell = Some(Shell::new(
                        ui.ctx().clone(),
                        toolkit::Theme::embedded(),
                        app,
                        bus,
                        Clipboard::none(),
                    ));
                }
                let shell = h.shell.as_mut().unwrap();
                shell.logic(ui.ctx());
                shell.ui(ui);
            },
            Holder {
                start: Some((app, window_bus)),
                shell: None,
            },
        );
    harness.run_steps(2);

    let client = Arc::new(
        runtime
            .block_on(NodedClient::connect_anonymous(&broker.url))
            .unwrap(),
    );
    // One call, the window stepping until its answer arrives.
    let mut call = |command_name: &str, body: Value| -> (u8, String) {
        let (client, command_name, body) =
            (client.clone(), command_name.to_owned(), body.to_string());
        let task = runtime.spawn(async move {
            client
                .call_with_headers_raw("term", &command_name, &Default::default(), &body)
                .await
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        while !task.is_finished() {
            assert!(Instant::now() < deadline, "no answer");
            harness.step();
            std::thread::sleep(Duration::from_millis(5));
        }
        let (rc, body, _) = runtime.block_on(task).unwrap().unwrap();
        (rc, body)
    };

    // The window's description covers the lane's commands and its own.
    let (rc, body) = call("app.describe", json!({}));
    assert_eq!(rc, 0, "{body}");
    let described: Value = serde_json::from_str(&body).unwrap();
    let names: Vec<&str> = described["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect();
    for command_name in [
        "term.tabs",
        "term.execute",
        "term.ui.tree",
        "term.window.state",
        "term.macros.status",
    ] {
        assert!(
            names.contains(&command_name),
            "{command_name} missing from {names:?}"
        );
    }

    // The lane still serves the terminal's own commands on the same name.
    let (rc, body) = call("term.macros.status", json!({}));
    assert_eq!(rc, 0, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["state"],
        "idle"
    );
    let (rc, body) = call("HELP", json!({}));
    assert_eq!(rc, 0, "{body}");
    assert!(body.contains("term.macros.status"));
    let (rc, body) = call("term.tabs", json!({}));
    assert_eq!(rc, 0, "{body}");
    assert!(body.contains("instance="), "{body}");

    // Commands, by id.
    let (rc, body) = call("term.actions", json!({}));
    assert_eq!(rc, 0, "{body}");
    let commands: Value = serde_json::from_str(&body).unwrap();
    assert!(
        commands["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "tab.new" && c["enabled"] == true),
        "{body}"
    );
    let (rc, body) = call("term.execute", json!({"id": "tab.new"}));
    assert_eq!((rc, body.as_str()), (0, r#"{"executed":"tab.new"}"#));
    assert_eq!(tabs.lock().unwrap().list().len(), 2);
    let (rc, body) = call("term.execute", json!({"id": "no.such"}));
    assert_eq!(rc, 10);
    assert!(body.contains("UNKNOWN_ACTION"), "{body}");
    let (rc, body) = call("term.execute", json!({}));
    assert_eq!(rc, 10);
    assert!(body.contains("INVALID_ARGUMENT"), "{body}");

    // Drive commands: the menus, the window, and a key through the keyboard
    // path (Ctrl+PageDown, the next tab).
    let (rc, body) = call("term.ui.menu.list", json!({}));
    assert_eq!(rc, 0, "{body}");
    assert!(body.contains("tab.new"), "{body}");
    let (rc, body) = call("term.window.state", json!({}));
    assert_eq!(rc, 0, "{body}");
    let before = tabs.lock().unwrap().active_id();
    let (rc, body) = call(
        "term.ui.key",
        json!({"key": "PageDown", "modifiers": ["ctrl"]}),
    );
    assert_eq!(rc, 0, "{body}");
    assert_ne!(tabs.lock().unwrap().active_id(), before, "Ctrl+PageDown");
    // A targeted command: term.execute names the tab.
    let (rc, body) = call("term.execute", json!({"id": "tab.select", "tab": before}));
    assert_eq!(rc, 0, "{body}");
    assert_eq!(tabs.lock().unwrap().active_id(), before, "tab.select by id");

    // A window that draws no frames (off screen) never answers: the lane
    // answers TIMEOUT, and the command does not run when frames resume.
    let tab_count = tabs.lock().unwrap().list().len();
    let late = {
        let client = client.clone();
        runtime.block_on(async move {
            client
                .call_with_headers_raw(
                    "term",
                    "term.execute",
                    &Default::default(),
                    &json!({"id": "tab.new"}).to_string(),
                )
                .await
        })
    };
    let (rc, body, _) = late.unwrap();
    assert_eq!(rc, 10);
    assert!(body.contains("TIMEOUT"), "{body}");
    let (rc, _) = call("term.window.state", json!({}));
    assert_eq!(rc, 0);
    assert_eq!(
        tabs.lock().unwrap().list().len(),
        tab_count,
        "an expired command ran late"
    );

    // Every run was published, with how it was reached.
    let runs: Vec<(String, String)> = runtime.block_on(async {
        let mut runs = Vec::new();
        while runs.len() < 3 {
            let next = tokio::time::timeout(Duration::from_secs(5), executed.recv())
                .await
                .expect("an executed event within 5 s");
            match next {
                Some(BoundedIncomingEvent::Command(cmd)) if cmd.topic() == Some(EXECUTED) => {
                    let body: Value = serde_json::from_str(&cmd.body).unwrap();
                    runs.push((
                        body["id"].as_str().unwrap().to_owned(),
                        body["origin"].as_str().unwrap().to_owned(),
                    ));
                }
                Some(_) => {}
                None => panic!("the subscription ended"),
            }
        }
        runs
    });
    assert_eq!(
        runs,
        vec![
            ("tab.new".to_owned(), "bus".to_owned()),
            ("tab.next".to_owned(), "shortcut".to_owned()),
            ("tab.select".to_owned(), "bus".to_owned()),
        ]
    );

    // Quit over the Bus answers before the lane stops.
    let (rc, body) = call("term.execute", json!({"id": "app.quit"}));
    assert_eq!((rc, body.as_str()), (0, r#"{"executed":"app.quit"}"#));
    assert!(tabs.lock().unwrap().is_empty());
    lane.join().unwrap();

    drop(harness);
    runtime.block_on(async {
        client.close().await;
        watcher.close().await;
    });
    drop(cleanup);
    reaper.join().unwrap();
}
