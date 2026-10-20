// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use serde_json::Value;
use std::{fs, path::{Path, PathBuf}, process::{Command, Output}};

pub fn binary(name: &str) -> PathBuf {
    std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join(name)
}

pub struct Bus {
    root: PathBuf,
    pub url: String,
}

impl Bus {
    pub fn start(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!("headless-bus-{}-{label}", std::process::id()));
        let output = helper("start", &root, &[binary("noded").to_str().unwrap()], None);
        check(&output);
        let record: Value = serde_json::from_slice(&fs::read(root.join("bus.json")).unwrap()).unwrap();
        Self { root, url: record["url"].as_str().unwrap().into() }
    }

    pub fn ready(&self, service: &str) {
        check(&helper("ready", &self.root, &[service], Some(&self.url)));
    }

    pub fn request(&self, service: &str, verb: &str, args: Value) -> Value {
        let output = helper("call", &self.root, &[verb, &args.to_string(), service], Some(&self.url));
        check(&output);
        let text = String::from_utf8_lossy(&output.stdout);
        let mut reply: Value = serde_json::from_str(text.lines().last().expect("Bus reply")).unwrap();
        if let Some(body) = reply["body"].as_str() {
            reply["body"] = serde_json::from_str(body).unwrap();
        }
        reply
    }

    pub fn call(&self, service: &str, verb: &str, args: Value) -> Value {
        let reply = self.request(service, verb, args);
        assert_eq!(reply["rc"], 0, "{service} {verb}: {reply}");
        reply["body"].clone()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let output = helper("stop", &self.root, &[], None);
        if !output.status.success() { eprintln!("stop private Bus: {output:?}"); }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn check(output: &Output) {
    assert!(output.status.success(), "Bus helper: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

fn helper(action: &str, root: &Path, rest: &[&str], url: Option<&str>) -> Output {
    Command::new(std::env::var_os("MIXOS_TEST_MIX").unwrap_or_else(|| "/opt/mixos/bin/mix".into()))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/bus.mix"))
        .arg(action).arg(root).args(rest)
        .env("MIXOS_NODED_URL", url.unwrap_or("ws://127.0.0.1:1"))
        .env("MIXOS_RUN", root.join("run"))
        .env("MIXOS_ETC", root.join("etc"))
        .env("MIXOS_VAR", root.join("var"))
        .env("MIXOS_NODE_CONFIG", root.join("etc/node.conf.mix"))
        .output().unwrap()
}
