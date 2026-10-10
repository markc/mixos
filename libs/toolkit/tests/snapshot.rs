// SPDX-License-Identifier: MIT OR Apache-2.0
//! Offscreen snapshots of the toolkit's look: a representative panel in the
//! ocean scheme's light and dark modes, rendered through wgpu and compared as
//! images (`tests/snapshots/*.png`). Regenerate with `UPDATE_SNAPSHOTS=1`
//! and look at the images before committing them.

use design::{DesignContext, Mode};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use toolkit::command::{Command, Registry, always};
use toolkit::{Icon, Strings, Theme, icons};

const FTL: &str = "\
menu-file = File
menu-bus = Bus
cmd-refresh = Refresh
cmd-quit = Quit
title = Services
body = Body text in the interface face, with numbers 0123456789.
";

#[derive(Default)]
struct Demo {
    refreshed: u32,
}

fn registry() -> Registry<Demo> {
    let mut r = Registry::new();
    r.add(Command {
        id: "app.quit",
        label: "cmd-quit",
        menu: Some("menu-file"),
        submenu: None,
        group: 0,
        shortcut: Some(egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Q)),
        icon: None,
        enabled: always,
        run: |_| {},
    })
    .add(Command {
        id: "bus.refresh",
        label: "cmd-refresh",
        menu: Some("menu-bus"),
        submenu: None,
        group: 0,
        shortcut: Some(egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::R)),
        icon: Some(Icon::RefreshCw),
        enabled: always,
        run: |s: &mut Demo| s.refreshed += 1,
    });
    r
}

fn panel(theme: &Theme, name: &str) {
    panel_with(theme, name, None);
}

/// The panel, optionally with the top-bar menu `open` clicked open.
fn panel_with(theme: &Theme, name: &str, open: Option<&str>) {
    let strings = Strings::new(FTL);
    let commands = registry();
    let state = Demo::default();
    let stroke = icons::stroke_width(theme);
    let mut harness = Harness::builder().with_size(egui::vec2(420.0, 260.0)).wgpu().build_ui(move |ui| {
        let _ = toolkit::titlebar::show(ui, "Demo", Some(Icon::Server), stroke, &commands, &state, &strings);
        egui::CentralPanel::default().show(ui, |ui| contents(ui, &strings, stroke));
    });
    toolkit::install(&harness.ctx, theme);
    harness.run();
    if let Some(menu) = open {
        harness.get_by_label(menu).click();
        harness.run();
    }
    harness.snapshot(name);
}

fn contents(ui: &mut egui::Ui, strings: &Strings, stroke: f32) {
    {
        ui.heading(strings.get("title"));
        ui.label(strings.get("body"));
        ui.weak(strings.get("body"));
        ui.horizontal(|ui| {
            let colour = ui.visuals().text_color();
            for icon in [Icon::Server, Icon::Plug, Icon::Search, Icon::Play, Icon::CircleCheck] {
                ui.add(icons::image(icon, stroke, 16.0, colour));
            }
        });
        ui.horizontal(|ui| {
            let _ = ui.button(strings.get("cmd-refresh"));
            let _ = ui.selectable_label(true, strings.get("menu-bus"));
            let mut text = String::from("bus.call");
            ui.text_edit_singleline(&mut text);
        });
        ui.code("{\"rc\": 0}");
    }
}

#[test]
fn panel_light() {
    panel(&Theme::for_context(DesignContext::revision_one()), "panel_light");
}

#[test]
fn menu_open_dark() {
    panel_with(&Theme::for_context(DesignContext { mode: Mode::Dark, ..DesignContext::revision_one() }), "menu_open_dark", Some("File"));
}

#[test]
fn panel_dark() {
    panel(&Theme::for_context(DesignContext { mode: Mode::Dark, ..DesignContext::revision_one() }), "panel_dark");
}
