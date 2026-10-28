// SPDX-License-Identifier: MIT OR Apache-2.0
#[test]
fn described_commands_match_the_public_registry() {
    let registry = strict::parse(include_str!(
        "../../../docs/docs/spec/bus/commands.conf.mix"
    ))
    .unwrap();
    let mut registered: Vec<_> = registry["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["owner"] == "clipd")
        .map(|row| {
            (
                row["name"].as_str().unwrap().to_owned(),
                row["class"] == "query",
            )
        })
        .collect();
    let mut served: Vec<_> = clipd::service::manifest()
        .into_iter()
        .filter(|c| c.name != "HELP")
        .map(|c| (c.name, c.read_only))
        .collect();
    registered.sort();
    served.sort();
    assert_eq!(served, registered);
}
