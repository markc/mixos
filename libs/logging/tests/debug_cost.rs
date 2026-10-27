// SPDX-License-Identifier: MIT OR Apache-2.0
//! Isolated-process resource checks and a reproducible release microbenchmark.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tracing::level_filters::LevelFilter;
use tracing_log::AsLog;

struct Counting;
static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn child(name: &str, ignored: bool) -> bool {
    if std::env::var_os("MIXOS_DEBUG_COST_CHILD").is_some() {
        return true;
    }
    for initial in if ignored {
        &["warn"][..]
    } else {
        &["warn", "off", "trace"][..]
    } {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", name, "--nocapture"]);
        if ignored {
            command.arg("--ignored");
        }
        let output = command
            .env("MIXOS_DEBUG_COST_CHILD", initial)
            .env_remove("RUST_LOG")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    false
}
fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}
fn sockets() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter(|path| path.as_os_str().as_encoded_bytes().starts_with(b"socket:"))
        .count()
}
fn set(level: &str) {
    let body = serde_json::json!({"level":level}).to_string();
    assert_eq!(
        logging::debug::request("fixture", "fixture.debug.set", &body)
            .unwrap()
            .0,
        0
    );
}
fn disabled_site(window_id: u64) {
    tracing::debug!(target: "logging", window_id, generation = 7_u64, app_id = "example.app", command = "window.place", result = "ok", x = 10, y = 20, "window placed");
}

#[test]
fn default_resources_and_reload_hints() {
    if !child("default_resources_and_reload_hints", false) {
        return;
    }
    let mut args = vec!["fixture".into()];
    let initial = std::env::var("MIXOS_DEBUG_COST_CHILD").unwrap();
    if initial != "warn" {
        args.push(format!("--debug={initial}"));
    }
    let before_threads = threads();
    let before_sockets = sockets();
    let _guard = logging::debug::init("fixture", "warn", &mut args).unwrap();
    assert_eq!(threads(), before_threads);
    if initial != "trace" {
        assert_eq!(sockets(), before_sockets, "journald must be lazy");
    }
    assert_eq!(
        LevelFilter::current(),
        initial.parse::<LevelFilter>().unwrap()
    );
    assert_eq!(log::max_level(), LevelFilter::current().as_log());
    assert_eq!(
        tracing::enabled!(target: "logging", tracing::Level::DEBUG),
        initial == "trace"
    );
    assert_eq!(
        tracing::enabled!(target: "logging", tracing::Level::TRACE),
        initial == "trace"
    );
    let span = tracing::warn_span!("startup", generation = 1, phase = tracing::field::Empty);
    let _entered = span.enter();
    span.record("phase", "ready");
    if initial != "trace" {
        assert_eq!(sockets(), before_sockets, "spans must not connect journald");
    }
    for level in [
        "warn", "off", "error", "warn", "info", "debug", "trace", "debug", "info", "warn", "error",
        "off",
    ] {
        set(level);
        if level == "debug" && std::path::Path::new("/run/systemd/journal/socket").exists() {
            assert_eq!(
                sockets(),
                before_sockets + 1,
                "first admitted event opens exactly one journal socket"
            );
        }
        assert_eq!(
            LevelFilter::current(),
            level.parse::<LevelFilter>().unwrap()
        );
        assert_eq!(log::max_level(), level.parse::<log::LevelFilter>().unwrap());
        assert_eq!(
            tracing::enabled!(target: "logging", tracing::Level::DEBUG),
            matches!(level, "debug" | "trace")
        );
        assert_eq!(
            tracing::enabled!(target: "logging", tracing::Level::TRACE),
            level == "trace"
        );
    }
    set("trace");
    assert_eq!(log::max_level(), log::LevelFilter::Trace);
    assert_eq!(
        logging::debug::request("fixture", "fixture.debug.set", r#"{"on":false}"#)
            .unwrap()
            .0,
        0
    );
    assert_eq!(log::max_level(), log::LevelFilter::Warn);
    for level in ["warn", "off"] {
        set(level);
        ALLOCATIONS.store(0, Ordering::Relaxed);
        COUNTING.store(true, Ordering::SeqCst);
        for id in 0..10_000 {
            disabled_site(std::hint::black_box(id));
            log::trace!(target: "wgpu_core", "disabled dependency {}", std::hint::black_box(id));
        }
        COUNTING.store(false, Ordering::SeqCst);
        assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), 0);
    }
    assert_eq!(threads(), before_threads);
    println!(
        "initial {initial}: threads before/after: {before_threads}/{}; disabled debug allocations: 0 at warn and off; initial socket delta: 0",
        threads()
    );
}

#[test]
#[ignore = "CBC release microbenchmark; run with --release --ignored --nocapture"]
fn disabled_debug_benchmark() {
    if !child("disabled_debug_benchmark", true) {
        return;
    }
    let _guard = logging::debug::init("fixture", "warn", &mut vec!["fixture".into()]).unwrap();
    const N: u64 = 20_000_000;
    for sample in 1..=5 {
        let start = std::time::Instant::now();
        for id in 0..N {
            std::hint::black_box(id);
        }
        let empty = start.elapsed().as_nanos() as f64 / N as f64;
        let start = std::time::Instant::now();
        for id in 0..N {
            disabled_site(std::hint::black_box(id));
        }
        let event = start.elapsed().as_nanos() as f64 / N as f64;
        println!(
            "sample {sample}: disabled debug {event:.3} ns/event; empty {empty:.3} ns/iteration; delta {:.3} ns",
            event - empty
        );
    }
}
