// SPDX-License-Identifier: MIT OR Apache-2.0
use inspector::model::{Node, Verb};
use inspector::{Effect, Engine, Snapshot};

#[test]
fn nodes_keep_service_identity_and_distinct_routed_selections() {
    let mut engine = Engine::new(str::to_owned);
    let Some(Effect::Discover { ticket }) = engine.take_effects().pop() else {
        panic!("discovery");
    };
    let mut snapshot = Snapshot::default();
    for (node, route, local) in [("alpha", "ced", true), ("beta", "ced.beta.bus", false)] {
        snapshot.nodes.push(Node {
            name: node.into(),
            local,
            services: vec![route.into()],
            ..Default::default()
        });
        let mut identity = bus::ServiceInfo::from_name("ced");
        identity.version = Some("1.2.3".into());
        identity.git_sha = Some("123456789abcdef".into());
        snapshot.identities.insert(route.into(), identity);
        snapshot.services.insert(
            route.into(),
            Ok(vec![Verb {
                name: "ced.ping".into(),
                args: String::new(),
                description: String::new(),
                read_only: Some(true),
            }]),
        );
    }
    engine.discovered(ticket, snapshot);
    engine.toggle("node:beta");
    engine.toggle("service:alpha:ced");
    engine.toggle("service:beta:ced");
    let rows = engine.rows();
    assert!(
        rows.iter()
            .any(|row| row["key"] == "node:alpha" && row["kind"] == "node")
    );
    let service = rows
        .iter()
        .find(|row| row["key"] == "service:beta:ced")
        .unwrap();
    assert_eq!(service["identity"]["version"], "1.2.3");
    assert!(service["label"].as_str().unwrap().contains("12345678"));
    let verb = rows
        .iter()
        .find(|row| row["key"] == "verb:beta:ced:ced.ping")
        .unwrap();
    assert_eq!(verb["selection"]["service"], "ced.beta.bus");
    engine.toggle("node:alpha");
    assert!(
        !engine
            .rows()
            .iter()
            .any(|row| row["key"] == "service:alpha:ced")
    );
}
