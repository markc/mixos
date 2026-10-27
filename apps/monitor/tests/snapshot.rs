// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Offscreen snapshots of Monitor driven through its real engine: a
//! four-core node with a disk, an interface, a minute of history and a
//! process tree. Regenerate with `UPDATE_SNAPSHOTS=1` and look at the images
//! before committing them.
//!
//! Performance workload (AGENTS.md §4.1): [`a_large_table_and_core_grid`]
//! draws a 2,000-row table and a 64-core grid; its timing is printed, not
//! asserted, so the figure is recorded with the run.
use design::{DesignContext, Mode, Scheme};
use egui_kittest::Harness;
use monitor::{actions, label, strings, view::view};
use serde_json::{Value, json};
use toolkit::Theme;
use vitals::{Effect, Engine, Fetch, Tab};

/// A deterministic minute: a slow wave per series.
fn wave(i: usize, phase: f64, low: f64, high: f64) -> f64 {
    let t = i as f64 / 9.0 + phase;
    low + (high - low) * (0.5 + 0.5 * t.sin())
}

fn snapshot(cores: usize, busy: f64) -> Value {
    json!({
        "host": "alpha", "ts": 1_700_000_000.0, "interval_s": 1.0, "uptime_s": 266_520.0,
        "cpu": {"model": "Example 16-Core Processor", "cores": cores, "busy": busy,
                "user": busy * 0.7, "system": busy * 0.3, "iowait": 0.4,
                "per_core": (0..cores).map(|c| json!({"cpu": c, "busy": wave(c, 0.0, 5.0, 80.0), "mhz": 3400 + c as u64 * 10})).collect::<Vec<_>>()},
        "memory": {"total": 34_359_738_368u64, "available": 21_474_836_480u64, "used": 12_884_901_888u64, "cached": 8_589_934_592u64},
        "swap": {"total": 8_589_934_592u64, "used": 268_435_456u64},
        "load": {"one": 1.25, "five": 0.98, "fifteen": 0.71},
        "tasks": {"total": 812, "running": 2, "blocked": 0},
        "disks": [{"name": "nvme0n1", "model": "Example NVMe 2TB", "rotational": false,
                   "read_bps": 1_258_291.0, "write_bps": 524_288.0, "busy": 6.5}],
        "filesystems": [{"mount": "/", "device": "/dev/nvme0n1p2", "kind": "btrfs",
                         "total": 1_000_000_000_000u64, "used": 412_000_000_000u64}],
        "net": [{"name": "eth0", "state": "up", "physical": true, "speed_mbps": 1000,
                 "rx_bps": 310_000.0, "tx_bps": 42_000.0, "rx_bytes": 9_876_543_210u64, "tx_bytes": 1_234_567_890u64}],
    })
}

fn table(rows: usize) -> Value {
    let names = [
        "systemd",
        "compd",
        "monitor",
        "firefox",
        "Web Content",
        "noded",
        "term",
        "mix",
    ];
    let rows: Vec<Value> = (0..rows)
        .map(|i| {
            let depth = if i == 0 { 0 } else { 1 + (i % 3 == 2) as u32 };
            json!({
                "pid": 1 + i as u32 * 7, "ppid": 1, "name": names[i % names.len()],
                "cmdline": format!("/usr/bin/{} --example {i}", names[i % names.len()].to_lowercase().replace(' ', "-")),
                "user": if i % 4 == 0 { "root" } else { "alice" }, "uid": 1000,
                "cpu": wave(i, 1.0, 0.0, 24.0), "memory": 50_000_000u64 + i as u64 * 7_340_032,
                "read_bps": if i % 4 == 0 { Value::Null } else { json!(i as f64 * 1024.0) },
                "write_bps": if i % 4 == 0 { Value::Null } else { json!(0.0) },
                "depth": depth,
            })
        })
        .collect();
    json!({"host": "alpha", "total": rows.len(), "count": rows.len(), "rows": rows})
}

/// An engine that has read `alpha` for a minute.
fn engine(cores: usize, rows: usize, tab: Tab) -> Engine {
    let mut e = Engine::new(label);
    let tickets: Vec<(Fetch, u64)> = e
        .take_effects()
        .into_iter()
        .filter_map(|x| match x {
            Effect::Fetch { kind, ticket, .. } => Some((kind, ticket)),
            _ => None,
        })
        .collect();
    let ticket = |kind| tickets.iter().find(|(k, _)| *k == kind).unwrap().1;
    // A minute of history, as monitord.history would seed it.
    let series = |name: &str, phase: f64, low: f64, high: f64| {
        (
            name.to_owned(),
            json!(
                (0..60)
                    .map(|i| json!([i, wave(i, phase, low, high), 0, 0]))
                    .collect::<Vec<_>>()
            ),
        )
    };
    let mut seeded = serde_json::Map::new();
    for (name, values) in [
        series("cpu.busy", 0.0, 8.0, 46.0),
        series("mem.used", 1.0, 1.1e10, 1.4e10),
        series("disk.nvme0n1.busy", 2.0, 0.0, 20.0),
        series("disk.nvme0n1.read", 0.5, 0.0, 3.0e6),
        series("disk.nvme0n1.write", 1.5, 0.0, 1.0e6),
        series("net.eth0.rx", 0.2, 1.0e4, 6.0e5),
        series("net.eth0.tx", 2.2, 1.0e3, 9.0e4),
    ] {
        seeded.insert(name, values);
    }
    for c in 0..cores {
        let (name, values) = series(&format!("cpu.core.{c}.busy"), c as f64 * 0.7, 2.0, 90.0);
        seeded.insert(name, values);
    }
    e.fetched(
        Fetch::History,
        ticket(Fetch::History),
        Ok(json!({"series": seeded})),
    );
    e.fetched(
        Fetch::Snapshot,
        ticket(Fetch::Snapshot),
        Ok(snapshot(cores, 27.4)),
    );
    e.fetched(Fetch::Processes, ticket(Fetch::Processes), Ok(table(rows)));
    e.fetched(
        Fetch::Peers,
        ticket(Fetch::Peers),
        Ok(json!({"node": "alpha", "peers": [{"name": "beta"}, {"name": "gamma"}]})),
    );
    e.set_tab(tab);
    e.take_effects();
    e
}

