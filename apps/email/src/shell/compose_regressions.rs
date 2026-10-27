// SPDX-License-Identifier: MIT OR Apache-2.0
// Regression coverage for the compose window review. All frames are offscreen.
use super::tests::{add_composer, compose_fixture, composer_choice};
use super::*;

fn queued(shell: &mut Shell, name: &str, events: Vec<UiEvent>) {
    shell.composers.windows[name]
        .lock()
        .unwrap()
        .pending
        .push(crate::composer::Input {
            events,
            request: None,
            mixed: false,
        });
}

fn logic_frame(shell: &mut Shell) {
    let ctx = shell.ctx.clone();
    let _ = ctx.run_logic(&egui::RawInput::default(), |_| shell.logic(&ctx));
}

fn bus_fixture() -> (Shell, test_broker::Broker) {
    use futures::StreamExt;
    let (mut shell, broker) = compose_fixture(false);
    shell.bus.quit();
    let (handle, mut deliveries) =
        citizen::start(APP, "email-regression-test", &broker.url).unwrap();
    shell.bus = handle;
    let tx = shell.tx.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            while let Some(delivery) = deliveries.next().await {
                if tx.send(Event::Delivery(delivery)).is_err() {
                    break;
                }
            }
        });
    });
    (shell, broker)
}

fn bus_answer(
    shell: &mut Shell,
    broker: &test_broker::Broker,
    command: &str,
    args: Value,
) -> (u8, Value) {
    bus_answer_with(shell, broker, command, args, |_| true)
}

fn bus_answer_with(
    shell: &mut Shell,
    broker: &test_broker::Broker,
    command: &str,
    args: Value,
    mut before_frame: impl FnMut(&mut Shell) -> bool,
) -> (u8, Value) {
    let url = broker.url.clone();
    let command = command.to_owned();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (caller, _deliveries) = citizen::start("tester", "compose-caller", &url).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let answer = runtime.block_on(caller.call("email-regression-test", &command, args));
        caller.quit();
        tx.send(answer).unwrap();
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if before_frame(shell) {
            logic_frame(shell);
        }
        shell
            .runtime
            .block_on(async { tokio::time::sleep(std::time::Duration::from_millis(2)).await });
        if let Ok(answer) = rx.try_recv() {
            let answer = answer.unwrap();
            return (answer.rc, serde_json::from_str(&answer.body).unwrap());
        }
        assert!(std::time::Instant::now() < deadline, "Bus answer timed out");
    }
}

#[test]
fn review_fetched_reply_and_forward_answer_with_their_new_windows() {
    let (mut shell, broker) = bus_fixture();
    for (command, name) in [("email.reply", "compose-1"), ("email.forward", "compose-2")] {
        let (rc, body) = bus_answer_with(
            &mut shell,
            &broker,
            command,
            json!({"id":"E1"}),
            |shell| {
                // Hold only the server effect; use the real command validation,
                // Then value and completion path, without an external mail server.
                let mut events = Vec::new();
                let mut handled = false;
                while let Ok(event) = shell.rx.try_recv() {
                    if let Event::Delivery(citizen::Delivery::Command { id, command, body }) =
                        &event
                        && matches!(command.as_str(), "email.reply" | "email.forward")
                    {
                        handled = true;
                        crate::commands::command(
                            &mut shell.state,
                            &shell.commands,
                            &shell.strings,
                            *id,
                            command,
                            body,
                        );
                        let work = shell.state.take_work();
                        for work in work {
                            if let Work::Query {
                                account,
                                reply,
                                then,
                                ..
                            } = work
                            {
                                let tag = Tag {
                                    generation: shell.state.account(&account).unwrap().generation(),
                                    account,
                                };
                                events.push(Event::Account(tag, Done::Answered(reply, then, Ok(json!({
                                "id":"E1", "subject":"original", "from":[{"email":"bob@example.invalid"}], "text":"body"
                            })))));
                            } else {
                                shell.state.queue_work(work);
                            }
                        }
                    } else {
                        events.push(event);
                    }
                }
                for event in events {
                    shell.tx.send(event).unwrap();
                }
                handled
            },
        );
        assert_eq!(rc, 0, "{body}");
        assert_eq!(body["window"], name);
        assert!(shell.composers.windows.contains_key(name));
    }
    shell.bus.quit();
}

