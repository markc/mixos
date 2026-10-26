// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The name prompt (New Folder, Rename) through its real view: Enter in the
//! field submits a valid name, and leaves an invalid one open.
use dopus::app::{Answer, App, Dialog};
use dopus::view::{UiEvent, view};
use dopus::{actions, strings};
use dopus_core::{DOpusConfig, DopusCore, PromptKind};
use egui_kittest::Harness;

struct State {
    app: App,
    events: Vec<UiEvent>,
    registry: toolkit::Registry<App>,
    strings: toolkit::Strings,
}

fn answers_after_enter(name: &str) -> Vec<Answer> {
    let dir = tempfile::tempdir().unwrap();
    let mut config = DOpusConfig::default();
    config.left.path = dir.path().to_owned();
    config.right.path = dir.path().to_owned();
    let (core, _events) = DopusCore::new(config, None);
    let mut app = App::new(core);
    app.open(Dialog::Prompt {
        token: 1,
        kind: PromptKind::NewFolder,
        text: name.into(),
    });
    let state = State {
        app,
        events: Vec::new(),
        registry: actions::registry(),
        strings: strings(),
    };
    let mut harness = Harness::builder()
        .with_context_setup(|ctx| toolkit::install(ctx, &toolkit::Theme::embedded()))
        .build_ui_state(
            |ui, s: &mut State| {
                let events = view(ui, &s.app, &s.registry, &s.strings);
                s.events.extend(events);
            },
            state,
        );
    // The field takes the keyboard on the first frames.
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    harness
        .state()
        .events
        .iter()
        .filter_map(|e| match e {
            UiEvent::Answer(answer) => Some(answer.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn enter_submits_a_valid_name() {
    assert_eq!(answers_after_enter("Projects"), [Answer::Accept]);
}

#[test]
fn enter_keeps_an_invalid_name_open() {
    assert!(answers_after_enter("a/b").is_empty());
}
