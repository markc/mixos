// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The generated `settings.describe` against its golden: one key per line.
//! A change to a key's kind, default, class, page or label shows here as a
//! one-line diff. To accept a change, replace the golden with the text this
//! test prints between the markers.

const GOLDEN: &str = include_str!("registry_describe.golden");

#[test]
fn describe_matches_the_golden() {
    let described = settings::registry::describe(None);
    let lines: Vec<String> = described["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| serde_json::to_string(key).unwrap())
        .collect();
    let actual = lines.join("\n") + "\n";
    if actual != GOLDEN {
        println!("--- registry_describe.golden ---\n{actual}--- end ---");
        let expected: Vec<&str> = GOLDEN.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            if expected.get(n) != Some(&line.as_str()) {
                panic!(
                    "line {}: expected {:?}\n actual {line}",
                    n + 1,
                    expected.get(n)
                );
            }
        }
        panic!("the golden has {} lines, describe {}", expected.len(), lines.len());
    }
}
