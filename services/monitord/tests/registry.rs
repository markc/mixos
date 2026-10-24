// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus command registry (`docs/docs/spec/bus/commands.conf.mix`, AGENTS.md §5)
//! lists exactly the commands monitord serves, every one a query.

const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/commands.conf.mix");

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(&format!("{key}:\"")).nth(1)?.split('"').next()
}

#[test]
fn every_served_command_is_registered_and_nothing_else() {
    let registered: Vec<&str> = REGISTRY
        .lines()
        .filter(|line| line.contains("owner:\"monitord\""))
        .filter_map(|line| field(line, "name"))
        .collect();
    let served: Vec<&str> = monitord::service::COMMANDS.iter().map(|v| v.name).collect();
    assert_eq!(registered, served);
}

#[test]
fn every_command_is_a_read_only_query() {
    for command in monitord::service::manifest() {
        assert!(command.read_only, "{} must be read-only", command.name);
        if command.name == "HELP" {
            continue;
        }
        let line = REGISTRY
            .lines()
            .find(|line| field(line, "name") == Some(command.name.as_str()))
            .unwrap_or_else(|| panic!("{} is not registered", command.name));
        assert_eq!(field(line, "class"), Some("query"), "{}", command.name);
    }
}
