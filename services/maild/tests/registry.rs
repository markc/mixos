// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Keep the repository registry and maild's advertised wire surface together.

const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/commands.conf.mix");

fn registered() -> Vec<&'static str> {
    REGISTRY
        .lines()
        .filter(|line| line.contains("owner:\"maild\""))
        .map(|line| {
            line.split("name:\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
        })
        .collect()
}

#[test]
fn every_advertised_command_is_registered_and_nothing_else() {
    let manifest = maild::bus::command_manifest();
    let names: Vec<&str> = manifest
        .iter()
        .filter(|command| command.name != "HELP")
        .map(|command| command.name.as_str())
        .collect();
    assert!(!names.is_empty());
    assert_eq!(registered(), names);
}

#[test]
fn read_only_commands_are_queries_and_mutations_are_intents() {
    for command in maild::bus::command_manifest() {
        if command.name == "HELP" {
            continue;
        }
        let key = format!("name:\"{}\"", command.name);
        let line = REGISTRY.lines().find(|line| line.contains(&key)).unwrap();
        let class = line
            .split("class:\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(
            class,
            if command.read_only { "query" } else { "intent" },
            "{}",
            command.name
        );
    }
}
