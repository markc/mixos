// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Monitor's view of monitord's replies, and its own Bus surface.
//!
//! Every field defaults, and unknown fields are ignored, so a newer
//! monitord that adds fields still reads, and a field monitord can't know
//! (`null`) stays `None` here rather than turning into zero.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const APP_ID: &str = "dev.mixos.monitor";

/// The local node's monitord.
pub const LOCAL: &str = "monitord";

/// The Bus address of `node`'s monitord: this node's when `None`, else
/// mesh-routed as `monitord.<node>.bus`.
pub fn target(node: Option<&str>) -> String {
    match node {
        None => LOCAL.to_owned(),
        Some(node) => format!("monitord.{node}.bus"),
    }
}

/// A node name as the mesh writes them: letters, digits, `-` and `_`.
pub fn valid_node(node: &str) -> bool {
    !node.is_empty()
        && node.len() <= 63
        && node
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Core {
    pub cpu: usize,
    pub busy: f64,
    pub mhz: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cpu {
    pub model: Option<String>,
    pub cores: usize,
    pub busy: f64,
    pub user: f64,
    pub system: f64,
    pub iowait: f64,
    pub steal: f64,
    pub per_core: Vec<Core>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    pub total: u64,
    pub available: u64,
    pub used: u64,
    pub free: u64,
    pub cached: u64,
    pub buffers: u64,
    pub shared: u64,
    pub dirty: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Swap {
    pub total: u64,
    pub used: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Load {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tasks {
    pub total: u64,
    pub running: u64,
    pub blocked: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Disk {
    pub name: String,
    pub model: Option<String>,
    pub rotational: Option<bool>,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub busy: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filesystem {
    pub mount: String,
    pub device: String,
    pub kind: String,
    pub total: u64,
    pub used: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interface {
    pub name: String,
    pub state: Option<String>,
    pub physical: bool,
    pub speed_mbps: Option<u64>,
    pub rx_bps: Option<f64>,
    pub tx_bps: Option<f64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// `monitord.snapshot`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub host: String,
    pub ts: f64,
    pub interval_s: f64,
    pub uptime_s: Option<f64>,
    pub cpu: Cpu,
    pub memory: Memory,
    pub swap: Swap,
    pub load: Option<Load>,
    pub tasks: Tasks,
    pub disks: Vec<Disk>,
    pub filesystems: Vec<Filesystem>,
    pub net: Vec<Interface>,
}

/// One row of `monitord.processes`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub cmdline: String,
    pub user: String,
    pub uid: u32,
    pub state: String,
    pub cpu: f64,
    pub memory: u64,
    pub threads: u64,
    pub nice: i64,
    pub started_at: u64,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub unit: Option<String>,
    pub depth: Option<u32>,
}

impl Process {
    /// Storage bytes per second, or `None` when monitord could not read it.
    pub fn io(&self) -> Option<f64> {
        match (self.read_bps, self.write_bps) {
            (None, None) => None,
            (r, w) => Some(r.unwrap_or(0.0) + w.unwrap_or(0.0)),
        }
    }
}

/// `monitord.processes`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Processes {
    pub host: Option<String>,
    pub total: usize,
    pub count: usize,
    pub rows: Vec<Process>,
}

/// The window's two pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tab {
    #[default]
    Processes,
    Performance,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Processes, Tab::Performance];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Processes => "processes",
            Tab::Performance => "performance",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }
}

/// The process table's columns. Each sorts the way monitord sorts its
/// `sort` key of the same name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    Name,
    Pid,
    User,
    #[default]
    Cpu,
    Memory,
    Io,
}

impl Column {
    pub const ALL: [Column; 6] = [
        Column::Name,
        Column::Pid,
        Column::User,
        Column::Cpu,
        Column::Memory,
        Column::Io,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Column::Name => "name",
            Column::Pid => "pid",
            Column::User => "user",
            Column::Cpu => "cpu",
            Column::Memory => "memory",
            Column::Io => "io",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// The performance page's sections: `cpu`, `memory`, `disk:<name>`,
/// `net:<name>`.
pub fn sections(snapshot: Option<&Snapshot>) -> Vec<String> {
    let mut out = vec!["cpu".to_owned(), "memory".to_owned()];
    if let Some(s) = snapshot {
        out.extend(s.disks.iter().map(|d| format!("disk:{}", d.name)));
        out.extend(s.net.iter().map(|i| format!("net:{}", i.name)));
    }
    out
}

/// The mesh peers in a `noded.peers` reply, this node left out.
pub fn peers(value: &Value) -> Vec<String> {
    let local = value.get("node").and_then(Value::as_str);
    let mut names: Vec<String> = value
        .pointer("/authority/routing_view")
        .or_else(|| value.get("peers"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.get("name")?.as_str())
        .filter(|name| Some(*name) != local && valid_node(name))
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Every verb the engine answers itself. The window-level verbs
/// (`monitor.ui.*`, `monitor.window*`) belong to the toolkit drive layer.
pub const VERBS: [&str; 19] = [
    "HELP",
    "app.describe",
    "monitor.ping",
    "monitor.info",
    "monitor.show",
    "monitor.refresh",
    "monitor.commands",
    "monitor.execute",
    "monitor.quit",
    "monitor.node",
    "monitor.tab",
    "monitor.section",
    "monitor.sort",
    "monitor.filter",
    "monitor.tree",
    "monitor.select",
    "monitor.rows",
    "monitor.dialog",
    "monitor.theme",
];

/// The engine's Bus surface (`HELP` / `app.describe` add the drive verbs).
pub fn describe() -> Value {
    json!({"schema":"monitor.v1","app_id":APP_ID,"verbs":[
        {"name":"HELP","description":"List every verb","read_only":true},
        {"name":"app.describe","description":"Describe the app and every verb with its arguments","read_only":true},
        {"name":"monitor.ping","description":"Probe Monitor","read_only":true},
        {"name":"monitor.info","description":"The window's state: node, page, section, sort, filter, selection, and the latest readings","read_only":true},
        {"name":"monitor.show","description":"Restore and focus the existing window","read_only":false},
        {"name":"monitor.refresh","description":"Read the node again now, paused or not","read_only":false},
        {"name":"monitor.commands","description":"List the app's commands with labels, shortcuts and enablement","read_only":true},
        {"name":"monitor.execute","args":{"id":"string"},"description":"Run one app command by id, as its menu item or shortcut would","read_only":false},
        {"name":"monitor.quit","description":"Close the window","read_only":false},
        {"name":"monitor.node","args":{"node":"a mesh node name, or null for this node"},"description":"Watch another node's monitord, or this node's","read_only":false},
        {"name":"monitor.tab","args":{"tab":"processes or performance"},"description":"Show a page","read_only":false},
        {"name":"monitor.section","args":{"section":"cpu, memory, disk:<name> or net:<name>"},"description":"Show one section of the performance page","read_only":false},
        {"name":"monitor.sort","args":{"column":"name, pid, user, cpu, memory or io","reverse":"optional bool"},"description":"Sort the process table, as clicking its header does","read_only":false},
        {"name":"monitor.filter","args":{"text":"string"},"description":"Filter the process table by name or command line","read_only":false},
        {"name":"monitor.tree","args":{"on":"bool"},"description":"Show processes as a tree, parents before children","read_only":false},
        {"name":"monitor.select","args":{"pid":"a process id, or null"},"description":"Select a process row","read_only":false},
        {"name":"monitor.rows","description":"The process rows the table shows, in order","read_only":true},
        {"name":"monitor.dialog","args":{"open":"\"about\", \"shortcuts\" or null"},"description":"Open a dialog, or close the open one","read_only":false},
        {"name":"monitor.theme","args":{"scheme":"optional scheme name or null to follow the session","style":"optional style, own, or null","mode":"optional light, dark or null","decorations":"optional csd, ssd or null","caption_side":"optional right, left or null"},"description":"Choose the window's theme: a present key sets that axis, an absent one leaves it","read_only":false}
    ]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_verb_is_described_once() {
        let described: Vec<_> = describe()["verbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(described, VERBS);
    }

    #[test]
    fn nodes_are_addressed_over_the_mesh() {
        assert_eq!(target(None), "monitord");
        assert_eq!(target(Some("alpha")), "monitord.alpha.bus");
        assert!(valid_node("node-2_b"));
        assert!(!valid_node("a.b") && !valid_node("") && !valid_node("x y"));
    }

    #[test]
    fn replies_read_leniently() {
        let s: Snapshot = serde_json::from_value(json!({
            "host": "alpha",
            "cpu": {"busy": 12.5, "per_core": [{"cpu": 0, "busy": 3.0, "mhz": null}]},
            "load": null,
            "brand_new_field": [1, 2, 3],
        }))
        .unwrap();
        assert_eq!(s.host, "alpha");
        assert_eq!(s.cpu.per_core[0].mhz, None);
        assert!(s.load.is_none());
        let p: Process =
            serde_json::from_value(json!({"pid": 7, "read_bps": null, "write_bps": 4.0})).unwrap();
        assert_eq!(p.io(), Some(4.0));
        assert_eq!(Process::default().io(), None);
    }

    #[test]
    fn peers_leave_out_this_node_and_bad_names() {
        assert_eq!(
            peers(
                &json!({"node":"alpha","peers":[{"name":"alpha"},{"name":"beta"},{"name":"bad.name"}]})
            ),
            vec!["beta"]
        );
    }

    #[test]
    fn sections_follow_the_snapshot() {
        let s = Snapshot {
            disks: vec![Disk {
                name: "sda".into(),
                ..Disk::default()
            }],
            net: vec![Interface {
                name: "eth0".into(),
                ..Interface::default()
            }],
            ..Snapshot::default()
        };
        assert_eq!(
            sections(Some(&s)),
            ["cpu", "memory", "disk:sda", "net:eth0"]
        );
        assert_eq!(sections(None), ["cpu", "memory"]);
    }
}
