// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus command registry (`docs/docs/spec/bus/commands.conf.mix`, AGENTS.md §5)
//! lists exactly the `comp.*` commands the compositor serves: the catalogue in
//! `comp_model::catalogue`, which is also what the port dispatches on.
use comp_model::catalogue::{INPUT_COMMANDS, READ_COMMANDS, all_commands, command_family};
use std::collections::BTreeSet;

const REGISTRY: &str = include_str!("../../../../../docs/docs/spec/bus/commands.conf.mix");

/// The registry's `comp.*` names owned by compd, from its one-entry-per-line
/// form.
fn registered() -> BTreeSet<String> {
    REGISTRY
        .lines()
        .filter(|line| line.contains("owner:\"compd\""))
        .filter_map(|line| line.split("name:\"").nth(1)?.split('"').next())
        .filter(|name| name.starts_with("comp."))
        .map(str::to_owned)
        .collect()
}

/// The registry's class for `name`.
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

#[test]
fn every_served_command_is_registered_and_nothing_else() {
    let served: BTreeSet<String> = all_commands().map(str::to_owned).collect();
    assert_eq!(served.len(), 39);
    assert_eq!(registered(), served);
    assert!(
        served
            .iter()
            .all(|command| command_family(command).is_some())
    );
}

/// The commands `libs/citizen` calls are served.
#[test]
fn citizen_commands_are_served() {
    for command in [
        "comp.window.wait",
        "comp.window.restore",
        "comp.window.focus",
        "comp.windows.list",
    ] {
        assert!(command_family(command).is_some(), "{command}");
    }
}

/// Snapshot reads are queries; the input family is synthetic input
/// (docs/docs/decisions/2026-10-11-verb-classes.md).
#[test]
fn classes_follow_the_families() {
    for command in READ_COMMANDS {
        assert_eq!(class_of(command), Some("query"), "{command}");
    }
    for command in INPUT_COMMANDS {
        assert_eq!(class_of(command), Some("input"), "{command}");
    }
    assert_eq!(class_of("comp.input.sequence"), Some("input"));
}
