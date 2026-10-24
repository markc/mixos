// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn workshop_new_refreshes_global_list_immediately() {
    let (_dir, roots, _) = fixture();
    let session = Session::with_roots("workshop", Some(roots.clone()));
    let created = session.create("Global new").unwrap();
    assert_eq!(created["scanned"], true);
    assert_eq!(created["path"], json!(roots.user.join("global-new.mix")));
    let catalogue = session.catalogue().unwrap();
    assert_eq!(catalogue.entries.len(), 1);
    assert_eq!(catalogue.entries[0].id, "global-new");
}
use crate::verbs::{self, Reply};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn management_verbs_refresh_tiers_and_refuse_invalid_mutations() {
    let (_dir, roots, session) = fixture();
    session.create("Managed").unwrap();
    let ask = |suffix, body| verbs::handle(&session, suffix, &body).wait();
    assert_eq!(
        ask("get", json!({"id":"managed"})).unwrap()["tier"],
        "user-app"
    );
    ask(
        "put",
        json!({"id":"managed","text":"-- title: Renamed\nprint(1)\n"}),
    )
    .unwrap();
    assert_eq!(
        ask("list", json!({})).unwrap()["macros"][0]["title"],
        "Renamed"
    );
    assert_eq!(
        ask("get", json!({"id":"missing"})).unwrap_err().code,
        "NOT_FOUND"
    );
    assert_eq!(
        ask("put", json!({"id":"managed","text":"x".repeat(256*1024+1)}))
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );
    for (suffix, body) in [
        ("get", json!({})),
        ("get", json!({"id":true})),
        ("put", json!({"id":"managed"})),
        ("remove", json!({"id":"managed","extra":0})),
        ("move", json!({"id":"managed","tier":"builtin-app"})),
    ] {
        assert_eq!(ask(suffix, body).unwrap_err().code, "INVALID_ARGUMENT");
    }
    roots
        .create("fixture", Tier::UserGlobal, "managed", "Clash")
        .unwrap();
    assert_eq!(
        ask("move", json!({"id":"managed","tier":"user-global"}))
            .unwrap_err()
            .code,
        "ALREADY_EXISTS"
    );
    std::fs::remove_file(roots.user.join("managed.mix")).unwrap();
    ask("move", json!({"id":"managed","tier":"user-global"})).unwrap();
    assert!(
        ask("list", json!({})).unwrap()["macros"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let global = Session::with_roots("workshop", Some(roots.clone()));
    assert_eq!(
        verbs::handle(&global, "get", &json!({"id":"managed"}))
            .wait()
            .unwrap()["tier"],
        "user-global"
    );
    verbs::handle(&global, "remove", &json!({"id":"managed"}))
        .wait()
        .unwrap();
    assert!(!roots.user.join("managed.mix").exists());
    std::fs::create_dir_all(roots.builtin.join("fixture")).unwrap();
    std::fs::write(
        roots.builtin.join("fixture/builtin.mix"),
        "-- title: Builtin\n",
    )
    .unwrap();
    assert!(ask("get", json!({"id":"builtin"})).is_ok());
    assert_eq!(ask("copy", json!({"id":"managed"})).unwrap_err().code, "NOT_FOUND");
    for (suffix, body) in [
        ("put", json!({"id":"builtin","text":"x"})),
        ("remove", json!({"id":"builtin"})),
        ("move", json!({"id":"builtin","tier":"user-global"})),
    ] {
        assert_eq!(ask(suffix, body).unwrap_err().code, "DENIED");
    }
    ask("copy", json!({"id":"builtin"})).unwrap();
    assert_eq!(ask("get", json!({"id":"builtin"})).unwrap()["tier"], "user-app");
    assert_eq!(ask("copy", json!({"id":"builtin"})).unwrap_err().code, "DENIED");
}

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
fn shutdown_reaps_a_run_and_fences_new_acceptance() {
    let (_dir, roots, session) = fixture();
    let path = roots.create("fixture", Tier::UserApp, "waiting", "Waiting").unwrap();
    std::fs::write(path, "while true\nsleep(1)\nend\n").unwrap();
    session.reload().unwrap();
    session.run("waiting", "test").unwrap();
    session.shutdown().unwrap();
    assert_eq!(session.status()["state"], "exited");
    assert_eq!(session.run("waiting", "test").unwrap_err().code, "UNAVAILABLE");
}

#[test]
fn created_file_survives_failed_or_incomplete_rescan() {
    let (_dir, _roots, session) = fixture();
    for missing in [false, true] {
        let answer = session
            .create_with_scan("New macro", || {
                if missing {
                    Ok(Catalogue::default())
                } else {
                    Err(Refusal::new("INTERNAL", "forced scan failure"))
                }
            })
            .unwrap();
        assert_eq!(answer["scanned"], false);
        assert_eq!(answer["title"], "New macro");
        assert!(answer["warning"].as_str().unwrap().contains(if missing {
            "missing after rescan"
        } else {
            "forced scan failure"
        }));
        let path = std::path::Path::new(answer["path"].as_str().unwrap());
        assert!(path.is_file());
        assert_eq!(path.file_stem().unwrap().to_str().unwrap(), answer["id"]);
    }
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
    assert_eq!(body["macros"][0]["shadows"], json!([]));
    let other = Session::with_roots("other", Some(roots.clone()));
    assert!(other.reload().unwrap().entries.is_empty());
    roots
        .create(
            "fixture",
            crate::Tier::UserGlobal,
            "global-only",
            "Global only",
        )
        .unwrap();
    session.reload().unwrap();
    assert_eq!(
        session.run("global-only", "bus").unwrap_err().code,
        "NOT_FOUND"
    );
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
    session.add_finish_hook(Arc::new(move |body| {
        tx.send(body).unwrap();
    }));
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
    session.add_finish_hook(Arc::new(move |body| {
        tx.send(body).unwrap();
    }));
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
    let unavailable = Session::with_roots("fixture", None);
    assert_eq!(
        verbs::handle(&unavailable, "new", &json!({"title":"Test"}))
            .wait()
            .unwrap_err()
            .code,
        "UNAVAILABLE"
    );
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
fn new_creates_in_the_user_app_tier_and_refreshes_list() {
    let dir = tempfile::tempdir().unwrap();
    let roots = Roots::resolve_with(|key| match key {
        "HOME" => Some(dir.path().into()),
        "XDG_CONFIG_HOME" => Some(dir.path().join("xdg")),
        "MIXOS_SHARE" => Some(dir.path().join("share")),
        _ => None,
    })
    .unwrap();
    let session = Session::with_roots("fixture", Some(roots.clone()));
    let reply = verbs::handle(&session, "new", &json!({"title":"Tidy imports"}));
    assert!(matches!(reply, Reply::Deferred(_)));
    let body = reply.wait().unwrap();
    assert_eq!(body["id"], "tidy-imports");
    assert_eq!(body["title"], "Tidy imports");
    assert_eq!(
        body["path"],
        json!(roots.user.join("fixture/tidy-imports.mix"))
    );
    let list = verbs::handle(&session, "list", &json!({})).wait().unwrap();
    assert_eq!(list["macros"][0]["id"], body["id"]);
    assert_eq!(list["macros"][0]["tier"], "user-app");
    assert_eq!(
        verbs::handle(&session, "new", &json!({"title":"   "}))
            .wait()
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );
    std::fs::remove_file(body["path"].as_str().unwrap()).unwrap();
    std::fs::remove_dir(roots.user.join("fixture")).unwrap();
    std::fs::remove_dir(&roots.user).unwrap();
    std::fs::write(&roots.user, "obstruction").unwrap();
    assert_eq!(
        verbs::handle(&session, "new", &json!({"title":"Test"}))
            .wait()
            .unwrap_err()
            .code,
        "INTERNAL"
    );
}

#[test]
fn subscribers_receive_once_in_order_outside_the_lock_and_can_be_removed() {
    let (_dir, roots, session) = fixture();
    roots
        .create("fixture", crate::Tier::UserApp, "test", "Test")
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let first = session.clone();
    let first_tx = tx.clone();
    let id = session.add_finish_hook(Arc::new(move |body| {
        let _ = first.status(); // would deadlock under the session lock
        first_tx.send((1, body)).unwrap();
    }));
    session.add_finish_hook(Arc::new(move |body| {
        tx.send((2, body)).unwrap();
    }));
    session.reload().unwrap();
    session.run("test", "gui:42").unwrap();
    let a = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let b = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!((a.0, b.0), (1, 2));
    assert_eq!(a.1, b.1);
    assert_eq!(a.1["origin"], "gui:42");
    session.remove_finish_hook(id);
    session.run("test", "bus").unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap().0, 2);
    assert!(rx.try_recv().is_err());
}

#[test]
fn a_run_started_by_a_subscriber_finishes_after_the_previous_delivery() {
    let (_dir, roots, session) = fixture();
    roots
        .create("fixture", crate::Tier::UserApp, "test", "Test")
        .unwrap();
    session.reload().unwrap();
    let next = session.clone();
    let (tx, rx) = mpsc::channel();
    let first_tx = tx.clone();
    let first = session.add_finish_hook(Arc::new(move |body| {
        if body["run"] == 1 {
            next.run("test", "bus").unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        first_tx.send((body["run"].as_u64().unwrap(), 1)).unwrap();
    }));
    let second = session.add_finish_hook(Arc::new(move |body| {
        tx.send((body["run"].as_u64().unwrap(), 2)).unwrap();
    }));
    session.run("test", "bus").unwrap();
    let events = (0..4)
        .map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events, vec![(1, 1), (1, 2), (2, 1), (2, 2)]);
    session.remove_finish_hook(first);
    session.remove_finish_hook(second);
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
    session.add_finish_hook(Arc::new(move |body| {
        tx.send(body).unwrap();
    }));
    session.run("tail", "bus").unwrap();
    let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(event["dropped"], true);
    let output = session.output();
    assert_eq!(output["run"], 1);
    assert_eq!(output["dropped"], true);
    assert_eq!(output["text"].as_str().unwrap().len(), crate::OUTPUT_LIMIT);
    assert!(output["text"].as_str().unwrap().ends_with("end\n"));
}
