// SPDX-License-Identifier: MIT OR Apache-2.0
#[test]
fn macros_status_dispatch_and_surface() {
    let body = citizen::macros::request("dopus", "status", "{}").wait().unwrap();
    assert_eq!(body["state"], "idle");
    assert!(dopus::verbs::VERBS.iter().any(|(v, _)| *v == "dopus.macros.status"));
}
