// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::verbs::{self, Reply};
use std::sync::mpsc;
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, Roots, Session) {
    let dir = tempfile::tempdir().unwrap();
    let roots = Roots {
        home: dir.path().into(),
        builtin: dir.path().join("builtin"),
        user: dir.path().join("user"),
        mix: "/opt/mixos/bin/mix".into(),
    };
    let session = Session::with_roots("fixture", Some(roots.clone()));
    (dir, roots, session)
}

#[test]
fn process_handles_share_the_same_state() {
    let a = Session::for_app("session-sharing-test");
    let b = Session::for_app("session-sharing-test");
    assert!(Arc::ptr_eq(&a.0, &b.0));
    assert!(!Arc::ptr_eq(
        &a.0,
        &Session::for_app("another-session-test").0
    ));
}

#[test]
fn initial_list_and_reload_are_deferred_and_cache_the_catalogue() {
    let (_dir, roots, session) = fixture();
    roots
        .create("fixture", crate::Tier::UserGlobal, "hello", "Global")
        .unwrap();
    roots
        .create("fixture", crate::Tier::UserApp, "hello", "App")
        .unwrap();
    let reply = verbs::handle(&session, "list", &json!({}));
    assert!(matches!(reply, Reply::Deferred(_)));
    let body = reply.wait().unwrap();
    assert_eq!(body["scanned"], true);
    assert_eq!(body["macros"][0]["title"], "App");
    assert_eq!(body["macros"][0]["shadows"], json!(["user-global"]));
    assert!(matches!(
        verbs::handle(&session, "list", &json!({})),
        Reply::Immediate(_)
    ));
    let reply = verbs::handle(&session, "reload", &json!({}));
    assert!(matches!(reply, Reply::Deferred(_)));
    reply.wait().unwrap();
    assert!(session.run("missing", "bus").unwrap_err().code == "NOT_FOUND");
}

#[test]
fn completion_is_once_per_run_for_success_failure_signal_and_spawn_failure() {
    let (_dir, mut roots, _) = fixture();
    assert!(roots.mix.is_file(), "CBC needs the native Mix binary");
    let path = roots
        .create("fixture", crate::Tier::UserApp, "test", "Test")
        .unwrap();
    let session = Session::with_roots("fixture", Some(roots.clone()));
    let (tx, rx) = mpsc::channel();
    session.set_finish_hook(Some(Arc::new(move |body| {
        tx.send(body).unwrap();
    })));
    session.reload().unwrap();
    for (run, source, expected, stop) in [
        (1, "exit(0)\n", json!(0), false),
        (2, "exit(7)\n", json!(7), false),
        (3, "while true\nsleep(1)\nend\n", Value::Null, true),
    ] {
        std::fs::write(&path, source).unwrap();
        let accepted = verbs::handle(&session, "run", &json!({"id":"test"}))
            .wait()
            .unwrap();
        assert_eq!(accepted["accepted"], true);
        assert_eq!(accepted["run"], run);
        if stop {
            assert_eq!(session.status()["state"], "running");
            assert!(session.run("test", "gui").unwrap_err().code == "BUSY");
            assert!(session.stop());
        }
        let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(event["run"], run);
        assert_eq!(event["id"], "test");
        assert_eq!(event["title"], "Test");
        assert_eq!(event["origin"], "bus");
        assert_eq!(event["state"], "exited");
        assert_eq!(event["exit"], expected);
        assert_eq!(session.status()["state"], "exited");
        assert!(rx.try_recv().is_err());
    }
    roots.mix = roots.home.join("missing-mix");
    let session = Session::with_roots("fixture", Some(roots));
    let (tx, rx) = mpsc::channel();
    session.set_finish_hook(Some(Arc::new(move |body| {
        tx.send(body).unwrap();
    })));
    session.reload().unwrap();
    session.run("test", "gui").unwrap();
    let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(event["state"], "failed_to_start");
    assert_eq!(event["origin"], "gui");
    assert!(event["error"].is_string());
    assert!(rx.try_recv().is_err());
}

#[test]
fn refusals_distinguish_arguments_availability_scan_and_channel_failures() {
    let (_dir, roots, session) = fixture();
    for (suffix, body) in [
        ("run", json!({})),
        ("run", json!({"id":""})),
        ("run", json!({"id":"test","extra":true})),
        ("new", json!({})),
        ("status", json!({"extra":true})),
        ("status", json!([])),
    ] {
        assert_eq!(
            verbs::handle(&session, suffix, &body)
                .wait()
                .unwrap_err()
                .code,
            "INVALID_ARGUMENT"
        );
    }
    assert_eq!(
        verbs::handle(&session, "new", &json!({"title":"Test"}))
            .wait()
            .unwrap_err()
            .code,
        "UNAVAILABLE"
    );
    let unavailable = Session::with_roots("fixture", None);
    assert_eq!(
        verbs::handle(&unavailable, "reload", &json!({}))
            .wait()
            .unwrap_err()
            .code,
        "UNAVAILABLE"
    );
    // Invalid app names fail root-folder resolution rather than producing diagnostics.
    let invalid = Session::with_roots("../invalid", Some(roots));
    assert_eq!(
        verbs::handle(&invalid, "reload", &json!({}))
            .wait()
            .unwrap_err()
            .code,
        "INTERNAL"
    );
    let (tx, rx) = mpsc::channel();
    drop(tx);
    assert_eq!(Reply::Deferred(rx).wait().unwrap_err().code, "INTERNAL");
}

#[test]
fn output_retains_the_tail_and_reports_dropped_bytes() {
    let (_dir, roots, session) = fixture();
    assert!(roots.mix.is_file(), "CBC needs the native Mix binary");
    let path = roots
        .create("fixture", crate::Tier::UserApp, "tail", "Tail")
        .unwrap();
    let source = format!("print(\"{}end\")\n", "x".repeat(crate::OUTPUT_LIMIT + 10));
    std::fs::write(path, source).unwrap();
    session.reload().unwrap();
    let (tx, rx) = mpsc::channel();
    session.set_finish_hook(Some(Arc::new(move |body| {
        tx.send(body).unwrap();
    })));
    session.run("tail", "bus").unwrap();
    let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(event["dropped"], true);
    let output = session.output();
    assert_eq!(output["run"], 1);
    assert_eq!(output["dropped"], true);
    assert_eq!(output["text"].as_str().unwrap().len(), crate::OUTPUT_LIMIT);
    assert!(output["text"].as_str().unwrap().ends_with("end\n"));
}
