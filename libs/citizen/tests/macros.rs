// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real Bus requests exercise the shared application transport adapter.
use bus::native_client::SupervisedClient;
use std::collections::BTreeMap;
use std::time::Duration;

#[test]
fn bus_macro_writes_refuse_other_services_and_workshop_deputy_calls() {
    let broker = test_broker::Broker::start();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let caller = runtime.block_on(SupervisedClient::connect("unrelated-app", &broker.url)).unwrap();
    let manager = runtime.block_on(SupervisedClient::connect("workshop-manager-test", &broker.url)).unwrap();
    let (owner, _deliveries) = citizen::start_with_macros("ced", "ced-acl-test", &broker.url, &[]).unwrap();
    let (workshop, _workshop_deliveries) = citizen::start_with_macros("workshop", "workshop-acl-test", &broker.url, &[]).unwrap();
    runtime.block_on(async {
        for suffix in ["put", "remove", "move", "copy", "new"] {
            for (app, service) in [("ced", "ced-acl-test"), ("workshop", "workshop-acl-test")] {
                for body in ["{}", r#"{"app":"ced"}"#] {
                    let verb = format!("{app}.macros.{suffix}");
                    let (rc, body, _) = caller.call_with_headers_raw(service, &verb, &BTreeMap::new(), body).await.unwrap();
                    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
                    assert_eq!(rc, 10);
                    assert_eq!(body["error_code"], "DENIED");
                    assert!(body["message"].as_str().unwrap().contains(&verb));
                }
            }
            // Invalid arguments demonstrate admission without writing real user files.
            let reply = manager.call_with_headers_raw("ced-acl-test", &format!("ced.macros.{suffix}"), &BTreeMap::new(), "{}").await.unwrap();
            assert_eq!(serde_json::from_str::<serde_json::Value>(&reply.1).unwrap()["error_code"], "INVALID_ARGUMENT");
        }
        for (verb, body) in [
            ("workshop.execute", r#"{"id":"macro.put"}"#),
            ("workshop.execute", r#"{"id":"bus.call"}"#),
            ("workshop.call", r#"{"service":"ced","verb":"ced.macros.put"}"#),
            ("workshop.call", r#"{}"#),
        ] {
            let reply = caller.call_with_headers_raw("workshop-acl-test", verb, &BTreeMap::new(), body).await.unwrap();
            assert_eq!(serde_json::from_str::<serde_json::Value>(&reply.1).unwrap()["error_code"], "DENIED");
        }
        caller.close().await;
        manager.close().await;
    });
    owner.quit(); workshop.quit();
    owner.wait_done().unwrap(); workshop.wait_done().unwrap();
}

#[test]
fn put_has_a_separate_larger_envelope_cap() {
    let oversized = " ".repeat(macros::verbs::PUT_ENVELOPE_LIMIT + 1);
    let (_, body) = citizen::macros::answer_body(
        citizen::macros::request("put-limit-test", ::macros::verbs::Caller::Workshop, "put", &oversized).wait(),
    );
    assert_eq!(
        body["message"],
        format!(
            "request exceeds {} bytes",
            macros::verbs::PUT_ENVELOPE_LIMIT
        )
    );
    let text = serde_json::json!({"id":"missing","text":"x".repeat(9000)}).to_string();
    let (_, body) = citizen::macros::answer_body(
        citizen::macros::request("put-limit-test", ::macros::verbs::Caller::Workshop, "put", &text).wait(),
    );
    assert_ne!(body["message"], "request exceeds 8192 bytes");
}

#[test]
fn macro_body_limit_is_checked_before_parsing_and_accepts_the_boundary() {
    for body in [
        " ".repeat(8193),
        "é".repeat(4097),
        format!("{{}}{}", " ".repeat(8191)),
    ] {
        let reply = citizen::macros::request("body-limit-test", ::macros::verbs::Caller::Workshop, "status", &body);
        assert!(matches!(reply, macros::verbs::Reply::Immediate(_)));
        let (rc, body) = citizen::macros::answer_body(reply.wait());
        assert_eq!(rc, 10);
        assert_eq!(body["error_code"], "INVALID_ARGUMENT");
        assert_eq!(body["message"], "request exceeds 8192 bytes");
    }
    let boundary = format!("{{}}{}", " ".repeat(8190));
    let (rc, _) = citizen::macros::answer_body(
        citizen::macros::request("body-limit-test", ::macros::verbs::Caller::Workshop, "status", &boundary).wait(),
    );
    assert_eq!(rc, 0);
}

#[test]
fn malformed_macro_json_has_a_typed_refusal() {
    let (rc, body) = citizen::macros::answer_body(
        citizen::macros::request("body-limit-test", ::macros::verbs::Caller::Workshop, "status", "{").wait(),
    );
    assert_eq!(rc, 10);
    assert_eq!(body["error_code"], "INVALID_ARGUMENT");
    assert!(body["message"].is_string());
}

#[test]
fn macro_status_is_served_without_a_ui_dispatch_or_polling() {
    let broker = test_broker::Broker::start();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let caller = runtime
        .block_on(SupervisedClient::connect("macro-caller", &broker.url))
        .unwrap();
    for app in [
        "email",
        "workshop",
        "monitor",
        "prefs",
        "screenshot",
        "ced",
        "dopus",
        "term",
    ] {
        let (handle, _deliveries) = citizen::start_with_macros(app, app, &broker.url, &[]).unwrap();
        let (rc, body, _) = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(5),
                caller.call_with_headers_raw(
                    app,
                    &format!("{app}.macros.status"),
                    &BTreeMap::new(),
                    "{}",
                ),
            )
            .await
            .unwrap()
            .unwrap()
        });
        assert_eq!(rc, 0, "{app}: {body}");
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["state"], "idle", "{app}");
        handle.quit();
        handle.wait_done().unwrap();
    }
    runtime.block_on(caller.close());
}
