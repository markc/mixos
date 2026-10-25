// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus command registry (`docs/docs/spec/bus/commands.conf.mix`, AGENTS.md §5)
//! lists exactly the commands Preferences describes, apart from the protocol commands
//! every service answers.
use prefs::shell::describe;

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
    assert_eq!(registered("prefs"), described);
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

/// Apps → Email's commands: the view is a query, the changes intents.
#[test]
fn the_email_commands_are_registered_with_their_classes() {
    assert_eq!(class_of("prefs.email"), Some("query"));
    assert_eq!(class_of("prefs.email.set"), Some("intent"));
    assert_eq!(class_of("prefs.email.password"), Some("intent"));
}

/// A command the app describes as read-only is a query
/// (docs/docs/decisions/2026-10-11-verb-classes.md).
#[test]
fn read_only_commands_are_queries() {
    let command_names = describe();
    for command_name in command_names["commands"].as_array().unwrap() {
        let name = command_name["name"].as_str().unwrap();
        if matches!(name, "HELP" | "app.describe") {
            continue;
        }
        if command_name["read_only"].as_bool() == Some(true) {
            assert_eq!(class_of(name), Some("query"), "{name}");
        }
    }
}

#[test]
fn every_registry_section_has_one_command_and_is_described() {
    let commands = prefs::actions::registry();
    let surface = describe();
    let described = surface["sections"].as_array().unwrap();
    let mut ids = std::collections::HashSet::new();
    for section in preferences::SECTIONS {
        assert!(ids.insert(section.id));
        let command = commands.get(section.command).expect("section command");
        assert_eq!(command.id, format!("view.section.{}", section.id));
        assert!(command.shortcut.is_none());
        assert_eq!(command.label, section.title);
        assert_eq!(command.submenu, Some(section.category.label()));
        assert!(described.iter().any(|s| s["id"] == section.id));
    }
    assert_eq!(described.len(), ids.len());
    assert_eq!(
        commands
            .iter()
            .filter(|c| c.id.starts_with("view.section."))
            .count(),
        ids.len()
    );
}

#[test]
fn flat_sidebar_has_no_category_commands() {
    assert!(!prefs::actions::registry().iter().any(|c| c.id.starts_with("view.category.")));
}
