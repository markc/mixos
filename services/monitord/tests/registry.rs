// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus verb registry (`docs/docs/spec/bus/verbs.conf.mix`, AGENTS.md §5)
//! lists exactly the verbs monitord serves, every one a query.

const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/verbs.conf.mix");

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(&format!("{key}:\"")).nth(1)?.split('"').next()
}

#[test]
fn every_served_verb_is_registered_and_nothing_else() {
    let registered: Vec<&str> = REGISTRY
        .lines()
        .filter(|line| line.contains("owner:\"monitord\""))
        .filter_map(|line| field(line, "name"))
        .collect();
    let served: Vec<&str> = monitord::service::VERBS.iter().map(|v| v.name).collect();
    assert_eq!(registered, served);
}

#[test]
fn every_verb_is_a_read_only_query() {
    for verb in monitord::service::manifest() {
        assert!(verb.read_only, "{} must be read-only", verb.name);
        if verb.name == "HELP" {
            continue;
        }
        let line = REGISTRY
            .lines()
            .find(|line| field(line, "name") == Some(verb.name.as_str()))
            .unwrap_or_else(|| panic!("{} is not registered", verb.name));
        assert_eq!(field(line, "class"), Some("query"), "{}", verb.name);
    }
}
