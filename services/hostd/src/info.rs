// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `host.info`: what this host is. Every source is a plain file under a root
//! directory (`/` in production), so tests run against a fixture tree.
use serde_json::{Value, json};
use std::path::Path;

/// The MixOS image modes a start path may write to `/run/mixos/mode`.
const MODES: &[&str] = &["nested", "shared", "isolated", "metal"];

pub fn collect(root: &Path) -> Value {
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).ok();
    let os = read("etc/os-release")
        .or_else(|| read("usr/lib/os-release"))
        .map(|text| os_release(&text))
        .unwrap_or_else(|| json!({}));
    json!({
        "hostname": read("proc/sys/kernel/hostname").map(|s| s.trim().to_owned()),
        "kernel": read("proc/sys/kernel/osrelease").map(|s| s.trim().to_owned()),
        "os": os,
        "mode": read("run/mixos/mode").and_then(|s| mode(&s)),
        "container": read("run/systemd/container").map(|s| s.trim().to_owned()),
        "uptime_s": read("proc/uptime").and_then(|s| uptime(&s)),
        "cpu": read("proc/cpuinfo").map(|s| cpu(&s)),
        "memory_kb": read("proc/meminfo").and_then(|s| mem_total(&s)),
    })
}

/// The few os-release fields a caller needs, unquoted.
pub fn os_release(text: &str) -> Value {
    let mut out = serde_json::Map::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let field = match key.trim() {
            "ID" => "id",
            "VERSION_ID" => "version_id",
            "PRETTY_NAME" => "pretty_name",
            _ => continue,
        };
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        out.insert(field.into(), json!(value));
    }
    Value::Object(out)
}

/// The mode marker is trusted only when it holds one known word.
pub fn mode(text: &str) -> Option<String> {
    let word = text.trim();
    MODES.contains(&word).then(|| word.to_owned())
}

pub fn uptime(text: &str) -> Option<u64> {
    let secs: f64 = text.split_whitespace().next()?.parse().ok()?;
    Some(secs as u64)
}

pub fn cpu(text: &str) -> Value {
    let model = text
        .lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, model)| model.trim().to_owned());
    let threads = text
        .lines()
        .filter(|line| line.starts_with("processor"))
        .count();
    json!({"model": model, "threads": threads})
}

pub fn mem_total(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release_unquotes_and_ignores_other_keys() {
        let v = os_release("NAME=\"X\"\nID=debian\nVERSION_ID=\"13\"\nPRETTY_NAME='Debian 13'\n");
        assert_eq!(
            v,
            json!({"id":"debian","version_id":"13","pretty_name":"Debian 13"})
        );
    }

    #[test]
    fn mode_accepts_only_known_words() {
        assert_eq!(mode("metal\n").as_deref(), Some("metal"));
        assert_eq!(mode("root\n"), None);
        assert_eq!(mode(""), None);
    }

    #[test]
    fn proc_parsers() {
        assert_eq!(uptime("12345.67 99.0\n"), Some(12345));
        assert_eq!(
            mem_total("MemTotal:       65536 kB\nMemFree: 1 kB\n"),
            Some(65536)
        );
        let c = cpu("processor\t: 0\nmodel name\t: Example CPU\nprocessor\t: 1\n");
        assert_eq!(c, json!({"model":"Example CPU","threads":2}));
    }

    #[test]
    fn collect_reads_a_fixture_tree() {
        let dir = tempfile::tempdir().unwrap();
        let put = |rel: &str, text: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        put("proc/sys/kernel/hostname", "example\n");
        put("usr/lib/os-release", "ID=arch\n");
        put("run/mixos/mode", "shared\n");
        let v = collect(dir.path());
        assert_eq!(v["hostname"], "example");
        assert_eq!(v["os"]["id"], "arch");
        assert_eq!(v["mode"], "shared");
        assert_eq!(v["kernel"], Value::Null);
    }
}
