// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client-side title bar's window behaviour: each caption button sends
//! its viewport command, the free bar maximizes on double-click, and its
//! menus come from the registry.
use egui::{ViewportCommand, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use toolkit::command::{Command, Registry, always};
use toolkit::{Icon, Strings, Theme, titlebar};

const FTL: &str = "menu-file = File\ncmd-quit = Quit\n";

fn registry() -> Registry<()> {
    let mut r = Registry::new();
    r.add(Command {
        id: "file.quit",
        label: "cmd-quit",
        menu: Some("menu-file"),
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: always,
        run: |_| {},
    });
    r
}

fn harness() -> Harness<'static> {
    let registry = registry();
    let strings = Strings::new(FTL);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(640.0, 200.0))
        .build_ui(move |ui| {
            let _ = titlebar::show(
                ui,
                "Demo",
                Some(Icon::Server),
                2.0,
                &registry,
                &(),
                &strings,
            );
            egui::CentralPanel::default().show(ui, |_| {});
            titlebar::edges(ui);
        });
    toolkit::install(&harness.ctx, &Theme::embedded());
    harness.run();
    harness
}

fn commands(harness: &Harness<'_>) -> Vec<ViewportCommand> {
    harness
        .output()
        .viewport_output
        .get(&ViewportId::ROOT)
        .map(|v| v.commands.clone())
        .unwrap_or_default()
}

#[test]
fn caption_buttons_send_their_window_commands() {
    for (label, expected) in [
        ("Minimize", ViewportCommand::Minimized(true)),
        ("Maximize", ViewportCommand::Maximized(true)),
        ("Close", ViewportCommand::Close),
    ] {
        let mut h = harness();
        h.get_by_label(label).click();
        h.step();
        assert!(
            commands(&h).contains(&expected),
            "{label}: {:?}",
            commands(&h)
        );
    }
}

#[test]
fn the_menus_come_from_the_registry() {
    let h = harness();
    let _ = h.get_by_label("File");
}

#[test]
fn double_clicking_the_free_bar_maximizes() {
    let mut h = harness();
    // The middle of the bar, clear of the menus and the caption buttons.
    let at = egui::pos2(320.0, 10.0);
    for _ in 0..2 {
        h.input_mut().events.push(egui::Event::PointerMoved(at));
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
    }
    assert!(
        commands(&h).contains(&ViewportCommand::Maximized(true)),
        "{:?}",
        commands(&h)
    );
}
