// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus verb registry (`docs/docs/spec/bus/verbs.conf.mix`, AGENTS.md §5)
//! lists exactly the verbs Preferences describes, apart from the protocol verbs
//! every service answers.
use prefs::shell::describe;

const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/verbs.conf.mix");

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
fn every_described_verb_is_registered_and_nothing_else() {
    let described: Vec<String> = describe()["verbs"]
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

/// Apps → Email's verbs: the view is a query, the changes intents.
#[test]
fn the_email_verbs_are_registered_with_their_classes() {
    assert_eq!(class_of("prefs.email"), Some("query"));
    assert_eq!(class_of("prefs.email.set"), Some("intent"));
    assert_eq!(class_of("prefs.email.password"), Some("intent"));
}

/// A verb the app describes as read-only is a query
/// (docs/docs/decisions/2026-10-11-verb-classes.md).
#[test]
fn read_only_verbs_are_queries() {
    let verbs = describe();
    for verb in verbs["verbs"].as_array().unwrap() {
        let name = verb["name"].as_str().unwrap();
        if matches!(name, "HELP" | "app.describe") {
            continue;
        }
        if verb["read_only"].as_bool() == Some(true) {
            assert_eq!(class_of(name), Some("query"), "{name}");
        }
    }
}

#[test]
fn every_registry_section_has_one_command_and_is_described() {
    let commands = prefs::commands::registry();
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
fn all_seven_category_commands_have_ordered_shortcuts_and_gate_empty_categories() {
    use preferences::{Category, Engine};
    let commands = prefs::commands::registry();
    let mut engine = Engine::new(prefs::label_with);
    let keys = [
        egui::Key::Num1,
        egui::Key::Num2,
        egui::Key::Num3,
        egui::Key::Num4,
        egui::Key::Num5,
        egui::Key::Num6,
        egui::Key::Num7,
    ];
    assert_eq!(
        Category::ALL.map(Category::name),
        ["overview", "appearance", "desktop", "input", "output", "apps", "system"]
    );
    for (category, key) in Category::ALL.into_iter().zip(keys) {
        let command = commands.get(category.command()).unwrap();
        assert_eq!(
            command.shortcut,
            Some(egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, key))
        );
        assert_eq!((command.enabled)(&engine), category.first_section().is_some());
        if let Some(first) = category.first_section() {
            engine.ui.expanded.clear();
            commands
                .execute(command.id, &mut engine, toolkit::command::Origin::Bus)
                .unwrap();
            assert_eq!(engine.section(), first);
            assert!(engine.ui.expanded.contains(&category));
        }
    }
    assert_eq!(
        commands
            .iter()
            .filter(|c| c.id.starts_with("view.category."))
            .count(),
        7
    );
    assert_eq!(class_of("prefs.search"), Some("query"));
}
