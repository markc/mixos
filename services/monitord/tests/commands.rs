// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Every command's reply shape, against a fixture `/proc` and `/sys` tree, so
//! the public contract is pinned without a broker or a real host.
use monitord::engine::{Config, Engine};
use monitord::history::Retention;
use monitord::kernel::Root;
use monitord::service::dispatch;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn stat_line(pid: u32, comm: &str, ppid: u32, ticks: u64) -> String {
    format!(
        "{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 4194560 100 0 0 0 {ticks} 0 0 0 20 0 1 0 100 1048576 256 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0"
    )
}

/// A two-core host with three processes, one disk and one interface.
fn host(root: &Path, busy: u64) {
    write(root, "proc/sys/kernel/hostname", "fixture\n");
    write(root, "proc/cpuinfo", "model name\t: Test CPU\n");
    write(
        root,
        "proc/stat",
        &format!(
            "cpu  {busy} 0 0 {} 0 0 0 0 0 0\ncpu0 {busy} 0 0 1000 0 0 0 0 0 0\ncpu1 0 0 0 1000 0 0 0 0 0 0\nctxt 10\nbtime 1700000000\nprocesses 5\nprocs_running 1\nprocs_blocked 0\n",
            2000
        ),
    );
    write(
        root,
        "proc/meminfo",
        "MemTotal: 2000000 kB\nMemFree: 500000 kB\nMemAvailable: 1500000 kB\nCached: 400000 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n",
    );
    write(root, "proc/loadavg", "0.10 0.20 0.30 1/50 999\n");
    write(root, "proc/uptime", "3600.5 7000.0\n");
    write(
        root,
        "proc/pressure/cpu",
        "some avg10=2.00 avg60=1.00 avg300=0.50 total=1\n",
    );
    write(
        root,
        "proc/diskstats",
        "   8 0 sda 100 0 2000 10 50 0 4000 20 0 30 40\n   7 0 loop0 1 0 1 0 0 0 0 0 0 0 0\n",
    );
    std::fs::create_dir_all(root.join("sys/block/sda")).unwrap();
    write(
        root,
        "proc/net/dev",
        "Inter-| head\n face | head\n    lo: 1 1 0 0 0 0 0 0 1 1 0 0 0 0 0 0\n  eth0: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0\n",
    );
    write(root, "sys/class/net/eth0/operstate", "up\n");
    write(root, "proc/self/mounts", "");
    // The fixture files belong to whoever runs the test.
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    write(
        root,
        "etc/passwd",
        &format!("root:x:0:0::/root:/bin/sh\ntester:x:{uid}:{uid}::/:/bin/sh\n"),
    );
    for (pid, comm, ppid) in [(1, "init", 0), (100, "shell", 1), (200, "worker (x)", 100)] {
        write(
            root,
            &format!("proc/{pid}/stat"),
            &stat_line(pid, comm, ppid, busy),
        );
        write(
            root,
            &format!("proc/{pid}/cmdline"),
            &format!("{comm}\0--flag\0"),
        );
        write(
            root,
            &format!("proc/{pid}/cgroup"),
            "0::/system.slice/fixture.service\n",
        );
        write(
            root,
            &format!("proc/{pid}/status"),
            "State:\tS (sleeping)\nVmHWM:\t 2048 kB\n",
        );
    }
}

async fn engine(root: &Path) -> Arc<Engine> {
    host(root, 100);
    let engine = Engine::new(Config {
        root: Root::new(root),
        interval: Duration::from_millis(200),
        db: Some(root.join("var/history.db")),
        retention: Retention::default(),
    });
    let e = engine.clone();
    tokio::task::spawn_blocking(move || e.tick()).await.unwrap();
    // The CPU did 100 more ticks of user time on core 0.
    host(root, 200);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let e = engine.clone();
    tokio::task::spawn_blocking(move || e.tick()).await.unwrap();
    engine
}

async fn ok(engine: &Arc<Engine>, command: &str, body: Value) -> Value {
    dispatch(engine, command, &body)
        .await
        .unwrap_or_else(|e| panic!("{command}: {e}"))
}