fn render(
    theme: Theme,
    name: &str,
    mut engine: Engine,
    section: Option<&str>,
    selected: Option<u32>,
) {
    if let Some(section) = section {
        engine.set_section(section).unwrap();
    }
    engine.ui.selected = selected;
    let registry = actions::registry();
    let strings = strings();
    let initial_theme = theme.clone();
    let mut harness = Harness::builder()
        .with_context_setup(move |ctx| toolkit::install(ctx, &initial_theme))
        .with_size(egui::vec2(1040.0, 680.0))
        .wgpu()
        .build_ui(move |ui| {
            let _ = view(ui, &engine, &registry, &strings);
        });
    toolkit::install(&harness.ctx, &theme);
    harness.run();
    harness.snapshot(name);
}

fn dark() -> Theme {
    Theme::embedded()
}

fn light() -> Theme {
    Theme::for_context(DesignContext::revision_one())
}

#[test]
fn processes_dark() {
    render(
        dark(),
        "processes_dark",
        engine(4, 40, Tab::Processes),
        None,
        Some(22),
    );
}

#[test]
fn processes_light() {
    render(
        light(),
        "processes_light",
        engine(4, 40, Tab::Processes),
        None,
        Some(22),
    );
}

#[test]
fn performance_cpu_dark() {
    render(
        dark(),
        "performance_cpu_dark",
        engine(16, 10, Tab::Performance),
        Some("cpu"),
        None,
    );
}

#[test]
fn performance_memory_light() {
    render(
        light(),
        "performance_memory_light",
        engine(4, 10, Tab::Performance),
        Some("memory"),
        None,
    );
}

#[test]
fn performance_disk_dark() {
    render(
        dark(),
        "performance_disk_dark",
        engine(4, 10, Tab::Performance),
        Some("disk:nvme0n1"),
        None,
    );
}

#[test]
fn performance_net_light() {
    render(
        light(),
        "performance_net_light",
        engine(4, 10, Tab::Performance),
        Some("net:eth0"),
        None,
    );
}

#[test]
fn performance_pages_fit_the_snapshot_pane_including_the_cpu_figures() {
    for theme in [dark(), light()] {
        for section in ["cpu", "memory", "disk:nvme0n1", "net:eth0"] {
            let mut engine = engine(16, 10, Tab::Performance);
            engine.set_section(section).unwrap();
            let registry = actions::registry();
            let strings = strings();
            let initial_theme = theme.clone();
            let mut h = Harness::builder()
                .with_context_setup(move |ctx| toolkit::install(ctx, &initial_theme))
                .with_size(egui::vec2(1040.0, 680.0))
                .build_ui_state(
                    move |ui, layout| {
                        let _ = view(ui, &engine, &registry, &strings);
                        *layout = monitor::view::performance_layout(ui.ctx());
                    },
                    None::<monitor::view::PerformanceLayout>,
                );
            h.run();
            let layout = h.state().as_ref().expect("Performance was rendered");
            assert!(
                layout.content_size.y <= layout.pane_size.y + 0.5,
                "{section} overflows vertically: {layout:?}"
            );
            assert!(
                layout.content_size.x <= layout.pane_size.x + 0.5,
                "{section} overflows horizontally: {layout:?}"
            );
        }
    }
}

/// Monitor's default look is the session's: Studio Dark when no theme is
/// chosen, the look the dark snapshots pin.
#[test]
fn the_default_theme_is_studio_dark() {
    let embedded = Theme::embedded();
    assert_eq!(
        (embedded.scheme(), embedded.mode()),
        (Scheme::Studio, Mode::Dark)
    );
}

/// The representative workload: a 2,000-process table and a 64-core grid,
/// ten frames each, timed. Printed, not asserted.
#[test]
fn a_large_table_and_core_grid() {
    for (name, engine) in [
        ("2000-process table", engine(4, 2000, Tab::Processes)),
        ("64-core grid", engine(64, 10, Tab::Performance)),
    ] {
        let registry = actions::registry();
        let strings = strings();
        let theme = dark();
        let initial_theme = theme.clone();
        let mut harness = Harness::builder()
            .with_context_setup(move |ctx| toolkit::install(ctx, &initial_theme))
            .with_size(egui::vec2(1040.0, 680.0))
            .build_ui(move |ui| {
                let _ = view(ui, &engine, &registry, &strings);
            });
        toolkit::install(&harness.ctx, &theme);
        harness.run();
        let start = std::time::Instant::now();
        for _ in 0..10 {
            harness.step();
        }
        let per_frame = start.elapsed() / 10;
        eprintln!("workload {name}: {per_frame:?} per frame (layout and tessellation, no GPU)");
    }
}
