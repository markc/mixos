// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Keep the repository registry and maild's advertised wire surface together.

const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/verbs.conf.mix");

fn registered() -> Vec<&'static str> {
    REGISTRY
        .lines()
        .filter(|line| line.contains("owner:\"maild\""))
        .map(|line| {
            line.split("name:\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
        })
        .collect()
}

#[test]
fn every_advertised_verb_is_registered_and_nothing_else() {
    let manifest = maild::bus::verb_manifest();
    let names: Vec<&str> = manifest
        .iter()
        .filter(|verb| verb.name != "HELP")
        .map(|verb| verb.name.as_str())
        .collect();
    assert!(!names.is_empty());
    assert_eq!(registered(), names);
}

#[test]
fn read_only_verbs_are_queries_and_mutations_are_intents() {
    for verb in maild::bus::verb_manifest() {
        if verb.name == "HELP" {
            continue;
        }
        let key = format!("name:\"{}\"", verb.name);
        let line = REGISTRY.lines().find(|line| line.contains(&key)).unwrap();
        let class = line
            .split("class:\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(
            class,
            if verb.read_only { "query" } else { "intent" },
            "{}",
            verb.name
        );
    }
}
