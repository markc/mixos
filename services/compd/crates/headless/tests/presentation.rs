// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Independent-rate probe against an owned render-node compd. Run with
//! --nocapture to retain the baseline and positive-control summary lines.
use std::{
    fs,
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[path = "support/process.rs"]
mod process;

fn measurement(process: &mut process::Process, stalled: bool) -> (f64, usize) {
    let probe = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("testkit-presentation-probe");
    assert!(probe.is_file(), "build testkit-presentation-probe first");
    let mut command = Command::new(probe);
    command
        .args([
            "--rate",
            "60",
            "--hold-s",
            "60",
            "--expect-output",
            "headless-0",
        ])
        .env("XDG_RUNTIME_DIR", &process.root)
        .env("WAYLAND_DISPLAY", "wayland-headless-test");
    if stalled {
        command.args(["--stall-every", "30", "--stall-ms", "50"]);
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    println!("{stdout}");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary = stdout
        .lines()
        .find(|line| line.starts_with("COMPD_PRESENTATION_PROBE PASS "))
        .unwrap();
    let value = |key: &str| {
        summary
            .split_whitespace()
            .find_map(|field| field.strip_prefix(key))
            .unwrap()
    };
    assert_eq!(value("commits="), "3600");
    let achieved: f64 = value("achieved_hz=").parse().unwrap();
    assert!((achieved / 60.0 - 1.0).abs() <= 0.01, "{summary}");
    (achieved, value("missed=").parse().unwrap())
}

#[test]
#[ignore = "needs a render node, built headless compd and testkit-presentation-probe; two 60 s runs"]
fn independent_60hz_and_stall_control() {
    let mut process = process::spawn(1, None, "presentation", None);
    process.extend_watchdog(Duration::from_secs(180));
    let deadline = Instant::now() + Duration::from_secs(30);
    while !process.root.join("wayland-headless-test").exists() {
        assert!(
            process.child.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string(process.root.join("compd.log")).unwrap()
        );
        assert!(Instant::now() < deadline, "compd socket startup timed out");
        thread::sleep(Duration::from_millis(10));
    }
    let (hz, clean) = measurement(&mut process, false);
    let (stall_hz, stalled) = measurement(&mut process, true);
    println!(
        "60 s baseline achieved_hz={hz} missed={clean}; stall achieved_hz={stall_hz} missed={stalled}"
    );
    // The baseline bound is set from the first observed worker run, before
    // this test is accepted; see the follow-up evidence commit.
    assert!(stalled > clean, "stall injection must raise missed");
}
