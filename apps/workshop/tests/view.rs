// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Workshop's drawing, driven through kittest with no Bus: the About and
//! Shortcuts dialogs close from their button and from Esc, the body's push
//! buttons run their commands and carry "Label (Shortcut)" tooltips, and
//! the search field reports the filter.
use design::{DesignContext, Mode, Scheme};
use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use inspector::model::Command;
use inspector::{Dialog, Effect, Engine, Selection, Snapshot};
use toolkit::Theme;
use workshop::view::{UiEvent, view};
use workshop::{actions, label, strings};

fn engine(dialog: Option<Dialog>) -> Engine {
    let mut s = Snapshot::default();
    let command_name = Command {
        name: "settings.get".into(),
        args: String::new(),
        description: "Read one setting".into(),
        read_only: Some(true),
    };
    s.services
        .insert("settingsd".into(), Ok(vec![command_name]));
    let mut e = Engine::new(label);
    let Some(Effect::Discover { ticket }) = e.take_effects().pop() else {
        panic!("initial discovery")
    };
    e.discovered(ticket, s);
    e.take_effects();
    e.ui.selected = Some(Selection {
        service: "settingsd".into(),
        command: "settings.get".into(),
    });
    e.ui.dialog = dialog;
    e
}

/// The window over `engine`; the state collects every frame's events.
fn harness(engine: Engine) -> Harness<'static, Vec<UiEvent>> {
    sized_harness(engine, egui::vec2(980.0, 620.0))
}

fn sized_harness(engine: Engine, size: egui::Vec2) -> Harness<'static, Vec<UiEvent>> {
    let registry = actions::registry();
    let strings = strings();
    let theme = Theme::for_context(DesignContext {
        scheme: Scheme::Pro,
        mode: Mode::Light,
        ..DesignContext::default()
    });
    let mut h = Harness::builder().with_size(size).build_ui_state(
        move |ui, events: &mut Vec<UiEvent>| events.extend(view(ui, &engine, &registry, &strings)),
        Vec::new(),
    );
    toolkit::install(&h.ctx, &theme);
    h.run();
    h
}

#[test]
fn auto_dock_is_right_when_wide_and_bottom_when_narrow() {
    for (size, right) in [
        (egui::vec2(1920.0, 1080.0), true),
        (egui::vec2(900.0, 700.0), false),
    ] {
        let h = sized_harness(engine(None), size);
        let traffic = h.get_by_label(&label("traffic")).rect();
        let details = h.get_by_label(&label("details")).rect();
        if right {
            assert!(traffic.left() > details.right(), "{traffic:?} {details:?}");
            let panel = |name| {
                egui::containers::panel::PanelState::load(&h.ctx, egui::Id::unique(name))
                    .unwrap()
                    .outer_rect
            };
            assert!((panel("traffic-right").top() - panel("services").top()).abs() < 1.0);
            assert!(
                (panel("traffic-right").bottom() - panel("toolkit-status-bar").top()).abs() < 1.0
            );
        } else {
            assert!(traffic.top() > details.bottom(), "{traffic:?} {details:?}");
        }
    }
}

#[test]
fn explicit_bottom_and_saved_sizes_are_honoured() {
    for side in [inspector::DockSide::Bottom, inspector::DockSide::Right] {
        let mut e = engine(None);
        e.ui.dock_side = side;
        e.set_dock_size(side, 0.4);
        let h = sized_harness(e, egui::vec2(1920.0, 1080.0));
        let traffic = h.get_by_label(&label("traffic")).rect();
        let details = h.get_by_label(&label("details")).rect();
        let (id, extent) = if side == inspector::DockSide::Right {
            assert!(traffic.left() > details.right());
            ("traffic-right", 1920.0)
        } else {
            assert!(traffic.top() > details.bottom());
            ("traffic-bottom", 1080.0)
        };
        let panel =
            egui::containers::panel::PanelState::load(&h.ctx, egui::Id::unique(id)).unwrap();
        let size = if side == inspector::DockSide::Right {
            panel.size().x
        } else {
            panel.size().y
        };
        assert!((size / extent - 0.4).abs() < 0.02, "{size} / {extent}");
    }
}

