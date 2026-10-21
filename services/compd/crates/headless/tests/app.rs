// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Build compd (backend-headless), noded, editd, testkit bins and
//! `cargo build -p ced --example ced-headless-probe` before this GPU target.
#[path = "support/bus.rs"]
mod bus;
#[path = "support/process.rs"]
mod process;

use bus::{Bus, binary};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

// Fixed before the first run; never inferred from captured pixels.
const IDLE_MIN: f64 = 0.99;
const TYPED_MAX: f64 = 0.999;
const LINE: &str = "headless agent typed this known line through the compositor";
const COMP: &str = "comp-headless";
const CED: &str = "ced-reference";

struct Service(Child);
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct App {
    pid: i32,
    log: std::path::PathBuf,
}
impl Drop for App {
    fn drop(&mut self) {
        if thread::panicking() {
            eprintln!("ced log: {}", fs::read_to_string(&self.log).unwrap_or_default());
        }
        // SAFETY: the helper handed this test its owned child's PID.
        unsafe { libc::kill(self.pid, libc::SIGKILL); }
    }
}

fn compare(a: &Path, b: &Path, threshold: f64, pass: bool) -> f64 {
    let output = Command::new(binary("testkit-ssim"))
        .arg(a).arg(b).args(["--threshold", &threshold.to_string()])
        .output().expect("build testkit-ssim first");
    let text = String::from_utf8_lossy(&output.stdout);
    println!("{text}");
    assert_eq!(output.status.code(), Some(i32::from(!pass)),
        "SSIM: {text} {}", String::from_utf8_lossy(&output.stderr));
    text.split_whitespace().find_map(|word| word.strip_prefix("ssim="))
        .expect("SSIM result").parse().unwrap()
}

fn capture(bus: &Bus, target: &Value, path: &Path) {
    let reply = bus.call(COMP, "comp.capture.frame", json!({
        "window":target,"path":path,"format":"png","cursor":false
    }));
    assert_eq!((reply["width"].as_u64(), reply["height"].as_u64()), (Some(800), Some(560)));
    assert_eq!(reply["source"], "offscreen");
    assert_eq!(reply["scale"], 1.0);
    assert_eq!(reply["window"], *target);
}

#[test]
#[ignore = "needs a GPU render node and built compd, noded, editd, testkit bins and ced-headless-probe"]
fn ced_wgpu_agent_typing_and_idle_ssim() {
    let bus = Bus::start("app");
    let mut compositor = process::spawn(1, None, "app", Some(&bus.url));
    compositor.extend_watchdog(Duration::from_secs(150));
    bus.ready(COMP);
    let root = &compositor.root;
    let log = fs::File::create(root.join("editd.log")).unwrap();
    let mut edit = Service(Command::new(binary("editd")).arg("serve")
        .env("MIXOS_NODED_URL", &bus.url).env("MIXOS_EDIT_RECOVERY", "0")
        .env("HOME", root).env("XDG_STATE_HOME", root.join("edit-state"))
        .stdout(Stdio::from(log.try_clone().unwrap())).stderr(log).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let reply = bus.request("edit", "edit.ping", json!({}));
        if reply["rc"] == 0 { break; }
        assert!(edit.0.try_wait().unwrap().is_none(), "editd exited");
        assert!(Instant::now() < deadline, "editd readiness: {reply}");
        thread::sleep(Duration::from_millis(50));
    }
    let before = root.join("before.png");
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testkit/scripts/app_reference.mix");
    let output = Command::new(std::env::var_os("MIXOS_TEST_MIX")
        .unwrap_or_else(|| "/opt/mixos/bin/mix".into()))
        .arg(helper).arg(&before).args(["--comp", COMP, "--output", "headless-0", "--keep", "--app"])
        .arg(binary("examples/ced-headless-probe"))
        .env("MIXOS_NODED_URL", &bus.url).env("XDG_RUNTIME_DIR", root)
        .env("WAYLAND_DISPLAY", "wayland-headless-test")
        .output().unwrap();
    assert!(output.status.success(), "reference helper: {} {}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let record: Value = serde_json::from_str(String::from_utf8_lossy(&output.stdout)
        .lines().last().expect("helper JSON")).unwrap();
    let _app = App { pid: record["pid"].as_i64().unwrap().try_into().unwrap(),
        log: record["log"].as_str().unwrap().into() };
    println!("APP_REFERENCE {record}");
    assert_eq!(record["capture"]["source"], "offscreen");
    assert_eq!(record["row"]["window_x"], 80.0);
    assert_eq!(record["row"]["window_y"], 60.0);
    let target = &record["window"];
    let state = bus.call(CED, "ced.state", json!({"text":true}));
    assert_eq!(state["text"], "");

    // Click the real editor, using its own last-frame layout. Do this before
    // the idle pair so keyboard focus and caret state are the same in both.
    let layout = bus.call(CED, "ced.layout", json!({}));
    let x = layout["editor"]["x"].as_f64().unwrap() + layout["gutter_w"].as_f64().unwrap() + 20.0;
    let y = layout["editor"]["y"].as_f64().unwrap() + layout["line_height"].as_f64().unwrap() / 2.0;
    bus.call(COMP, "comp.input.sequence", json!({"seat":"agent","steps":[
        {"verb":"comp.input.pointer.move","args":{"window":target,"x":x,"y":y,"require_hit":true}},
        {"verb":"comp.input.pointer.button","args":{"button":"left","action":"click"}}
    ]}));
    thread::sleep(Duration::from_millis(300));
    capture(&bus, target, &before);
    thread::sleep(Duration::from_millis(100));
    let idle = root.join("idle.png");
    capture(&bus, target, &idle);
    assert!(compare(&before, &idle, IDLE_MIN, true) >= IDLE_MIN);

    let reply = bus.call(COMP, "comp.input.sequence", json!({"seat":"agent","steps":[
        {"verb":"comp.input.key","args":{"window":target,"text":LINE}}
    ]}));
    assert!(reply.get("error").is_none(), "{reply}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = bus.call(CED, "ced.state", json!({"text":true}));
        if state["text"] == LINE && state["pending"] == 0 && state["inflight"] == false {
            println!("CED_READBACK {state}");
            break;
        }
        assert!(Instant::now() < deadline, "typed text did not settle: {state}");
        thread::sleep(Duration::from_millis(50));
    }
    thread::sleep(Duration::from_millis(200));
    let typed = root.join("typed.png");
    capture(&bus, target, &typed);
    assert!(compare(&before, &typed, TYPED_MAX, false) < TYPED_MAX);

    let trace = fs::read_to_string(&_app.log).unwrap();
    let adapter = trace.lines().filter(|line| line.contains("AdapterInfo") || line.contains("adapter:")).collect::<Vec<_>>();
    assert!(!adapter.is_empty(), "wgpu adapter was not logged: {trace}");
    for line in adapter { println!("CED_WGPU {line}"); }
    let creation = trace.lines().find(|line| line.contains("new id wl_buffer") &&
        (line.contains(".create_prime_buffer(") || line.contains(".create_buffer(") || line.contains(".create_immed(")))
        .expect("Wayland trace must identify the first actual buffer creation");
    let kind = if creation.contains("wl_shm_pool") { "wl_shm" }
        else if creation.contains("wl_drm") { "wl_drm" }
        else if creation.contains("zwp_linux_buffer_params") { "linux-dmabuf" }
        else { panic!("unrecognised buffer request: {creation}") };
    println!("CED_BUFFER {kind}: {creation}");
    println!("ced: idle >= {IDLE_MIN}, typed < {TYPED_MAX}; exact agent text readback");
}
