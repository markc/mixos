// SPDX-License-Identifier: MIT OR Apache-2.0
//! BusViewer's drawing, driven through kittest with no Bus: the About and
//! Shortcuts dialogs close from their button and from Esc, the body's push
//! buttons run their commands and carry "Label (Shortcut)" tooltips, and
//! the search field reports the filter.
use busviewer::view::{UiEvent, view};
use busviewer::{commands, label, strings};
use design::{DesignContext, Mode, Scheme};
use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use inspector::model::Verb;
use inspector::{Dialog, Effect, Engine, Selection, Snapshot};
use toolkit::Theme;

fn engine(dialog: Option<Dialog>) -> Engine {
    let mut s = Snapshot::default();
    let verb = Verb {
        name: "settings.get".into(),
        args: String::new(),
        description: "Read one setting".into(),
        read_only: Some(true),
    };
    s.services.insert("settingsd".into(), Ok(vec![verb]));
    let mut e = Engine::new(label);
    let Some(Effect::Discover { ticket }) = e.take_effects().pop() else {
        panic!("initial discovery")
    };
    e.discovered(ticket, s);
    e.take_effects();
    e.ui.selected = Some(Selection {
        service: "settingsd".into(),
        verb: "settings.get".into(),
    });
    e.ui.dialog = dialog;
    e
}

/// The window over `engine`; the state collects every frame's events.
fn harness(engine: Engine) -> Harness<'static, Vec<UiEvent>> {
    let registry = commands::registry();
    let strings = strings();
    let theme = Theme::for_context(DesignContext {
        scheme: Scheme::Pro,
        mode: Mode::Light,
        ..DesignContext::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(980.0, 620.0))
        .build_ui_state(
            move |ui, events: &mut Vec<UiEvent>| {
                events.extend(view(ui, &engine, &registry, &strings, 2.0))
            },
            Vec::new(),
        );
    toolkit::install(&h.ctx, &theme);
    h.run();
    h
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
    assert!(h.state().contains(&UiEvent::Command("bus.call")));
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
    let field = h.get_by_role(egui::accesskit::Role::TextInput);
    field.click();
    h.run();
    h.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("settings");
    h.run();
    assert!(
        h.state().contains(&UiEvent::Filter("settings".into())),
        "{:?}",
        h.state()
    );
}
