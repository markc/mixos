// SPDX-License-Identifier: MIT OR Apache-2.0
#[test]
fn macros_status_dispatch_and_surface() {
    let body = inspector::bus::macros::request("busviewer", "status", "{}")
        .wait()
        .unwrap();
    assert_eq!(body["state"], "idle");
    let surface = busviewer::shell::describe();
    for suffix in ["list", "reload", "run", "status", "output", "stop", "new"] {
        let name = format!("busviewer.macros.{suffix}");
        assert!(
            surface["verbs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["name"] == name)
        );
    }
}
