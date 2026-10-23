// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
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
            let _ = titlebar::show(ui, "Demo", Some(Icon::Server), &registry, &(), &strings);
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

fn plain(id: &'static str) -> Command<bool> {
    Command {
        id,
        label: "cmd-quit",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: always,
        run: |_| {},
    }
}

fn bar_registry() -> Registry<bool> {
    let mut r = Registry::new();
    r.add(plain("view.mode"));
    r.add(plain("help.keys"));
    r.add(plain("view.search"));
    r.add(Command {
        enabled: |s| *s,
        ..plain("doc.new")
    });
    r.add_toggle(plain("view.sidebar"), |s| *s);
    r
}

fn bar(state: bool, actions: &[titlebar::Action]) -> Vec<titlebar::IconBarItem> {
    let ctx = egui::Context::default();
    let strings = Strings::new(FTL);
    titlebar::icon_bar(&ctx, &bar_registry(), &state, &strings, actions, true)
}

#[test]
fn the_icon_bar_puts_app_actions_before_the_standard_tail() {
    let items = bar(
        true,
        &[
            titlebar::Action::new("doc.new", Icon::SquarePen),
            titlebar::Action::new("view.sidebar", Icon::PanelLeft),
        ],
    );
    let ids: Vec<_> = items.iter().map(|i| i.command).collect();
    assert_eq!(
        ids,
        [
            "doc.new",
            "view.sidebar",
            "view.search",
            "help.keys",
            "view.mode"
        ]
    );
    let icons: Vec<_> = items.iter().map(|i| i.icon).collect();
    assert_eq!(
        icons,
        [
            Icon::SquarePen,
            Icon::PanelLeft,
            Icon::Search,
            Icon::Keyboard,
            Icon::Sun
        ]
    );
}

#[test]
fn the_icon_bar_reads_enablement_and_toggles_from_the_registry() {
    let actions = [
        titlebar::Action::new("doc.new", Icon::SquarePen),
        titlebar::Action::new("view.sidebar", Icon::PanelLeft),
    ];
    let off = bar(false, &actions);
    assert!(!off[0].enabled && !off[1].toggled);
    let on = bar(true, &actions);
    assert!(on[0].enabled && on[1].toggled);
}

#[test]
fn the_icon_bar_skips_unknown_and_duplicate_standard_actions() {
    let items = bar(
        true,
        &[
            titlebar::Action::new("doc.missing", Icon::Square),
            titlebar::Action::new("view.mode", Icon::Moon),
        ],
    );
    let ids: Vec<_> = items.iter().map(|i| i.command).collect();
    assert_eq!(ids, ["view.search", "help.keys", "view.mode"]);
}
