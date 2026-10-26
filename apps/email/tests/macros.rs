// SPDX-License-Identifier: MIT OR Apache-2.0
#[test]
fn macros_status_dispatch_and_surface() {
    let body = citizen::macros::request("email", "status", "{}")
        .wait()
        .unwrap();
    assert_eq!(body["state"], "idle");
    let surface = email::commands::describe();
    for suffix in ["list", "reload", "run", "status", "output", "stop", "new"] {
        let name = format!("email.macros.{suffix}");
        assert!(
            surface["commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["name"] == name)
        );
    }
}
