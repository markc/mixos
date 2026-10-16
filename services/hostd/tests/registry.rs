// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus verb registry (`docs/docs/spec/bus/verbs.conf.mix`, AGENTS.md §5)
//! lists exactly the verbs hostd serves, with reads registered as queries.

const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/verbs.conf.mix");

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(&format!("{key}:\"")).nth(1)?.split('"').next()
}

#[test]
fn every_served_verb_is_registered_and_nothing_else() {
    let registered: Vec<&str> = REGISTRY
        .lines()
        .filter(|line| line.contains("owner:\"hostd\""))
        .filter_map(|line| field(line, "name"))
        .collect();
    let served: Vec<&str> = hostd::service::VERBS.iter().map(|v| v.name).collect();
    assert_eq!(registered, served);
}

/// Anything that isn't a read changes what hostd or the host holds.
#[test]
fn other_verbs_are_intents() {
    for verb in hostd::service::VERBS {
        if verb.tier == hostd::service::Tier::Read {
            continue;
        }
        let line = REGISTRY
            .lines()
            .find(|line| field(line, "name") == Some(verb.name))
            .unwrap_or_else(|| panic!("{} is not registered", verb.name));
        assert_eq!(field(line, "class"), Some("intent"), "{}", verb.name);
    }
}

#[test]
fn read_only_verbs_are_queries() {
    for verb in hostd::service::manifest() {
        if verb.name == "HELP" || !verb.read_only {
            continue;
        }
        let line = REGISTRY
            .lines()
            .find(|line| field(line, "name") == Some(verb.name.as_str()))
            .unwrap_or_else(|| panic!("{} is not registered", verb.name));
        assert_eq!(field(line, "class"), Some("query"), "{}", verb.name);
    }
}
