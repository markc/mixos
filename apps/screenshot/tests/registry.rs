// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus verb registry (`docs/docs/spec/bus/verbs.conf.mix`, AGENTS.md §5)
//! lists exactly the verbs screenshot describes (its own and the window's
//! drive verbs), apart from the protocol verbs every service answers; and
//! each verb's class agrees with what the app says of it.
use screenshot::jobs::describe;

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

fn described() -> Vec<String> {
    describe(true)["verbs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["name"].as_str())
        .filter(|name| !matches!(*name, "HELP" | "app.describe"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_described_verb_is_registered_and_nothing_else() {
    let described = described();
    assert!(!described.is_empty());
    assert_eq!(registered("screenshot"), described);
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

/// A read-only verb is a query; activation is system
/// (docs/docs/decisions/2026-10-11-verb-classes.md).
#[test]
fn classes_follow_what_the_verbs_do() {
    for verb in describe(true)["verbs"].as_array().unwrap() {
        let name = verb["name"].as_str().unwrap();
        if matches!(name, "HELP" | "app.describe") {
            continue;
        }
        if verb["read_only"].as_bool() == Some(true) {
            assert_eq!(class_of(name), Some("query"), "{name}");
        }
    }
    assert_eq!(class_of("screenshot.show"), Some("system"));
    assert_eq!(class_of("screenshot.save"), Some("intent"));
    assert_eq!(class_of("screenshot.recent"), Some("query"));
}

/// Cap's verbs were renamed, never reused: none is registered any more.
#[test]
fn the_iced_cap_verbs_are_gone() {
    assert!(registered("cap").is_empty());
    assert!(!REGISTRY.contains("name:\"cap."));
}
