// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};
pub struct Process {
    pub child: Child,
    pub root: PathBuf,
    stop: mpsc::Sender<()>,
    watchdog: Option<thread::JoinHandle<()>>,
}

impl Drop for Process {
    fn drop(&mut self) {
        if thread::panicking() {
            eprintln!(
                "compd log: {}",
                fs::read_to_string(self.root.join("compd.log")).unwrap_or_default()
            );
        }
        let _ = self.stop.send(());
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.join();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn spawn(scale: i32, explicit_node: Option<&str>, label: &str, bus: Option<&str>) -> Process {
    spawn_options(scale, explicit_node, label, bus, "60")
}

#[allow(dead_code)] // Used only by boot.rs; each integration target shares this module.
pub fn spawn_at_refresh(refresh: &str) -> Process {
    spawn_options(1, None, "refresh", None, refresh)
}

fn spawn_options(
    scale: i32,
    explicit_node: Option<&str>,
    label: &str,
    bus: Option<&str>,
    refresh: &str,
) -> Process {
    let binary = std::env::var_os("COMPD_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("compd")
        });
    assert!(
        binary.is_file(),
        "build compd with backend-headless before this test: {}",
        binary.display()
    );
    let root = std::env::temp_dir().join(format!("compd-headless-{}-{label}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let node =
        std::env::var("COMPD_TEST_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
    let mut config = model::environment::config::base::default_settings();
    // Default priority is realtime: headless must override the startup policy.
    assert_eq!(config.priority, "realtime");
    config.render_node = if explicit_node.is_some() {
        "/dev/dri/nonexistent-render-node".into()
    } else {
        node.clone()
    };
    let settings = root.join("settings.json");
    fs::write(&settings, serde_json::to_vec(&config).unwrap()).unwrap();
    let log = root.join("compd.log");
    let stderr = fs::File::create(&log).unwrap();
    let mut command = Command::new(binary);
    command
        .args([
            "--headless",
            "--socket",
            "wayland-headless-test",
            "--size",
            "1280x720",
            "--config-file",
        ])
        .arg(settings)
        .env("XDG_RUNTIME_DIR", &root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("HOME", &root)
        .env("MIXOS_ETC", root.join("etc"))
        .env("MIXOS_VAR", root.join("var"))
        .env("MIXOS_RUN", root.join("run"))
        .env("MIXOS_NODED_URL", bus.unwrap_or("ws://127.0.0.1:1"))
        .env("COMPD_HEADLESS_REFRESH_HZ", refresh)
        .env("COMPD_FRAME_TRACE", "1")
        .env("COMPD_FRAME_TRACE_FILE", root.join("frames.log"))
        .env("COMPD_FRAME_TRACE_LIMIT", "0")
        // Fixed built-in 24px arrow: no host theme or cursor animation.
        .env("XCURSOR_PATH", root.join("cursors"))
        .env("XCURSOR_THEME", "headless-test")
        .env("XCURSOR_SIZE", "24")
        // Deliberately request XWayland: --headless must keep it off.
        .env("COMPD_XWAYLAND", "1")
        .env_remove("NOTIFY_SOCKET")
        .env_remove("MIXOS_VT")
        .stdout(Stdio::from(stderr.try_clone().unwrap()))
        .stderr(stderr);
    if let Some(node) = explicit_node {
        command.args(["--render-node", node]);
    }
    if scale != 1 {
        command.args(["--scale", &scale.to_string()]);
    }
    let child = command.spawn().expect("start compd");
    let pid = child.id();
    let (stop, stopped) = mpsc::channel();
    let watchdog = thread::spawn(move || {
        if stopped.recv_timeout(Duration::from_secs(45)).is_err() {
            // Break a stuck Wayland roundtrip too; never leave the child behind.
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    });
    Process {
        child,
        root,
        stop,
        watchdog: Some(watchdog),
    }
}
