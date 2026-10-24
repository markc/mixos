// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus command registry (`docs/docs/spec/bus/commands.conf.mix`, AGENTS.md §5)
//! lists exactly the commands Email describes: its own (`app.describe` reports
//! them in this order) and the toolkit's drive commands under its prefix, apart
//! from the protocol commands every service answers.
use email::APP;
use email::commands::{COMMANDS, describe};

const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/commands.conf.mix");

/// The registry's names owned by `owner`, from its one-entry-per-line form.
fn registered(owner: &str) -> Vec<String> {
    let owned = format!("owner:\"{owner}\"");
    REGISTRY
        .lines()
        .filter(|line| line.contains(&owned))
        .filter_map(|line| {
            line.split("name:\"")
                .nth(1)?
                .split('"')
                .next()
                .map(str::to_owned)
        })
        .collect()
}

/// What `app.describe` lists, without the protocol commands.
fn described() -> Vec<String> {
    describe()["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["name"].as_str())
        .filter(|name| !matches!(*name, "HELP" | "app.describe"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_described_command_is_registered_and_nothing_else() {
    let described = described();
    assert!(!described.is_empty());
    assert_eq!(registered("email"), described);
    // The drive commands are there under the app's prefix.
    for command in toolkit::drive::COMMANDS {
        assert!(described.contains(&format!("{APP}.{command}")), "{command}");
    }
}

/// The registry's class for `name`, from its one-entry-per-line form.
fn class_of(name: &str) -> Option<&'static str> {
    let key = format!("name:\"{name}\"");
    REGISTRY
        .lines()
        .find(|line| line.contains(&key))?
        .split("class:\"")
        .nth(1)?
        .split('"')
        .next()
}

/// A command the app describes as read-only is a query
/// (docs/docs/decisions/2026-10-11-verb-classes.md), and one that changes
/// something is not.
#[test]
fn read_only_commands_are_queries() {
    for command in COMMANDS {
        if matches!(command.name, "HELP" | "app.describe") {
            continue;
        }
        let class = class_of(command.name);
        if command.read_only {
            assert_eq!(class, Some("query"), "{}", command.name);
        } else {
            assert_ne!(class, Some("query"), "{}", command.name);
        }
    }
}
