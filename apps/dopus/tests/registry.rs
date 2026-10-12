// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Bus verb registry (`docs/spec/bus/verbs.conf.mix`, AGENTS.md §5)
//! lists exactly the verbs DOpus registers: its own (`dopus.describe`
//! reports them in this order) and the toolkit's drive verbs under its
//! prefix, apart from the protocol verbs every service answers.
use dopus::shell::APP;
use dopus::verbs::VERBS;

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

/// What `dopus.describe` lists: [`VERBS`], then the drive verbs (the
/// shell's `extra_verbs`).
fn described() -> Vec<String> {
    VERBS
        .iter()
        .map(|(verb, _)| (*verb).to_owned())
        .chain(toolkit::drive::VERBS.iter().map(|v| format!("{APP}.{v}")))
        .collect()
}

#[test]
fn every_described_verb_is_registered_and_nothing_else() {
    let described = described();
    assert!(!described.is_empty());
    assert_eq!(registered("dopus"), described);
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

/// A verb the app describes as read-only is a query
/// (docs/decisions/2026-10-11-verb-classes.md).
#[test]
fn read_only_verbs_are_queries() {
    for (name, read_only) in VERBS {
        if *read_only {
            assert_eq!(class_of(name), Some("query"), "{name}");
        }
    }
}