#[test]
fn review_child_builder_has_its_own_app_id() {
    let (mut shell, _broker) = compose_fixture(false);
    add_composer(&mut shell, "one");
    add_composer(&mut shell, "two");
    let viewport = shell.composers.windows["compose-2"]
        .lock()
        .unwrap()
        .viewport;
    let ctx = shell.ctx.clone();
    let output = ctx.run_ui(egui::RawInput::default(), |ui| shell.composers.show(ui));
    assert_eq!(
        output.viewport_output[&viewport].builder.app_id.as_deref(),
        Some("dev.mixos.email.compose-2")
    );
    output.drop_without_applying_deltas();
    shell.bus.quit();
}

#[test]
fn review_mailbox_ui_quit_survives_composer_input_in_logic() {
    let (mut shell, _broker) = compose_fixture(false);
    add_composer(&mut shell, "one");
    // A mailbox UI pass executes Quit before the following logic pass.
    apply_ui_events(
        &mut shell.state,
        &shell.commands,
        vec![UiEvent::Command("file.quit", Origin::Menu)],
        None,
    );
    queued(
        &mut shell,
        "compose-1",
        vec![UiEvent::ComposeField(
            crate::state::ComposeField::Subject,
            "edited".into(),
        )],
    );
    logic_frame(&mut shell);
    assert!(shell.composers.quitting);
    assert_eq!(
        shell.composers.windows["compose-1"]
            .lock()
            .unwrap()
            .view
            .confirm,
        Some(crate::state::Confirm::Quit)
    );
    assert!(!shell.exiting);
    shell.bus.quit();
}

#[test]
fn review_save_as_survives_keep_close_and_quit_and_reports_failure() {
    for quitting in [false, true] {
        let (mut shell, _broker) = compose_fixture(false);
        shell.state.compose();
        shell.state.apply_ui(UiEvent::ComposeField(
            crate::state::ComposeField::Subject,
            "save me".into(),
        ));
        shell
            .state
            .write_document("/tmp/compose-review-save.md".into(), false);
        let ticket = shell
            .state
            .take_work()
            .into_iter()
            .find_map(|w| match w {
                Work::MailDocument(crate::documents::DocumentWork::Save { ticket, .. }) => {
                    Some(ticket)
                }
                _ => None,
            })
            .unwrap();
        shell.sync_composers();
        if quitting {
            shell.state.exiting = true;
            shell.advance_close();
        } else {
            queued(&mut shell, "compose-1", vec![UiEvent::CloseCompose]);
            shell.composer_input();
        }
        composer_choice(&mut shell, "compose-1", crate::state::DraftChoice::Keep);
        assert!(
            shell.composers.windows.contains_key("compose-1"),
            "pending Save As must retain its owner"
        );
        assert!(!shell.state.exiting);
        shell
            .tx
            .send(Event::MailDocument(
                ticket,
                crate::documents::Answer::Saved(Err((code::DOCUMENT_IO, "save failed".into()))),
            ))
            .unwrap();
        shell.pump();
        assert_eq!(shell.state.status, "save failed");
        shell.sync_composers();
        assert!(shell.composers.windows.is_empty());
        shell.bus.quit();
    }
}

#[test]
fn review_bus_compose_returns_window_and_creation_beats_older_focus() {
    let (mut shell, broker) = bus_fixture();
    add_composer(&mut shell, "one");
    shell.composers.windows["compose-1"].lock().unwrap().focused = Some(std::time::Instant::now());
    let (rc, body) = bus_answer(
        &mut shell,
        &broker,
        "email.compose",
        json!({"subject":"two"}),
    );
    assert_eq!(rc, 0, "{body}");
    assert_eq!(body["window"], "compose-2");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("attachment.txt");
    std::fs::write(&path, "attachment").unwrap();
    let (rc, body) = bus_answer(
        &mut shell,
        &broker,
        "email.compose.attachment.add",
        json!({"paths":[path]}),
    );
    assert_eq!(rc, 0, "{body}");
    assert_eq!(
        shell.composers.windows["compose-2"]
            .lock()
            .unwrap()
            .view
            .compose
            .as_ref()
            .unwrap()
            .draft
            .attachments,
        vec![path]
    );
    assert!(
        shell.composers.windows["compose-1"]
            .lock()
            .unwrap()
            .view
            .compose
            .as_ref()
            .unwrap()
            .draft
            .attachments
            .is_empty()
    );
    shell.bus.quit();
}

