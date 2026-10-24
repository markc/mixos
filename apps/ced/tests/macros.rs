// SPDX-License-Identifier: MIT OR Apache-2.0
#[test]
fn macros_status_dispatch_and_surface() {
    let body = citizen::macros::request("ced", citizen::macros::Caller::Other, "status", "{}")
        .wait()
        .unwrap();
    assert_eq!(body["state"], "idle");
    assert!(
        documents::commands::COMMANDS
            .iter()
            .any(|(v, _)| *v == "ced.macros.status")
    );
}
