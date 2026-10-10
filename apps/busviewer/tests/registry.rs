// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Bus verb registry (`docs/spec/bus/verbs.conf.mix`, AGENTS.md §5)
//! lists exactly the verbs BusViewer describes, apart from the protocol
//! verbs every service answers.
use busviewer::shell::describe;

const REGISTRY: &str = include_str!("../../../docs/spec/bus/verbs.conf.mix");

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
    assert_eq!(registered("busviewer"), described);
}