#[test]
fn dock_sizes_remain_proportional_when_the_window_resizes() {
    for (side, id) in [
        (inspector::DockSide::Right, "traffic-right"),
        (inspector::DockSide::Bottom, "traffic-bottom"),
    ] {
        let mut e = engine(None);
        e.ui.dock_side = side;
        e.set_dock_size(side, 0.4);
        let mut h = sized_harness(e, egui::vec2(1920.0, 1080.0));
        h.set_size(egui::vec2(1400.0, 800.0));
        h.run();
        let panel =
            egui::containers::panel::PanelState::load(&h.ctx, egui::Id::unique(id)).unwrap();
        let share = if side == inspector::DockSide::Right {
            panel.size().x / 1400.0
        } else {
            panel.size().y / 800.0
        };
        assert!((share - 0.4).abs() < 0.02, "{share}");
    }
}

#[test]
fn the_header_button_flips_the_effective_side() {
    for (size, target) in [
        (egui::vec2(1920.0, 1080.0), inspector::DockSide::Bottom),
        (egui::vec2(900.0, 700.0), inspector::DockSide::Right),
    ] {
        let mut h = sized_harness(engine(None), size);
        h.get_by_label(&label(if target == inspector::DockSide::Bottom {
            "traffic-switch-bottom"
        } else {
            "traffic-switch-right"
        }))
        .click();
        h.run();
        assert!(h.state().contains(&UiEvent::DockSide(target)));
    }
}

#[test]
fn collapsed_traffic_reopens_with_one_click() {
    let mut e = engine(None);
    e.ui.dock_side = inspector::DockSide::Right;
    e.workshop.dock_open = false;
    let mut h = sized_harness(e, egui::vec2(1920.0, 1080.0));
    h.get_by_label(&label("traffic")).click();
    h.run();
    assert!(h.state().contains(&UiEvent::Dock(true)));
}

fn closed(h: &Harness<'_, Vec<UiEvent>>) -> bool {
    h.state().contains(&UiEvent::CloseDialog)
}

#[test]
fn the_about_dialog_closes_from_done_and_from_escape() {
    let mut h = harness(engine(Some(Dialog::About)));
    let _ = h.get_by_role_and_label(egui::accesskit::Role::Dialog, &label("about"));
    assert!(!closed(&h));
    h.get_by_label(&label("done")).click();
    h.run();
    assert!(closed(&h), "Done closes it");

    let mut h = harness(engine(Some(Dialog::Shortcuts)));
    h.key_press(Key::Escape);
    h.run();
    assert!(closed(&h), "Esc closes it");
}

#[test]
fn the_body_buttons_run_their_commands_and_name_their_shortcuts() {
    let mut h = harness(engine(None));
    h.get_by_label(&label("call")).click();
    h.run();
    assert!(
        h.state()
            .contains(&UiEvent::Command("bus.call", toolkit::action::Origin::Ui))
    );
    let at = h.get_by_label(&label("call")).rect().center();
    h.hover_at(at);
    h.run_steps(4);
    let tooltip = format!(
        "{} ({})",
        label("call"),
        h.ctx.format_shortcut(&egui::KeyboardShortcut::new(
            egui::Modifiers::COMMAND,
            Key::Enter
        ))
    );
    let _ = h.get_by_label(&tooltip);
}

#[test]
fn the_search_field_reports_the_filter() {
    let mut h = harness(engine(None));
    let search = label("search");
    let field = h.get_by_label(&search);
    field.click();
    h.run();
    h.get_by_label(&label("search")).type_text("settings");
    h.run();
    assert!(
        h.state().contains(&UiEvent::Filter("settings".into())),
        "{:?}",
        h.state()
    );
}
