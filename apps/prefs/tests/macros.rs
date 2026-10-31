// SPDX-License-Identifier: MIT OR Apache-2.0
#[test]
fn macros_status_dispatch_and_surface() {
    let body = citizen::macros::request("prefs", "status", "{}")
        .wait()
        .unwrap();
    assert_eq!(body["state"], "idle");
    let surface = prefs::shell::describe();
    for suffix in ["list", "reload", "run", "status", "output", "stop", "new"] {
        let name = format!("prefs.macros.{suffix}");
        assert!(
            surface["commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["name"] == name)
        );
    }
}

#[test]
fn navigation_commands_used_by_macros_reach_every_page() {
    let commands = prefs::actions::registry();
    let mut engine = preferences::Engine::new(prefs::label_with);
    for page in preferences::pages::all() {
        commands
            .execute_with(
                "page.open",
                &mut engine,
                serde_json::json!({"page": page.id}),
                toolkit::action::Origin::Bus,
            )
            .unwrap();
        assert_eq!(engine.nav.page, page.id);
    }
    commands
        .execute("view.search", &mut engine, toolkit::action::Origin::Bus)
        .unwrap();
    assert!(engine.ui.search_focus);
}
