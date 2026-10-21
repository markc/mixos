// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Real render-node compositor, Wayland clients and an owned private Bus.
//! Build compd (backend-headless), noded and testkit binaries before running.
#[path = "support/pattern.rs"]
mod pattern;
#[path = "support/process.rs"]
mod process;

use pattern::{COLOURS, Pattern};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};
use testkit::TestClient;
use wayland_client::protocol::wl_shm;

fn binary(name: &str) -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(name)
}

struct Bus {
    root: PathBuf,
    url: String,
}
impl Bus {
    fn start(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("headless-bus-{}-{label}", std::process::id()));
        let output = helper("start", &root, &[binary("noded").to_str().unwrap()], None);
        assert!(
            output.status.success(),
            "private broker: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let record: Value =
            serde_json::from_slice(&fs::read(root.join("bus.json")).unwrap()).unwrap();
        Self {
            root,
            url: record["url"].as_str().unwrap().into(),
        }
    }
    fn call(&self, verb: &str, args: Value) -> Value {
        let output = helper(
            "call",
            &self.root,
            &[verb, &args.to_string()],
            Some(&self.url),
        );
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "Bus {verb}: {text} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reply: Value = serde_json::from_str(text.lines().last().expect("Bus reply")).unwrap();
        assert_eq!(reply["rc"], 0, "{verb}: {reply}");
        match &reply["body"] {
            Value::String(body) => serde_json::from_str(body).unwrap(),
            body => body.clone(),
        }
    }
    fn ready(&self) {
        let output = helper("ready", &self.root, &[], Some(&self.url));
        assert!(
            output.status.success(),
            "headless Bus readiness: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn window(&self) -> Value {
        let reply = self.call("comp.windows.list", json!({"app_id":"headless-pattern"}));
        reply["windows"]
            .as_array()
            .unwrap()
            .first()
            .expect("mapped pattern window")
            .clone()
    }
}
impl Drop for Bus {
    fn drop(&mut self) {
        let output = helper("stop", &self.root, &[], None);
        assert!(
            output.status.success(),
            "stop owned broker: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn helper(action: &str, root: &Path, rest: &[&str], url: Option<&str>) -> Output {
    let mut command = Command::new(
        std::env::var_os("MIXOS_TEST_MIX").unwrap_or_else(|| "/opt/mixos/bin/mix".into()),
    );
    command
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/bus.mix"))
        .arg(action)
        .arg(root)
        .args(rest)
        .env("MIXOS_NODED_URL", url.unwrap_or("ws://127.0.0.1:1"))
        .env("MIXOS_RUN", root.join("run"))
        .env("MIXOS_ETC", root.join("etc"))
        .env("MIXOS_VAR", root.join("var"))
        .env("MIXOS_NODE_CONFIG", root.join("etc/node.conf.mix"));
    command.output().unwrap()
}
fn connect(process: &mut process::Process) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        if let Ok(stream) = UnixStream::connect(process.root.join("wayland-headless-test")) {
            return stream;
        }
        assert!(
            process.child.try_wait().unwrap().is_none(),
            "compd exited: {}",
            fs::read_to_string(process.root.join("compd.log")).unwrap()
        );
        assert!(Instant::now() < deadline, "compd startup timed out");
        thread::sleep(Duration::from_millis(10));
    }
}
fn pump(client: &mut TestClient, predicate: impl Fn(&TestClient) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate(client) {
        client.flush();
        client.read();
        assert!(
            Instant::now() < deadline,
            "Wayland capture timed out: {:?}",
            client.state.frames
        );
        thread::sleep(Duration::from_millis(2));
    }
}
fn roundtrip(client: &mut TestClient) {
    let sync = client.sync();
    pump(client, |c| c.state.syncs >= sync);
}
fn pixel(bytes: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
    let at = (y * width + x) * 4;
    [bytes[at + 2], bytes[at + 1], bytes[at]]
}
fn rgb(colour: u32) -> [u8; 3] {
    [(colour >> 16) as u8, (colour >> 8) as u8, colour as u8]
}

fn monotonic_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: time is a valid writable timespec.
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) },
        0
    );
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

fn frame_count(root: &Path) -> usize {
    let trace = fs::read_to_string(root.join("frames.log")).unwrap();
    assert!(!trace.contains("stage=trace_limit "));
    for line in trace.lines() {
        assert!(
            line.contains(" dropped=0 "),
            "incomplete frame trace: {line}"
        );
    }
    trace
        .lines()
        .filter(|line| line.contains("stage=comp_frame_reasons "))
        .count()
}

