// SPDX-License-Identifier: MIT OR Apache-2.0
#[test]
fn macros_status_dispatch_and_surface() {
    let body = citizen::macros::request("prefs", citizen::macros::Caller::Other, "status", "{}")
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
fn navigation_commands_used_by_macros_share_the_section_registry() {
    let commands = prefs::actions::registry();
    let mut engine = preferences::Engine::new(prefs::label_with);
    for section in preferences::SECTIONS {
        commands
            .execute(section.command, &mut engine, toolkit::action::Origin::Bus)
            .unwrap();
        assert_eq!(engine.section(), section.section);
    }
    commands
        .execute("view.search", &mut engine, toolkit::action::Origin::Bus)
        .unwrap();
    assert!(engine.ui.search_focus);
}
