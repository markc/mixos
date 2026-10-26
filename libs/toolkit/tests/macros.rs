// SPDX-License-Identifier: MIT OR Apache-2.0
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use toolkit::menu::{self, Entry, Menu, Row};

struct Model {
    open: bool,
    picks: Vec<menu::Pick>,
}

fn harness(render: bool) -> Harness<'static, Model> {
    let builder = Harness::builder().with_size(egui::vec2(800.0, 700.0));
    let builder = if render { builder.wgpu() } else { builder };
    let mut h = builder.build_ui_state(
        |ui, state: &mut Model| {
            egui::CentralPanel::default().show(ui, |ui| {
                let menu = Menu {
                    title: "Macros".into(),
                    entries: vec![
                        Entry::Row(Row::command("new", "New macro…", None, true)),
                        Entry::Row(Row::command("workshop", "Open Workshop", None, true)),
                        Entry::Separator,
                        Entry::Row(Row::command("macro.slot.0", "Hello desktop", None, true)),
                    ],
                };
                if let Some(pick) = menu::anchored(
                    ui,
                    egui::Id::unique("test-macros"),
                    egui::Rect::from_min_size(egui::pos2(300.0, 20.0), egui::vec2(24.0, 24.0)),
                    &mut state.open,
                    menu,
                    &[("macro.slot.0", "Right-click to edit".into())],
                ) {
                    state.picks.push(pick);
                }
            });
        },
        Model {
            open: true,
            picks: Vec::new(),
        },
    );
    toolkit::install(&h.ctx, &toolkit::Theme::embedded());
    h.run_steps(40);
    h
}

#[test]
fn escape_releases_the_menu_keyboard() {
    let mut h = harness(false);
    assert!(menu::is_open(&h.ctx));
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(!h.state().open);
    assert!(!menu::is_open(&h.ctx));
}

#[test]
fn macro_menu_snapshot() {
    let mut h = harness(true);
    h.snapshot("macros_menu");
}

#[test]
fn pointer_release_runs_or_edits_and_press_alone_does_nothing() {
    for (button, edit) in [
        (egui::PointerButton::Primary, false),
        (egui::PointerButton::Secondary, true),
    ] {
        let mut h = harness(false);
        let pos = h.get_by_label("Hello desktop").rect().center();
        h.event(egui::Event::PointerMoved(pos));
        h.event(egui::Event::PointerButton {
            pos,
            button,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        assert!(h.state().picks.is_empty());
        h.event(egui::Event::PointerButton {
            pos,
            button,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        assert_eq!(
            h.state().picks,
            [menu::Pick {
                id: "macro.slot.0",
                edit
            }]
        );
        assert!(!h.state().open);
        assert!(!menu::is_open(&h.ctx));
    }
}

#[test]
fn outside_press_closes_and_shift_enter_edits_the_letter_jump() {
    let mut h = harness(false);
    h.event(egui::Event::PointerMoved(egui::pos2(20.0, 600.0)));
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(20.0, 600.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    assert!(!h.state().open);
    let mut h = harness(false);
    h.event(egui::Event::Text("h".into()));
    h.run();
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Enter);
    h.run();
    assert_eq!(
        h.state().picks,
        [menu::Pick {
            id: "macro.slot.0",
            edit: true
        }]
    );
}
