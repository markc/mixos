// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
const REGISTRY: &str = include_str!("../../../docs/docs/spec/bus/commands.conf.mix");

#[test]
fn manifest_matches_registered_names_and_classes() {
    let registered: Vec<_> = REGISTRY
        .lines()
        .filter(|l| l.contains("owner:\"indexd\""))
        .map(|l| {
            l.split("name:\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
        })
        .collect();
    let manifest = indexd::bus::command_manifest();
    let names: Vec<_> = manifest.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(registered, names);
    assert_eq!(names.len(), 11);
    for command in manifest {
        let line = REGISTRY
            .lines()
            .find(|l| l.contains(&format!("name:\"{}\"", command.name)))
            .unwrap();
        assert!(
            line.contains(if command.read_only {
                "class:\"query\""
            } else {
                "class:\"intent\""
            }),
            "{}",
            command.name
        );
    }
}
