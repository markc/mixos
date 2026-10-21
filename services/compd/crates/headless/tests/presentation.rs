// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Independent-rate probe against an owned render-node compd. Run with
//! --nocapture to retain the baseline and positive-control summary lines.
//! For timing, select an optimised compd through COMPD_TEST_BINARY. The
//! harness and probe need not be optimised; buffer preparation precedes the
//! timer deadline. A debug compositor's render time counts towards misses.
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
    assert_eq!(value("stalls="), if stalled { "120" } else { "0" });
    let achieved: f64 = value("achieved_hz=").parse().unwrap();
    assert!((achieved / 60.0 - 1.0).abs() <= 0.01, "{summary}");
    if !stalled {
        let presented: usize = value("presented=").parse().unwrap();
        let presented_hz: f64 = value("presented_hz=").parse().unwrap();
        let cadence_missed: usize = value("cadence_missed=").parse().unwrap();
        assert!(presented >= 3540, "{summary}");
        assert!((presented_hz / 60.0 - 1.0).abs() <= 0.02, "{summary}");
        // Optimised 60 s calibration: 3576 presentations at 59.599805 Hz,
        // with one gap greater than 1.5 intervals. Allow at most 1% (36)
        // such gaps; injected stalls are checked separately.
        assert!(cadence_missed <= 36, "{summary}");
    }
    (
        achieved,
        value("missed=").parse().unwrap(),
        value("cadence_missed=").parse().unwrap(),
    )
}

#[test]
#[ignore = "needs a render node, built headless compd and testkit-presentation-probe; two 60 s runs"]
fn independent_60hz_and_stall_control() {
    let mut process = process::spawn_measurement();
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
    let (hz, clean, clean_cadence) = measurement(&mut process, false);
    let (stall_hz, stalled, stalled_cadence) = measurement(&mut process, true);
    println!(
        "60 s baseline achieved_hz={hz} missed={clean} cadence_missed={clean_cadence}; \
         stall achieved_hz={stall_hz} missed={stalled} cadence_missed={stalled_cadence}"
    );
    // The client-deadline `missed` is reported, never asserted: two
    // free-running 60 Hz clocks beat against it, and 60 s runs on the same
    // worker have shown 328, 395 and 774 depending on their phase. Presentation
    // cadence is the compositor's own lateness metric: the baseline is checked
    // in `measurement`, and every injected 50 ms stall (120 per run, each
    // longer than 1.5 intervals) must show up there.
    assert!(
        stalled_cadence >= 100,
        "stall injection must raise cadence_missed: {stalled_cadence}"
    );
}
