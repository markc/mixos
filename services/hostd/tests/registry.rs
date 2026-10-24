// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus command registry (`docs/docs/spec/bus/commands.conf.mix`, AGENTS.md §5)
//! lists exactly the commands hostd serves, with reads registered as queries.

const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/commands.conf.mix");

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(&format!("{key}:\"")).nth(1)?.split('"').next()
}

#[test]
fn every_served_command_is_registered_and_nothing_else() {
    let registered: Vec<&str> = REGISTRY
        .lines()
        .filter(|line| line.contains("owner:\"hostd\""))
        .filter_map(|line| field(line, "name"))
        .collect();
    let served: Vec<&str> = hostd::service::COMMANDS.iter().map(|v| v.name).collect();
    assert_eq!(registered, served);
}

/// Anything that isn't a read changes what hostd or the host holds.
#[test]
fn other_commands_are_intents() {
    for command in hostd::service::COMMANDS {
        if command.tier == hostd::service::Tier::Read {
            continue;
        }
        let line = REGISTRY
            .lines()
            .find(|line| field(line, "name") == Some(command.name))
            .unwrap_or_else(|| panic!("{} is not registered", command.name));
        assert_eq!(field(line, "class"), Some("intent"), "{}", command.name);
    }
}

#[test]
fn read_only_commands_are_queries() {
    for command in hostd::service::manifest() {
        if command.name == "HELP" || !command.read_only {
            continue;
        }
        let line = REGISTRY
            .lines()
            .find(|line| field(line, "name") == Some(command.name.as_str()))
            .unwrap_or_else(|| panic!("{} is not registered", command.name));
        assert_eq!(field(line, "class"), Some("query"), "{}", command.name);
    }
}
