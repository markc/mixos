// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The client-side title bar's window behaviour: each caption button sends
//! its viewport command, the free bar maximizes on double-click, and its
//! menus come from the registry.
use egui::{ViewportCommand, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use toolkit::action::{Registry, always};
use toolkit::{Icon, Strings, Theme, titlebar};

const FTL: &str = "menu-file = File\ncmd-quit = Quit\n";

fn registry() -> Registry<()> {
    let mut r = Registry::new();
    r.add(toolkit::Action {
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
            // build_ui adds an outer frame; window chrome uses the viewport root.
            let viewport = ui.ctx().content_rect();
            let mut root = ui.new_child(egui::UiBuilder::new().max_rect(viewport));
            root.set_clip_rect(viewport);
            let _ = titlebar::show(
                &mut root,
                "Demo",
                Some(Icon::Server),
                &registry,
                &(),
                &strings,
            );
            egui::CentralPanel::default().show(&mut root, |_| {});
            titlebar::edges(&mut root);
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
fn default_menu_and_caption_positions_keep_the_inner_margin() {
    let h = harness();
    let metrics = toolkit::chrome::Chrome::of(&h.ctx).metrics;
    assert_eq!(
        h.get_by_label("File").rect().left(),
        metrics.title_bar_margin + metrics.mark + metrics.mark_gap,
    );
    assert_eq!(h.get_by_label("Close").rect().right(), 640.0);
}

fn small_drag(h: &mut Harness<'_>, from: egui::Pos2) -> bool {
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    let mut moved = commands(h).contains(&ViewportCommand::StartDrag);
    let to = from + egui::vec2(7.0, 0.0);
    h.input_mut().events.push(egui::Event::PointerMoved(to));
    h.step();
    moved |= commands(h).contains(&ViewportCommand::StartDrag);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    moved || commands(h).contains(&ViewportCommand::StartDrag)
}

#[test]
fn dragging_click_only_chrome_never_moves_the_window() {
    for label in ["File", "Minimize"] {
        let mut h = harness();
        let from = h.get_by_label(label).rect().center();
        assert!(
            !small_drag(&mut h, from),
            "drag from {label} moved the window"
        );
    }
    let mut h = harness();
    assert!(small_drag(&mut h, egui::pos2(200.0, 10.0)));
}

#[test]
fn double_clicking_the_free_bar_maximizes() {
    let mut h = harness();
    // The middle of the bar, clear of the menus and the caption buttons.
    let at = egui::pos2(200.0, 10.0);
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

#[test]
fn mark_and_empty_bar_request_the_wayland_window_menu() {
    for wayland in [false, true] {
        design::decorations::with_resolver(
            std::sync::Arc::new(design::decorations::Resolver::new(
                move || design::decorations::SessionFacts {
                    wayland,
                    desktops: "KDE".into(),
                    decoration_global: Some(true),
                },
                || None,
                |_| {},
            )),
            || {
                for (button, mark) in [
                    (egui::PointerButton::Primary, true),
                    (egui::PointerButton::Secondary, false),
                ] {
                    let mut h = harness();
                    toolkit::install(
                        &h.ctx,
                        &Theme::embedded()
                            .framed(design::Decorations::Client, design::CaptionSide::Auto),
                    );
                    h.run();
                    let m = toolkit::chrome::Chrome::of(&h.ctx).metrics;
                    let at = egui::pos2(
                        if mark {
                            m.title_bar_margin + m.mark / 2.0
                        } else {
                            200.0
                        },
                        m.title_bar_height / 2.0,
                    );
                    h.input_mut().events.push(egui::Event::PointerMoved(at));
                    h.step();
                    for pressed in [true, false] {
                        h.input_mut().events.push(egui::Event::PointerButton {
                            pos: at,
                            button,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        });
                        h.step();
                    }
                    let key = egui::Id::unique(("eframe.window-menu", ViewportId::ROOT));
                    assert_eq!(
                        h.ctx.data(|d| d.get_temp::<egui::Pos2>(key)),
                        wayland.then_some(at)
                    );
                    assert_eq!(
                        h.ctx.data(|d| d.get_temp::<egui::Pos2>(egui::Id::new((
                            "eframe.window-menu",
                            ViewportId::from_hash_of("child")
                        )))),
                        None
                    );
                    assert!(!commands(&h).contains(&ViewportCommand::StartDrag));
                }
            },
        );
    }
}

fn plain(id: &'static str) -> toolkit::Action<bool> {
    toolkit::Action {
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
    r.add(plain("help.macros"));
    r.add(plain("help.keys"));
    r.add(plain("view.search"));
    r.add(toolkit::Action {
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
            "help.macros",
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
            Icon::ScrollText,
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
            titlebar::Action::new("help.macros", Icon::Keyboard),
            titlebar::Action::new("view.mode", Icon::Moon),
        ],
    );
    let ids: Vec<_> = items.iter().map(|i| i.command).collect();
    assert_eq!(
        ids,
        ["help.macros", "view.search", "help.keys", "view.mode"]
    );
}

#[test]
fn compact_menu_titlebars() {
    use design::{CaptionSide, MenuSide, MenuStyle, RequestedDecorations};
    let mut results = egui_kittest::SnapshotResults::new();
    for decorations in [RequestedDecorations::Client, RequestedDecorations::Server] {
        for style in MenuStyle::ALL {
            for side in MenuSide::ALL {
                let registry = registry();
                let strings = Strings::new(FTL);
                let mut h = Harness::builder()
                    .with_size(egui::vec2(640.0, 240.0))
                    .wgpu()
                    .build_ui(move |ui| {
                        let _ = titlebar::show_bar(
                            ui,
                            "Demo",
                            Some(Icon::Server),
                            &registry,
                            &(),
                            &strings,
                            &[],
                        );
                        egui::CentralPanel::default().show(ui, |_| {});
                    });
                toolkit::install(
                    &h.ctx,
                    &Theme::embedded()
                        .framed(decorations, CaptionSide::Right)
                        .with_menus(style, side),
                );
                toolkit::drive::install(&h.ctx, "demo");
                toolkit::drive::set_service(&h.ctx, Some("demo"));
                h.run();
                h.snapshot(format!(
                    "menu_{}_{}_{}",
                    decorations.name(),
                    style.name(),
                    side.name()
                ));
                if style.effective(toolkit::chrome::Chrome::of(&h.ctx).decorations)
                    == MenuStyle::Button
                {
                    h.get_by_label("Menus").click();
                    h.run();
                    h.get_by_label("File").click();
                    h.run();
                    h.get_by_label("Quit");
                    h.snapshot(format!(
                        "menu_open_{}_{}_{}",
                        decorations.name(),
                        style.name(),
                        side.name()
                    ));
                }
                results.extend(h.take_snapshot_results());
            }
        }
    }
}

#[test]
fn titlebar_without_a_drive_keeps_an_app_menu_button() {
    let mut h = harness();
    toolkit::install(
        &h.ctx,
        &Theme::embedded()
            .framed(
                design::RequestedDecorations::Server,
                design::CaptionSide::Right,
            )
            .with_menus(design::MenuStyle::Titlebar, design::MenuSide::Left),
    );
    h.run();
    assert_eq!(toolkit::menu::form(&h.ctx)["effective"], "button");
    h.get_by_label("Menus").click();
    h.run();
    h.get_by_label("File").click();
    h.run();
    h.get_by_label("Quit");
}

#[test]
fn title_band_layouts_and_hosted_actions() {
    on_mixos(title_band_layouts_and_hosted_actions_on_host);
}

fn title_band_layouts_and_hosted_actions_on_host() {
    let mut results = egui_kittest::SnapshotResults::new();
    for (name, decorations, service, title, menu, actions, captions) in [
        (
            "csd_center",
            design::Decorations::Client,
            true,
            "center",
            "left",
            "right",
            "auto",
        ),
        (
            "csd_right",
            design::Decorations::Client,
            true,
            "right",
            "center",
            "left",
            "center",
        ),
        (
            "ssd_hosted",
            design::Decorations::Server,
            true,
            "center",
            "center",
            "left",
            "left",
        ),
        (
            "ssd_fallback",
            design::Decorations::Server,
            false,
            "left",
            "right",
            "center",
            "right",
        ),
    ] {
        let registry = registry();
        let strings = Strings::new(FTL);
        let mut h = Harness::builder()
            .with_size(egui::vec2(640.0, 200.0))
            .wgpu()
            .build_ui(move |ui| {
                titlebar::show_bar(
                    ui,
                    "Demo",
                    Some(Icon::Server),
                    &registry,
                    &(),
                    &strings,
                    &[titlebar::Action::new("file.quit", Icon::X)],
                );
                egui::CentralPanel::default().show(ui, |_| {});
            });
        toolkit::drive::install(&h.ctx, "demo");
        toolkit::drive::set_service(&h.ctx, service.then_some("demo"));
        toolkit::install(
            &h.ctx,
            &Theme::embedded()
                .framed(decorations, design::CaptionSide::resolve(captions))
                .with_menus(design::MenuStyle::Titlebar, design::MenuSide::resolve(menu))
                .with_band(design::title_band::Settings::from_names(
                    title, actions, "titlebar", "mixos",
                )),
        );
        h.run();
        let hosted = decorations == design::Decorations::Server && service;
        let form = titlebar::hosting::form(&h.ctx);
        assert_eq!(form["hosted"], hosted);
        if hosted {
            assert_eq!(form["bar"]["items"][0]["command"], "file.quit");
            assert!(form["bar"]["revision"].as_u64().unwrap() > 0);
            assert!(h.query_all_by_label("Quit").next().is_none());
        } else {
            assert_eq!(form["effective"], "row");
        }
        h.snapshot(format!("title_band_{name}"));
        results.extend(h.take_snapshot_results());
    }
}

fn on_mixos(operation: impl FnOnce()) {
    design::decorations::with_resolver(
        std::sync::Arc::new(design::decorations::Resolver::new(
            || design::decorations::SessionFacts {
                wayland: true,
                desktops: "MixOS".into(),
                decoration_global: Some(true),
            },
            || None,
            |_| {},
        )),
        operation,
    );
}
