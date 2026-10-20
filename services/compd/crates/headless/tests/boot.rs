// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Real compd process, render node and Wayland registry. Build compd with
//! backend-headless first; COMPD_TEST_BINARY may select an explicit binary.

use std::{fs, os::unix::{fs::PermissionsExt, net::UnixStream}, path::PathBuf,
    process::{Child, Command, Stdio}, sync::mpsc, thread, time::{Duration, Instant}};
use wayland_client::{Connection, Dispatch, QueueHandle, protocol::{wl_output, wl_registry}};

#[derive(Default)]
struct OutputState {
    name: Option<String>,
    mode: Option<(i32, i32, i32)>,
    scale: Option<i32>,
    outputs: Vec<wl_output::WlOutput>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for OutputState {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event,
        _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            if interface == "wl_output" {
                state.outputs.push(registry.bind(name, version.min(4), qh, ()));
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for OutputState {
    fn event(state: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event,
        _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_output::Event::Name { name } => state.name = Some(name),
            wl_output::Event::Mode { width, height, refresh, .. } => state.mode = Some((width, height, refresh)),
            wl_output::Event::Scale { factor } => state.scale = Some(factor),
            _ => {}
        }
    }
}

struct Process {
    child: Child,
    root: PathBuf,
    stop: mpsc::Sender<()>,
    watchdog: Option<thread::JoinHandle<()>>,
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(watchdog) = self.watchdog.take() { let _ = watchdog.join(); }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn boot(scale: i32, explicit_node: bool) {
    let binary = std::env::var_os("COMPD_TEST_BINARY").map(PathBuf::from).unwrap_or_else(|| {
        std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("compd")
    });
    assert!(binary.is_file(), "build compd with backend-headless before this test: {}", binary.display());
    let root = std::env::temp_dir().join(format!("compd-headless-{}-{scale}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let node = std::env::var("COMPD_TEST_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
    let mut config = model::environment::config::base::default_settings();
    // Default priority is realtime: headless must override the startup policy.
    assert_eq!(config.priority, "realtime");
    config.render_node = if explicit_node { "/dev/dri/nonexistent-render-node".into() } else { node.clone() };
    let settings = root.join("settings.json");
    fs::write(&settings, serde_json::to_vec(&config).unwrap()).unwrap();
    let log = root.join("compd.log");
    let stderr = fs::File::create(&log).unwrap();
    let mut command = Command::new(binary);
    command.args(["--headless", "--socket", "wayland-headless-test", "--size", "1280x720", "--scale", &scale.to_string(), "--config-file"])
        .arg(settings)
        .env("XDG_RUNTIME_DIR", &root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("HOME", &root)
        // Deliberately request XWayland: --headless must keep it off.
        .env("COMPD_XWAYLAND", "1")
        .env_remove("NOTIFY_SOCKET")
        .env_remove("MIXOS_VT")
        .stdout(Stdio::from(stderr.try_clone().unwrap())).stderr(stderr);
    if explicit_node { command.args(["--render-node", &node]); }
    let child = command.spawn().expect("start compd");
    let pid = child.id();
    let (stop, stopped) = mpsc::channel();
    let watchdog = thread::spawn(move || {
        if stopped.recv_timeout(Duration::from_secs(45)).is_err() {
            // Break a stuck Wayland roundtrip too; never leave the child behind.
            unsafe { libc::kill(pid as i32, libc::SIGKILL); }
        }
    });
    let mut process = Process { child, root, stop, watchdog: Some(watchdog) };
    let deadline = Instant::now() + Duration::from_secs(30);
    let stream = loop {
        if let Some(status) = process.child.try_wait().unwrap() {
            panic!("compd exited {status}: {}", fs::read_to_string(&log).unwrap());
        }
        if let Ok(stream) = UnixStream::connect(process.root.join("wayland-headless-test")) { break stream; }
        assert!(Instant::now() < deadline, "startup timed out: {}", fs::read_to_string(&log).unwrap());
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
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<_> = stat.rsplit_once(')').unwrap().1.split_whitespace().collect();
    assert_eq!(fields[15], "20", "stat field 18: normal priority");
    assert_eq!(fields[16], "0", "stat field 19: nice 0");
    let sched = fs::read_to_string(format!("/proc/{pid}/sched")).unwrap();
    let policy = sched.lines().find_map(|line| line.trim().strip_prefix("policy").and_then(|v| v.trim().strip_prefix(':')).map(str::trim)).unwrap();
    assert_eq!(policy, "0", "SCHED_OTHER");
    println!("headless-0: 1280x720@60000 scale {scale}; SCHED_OTHER priority 20 nice 0; explicit render node {explicit_node}");
}

#[test]
#[ignore = "needs a GPU render node and a built compd with backend-headless"]
fn boot_mode_scale_and_scheduling() { boot(2, true); }

#[test]
#[ignore = "needs a GPU render node and a built compd with backend-headless"]
fn boot_config_render_node_and_default_scale() { boot(1, false); }
