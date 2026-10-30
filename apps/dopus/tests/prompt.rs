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

/// Bus actions, real worker IO and drive input all use the normal window path.
#[test]
fn bus_file_operations_accept_driven_prompts_confirmations_and_drops() {
    use dopus::commands::{Command, ServerMeta};
    use dopus_core::{CoreEvent, PaneId};
    use serde_json::json;
    use toolkit::drive;
    struct Window {
        app: App,
        events: std::sync::mpsc::Receiver<CoreEvent>,
        answers: Vec<drive::Answer>,
    }
    let dir = tempfile::tempdir().unwrap();
    let left = dir.path().join("left");
    let right = dir.path().join("right");
    std::fs::create_dir(&left).unwrap();
    std::fs::create_dir(&right).unwrap();
    let mut config = DOpusConfig::default();
    config.left.path = left.clone();
    config.right.path = right.clone();
    let (core, events) = DopusCore::new(config, None);
    let registry = actions::registry();
    let strings = strings();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(980.0, 620.0))
        .with_context_setup(|ctx| {
            toolkit::install(ctx, &toolkit::Theme::embedded());
            drive::install(ctx, "dopus");
        })
        .build_ui_state(
            move |ui, window: &mut Window| {
                for event in window.events.try_iter() {
                    let derived = window.app.core.on_event(event);
                    window.app.absorb(derived);
                }
                let derived = window.app.core.tick(std::time::Instant::now());
                window.app.absorb(derived);
                let events = view(ui, &window.app, &registry, &strings);
                dopus::view::apply(&mut window.app, &registry, events);
                window.answers.extend(drive::logic(ui.ctx()));
            },
            Window {
                app: App::new(core),
                events,
                answers: Vec::new(),
            },
        );
    let meta = ServerMeta {
        service: "dopus".into(),
        headless: false,
        window_available: true,
        config_path: None,
        theme_scheme: String::new(),
        theme_mode: String::new(),
        appearance: Default::default(),
        actions: Vec::new(),
        extra_commands: Vec::new(),
    };
    let wait = |h: &mut Harness<'_, Window>, done: &dyn Fn(&Window) -> bool| {
        for _ in 0..500 {
            h.step();
            if done(h.state())
                && !h.state().app.core.pane(PaneId::Left).listing
                && !h.state().app.core.pane(PaneId::Right).listing
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!(
            "file operation did not finish: status={:?}, dialog={:?}, answers={:?}",
            h.state().app.status,
            h.state().app.dialog,
            h.state().answers
        );
    };
    let action = |h: &mut Harness<'_, Window>, id: &str| {
        h.state_mut().app.effects.clear();
        h.state_mut().app.serve(
            &Command {
                id: 1,
                command: "dopus.execute".into(),
                body: json!({"id":id}).to_string(),
            },
            &meta,
        );
        assert!(
            matches!(
                &h.state().app.effects[..],
                [dopus::app::Effect::Reply { rc: 0, .. }]
            ),
            "{id}: {:?}",
            h.state().app.effects
        );
        // A new modal first draws an invisible sizing pass. Settle its
        // placement and keyboard focus before submitting drive input.
        h.run();
    };
    let drive_answer = |h: &mut Harness<'_, Window>,
                        command: &str,
                        args: serde_json::Value|
     -> serde_json::Value {
        h.state_mut().answers.clear();
        let immediate = drive::request(&h.ctx, 2, command, &args);
        h.state_mut().answers.extend(immediate);
        wait(h, &|w| w.answers.iter().any(|a| a.id == 2));
        let answer = h.state().answers.iter().find(|a| a.id == 2).unwrap();
        assert_eq!(answer.rc, 0, "{command} {args}: {}", answer.body);
        answer.body.clone()
    };
    let drive_input = |h: &mut Harness<'_, Window>, command: &str, args: serde_json::Value| {
        drive_answer(h, command, args);
    };
    // "Name" also labels both panes' column headers, so find the prompt's
    // text field in the tree and click it by id.
    let click_name_field = |h: &mut Harness<'_, Window>| {
        let tree = drive_answer(h, "ui.tree", json!({}));
        let id = tree["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["role"] == "TextInput" && n["label"] == dopus::label("prop-name"))
            .expect("the prompt's name field")["id"]
            .as_str()
            .unwrap()
            .to_owned();
        drive_input(h, "ui.click", json!({"id": id}));
    };
    wait(&mut harness, &|w| !w.app.core.pane(PaneId::Left).listing);
    action(&mut harness, "file.new-folder");
    assert!(matches!(
        harness.state().app.dialog,
        Some(Dialog::Prompt { .. })
    ));
    click_name_field(&mut harness);
    drive_input(
        &mut harness,
        "ui.key",
        json!({"key":"A","modifiers":["ctrl"]}),
    );
    drive_input(&mut harness, "ui.type", json!({"text":"Fresh"}));
    assert!(matches!(
        &harness.state().app.dialog,
        Some(Dialog::Prompt { text, .. }) if text == "Fresh"
    ));
    drive_input(
        &mut harness,
        "ui.click",
        json!({"label":dopus::label("ok")}),
    );
    wait(&mut harness, &|w| {
        left.join("Fresh").is_dir() && !w.app.core.availability().operation_running
    });
    // Use a file for rename/copy/move/delete so every operation proves bytes.
    std::fs::write(left.join("original.txt"), "payload").unwrap();
    harness.state_mut().app.core.refresh_in(PaneId::Left);
    wait(&mut harness, &|w| {
        w.app
            .core
            .visible_rows(PaneId::Left)
            .iter()
            .any(|r| r.entry.path == left.join("original.txt"))
    });
    harness
        .state_mut()
        .app
        .core
        .select_path(PaneId::Left, Some(left.join("original.txt")));
    action(&mut harness, "file.rename");
    click_name_field(&mut harness);
    drive_input(
        &mut harness,
        "ui.key",
        json!({"key":"A","modifiers":["ctrl"]}),
    );
    drive_input(&mut harness, "ui.type", json!({"text":"renamed.txt"}));
    assert!(matches!(
        &harness.state().app.dialog,
        Some(Dialog::Prompt { text, .. }) if text == "renamed.txt"
    ));
    drive_input(
        &mut harness,
        "ui.click",
        json!({"label":dopus::label("ok")}),
    );
    wait(&mut harness, &|w| {
        left.join("renamed.txt").is_file() && !w.app.core.availability().operation_running
    });
    harness
        .state_mut()
        .app
        .core
        .select_path(PaneId::Left, Some(left.join("renamed.txt")));
    action(&mut harness, "file.copy-other-pane");
    wait(&mut harness, &|w| {
        right.join("renamed.txt").is_file() && !w.app.core.availability().operation_running
    });
    assert_eq!(
        std::fs::read_to_string(right.join("renamed.txt")).unwrap(),
        "payload"
    );
    // Drop Copy uses the same driven answer path as a real gesture.
    let dropped = left.join("dropped.txt");
    std::fs::write(&dropped, "drop").unwrap();
    harness
        .state_mut()
        .app
        .dropped(PaneId::Left, vec![dropped.clone()], right.clone());
    harness.run();
    drive_input(
        &mut harness,
        "ui.click",
        json!({"label":dopus::label("copy-here")}),
    );
    wait(&mut harness, &|w| {
        right.join("dropped.txt").is_file() && !w.app.core.availability().operation_running
    });
    assert!(dropped.is_file());
    // Remove the copy first: the worker refuses destination collisions.
    std::fs::remove_file(right.join("renamed.txt")).unwrap();
    harness
        .state_mut()
        .app
        .core
        .select_path(PaneId::Left, Some(left.join("renamed.txt")));
    action(&mut harness, "file.move-other-pane");
    wait(&mut harness, &|w| {
        !left.join("renamed.txt").exists()
            && right.join("renamed.txt").is_file()
            && !w.app.core.availability().operation_running
    });
    harness.state_mut().app.core.set_active_pane(PaneId::Right);
    harness
        .state_mut()
        .app
        .core
        .select_path(PaneId::Right, Some(right.join("renamed.txt")));
    action(&mut harness, "file.delete");
    assert!(matches!(
        harness.state().app.dialog,
        Some(Dialog::Confirm { .. })
    ));
    assert!(
        right.join("renamed.txt").is_file(),
        "delete still waits for confirmation"
    );
    drive_input(
        &mut harness,
        "ui.click",
        json!({"label":dopus::label("yes")}),
    );
    wait(&mut harness, &|w| {
        !right.join("renamed.txt").exists() && !w.app.core.availability().operation_running
    });
}
