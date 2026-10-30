// SPDX-License-Identifier: MIT OR Apache-2.0
use egui_kittest::{Harness, kittest::Queryable};
use toolkit::{
    Action, Registry, Strings,
    action::{Origin, always},
};

#[derive(Default)]
struct App {
    open: bool,
    ran: bool,
}

#[test]
fn command_search_filters_runs_through_registry_and_dismisses() {
    let mut registry = Registry::new();
    registry.search_action(toolkit::search::command(always, |a: &mut App| {
        a.open = true
    }));
    registry.add(Action {
        id: "file.save",
        label: "save",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: always,
        run: |a| a.ran = true,
    });
    let strings = Strings::new("save = Save document\n");
    let mut app = App::default();
    registry
        .execute("view.search", &mut app, Origin::Bus)
        .unwrap();
    let mut h = Harness::builder().build_ui_state(
        move |ui, app: &mut App| {
            if let Some(chosen) =
                toolkit::search::show(ui.ctx(), &registry, app, &strings, app.open)
            {
                app.open = false;
                if let Some((id, origin)) = chosen {
                    registry.execute(id, app, origin).unwrap();
                    assert_eq!(origin, Origin::Palette);
                }
            }
        },
        app,
    );
    h.run();
    h.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("save");
    h.run();
    h.get_by_label("Save document").click();
    h.run();
    assert!(h.state().ran);
    assert!(!h.state().open);
    h.state_mut().open = true;
    h.run();
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(!h.state().open);
}