#[tokio::test]
async fn snapshot_reports_the_fixture_host() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path()).await;
    let s = ok(&engine, "monitord.snapshot", json!({})).await;
    assert_eq!(s["host"], "fixture");
    assert_eq!(s["cpu"]["model"], "Test CPU");
    assert_eq!(s["cpu"]["cores"], 2);
    // 100 busy ticks of the 100 that passed on core 0; core 1 idle.
    assert_eq!(s["cpu"]["per_core"][0]["busy"], 100.0);
    assert_eq!(s["cpu"]["per_core"][1]["busy"], 0.0);
    assert_eq!(s["memory"]["used"], 500_000 * 1024);
    assert_eq!(s["load"]["one"], 0.1);
    assert_eq!(s["pressure"]["cpu"]["some"]["avg10"], 2.0);
    assert_eq!(
        s["disks"].as_array().unwrap().len(),
        1,
        "loop0 is not a disk"
    );
    assert_eq!(s["disks"][0]["name"], "sda");
    assert_eq!(s["net"][0]["name"], "eth0");
    assert_eq!(s["net"][0]["state"], "up");
}

#[tokio::test]
async fn processes_sort_filter_and_tree() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path()).await;
    let all = ok(&engine, "monitord.processes", json!({"sort": "pid"})).await;
    assert_eq!(all["total"], 3);
    assert_eq!(all["rows"][2]["name"], "worker (x)");
    assert_eq!(all["rows"][2]["unit"], "fixture.service");
    assert_eq!(all["rows"][0]["user"], "tester");
    let tree = ok(
        &engine,
        "monitord.processes",
        json!({"tree": true, "filter": "worker"}),
    )
    .await;
    let path: Vec<(u64, u64)> = tree["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["pid"].as_u64().unwrap(), r["depth"].as_u64().unwrap()))
        .collect();
    assert_eq!(path, [(1, 0), (100, 1), (200, 2)]);
    let refused = dispatch(&engine, "monitord.processes", &json!({"sort": "colour"})).await;
    assert_eq!(refused.unwrap_err()["status"], "bad_request");
    let unknown = dispatch(&engine, "monitord.processes", &json!({"bogus": 1})).await;
    assert_eq!(unknown.unwrap_err()["status"], "bad_request");
}

#[tokio::test]
async fn one_process_in_full() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path()).await;
    let p = ok(&engine, "monitord.process", json!({"pid": 100})).await;
    assert_eq!(p["name"], "shell");
    assert_eq!(p["cmdline"], "shell --flag");
    assert_eq!(p["children"], json!([200]));
    assert_eq!(p["memory_peak"], 2048 * 1024);
    let missing = dispatch(&engine, "monitord.process", &json!({"pid": 4242})).await;
    assert_eq!(missing.unwrap_err()["status"], "not_found");
    let no_pid = dispatch(&engine, "monitord.process", &json!({})).await;
    assert_eq!(no_pid.unwrap_err()["status"], "bad_request");
}

#[tokio::test]
async fn history_comes_from_the_live_ring() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path()).await;
    let series = ok(&engine, "monitord.series", json!({})).await;
    let live: Vec<&str> = series["live"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        live.contains(&"cpu.busy") && live.contains(&"cpu.core.1.busy"),
        "{live:?}"
    );
    let h = ok(
        &engine,
        "monitord.history",
        json!({"series": ["cpu.core.*", "mem.used"], "since": 60}),
    )
    .await;
    assert_eq!(h["tier"], "live");
    let got = h["series"].as_object().unwrap();
    assert!(got.contains_key("cpu.core.0.busy") && got.contains_key("cpu.core.1.busy"));
    assert_eq!(got["mem.used"][0][1], 500_000.0 * 1024.0);
    let refused = dispatch(&engine, "monitord.history", &json!({})).await;
    assert_eq!(refused.unwrap_err()["status"], "bad_request");
}

#[tokio::test]
async fn status_names_the_component_and_its_database() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path()).await;
    let s = ok(&engine, "monitord.status", json!({})).await;
    assert_eq!(s["component"], "monitord");
    assert_eq!(s["version"], monitord::COMPONENT_VERSION);
    assert!(s["history"]["error"].is_null(), "{s}");
    assert_eq!(s["history"]["retention"]["minute_days"], 7);
    let unknown = dispatch(&engine, "monitord.reboot", &json!({})).await;
    assert_eq!(unknown.unwrap_err()["error_code"], "UNKNOWN_COMMAND");
}

#[tokio::test]
async fn connections_reject_unknown_arguments_before_sampling() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path()).await;
    for body in [json!({"sort":"cpu"}), json!({"state":"oops"}), json!({"port":65536}), json!({"filter":"oops"})] {
        assert_eq!(dispatch(&e, "monitord.connections", &body).await.unwrap_err()["status"], "bad_request");
    }
    assert_eq!(ok(&e,"monitord.status",json!({})).await["connection_sampler"]["active"],false);
}
