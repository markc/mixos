// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus command registry (`docs/docs/spec/bus/commands.conf.mix`, AGENTS.md §5)
//! lists exactly the commands DOpus registers: its own (`dopus.describe`
//! reports them in this order) and the toolkit's drive commands under its
//! prefix, apart from the protocol commands every service answers.
use dopus::commands::COMMANDS;
use dopus::shell::APP;

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

/// What `dopus.describe` lists: [`COMMANDS`], then the drive commands (the
/// shell's `extra_commands`).
fn described() -> Vec<String> {
    COMMANDS
        .iter()
        .map(|(command, _)| (*command).to_owned())
        .chain(
            toolkit::drive::COMMANDS
                .iter()
                .map(|v| format!("{APP}.{v}")),
        )
        .collect()
}

#[test]
fn every_described_command_is_registered_and_nothing_else() {
    let described = described();
    assert!(!described.is_empty());
    assert_eq!(registered("dopus"), described);
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
/// (docs/docs/decisions/2026-10-11-verb-classes.md).
#[test]
fn read_only_commands_are_queries() {
    for (name, read_only) in COMMANDS {
        if *read_only {
            assert_eq!(class_of(name), Some("query"), "{name}");
        }
    }
}
