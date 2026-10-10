// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Bus verb registry (`docs/spec/bus/verbs.conf.mix`, AGENTS.md §5) covers
//! exactly the verbs Prefs describes, apart from the protocol verbs every
//! service answers. A verb is covered by one concrete entry owned by `prefs`,
//! or by one family pattern with `<app>` read as `prefs`. Retired entries are
//! left out of the live set.
use prefs::shell::describe;

const REGISTRY: &str = include_str!("../../../docs/spec/bus/verbs.conf.mix");
const APP: &str = "prefs";

/// The value of `key:"..."` on one registry line, if the line has it.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("{key}:\"");
    let start = line.find(pat.as_str())? + pat.len();
    let rest = &line[start..];
    Some(&rest[..rest.find('"')?])
}

/// `pattern` matches `name` segment by segment; `*` matches exactly one segment.
fn matches(pattern: &str, name: &str) -> bool {
    let p: Vec<&str> = pattern.split('.').collect();
    let n: Vec<&str> = name.split('.').collect();
    p.len() == n.len()
        && p.iter()
            .zip(&n)
            .all(|(seg, got)| *seg == "*" || *seg == *got)
}

/// Live concrete names owned by `APP`, and live family patterns with `<app>`
/// resolved to `APP`.
fn live_entries() -> (Vec<String>, Vec<String>) {
    let (mut concrete, mut families) = (Vec::new(), Vec::new());
    for line in REGISTRY.lines() {
        if field(line, "status") == Some("retired") {
            continue;
        }
        if let Some(name) = field(line, "name") {
            if field(line, "owner") == Some(APP) {
                concrete.push(name.to_owned());
            }
        } else if let Some(pattern) = field(line, "pattern") {
            families.push(pattern.replace("<app>", APP));
        }
    }
    (concrete, families)
}

#[test]
fn every_described_verb_is_covered_and_nothing_else() {
    let described: Vec<String> = describe()["verbs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["name"].as_str())
        .filter(|name| !matches!(*name, "HELP" | "app.describe"))
        .map(str::to_owned)
        .collect();
    assert!(!described.is_empty());

    let (concrete, families) = live_entries();
    assert!(
        !families.is_empty(),
        "no family patterns found in the registry"
    );

    // Every described verb is matched by exactly one entry: one concrete entry
    // or one family pattern, never both and never two families.
    for name in &described {
        let hits = usize::from(concrete.contains(name))
            + families.iter().filter(|p| matches(p, name)).count();
        assert_eq!(
            hits, 1,
            "{name} is matched by {hits} registry entries; exactly one is required"
        );
    }
    // Families must not overlap: no described name may be matched by two.
    for (i, a) in families.iter().enumerate() {
        for b in &families[i + 1..] {
            let overlap = described.iter().any(|d| matches(a, d) && matches(b, d));
            assert!(!overlap, "families {a} and {b} overlap");
        }
    }
    // Every concrete entry is described; none is stale.
    for name in &concrete {
        assert!(
            described.contains(name),
            "{name} is registered for {APP} but not described"
        );
    }
    // Every family covers at least one described verb; none is dead.
    for pattern in &families {
        assert!(
            described.iter().any(|d| matches(pattern, d)),
            "family {pattern} covers no described verb"
        );
    }
    // No duplicate concrete entries.
    let mut unique = concrete.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        concrete.len(),
        "duplicate concrete entries for {APP}"
    );
}