#[test]
#[ignore = "needs a GPU render node and built compd, noded and testkit binaries"]
fn pattern_mixed_cursor_captures_and_presentation_ledger() {
    let bus = Bus::start("pixels");
    let mut process = process::spawn(1, None, "pixels", Some(&bus.url));
    let stream = connect(&mut process);
    bus.ready();
    let wall_start = monotonic_ns();
    let mut pattern = Pattern::new(stream);
    pattern.wait_callback(0);
    // Headless has no human sources. Move the fixed built-in 24px amber
    // agent arrow; no host cursor theme or animation enters the test.
    bus.call(
        "comp.input.pointer.move",
        json!({"seat":"agent","output":"headless-0","x":160,"y":160}),
    );
    for _ in 0..20 {
        pattern.pump();
        thread::sleep(Duration::from_millis(5));
    }
    let mut capture = TestClient::new(connect(&mut process));
    roundtrip(&mut capture);
    capture.bind_globals();
    roundtrip(&mut capture);
    let (off, oi) = capture.capture_with(None, false);
    let (on, ni) = capture.capture_with(None, true);
    pump(&mut capture, |c| {
        c.state.frames[oi].buffer_done && c.state.frames[ni].buffer_done
    });
    for index in [oi, ni] {
        assert_eq!(capture.state.frames[index].buffer.unwrap().1, 1280);
        assert_eq!(capture.state.frames[index].buffer.unwrap().2, 720);
    }
    let (off_buffer, off_file) = capture.shm_buffer_with(1280, 720, 5120, wl_shm::Format::Xrgb8888);
    let (on_buffer, on_file) = capture.shm_buffer_with(1280, 720, 5120, wl_shm::Format::Xrgb8888);
    // Both copies enter the same Wayland flush, before the one-shot frame.
    off.copy(&off_buffer);
    on.copy(&on_buffer);
    capture.flush();
    pump(&mut capture, |c| {
        c.state.frames[oi].ready && c.state.frames[ni].ready
    });
    let off_pixels = capture.read_buffer(off_file);
    let on_pixels = capture.read_buffer(on_file);
    for ((x, y), colour) in [(320, 180), (960, 180), (320, 540), (960, 540)]
        .into_iter()
        .zip(COLOURS)
    {
        assert_eq!(pixel(&off_pixels, 1280, x, y), rgb(colour));
    }
    assert_ne!(
        pixel(&off_pixels, 1280, 320, 180),
        [255, 0, 255],
        "negative colour control"
    );
    let mut changed = 0;
    let mut amber = 0;
    for y in 0..720 {
        for x in 0..1280 {
            let a = pixel(&off_pixels, 1280, x, y);
            let b = pixel(&on_pixels, 1280, x, y);
            if (160..184).contains(&x) && (160..184).contains(&y) {
                assert_eq!(a, rgb(COLOURS[0]), "cursorless pixel");
                changed += usize::from(a != b);
                amber += usize::from(b == [255, 159, 28]);
            } else {
                assert_eq!(a, b, "unexpected difference at {x},{y}");
            }
        }
    }
    let (opaque_pixels, fill_pixels) =
        world::seat::pointer::texture::pointer_load::agent_arrow_pixel_counts(24);
    assert_eq!(
        changed, opaque_pixels,
        "complete agent arrow must be captured"
    );
    assert_eq!(
        amber, fill_pixels,
        "complete agent arrow fill must be captured"
    );
    // Also exercise the shipped screenshot client against the same pattern.
    let shot = process.root.join("screenshot.ppm");
    let output = Command::new(binary("testkit-screenshot"))
        .arg(&shot)
        .env("XDG_RUNTIME_DIR", &process.root)
        .env("WAYLAND_DISPLAY", "wayland-headless-test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "screenshot: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ppm = fs::read(&shot).unwrap();
    let header = b"P6\n1280 720\n255\n";
    assert!(ppm.starts_with(header));
    for ((x, y), colour) in [(320, 180), (960, 180), (320, 540), (960, 540)]
        .into_iter()
        .zip(COLOURS)
    {
        let at = header.len() + (y * 1280 + x) * 3;
        assert_eq!(&ppm[at..at + 3], &rgb(colour));
    }
    println!(
        "four pattern points + negative control; screenshot; mixed cursor frame: {changed} changed pixels confined to [160,160,24,24], {amber} amber pixels"
    );

    let before = bus.window()["presentation"]["presented"].as_u64().unwrap();
    for _ in 0..12 {
        let previous = pattern.state.callbacks;
        pattern.commit(true);
        pattern.wait_callback(previous);
    }
    thread::sleep(Duration::from_millis(150));
    let after = bus.window()["presentation"]["presented"].as_u64().unwrap();
    assert!(
        after >= before + 10,
        "damage commits did not present: {before} -> {after}"
    );
    for _ in 0..12 {
        let previous = pattern.state.callbacks;
        pattern.commit(false);
        pattern.wait_callback(previous);
    }
    thread::sleep(Duration::from_millis(150));
    let still = bus.window()["presentation"]["presented"].as_u64().unwrap();
    assert_eq!(after, still, "callback-only commits must not present");
    // All callbacks have been consumed and the client stops committing. Count
    // both presented and empty draws through compd's existing frame ledger.
    thread::sleep(Duration::from_millis(200));
    let frames = frame_count(&process.root);
    assert!(frames > 0, "frame tracing must be active");
    let idle_start = Instant::now();
    while idle_start.elapsed() < Duration::from_secs(1) {
        pattern.pump();
        assert_eq!(
            frame_count(&process.root),
            frames,
            "idle timer kept drawing"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        still,
        bus.window()["presentation"]["presented"].as_u64().unwrap()
    );
    assert_eq!(pattern.state.clock, Some(libc::CLOCK_MONOTONIC as u32));
    assert!(pattern.state.presented >= 12);
    assert!(pattern.state.discarded >= 10);
    assert!(
        pattern
            .state
            .times
            .windows(2)
            .all(|pair| pair[0] <= pair[1])
    );
    let wall_end = monotonic_ns();
    assert!(!pattern.state.times.is_empty());
    for time in &pattern.state.times {
        assert!(
            (wall_start..=wall_end).contains(time),
            "presentation {time} outside monotonic test window {wall_start}..={wall_end}"
        );
    }
    println!(
        "presentation.presented {before} -> {after} -> {still}; {} callbacks, {} presented / {} discarded feedbacks; CLOCK_MONOTONIC",
        pattern.state.callbacks, pattern.state.presented, pattern.state.discarded
    );
    println!(
        "idle frame count stayed at {frames} for {:?}; timestamps within {wall_start}..={wall_end}",
        idle_start.elapsed()
    );
    // Different content still presents after the idle/callback-only stretch.
    pattern.change_buffer();
    let previous = pattern.state.callbacks;
    pattern.commit(true);
    pattern.wait_callback(previous);
}

#[test]
#[ignore = "needs a GPU render node and built compd and noded binaries"]
fn bus_capture_completes_on_idle_output() {
    let bus = Bus::start("idle");
    let mut process = process::spawn(1, None, "idle", Some(&bus.url));
    drop(connect(&mut process));
    bus.ready();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        bus.call("comp.windows.list", json!({}))["windows"],
        json!([])
    );
    bus.call(
        "comp.input.pointer.move",
        json!({"seat":"agent","output":"headless-0","x":160,"y":160}),
    );
    for cursor in [false, true] {
        let path = process.root.join(format!("idle-{cursor}.ppm"));
        let start = Instant::now();
        let reply = bus.call(
            "comp.capture.frame",
            json!({"output":"headless-0","path":path,"format":"ppm","cursor":cursor}),
        );
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(1500),
            "idle capture took {elapsed:?}"
        );
        assert_eq!(reply["width"], 1280);
        assert_eq!(reply["height"], 720);
        assert_eq!(reply["source"], "offscreen");
        let ppm = fs::read(path).unwrap();
        assert!(ppm.starts_with(b"P6\n1280 720\n255\n"));
        assert_eq!(ppm.len(), "P6\n1280 720\n255\n".len() + 1280 * 720 * 3);
        // frames::scene paints the empty desktop with BACKGROUND_COLOR
        // [0.07, 0.08, 0.10, 1.0], covering the framebuffer's GL clear colour.
        for (i, pixel) in ppm[b"P6\n1280 720\n255\n".len()..]
            .chunks_exact(3)
            .enumerate()
        {
            let (x, y) = (i % 1280, i / 1280);
            if !cursor || !(160..184).contains(&x) || !(160..184).contains(&y) {
                assert_eq!(
                    pixel,
                    [18, 20, 25],
                    "empty desktop colour cursor={cursor} at {x},{y}"
                );
            }
        }
        println!("idle Bus capture cursor={cursor}: {elapsed:?}, 1280x720 offscreen");
    }
}
