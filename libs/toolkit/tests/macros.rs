// SPDX-License-Identifier: MIT OR Apache-2.0
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use toolkit::macro_panel::{self, Action, Data, Draft, Row};

#[derive(Default)]
struct Panel {
    open: bool,
    data: Data,
    draft: Draft,
    actions: Vec<Action>,
}

fn harness(data: Data, render: bool) -> Harness<'static, Panel> {
    let builder = Harness::builder().with_size(egui::vec2(800.0, 700.0));
    let builder = if render { builder.wgpu() } else { builder };
    let mut h = builder.build_ui_state(
        |ui, state: &mut Panel| {
            egui::CentralPanel::default().show(ui, |_| {});
            state.actions.extend(macro_panel::show(
                ui.ctx(),
                &mut state.open,
                &state.data,
                &mut state.draft,
            ));
        },
        Panel {
            open: true,
            data,
            ..Panel::default()
        },
    );
    toolkit::install(&h.ctx, &toolkit::Theme::embedded());
    h.run();
    h.run_steps(40);
    h
}

fn populated() -> Data {
    Data {
        rows: vec![Row {
            id: "hello".into(),
            title: "Hello desktop".into(),
            tier: "User for this app".into(),
            path: "/home/user/.config/mixos/macros/ced/hello.mix".into(),
            shadows: "Built-in global".into(),
        }],
        ..Data::default()
    }
}

#[test]
fn empty_catalogue_still_offers_creation_folders_and_reload() {
    for (label, action) in [
        ("New global", Action::New(false)),
        ("New for this app", Action::New(true)),
        ("Open global folder", Action::OpenFolder(false)),
        ("Open app folder", Action::OpenFolder(true)),
        ("Reload", Action::Reload),
    ] {
        let mut h = harness(Data::default(), false);
        h.get_by_label(label).click();
        h.run();
        assert!(h.state().actions.contains(&action), "{label}");
    }
}

#[test]
fn rows_offer_run_and_edit_and_a_running_macro_offers_stop() {
    for (label, action) in [("Run", Action::Run(0)), ("Edit", Action::Edit(0))] {
        let mut h = harness(populated(), false);
        h.get_by_label(label).click();
        h.run();
        assert!(h.state().actions.contains(&action));
    }
    let mut data = populated();
    data.running = true;
    let mut h = harness(data, false);
    h.get_by_label("Stop").click();
    h.run();
    assert!(h.state().actions.contains(&Action::Stop));
    assert!(!h.state().actions.contains(&Action::Run(0)));
}

#[test]
fn close_updates_the_open_flag() {
    let mut h = harness(Data::default(), false);
    h.get_by_label("Close").click();
    h.run();
    assert!(!h.state().open);
}

#[derive(Default)]
struct AppPanel {
    open: bool,
    modal: bool,
}

fn app_harness() -> Harness<'static, AppPanel> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(800.0, 700.0))
        .build_ui_state(
            |ui, state: &mut AppPanel| {
                let toggle = toolkit::macros::command(
                    |s: &AppPanel| s.open || !s.modal,
                    |s: &mut AppPanel| s.open = !s.open,
                );
                let mut fired = false;
                egui::CentralPanel::default().show(ui, |ui| {
                    fired = ui.add_enabled((toggle.enabled)(state), egui::Button::new("Toggle macros")).clicked();
                });
                let close = toolkit::macros::show_app(ui.ctx(), "ced", state.open);
                // Apps queue UI commands, then apply the panel's unguarded,
                // idempotent close. Both can arrive in the same frame.
                if fired && !close {
                    (toggle.run)(state);
                }
                if close {
                    state.open = false;
                }
            },
            AppPanel { open: true, modal: true },
        );
    toolkit::install(&h.ctx, &toolkit::Theme::embedded());
    h.run_steps(40);
    h
}

#[test]
fn app_panel_closes_even_when_opening_commands_are_disabled() {
    let mut h = app_harness();
    h.get_by_label("Close").click();
    h.run_steps(40);
    assert!(!h.state().open);
    assert!(h.state().modal);
    h.run_steps(40);
    assert!(!h.state().open);
}

#[test]
fn app_panel_toggle_and_outside_dismissal_do_not_reopen_it() {
    let mut h = app_harness();
    h.get_by_label("Toggle macros").click();
    h.run_steps(40);
    assert!(!h.state().open);
    h.run_steps(40);
    assert!(!h.state().open);
}

#[test]
fn macro_panel_snapshots() {
    let mut empty = harness(Data::default(), true);
    empty.snapshot("macros_empty");
    let mut data = populated();
    data.running = true;
    data.status = "Running".into();
    data.output = "Hello from Mix\nWorking…\n".into();
    data.dropped = true;
    let mut running = harness(data, true);
    running.snapshot("macros_running");
}