#[test]
fn review_bus_new_creates_and_requests_native_paint_in_the_command_frame() {
    let (mut shell, _broker) = compose_fixture(false);
    add_composer(&mut shell, "one");
    let ctx = shell.ctx.clone();
    let output = ctx.run_logic(&egui::RawInput::default(), |_| {
        shell.command(1, "email.execute", r#"{"id":"compose.new"}"#)
    });
    assert!(shell.composers.windows.contains_key("compose-2"));
    assert!(
        output.viewport_commands[&egui::ViewportId::ROOT]
            .contains(&egui::ViewportCommand::RequestPaintWhileHidden)
    );
    shell.bus.quit();
}

#[test]
fn review_bus_new_and_document_open_return_the_new_window() {
    let (mut shell, broker) = bus_fixture();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("message.md");
    std::fs::write(&path, "---\nmail-subject: document\n---\nBody\n").unwrap();
    for (command, args, name) in [
        ("email.execute", json!({"id":"compose.new"}), "compose-1"),
        ("email.compose", json!({"document":path}), "compose-2"),
    ] {
        let (rc, body) = bus_answer(&mut shell, &broker, command, args);
        assert_eq!(rc, 0, "{body}");
        assert_eq!(body["window"], name);
    }
    shell.bus.quit();
}

#[test]
fn review_failed_bus_document_does_not_leave_a_loading_child() {
    let (mut shell, broker) = bus_fixture();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.md");
    std::fs::write(&path, "# ordinary Markdown\n").unwrap();
    let (rc, _) = bus_answer(
        &mut shell,
        &broker,
        "email.compose",
        json!({"document":path}),
    );
    assert_eq!(rc, 10);
    assert!(shell.composers.windows.is_empty());
    shell.bus.quit();
}

#[test]
fn review_compose_only_synchronous_document_failure_is_visible() {
    let (mut shell, _broker) = compose_fixture(true);
    shell.compose_launch(true, Some("relative.md".into()));
    assert_eq!(
        shell.composers.windows["compose-1"]
            .lock()
            .unwrap()
            .view
            .status,
        label("document-absolute-path")
    );
    shell.bus.quit();
}

#[test]
fn review_synchronous_document_drop_failure_reaches_its_composer() {
    let (mut shell, _broker) = compose_fixture(true);
    add_composer(&mut shell, "one");
    queued(
        &mut shell,
        "compose-1",
        vec![UiEvent::ComposeDrop(vec!["relative.md".into()])],
    );
    shell.composer_input();
    let window = shell.composers.windows["compose-1"].lock().unwrap();
    assert_eq!(
        window.view.compose.as_ref().unwrap().error.as_deref(),
        Some(label("document-absolute-path").as_str())
    );
    shell.bus.quit();
}

#[test]
fn review_readme_drop_attaches_to_its_composer() {
    let (mut shell, _broker) = compose_fixture(false);
    add_composer(&mut shell, "one");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("README.md");
    std::fs::write(&path, "# read me\n").unwrap();
    queued(
        &mut shell,
        "compose-1",
        vec![UiEvent::ComposeDrop(vec![path.clone()])],
    );
    shell.composer_input();
    for _ in 0..100 {
        shell
            .runtime
            .block_on(async { tokio::time::sleep(std::time::Duration::from_millis(2)).await });
        shell.pump();
    }
    assert_eq!(shell.composers.windows.len(), 1);
    assert_eq!(
        shell.composers.windows["compose-1"]
            .lock()
            .unwrap()
            .view
            .compose
            .as_ref()
            .unwrap()
            .draft
            .attachments,
        vec![path]
    );
    shell.bus.quit();
}

#[test]
fn review_mailbox_drops_open_a_composer_with_attachments() {
    #[derive(Debug)]
    struct DropPath(PathBuf);
    impl egui::DroppedFile for DropPath {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }
    for filename in ["notes.txt", "README.md"] {
        let (mut shell, _broker) = compose_fixture(false);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(filename);
        std::fs::write(&path, "ordinary file\n").unwrap();
        let input = egui::RawInput {
            dropped_files: vec![Arc::new(DropPath(path.clone()))],
            ..Default::default()
        };
        let ctx = shell.ctx.clone();
        ctx.run_ui(input, |ui| shell.ui(ui))
            .drop_without_applying_deltas();
        for _ in 0..100 {
            shell
                .runtime
                .block_on(async { tokio::time::sleep(std::time::Duration::from_millis(2)).await });
            shell.pump();
        }
        assert_eq!(shell.composers.windows.len(), 1);
        assert_eq!(
            shell
                .composers
                .windows
                .values()
                .next()
                .unwrap()
                .lock()
                .unwrap()
                .view
                .compose
                .as_ref()
                .unwrap()
                .draft
                .attachments,
            vec![path]
        );
        shell.bus.quit();
    }
}

#[test]
fn review_bus_quit_reports_the_windows_that_will_ask() {
    let (mut shell, broker) = bus_fixture();
    add_composer(&mut shell, "one");
    add_composer(&mut shell, "two");
    let (rc, body) = bus_answer(&mut shell, &broker, "email.quit", json!({}));
    assert_eq!(rc, 0);
    assert_eq!(body["confirming"], true);
    assert_eq!(body["windows"], json!(["compose-1", "compose-2"]));
    shell.bus.quit();
}

#[test]
fn review_send_outcome_reaches_mailbox_after_child_closes() {
    for unfiled in [None, Some("filing failed".to_owned())] {
        let (mut shell, _broker) = compose_fixture(false);
        add_composer(&mut shell, "one");
        shell.composers.windows["compose-1"]
            .lock()
            .unwrap()
            .view
            .compose
            .as_mut()
            .unwrap()
            .sending = Some(99);
        let report = jmap::SendReport {
            unfiled,
            ..Default::default()
        };
        let tag = Tag {
            account: "a".into(),
            generation: shell.state.account("a").unwrap().generation(),
        };
        shell
            .tx
            .send(Event::Account(
                tag,
                Done::Sent(99, Default::default(), Ok(report)),
            ))
            .unwrap();
        shell.pump();
        assert!(shell.composers.windows.is_empty());
        assert!(shell.state.status.contains("sent") || shell.state.status.contains("Sent"));
        if shell.state.status != label("sent") {
            assert!(shell.state.status.contains("filing failed"));
        }
        shell.bus.quit();
    }
}

#[test]
fn review_keep_editing_unrelated_close_does_not_cancel_owner_close() {
    let (mut shell, _broker) = compose_fixture(false);
    add_composer(&mut shell, "one");
    shell.composers.owner_closing = true;
    queued(&mut shell, "compose-1", vec![UiEvent::CloseCompose]);
    shell.composer_input();
    composer_choice(&mut shell, "compose-1", crate::state::DraftChoice::Back);
    assert!(shell.composers.owner_closing);
    shell.bus.quit();
}

#[test]
fn review_window_refusals_are_typed_and_do_not_mutate_composers() {
    let (mut shell, broker) = bus_fixture();
    add_composer(&mut shell, "one");
    for (window, expected) in [
        (json!("missing"), code::NOT_FOUND),
        (json!(7), code::INVALID_ARGUMENT),
    ] {
        for (command, mut args) in [
            ("email.execute", json!({"id":"compose.close"})),
            ("email.execute", json!({"id":"compose.new"})),
            ("email.execute", json!({"id":"file.quit"})),
            ("email.info", json!({})),
            ("email.compose.attachment.add", json!({"paths":[]})),
            (
                "email.compose.attachment.remove",
                json!({"path":"/tmp/missing"}),
            ),
        ] {
            args["window"] = window.clone();
            let (rc, body) = bus_answer(&mut shell, &broker, command, args);
            assert_eq!(rc, 10, "{command}: {body}");
            assert_eq!(body["error_code"], expected, "{command}: {body}");
            assert_eq!(shell.composers.windows.len(), 1);
            assert!(
                shell.composers.windows["compose-1"]
                    .lock()
                    .unwrap()
                    .view
                    .confirm
                    .is_none()
            );
        }
    }
    shell.bus.quit();
}

#[test]
fn review_compose_only_offers_kept_drafts_before_a_blank_message() {
    let (mut shell, _broker) = compose_fixture(true);
    shell.state.offer_drafts(vec![crate::drafts::SavedDraft {
        id: "kept".into(),
        account: "a".into(),
        subject: "kept subject".into(),
        ..Default::default()
    }]);
    shell.compose_launch(true, None);
    let window = shell.composers.windows["compose-1"].clone();
    {
        let w = window.lock().unwrap();
        assert!(w.view.compose.is_none());
        assert_eq!(w.view.restore.len(), 1);
    }
    queued(
        &mut shell,
        "compose-1",
        vec![UiEvent::RestoreDraft(crate::state::DraftChoice::Keep)],
    );
    shell.composer_input();
    assert_eq!(
        window
            .lock()
            .unwrap()
            .view
            .compose
            .as_ref()
            .unwrap()
            .draft
            .subject,
        "kept subject"
    );
    shell.bus.quit();
}
