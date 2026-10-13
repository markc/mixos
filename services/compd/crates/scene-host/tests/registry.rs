// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus verb registry (`docs/spec/bus/verbs.conf.mix`, AGENTS.md §5)
//! lists exactly the `shell.*` verbs the scene host serves under its default
//! service name: `SceneVerb::names`, which is also what `shell.info` lists.
use scene_host::{DEFAULT_SERVICE, SceneVerb};
use std::collections::BTreeSet;

const REGISTRY: &str = include_str!("../../../../../docs/spec/bus/verbs.conf.mix");

/// The registry's `shell.*` names owned by compd, from its one-entry-per-line
/// form.
fn registered() -> BTreeSet<String> {
    REGISTRY
        .lines()
        .filter(|line| line.contains("owner:\"compd\""))
        .filter_map(|line| line.split("name:\"").nth(1)?.split('"').next())
        .filter(|name| name.starts_with("shell."))
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
    assert_eq!(DEFAULT_SERVICE, "shell");
    let served: BTreeSet<String> = SceneVerb::names(DEFAULT_SERVICE).into_iter().collect();
    assert_eq!(served.len(), 33);
    assert_eq!(registered(), served);
}

/// A verb that can change what is mounted or drawn is an intent; the rest
/// are queries (docs/decisions/2026-10-11-verb-classes.md).
#[test]
fn classes_follow_mutation() {
    for name in SceneVerb::names(DEFAULT_SERVICE) {
        let verb = SceneVerb::parse(DEFAULT_SERVICE, &name).unwrap();
        let expected = if verb.mutates() { "intent" } else { "query" };
        assert_eq!(class_of(&name), Some(expected), "{name}");
    }
}

/// compd's truth verb sits beside the frozen `comp.*` surface.
#[test]
fn truth_verb_is_a_registered_query() {
    assert_eq!(class_of("compd.truth"), Some("query"));
}
