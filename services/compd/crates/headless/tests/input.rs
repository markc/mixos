// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Real agent input and service isolation on a private broker. Build compd,
//! noded and testkit-input-probe before running the ignored render-node tests.
#[path = "support/bus.rs"]
mod bus;
#[path = "support/process.rs"]
mod process;
#[path = "support/seats.rs"]
mod seats;

use bus::{Bus, binary};
use comp_model::catalogue::{FOCUS_TOPIC_SUFFIX, topic_name};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use world::comp::injection::BackendInput;

const SERVICE: &str = "comp-headless";

struct Probe {
    child: Child,
    log: PathBuf,
}
impl Drop for Probe {
    fn drop(&mut self) {
        if thread::panicking() {
            eprintln!("probe log: {}", self.text());
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Probe {
    fn start(process: &mut process::Process, app: &str) -> Self {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !process.root.join("wayland-headless-test").exists() {
            assert!(process.child.try_wait().unwrap().is_none(), "compd exited");
            assert!(Instant::now() < deadline, "socket timeout");
            thread::sleep(Duration::from_millis(10));
        }
        let log = process.root.join("probe.log");
        let file = fs::File::create(&log).unwrap();
        let child = Command::new(binary("testkit-input-probe"))
            .args([
                "--seats",
                "--size",
                "400x240",
                "--seconds",
                "35",
                "--app-id",
                app,
            ])
            .env("XDG_RUNTIME_DIR", &process.root)
            .env("WAYLAND_DISPLAY", "wayland-headless-test")
            .stdout(Stdio::from(file.try_clone().unwrap()))
            .stderr(file)
            .spawn()
            .unwrap();
        let mut probe = Self { child, log };
        probe.wait_for("PROBE seats_ready");
        probe.wait_for("PROBE presented 1");
        probe
    }
    fn text(&self) -> String {
        fs::read_to_string(&self.log).unwrap()
    }
    fn wait_for(&mut self, line: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.text().lines().any(|entry| entry == line) {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "probe exited: {}",
                self.text()
            );
            assert!(Instant::now() < deadline, "missing {line}: {}", self.text());
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn no_human_input(&self) {
        for line in self
            .text()
            .lines()
            .filter(|line| line.starts_with("PROBE seat \"seat0\" "))
        {
            assert!(
                line.contains(" bound keyboard=0 pointer=0 "),
                "human input: {line}"
            );
        }
    }
}

fn window(bus: &Bus, service: &str, app: &str) -> Value {
    let reply = bus.call(service, "comp.windows.list", json!({}));
    let rows = reply["windows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{service}: {reply}");
    assert_eq!(rows[0]["app_id"], app, "{reply}");
    rows[0].clone()
}

fn target(row: &Value) -> Value {
    json!({"id":row["id"],"generation":row["generation"]})
}

#[test]
fn backend_authority_and_coordinate_bounds_preserve_session_rules() {
    assert!(BackendInput::Headless.agent_authoritative(true));
    assert!(BackendInput::Headless.agent_authoritative(false));
    assert!(BackendInput::Session.agent_authoritative(false));
    assert!(!BackendInput::Session.agent_authoritative(true));
    assert_eq!(BackendInput::default(), BackendInput::Session);
    for (x, y, expected) in [
        (-20.0, 900.0, (0.0, 719.0)),
        (1280.0, -1.0, (1279.0, 0.0)),
        (10.25, 20.5, (10.25, 20.5)),
    ] {
        assert_eq!(
            BackendInput::Headless.output_position(x, y, 1280, 720),
            Some(expected)
        );
    }
    assert_eq!(
        BackendInput::Session.output_position(-1.0, 0.0, 1280, 720),
        None
    );
    assert_eq!(
        BackendInput::Session.output_position(1280.0, 0.0, 1280, 720),
        None
    );
    assert_eq!(
        BackendInput::Session.output_position(1279.5, 719.5, 1280, 720),
        Some((1279.5, 719.5))
    );
}

#[test]
#[ignore = "needs a GPU render node and built compd, noded and testkit-input-probe"]
fn agent_sequence_types_into_a_probe_window() {
    let bus = Bus::start("input");
    let mut process = process::spawn(1, None, "input", Some(&bus.url));
    let mut probe = Probe::start(&mut process, "headless-input");
    bus.ready(SERVICE);
    let row = window(&bus, SERVICE, "headless-input");
    // Read the initial pointer before moving it (the camera is pinned).
    let seats = bus.call(SERVICE, "comp.props.get", json!({"path":"input.seats"}));
    println!("initial seats: {seats}");
    assert_eq!(seats["agent"]["pointer"]["output"], "headless-0");
    assert_eq!(seats["agent"]["pointer"]["x"], 640.0);
    assert_eq!(seats["agent"]["pointer"]["y"], 360.0);
    let output_x = row["window_x"].as_f64().unwrap() + 40.0;
    let output_y = row["window_y"].as_f64().unwrap() + 50.0;
    let reply = bus.call(SERVICE, "comp.input.sequence", json!({"seat":"agent","steps":[
        {"verb":"comp.input.pointer.move","args":{"output":"headless-0","x":output_x,"y":output_y}},
        {"verb":"comp.input.pointer.button","args":{"button":"left","action":"click"}},
        {"verb":"comp.input.key","args":{"window":target(&row),"text":"abc"}}
    ]}));
    assert_eq!(reply["steps"].as_array().unwrap().len(), 3, "{reply}");
    for line in [
        "enter 40 50",
        "button 272 1 40 50",
        "button 272 0 40 50",
        "keyboard_enter",
        "key 30 1",
        "key 30 0",
        "key 48 1",
        "key 48 0",
        "key 46 1",
        "key 46 0",
    ] {
        probe.wait_for(&format!("PROBE seat \"agent\" {line}"));
    }
    // The single-step output-name path must project to the same window
    // point as the window-local path, independently of initial placement.
    let moved = bus.call(
        SERVICE,
        "comp.input.pointer.move",
        json!({"seat":"agent","output":"headless-0","x":output_x+1.0,"y":output_y+1.0}),
    );
    assert_eq!(moved["pointer"]["x"], output_x + 1.0);
    probe.wait_for("PROBE seat \"agent\" motion 41 51");
    bus.call(
        SERVICE,
        "comp.input.pointer.button",
        json!({"seat":"agent","action":"click"}),
    );
    bus.call(SERVICE, "comp.input.key", json!({"seat":"agent","key":"d"}));
    bus.call(
        SERVICE,
        "comp.input.key",
        json!({"seat":"agent","text":"e"}),
    );
    probe.wait_for("PROBE seat \"agent\" key 32 0");
    probe.wait_for("PROBE seat \"agent\" key 18 0");
    for (x, y, expected_x, expected_y) in [(-100, 900, 0.0, 719.0), (9000, -20, 1279.0, 0.0)] {
        let reply = bus.call(
            SERVICE,
            "comp.input.pointer.move",
            json!({"seat":"agent","output":"headless-0","x":x,"y":y}),
        );
        assert_eq!(reply["pointer"]["x"], expected_x, "{reply}");
        assert_eq!(reply["pointer"]["y"], expected_y, "{reply}");
    }
    let unknown = bus.request(
        SERVICE,
        "comp.input.pointer.move",
        json!({"output":"unknown-0","x":0,"y":0}),
    );
    assert_eq!(unknown["body"]["error"], "unknown_output", "{unknown}");
    probe.no_human_input();
    println!(
        "sequence + single verbs: agent-only enter/motion/button/text; headless-0 bounds clamped"
    );
}

#[test]
#[ignore = "needs a GPU render node and built compd, noded and testkit-input-probe"]
fn agent_runs_survive_without_a_vt() {
    let bus = Bus::start("yield");
    let mut process = process::spawn(1, None, "yield", Some(&bus.url));
    let mut probe = Probe::start(&mut process, "headless-yield");
    bus.ready(SERVICE);
    let row = window(&bus, SERVICE, "headless-yield");
    // 200 taps inject 400 events, crossing SEQUENCE_YIELD_EVENTS (256).
    // A delayed final step also crosses a calloop timer dispatch.
    let mut steps =
        vec![json!({"verb":"comp.input.key","args":{"window":target(&row),"key":"a"}}); 200];
    steps.push(json!({"verb":"comp.input.key","args":{"key":"b"},"delay_ms":50}));
    let reply = bus.call(
        SERVICE,
        "comp.input.sequence",
        json!({"seat":"agent","steps":steps}),
    );
    assert_eq!(reply["steps"].as_array().unwrap().len(), 201, "{reply}");
    assert!(reply.get("input_cleared").is_none());
    assert!(reply.get("error").is_none(), "{reply}");
    probe.wait_for("PROBE seat \"agent\" key 48 0");
    assert_eq!(
        probe
            .text()
            .lines()
            .filter(|line| *line == "PROBE seat \"agent\" key 30 1")
            .count(),
        200
    );
    probe.no_human_input();
    assert!(process.child.try_wait().unwrap().is_none());
    println!(
        "201 steps / 402 key events completed without a VT, through yield and delay; no input_cleared"
    );
}

#[test]
#[ignore = "needs a GPU render node and built compd, noded and testkit clients"]
fn lock_clears_agent_input() {
    let bus = Bus::start("lock");
    let mut process = process::spawn(1, None, "lock", Some(&bus.url));
    let mut probe = Probe::start(&mut process, "headless-lock");
    bus.ready(SERVICE);
    let row = window(&bus, SERVICE, "headless-lock");
    thread::scope(|scope| {
        let run = scope.spawn(|| bus.request(SERVICE, "comp.input.sequence", json!({
            "seat":"agent", "steps":[
                {"verb":"comp.input.key","args":{"window":target(&row),"key":"a"}},
                {"verb":"comp.input.key","args":{"key":"b"},"delay_ms":3000}
            ]
        })));
        // The first event proves admission and that the delayed run is active.
        probe.wait_for("PROBE seat \"agent\" key 30 0");
        assert!(!run.is_finished(), "sequence must still be waiting when locked");
        let log = process.root.join("lock.log");
        let file = fs::File::create(&log).unwrap();
        let child = Command::new(binary("testkit-lock-client"))
            .args(["--hold", "30"])
            .env("XDG_RUNTIME_DIR", &process.root)
            .env("WAYLAND_DISPLAY", "wayland-headless-test")
            .stdout(Stdio::from(file.try_clone().unwrap()))
            .stderr(file)
            .spawn()
            .unwrap();
        let mut lock = Probe { child, log };
        lock.wait_for("locked");
        let reply = run.join().unwrap();
        assert_eq!(reply["body"]["error"], "input_cleared", "{reply}");
        assert_ne!(reply["rc"], 0, "{reply}");
        // Allow focus-leave cleanup to reach the probe, then wait beyond the
        // cancelled step's original deadline: no agent event may follow it.
        thread::sleep(Duration::from_millis(200));
        let agent_events = |text: String| text.lines()
            .filter(|line| line.starts_with("PROBE seat \"agent\" "))
            .map(str::to_owned).collect::<Vec<_>>();
        let before = agent_events(probe.text());
        thread::sleep(Duration::from_millis(3200));
        assert_eq!(agent_events(probe.text()), before);
        assert!(!probe.text().contains("PROBE seat \"agent\" key 48 "));
        assert_eq!(unsafe { libc::kill(lock.child.id() as i32, libc::SIGUSR1) }, 0);
        lock.wait_for("unlocked");
        let reply = bus.call(SERVICE, "comp.input.sequence", json!({
            "seat":"agent", "steps":[
                {"verb":"comp.input.key","args":{"window":target(&row),"key":"c"}}
            ]
        }));
        assert_eq!(reply["steps"].as_array().unwrap().len(), 1, "{reply}");
        probe.wait_for("PROBE seat \"agent\" key 46 0");
    });
    probe.no_human_input();
}

#[test]
#[ignore = "needs a GPU render node and built compd, noded and testkit-input-probe"]
fn scale_two_centres_clamps_and_delivers_logical_coordinates() {
    let bus = Bus::start("scale-two");
    let mut process = process::spawn(2, None, "scale-two", Some(&bus.url));
    let mut probe = Probe::start(&mut process, "headless-scale-two");
    bus.ready(SERVICE);
    let row = window(&bus, SERVICE, "headless-scale-two");
    let seats = bus.call(SERVICE, "comp.props.get", json!({"path":"input.seats"}));
    assert_eq!(seats["agent"]["pointer"]["x"], 320.0, "{seats}");
    assert_eq!(seats["agent"]["pointer"]["y"], 180.0, "{seats}");
    let clamped = bus.call(SERVICE, "comp.input.pointer.move", json!({
        "seat":"agent", "output":"headless-0", "x":10000, "y":10000
    }));
    assert_eq!(clamped["pointer"]["x"], 639.0, "{clamped}");
    assert_eq!(clamped["pointer"]["y"], 359.0, "{clamped}");
    // Leave the surface before asserting its next enter. Output positions
    // and the probe's local positions must both stay logical at scale two.
    bus.call(SERVICE, "comp.input.pointer.move", json!({
        "seat":"agent", "output":"headless-0", "x":0, "y":0
    }));
    let x = row["window_x"].as_f64().unwrap() + 40.0;
    let y = row["window_y"].as_f64().unwrap() + 50.0;
    bus.call(SERVICE, "comp.input.pointer.move", json!({
        "seat":"agent", "output":"headless-0", "x":x, "y":y
    }));
    probe.wait_for("PROBE seat \"agent\" enter 40 50");
    bus.call(SERVICE, "comp.input.pointer.move", json!({
        "seat":"agent", "output":"headless-0", "x":x+1.0, "y":y+1.0
    }));
    probe.wait_for("PROBE seat \"agent\" motion 41 51");
    probe.no_human_input();
}

#[test]
#[ignore = "needs a GPU render node and built compd, noded and testkit-input-probe"]
fn two_instances_register_distinct_services() {
    let bus = Bus::start("two");
    let mut first = process::spawn(1, None, "two-a", Some(&bus.url));
    let mut second = process::spawn_service("two-b", &bus.url, "comp-rec");
    let _observer = bus.observe();
    let counts = || bus.call("headless-observer", "observer.count", json!({}));
    assert_eq!(counts(), json!({"first":0,"second":0}));
    let mut a = Probe::start(&mut first, "headless-first");
    let wait_counts = |expected: Value| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let actual = counts();
            if actual == expected {
                break;
            }
            assert!(Instant::now() < deadline, "topic counts {actual}, expected {expected}");
            thread::sleep(Duration::from_millis(20));
        }
        // Give an erroneously cross-published edge time to reach the other
        // subscription, then require silence there as well as the own edge.
        thread::sleep(Duration::from_millis(200));
        assert_eq!(counts(), expected);
    };
    wait_counts(json!({"first":1,"second":0}));
    let b = Probe::start(&mut second, "headless-second");
    wait_counts(json!({"first":1,"second":1}));
    for service in [SERVICE, "comp-rec"] {
        bus.ready(service);
    }
    let row_a = window(&bus, SERVICE, "headless-first");
    let row_b = window(&bus, "comp-rec", "headless-second");
    let info_a = bus.call(SERVICE, "comp.props.get", json!({"path":"info"}));
    let info_b = bus.call("comp-rec", "comp.props.get", json!({"path":"info"}));
    assert_eq!(info_a["service"], SERVICE);
    assert_eq!(info_b["service"], "comp-rec");
    assert!(info_a["instance"].is_string(), "{info_a}");
    assert_ne!(info_a["instance"], info_b["instance"]);
    // Focus names come from the same resolver used by the transport. An
    // empty human seat has no keyboard focus edge to publish; mapped and
    // session topics prove the live publisher namespaces on the broker.
    assert_eq!(
        topic_name(SERVICE, FOCUS_TOPIC_SUFFIX),
        "comp-headless.focus.changed"
    );
    assert_eq!(
        topic_name("comp-rec", FOCUS_TOPIC_SUFFIX),
        "comp-rec.focus.changed"
    );
    assert_ne!(
        topic_name(SERVICE, FOCUS_TOPIC_SUFFIX),
        topic_name("comp-rec", FOCUS_TOPIC_SUFFIX)
    );
    let topics = bus.call("noded", "topic.list", json!({}));
    println!("broker topics: {topics}");
    for service in [SERVICE, "comp-rec"] {
        for suffix in ["surface.mapped", "session.changed"] {
            let expected = format!("{service}.{suffix}");
            assert!(
                topics
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["name"] == expected && row["last_publisher"] == service),
                "{topics}"
            );
        }
    }
    bus.call(
        SERVICE,
        "comp.input.key",
        json!({"window":target(&row_a),"text":"a"}),
    );
    a.wait_for("PROBE seat \"agent\" key 30 0");
    assert!(!b.text().contains("PROBE seat \"agent\" key "));
    let before_b = bus.call("comp-rec", "comp.window.stats", target(&row_b));
    let reset = bus.call(SERVICE, "comp.window.stats.reset", target(&row_a));
    let after_b = bus.call("comp-rec", "comp.window.stats", target(&row_b));
    assert!(before_b["since_us"].is_number(), "{before_b}");
    assert_eq!(before_b["since_us"], after_b["since_us"]);
    assert_ne!(after_b["since_us"], reset["since_us"]);
    assert!(after_b["presented"].as_u64().unwrap() >= before_b["presented"].as_u64().unwrap());
    window(&bus, SERVICE, "headless-first");
    window(&bus, "comp-rec", "headless-second");
    assert!(first.child.try_wait().unwrap().is_none());
    assert!(second.child.try_wait().unwrap().is_none());
    println!(
        "comp-headless and comp-rec: distinct instance/focus topics, windows and input; both alive"
    );
}

#[test]
#[ignore = "needs a GPU render node and built compd, noded and testkit-input-probe"]
fn human_seat_has_no_sources() {
    let bus = Bus::start("human");
    let mut process = process::spawn(1, None, "human", Some(&bus.url));
    let probe = Probe::start(&mut process, "headless-seats");
    bus.ready(SERVICE);
    let caps = seats::capabilities(&process.root);
    assert_eq!(caps.len(), 2);
    assert!(
        caps["seat0"].is_empty(),
        "seat0 advertised capabilities: {caps:?}"
    );
    assert_eq!(
        caps["agent"],
        wayland_client::protocol::wl_seat::Capability::Pointer
            | wayland_client::protocol::wl_seat::Capability::Keyboard
    );
    let text = probe.text();
    assert!(
        text.contains("PROBE seat \"seat0\" bound keyboard=0 pointer=0 "),
        "{text}"
    );
    assert!(
        text.contains("PROBE seat \"agent\" bound keyboard=1 pointer=1 "),
        "{text}"
    );
    for (verb, args) in [
        (
            "comp.input.pointer.move",
            json!({"seat":"human","output":"headless-0","x":20,"y":20}),
        ),
        (
            "comp.input.pointer.button",
            json!({"seat":"human","action":"click"}),
        ),
        ("comp.input.key", json!({"seat":"human","key":"a"})),
        ("comp.input.key", json!({"seat":"human","text":"abc"})),
        ("comp.input.pointer.scroll", json!({"seat":"human","dy":10})),
    ] {
        let reply = bus.request(SERVICE, verb, args);
        assert_ne!(reply["rc"], 0, "{reply}");
        assert_eq!(reply["body"]["error"], "seat_unavailable", "{reply}");
        assert_eq!(reply["body"]["seat"], "human", "{reply}");
        assert_eq!(reply["body"]["reason"], "no_sources", "{reply}");
    }
    probe.no_human_input();
    assert!(process.child.try_wait().unwrap().is_none());
    bus.call(SERVICE, "comp.windows.list", json!({}));
    println!(
        "seat0: no pointer/keyboard; agent: both; five human verbs refused seat_unavailable and compd stayed up"
    );
}
