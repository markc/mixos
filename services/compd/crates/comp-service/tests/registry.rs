// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus verb registry (`docs/spec/bus/verbs.conf.mix`, AGENTS.md §5)
//! lists exactly the `comp.*` verbs the compositor serves: the catalogue in
//! `comp_model::catalogue`, which is also what the port dispatches on.
use comp_model::catalogue::{INPUT_VERBS, READ_VERBS, all_verbs, verb_family};
use std::collections::BTreeSet;

const REGISTRY: &str = include_str!("../../../../../docs/spec/bus/verbs.conf.mix");

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
fn every_served_verb_is_registered_and_nothing_else() {
    let served: BTreeSet<String> = all_verbs().map(str::to_owned).collect();
    assert_eq!(served.len(), 38);
    assert_eq!(registered(), served);
    assert!(served.iter().all(|verb| verb_family(verb).is_some()));
}

/// The verbs `libs/citizen` calls are served.
#[test]
fn citizen_verbs_are_served() {
    for verb in [
        "comp.window.wait",
        "comp.window.restore",
        "comp.window.focus",
        "comp.windows.list",
    ] {
        assert!(verb_family(verb).is_some(), "{verb}");
    }
}

/// Snapshot reads are queries; the input family is synthetic input
/// (docs/decisions/2026-10-11-verb-classes.md).
#[test]
fn classes_follow_the_families() {
    for verb in READ_VERBS {
        assert_eq!(class_of(verb), Some("query"), "{verb}");
    }
    for verb in INPUT_VERBS {
        assert_eq!(class_of(verb), Some("input"), "{verb}");
    }
    assert_eq!(class_of("comp.input.sequence"), Some("input"));
}
