// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus command registry (`docs/docs/spec/bus/commands.conf.mix`, AGENTS.md §5)
//! lists exactly the commands Monitor describes, apart from the protocol commands
//! every service answers.
use monitor::shell::describe;

const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/commands.conf.mix");

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(&format!("{key}:\"")).nth(1)?.split('"').next()
}

fn registered(owner: &str) -> Vec<String> {
    let owned = format!("owner:\"{owner}\"");
    REGISTRY
        .lines()
        .filter(|line| line.contains(&owned))
        .filter_map(|line| field(line, "name").map(str::to_owned))
        .collect()
}

#[test]
fn every_described_command_is_registered_and_nothing_else() {
    let described: Vec<String> = describe()["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["name"].as_str())
        .filter(|name| !matches!(*name, "HELP" | "app.describe"))
        .map(str::to_owned)
        .collect();
    assert!(!described.is_empty());
    assert_eq!(registered("monitor"), described);
}

/// A command the app describes as read-only is a query
/// (docs/docs/decisions/2026-10-11-verb-classes.md).
#[test]
fn read_only_commands_are_queries() {
    for command in describe()["commands"].as_array().unwrap() {
        let name = command["name"].as_str().unwrap();
        if matches!(name, "HELP" | "app.describe") || command["read_only"].as_bool() != Some(true) {
            continue;
        }
        let class = REGISTRY
            .lines()
            .find(|line| field(line, "name") == Some(name))
            .and_then(|line| field(line, "class"));
        assert_eq!(class, Some("query"), "{name}");
    }
}
