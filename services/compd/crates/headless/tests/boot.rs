// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Real compd process, render node and Wayland registry. Build compd with
//! backend-headless first; COMPD_TEST_BINARY may select an explicit binary.

use std::{
    fs,
    os::unix::{fs::FileTypeExt, net::UnixStream},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    protocol::{wl_output, wl_registry},
};

#[derive(Default)]
struct OutputState {
    name: Option<String>,
    mode: Option<(i32, i32, i32)>,
    scale: Option<i32>,
    outputs: Vec<wl_output::WlOutput>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for OutputState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "wl_output"
        {
            state
                .outputs
                .push(registry.bind(name, version.min(4), qh, ()));
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for OutputState {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Name { name } => state.name = Some(name),
            wl_output::Event::Mode {
                width,
                height,
                refresh,
                ..
            } => state.mode = Some((width, height, refresh)),
            wl_output::Event::Scale { factor } => state.scale = Some(factor),
            _ => {}
        }
    }
}

#[path = "support/process.rs"]
mod process;
#[path = "support/pattern.rs"]
#[allow(dead_code)] // Shared client; this target exercises only callback pacing.
mod pattern;
use process::spawn;

fn boot(scale: i32, explicit_node: bool) {
    let node =
        std::env::var("COMPD_TEST_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
    let mut process = spawn(
        scale,
        explicit_node.then_some(node.as_str()),
        &format!("boot-{scale}"),
        None,
    );
    let pid = process.child.id();
    let log = process.root.join("compd.log");
    let deadline = Instant::now() + Duration::from_secs(30);
    let stream = loop {
        if let Some(status) = process.child.try_wait().unwrap() {
            panic!(
                "compd exited {status}: {}",
                fs::read_to_string(&log).unwrap()
            );
        }
        if let Ok(stream) = UnixStream::connect(process.root.join("wayland-headless-test")) {
            break stream;
        }
        assert!(
            Instant::now() < deadline,
            "startup timed out: {}",
            fs::read_to_string(&log).unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    };
    let connection = Connection::from_socket(stream).unwrap();
    let mut queue = connection.new_event_queue::<OutputState>();
    let _registry = connection.display().get_registry(&queue.handle(), ());
    let mut state = OutputState::default();
    queue.roundtrip(&mut state).unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.outputs.len(), 1);
    assert_eq!(state.name.as_deref(), Some("headless-0"));
    assert_eq!(state.mode, Some((1280, 720, 60_000)));
    assert_eq!(state.scale, Some(scale));
    while !fs::read_to_string(&log)
        .unwrap()
        .contains("headless: first offscreen frame completed")
    {
        assert!(
            Instant::now() < deadline,
            "first frame timed out: {}",
            fs::read_to_string(&log).unwrap()
        );
        assert!(
            process.child.try_wait().unwrap().is_none(),
            "compd died before first frame"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let output = fs::read_to_string(&log).unwrap();
    assert!(
        output.contains("priority: headless keeps inherited scheduling and nice value"),
        "headless scheduling policy was not applied: {output}"
    );
    assert!(output.contains("headless: XWayland disabled"), "{output}");
    assert!(!output.contains("XWayland ready"), "{output}");
    assert!(
        !process
            .root
            .join("compd/wayland-headless-test.xwayland.env")
            .exists()
    );
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    assert_eq!(fields[15], "20", "stat field 18: normal priority");
    assert_eq!(fields[16], "0", "stat field 19: nice 0");
    let sched = fs::read_to_string(format!("/proc/{pid}/sched")).unwrap();
    let policy = sched
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("policy")
                .and_then(|v| v.trim().strip_prefix(':'))
                .map(str::trim)
        })
        .unwrap();
    assert_eq!(policy, "0", "SCHED_OTHER");
    println!(
        "headless-0: 1280x720@60000 scale {scale}; SCHED_OTHER priority 20 nice 0; explicit render node {explicit_node}"
    );
}

#[test]
#[ignore = "needs a GPU render node and a built compd with backend-headless"]
fn boot_mode_scale_and_scheduling() {
    boot(2, true);
}

#[test]
#[ignore = "needs a GPU render node and a built compd with backend-headless"]
fn boot_config_render_node_and_default_scale() {
    boot(1, false);
}

#[test]
#[ignore = "needs a GPU render node and a built compd with backend-headless"]
fn lower_refresh_sets_the_virtual_mode() {
    let mut process = process::spawn_at_refresh("30");
    let deadline = Instant::now() + Duration::from_secs(25);
    let stream = loop {
        if let Ok(stream) = UnixStream::connect(process.root.join("wayland-headless-test")) {
            break stream;
        }
        assert!(process.child.try_wait().unwrap().is_none(), "compd exited");
        assert!(Instant::now() < deadline, "startup timed out");
        thread::sleep(Duration::from_millis(10));
    };
    let conn = Connection::from_socket(stream).unwrap();
    let mut queue = conn.new_event_queue::<OutputState>();
    let _registry = conn.display().get_registry(&queue.handle(), ());
    let mut state = OutputState::default();
    queue.roundtrip(&mut state).unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.mode, Some((1280, 720, 30_000)));
    let stream = UnixStream::connect(process.root.join("wayland-headless-test")).unwrap();
    let mut pattern = pattern::Pattern::new(stream);
    pattern.wait_callback(0);
    let start = Instant::now();
    let before = pattern.state.callbacks;
    while start.elapsed() < Duration::from_secs(2) {
        let previous = pattern.state.callbacks;
        pattern.commit(true);
        pattern.wait_callback(previous);
    }
    let elapsed = start.elapsed();
    let callbacks = pattern.state.callbacks - before;
    let hz = callbacks as f64 / elapsed.as_secs_f64();
    assert!((25.5..=34.5).contains(&hz), "30 Hz pacing: {callbacks} callbacks in {elapsed:?} = {hz:.2} Hz");
    println!("headless virtual output 30 Hz; {callbacks} damage callbacks in {elapsed:?} = {hz:.2} Hz");
}

fn refuses_node(node: &str, label: &str, reason: &str) {
    let mut process = spawn(1, Some(node), label, None);
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = process.child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "invalid render node did not exit"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let output = fs::read_to_string(process.root.join("compd.log")).unwrap();
    assert!(
        !status.success(),
        "invalid render node was accepted: {output}"
    );
    assert!(output.contains(reason), "missing {reason}: {output}");
    println!("render node {node} refused with {reason}");
}

#[test]
#[ignore = "needs a built compd with backend-headless"]
fn render_node_nonexistent_is_refused() {
    refuses_node(
        "/dev/dri/nonexistent-render-node",
        "missing-node",
        "headless-render-node-invalid:",
    );
}

#[test]
#[ignore = "needs a built compd with backend-headless"]
fn render_node_primary_or_non_render_is_refused() {
    let card = fs::read_dir("/dev/dri")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("card")
        });
    let node = card.or_else(|| {
        fs::metadata("/dev/null")
            .ok()
            .filter(|metadata| metadata.file_type().is_char_device())
            .map(|_| PathBuf::from("/dev/null"))
    });
    let Some(node) = node else {
        println!("SKIP: no primary DRM node or non-render character device available");
        return;
    };
    let reason = if node.starts_with("/dev/dri") {
        "headless-render-node-required:"
    } else {
        "headless-render-node-invalid:"
    };
    refuses_node(node.to_str().unwrap(), "non-render-node", reason);
}
