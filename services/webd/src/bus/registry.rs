// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The public registry must match the service's HELP metadata exactly.
#[test]
fn registry_matches_registered_commands_and_classes() {
    use std::collections::BTreeMap;
    let registry: serde_json::Value = config::from_conf_mix_str(include_str!(
        "../../../../docs/docs/spec/bus/commands.conf.mix"
    ))
    .unwrap();
    let registered: BTreeMap<String, String> = registry["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["owner"] == "webd")
        .map(|entry| {
            (
                entry["name"].as_str().unwrap().to_owned(),
                entry["class"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let declared: BTreeMap<String, String> = super::command_manifest()
        .into_iter()
        .filter(|command| command.name != "HELP")
        .map(|command| {
            (
                command.name,
                if command.read_only { "query" } else { "intent" }.to_owned(),
            )
        })
        .collect();
    assert_eq!(registered, declared);
}
