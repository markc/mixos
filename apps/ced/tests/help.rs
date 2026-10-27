// SPDX-License-Identifier: MIT OR Apache-2.0
//! Discovery includes the handlers hosted outside the document engine.

#[test]
fn help_lists_each_hosted_command_once_with_registry_query_flags() {
    let registry = include_str!("../../../docs/docs/spec/bus/commands.conf.mix");
    for window in [false, true] {
        let entries = ced::bus::help(window);
        assert_eq!(entries[0].name, "HELP");
        assert_eq!(entries, ced::bus::help(window), "stable order");
        let mut names = std::collections::BTreeSet::new();
        for entry in entries {
            assert!(names.insert(entry.name.clone()), "duplicate {}", entry.name);
            if matches!(entry.name.as_str(), "HELP" | "INFO" | "app.describe") {
                assert!(entry.read_only);
                continue;
            }
            let row = registry
                .lines()
                .find(|row| row.contains(&format!("name:\"{}\"", entry.name)))
                .unwrap();
            assert_eq!(
                entry.read_only,
                row.contains("class:\"query\""),
                "{}",
                entry.name
            );
        }
        for suffix in citizen::macros::SUFFIXES {
            assert!(names.contains(&format!("ced.macros.{suffix}")));
        }
        assert!(names.contains("ced.execute"));
        assert!(!names.contains(concat!("ced.", "action")));
        for suffix in toolkit::drive::COMMANDS {
            assert_eq!(names.contains(&format!("ced.{suffix}")), window);
        }
    }
}
