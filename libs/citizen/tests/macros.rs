// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real Bus requests exercise the shared application transport adapter.
use bus::native_client::SupervisedClient;
use std::collections::BTreeMap;
use std::time::Duration;

#[test]
fn bus_macro_writes_refuse_other_services_and_workshop_deputy_calls() {
    let broker = test_broker::Broker::start();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let caller = runtime
        .block_on(SupervisedClient::connect("unrelated-app", &broker.url))
        .unwrap();
    let manager = runtime
        .block_on(SupervisedClient::connect(
            "workshop-manager-test",
            &broker.url,
        ))
        .unwrap();
    let (owner, _deliveries) =
        citizen::start_with_macros("ced", "ced-acl-test", &broker.url, &[]).unwrap();
    let (workshop, _workshop_deliveries) =
        citizen::start_with_macros("workshop", "workshop-acl-test", &broker.url, &[]).unwrap();
    runtime.block_on(async {
        for suffix in ["put", "remove", "move", "copy", "new"] {
            for (app, service) in [("ced", "ced-acl-test"), ("workshop", "workshop-acl-test")] {
                for body in ["{}", r#"{"app":"ced"}"#] {
                    let command = format!("{app}.macros.{suffix}");
                    let (rc, body, _) = caller
                        .call_with_headers_raw(service, &command, &BTreeMap::new(), body)
                        .await
                        .unwrap();
                    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
                    assert_eq!(rc, 10);
                    assert_eq!(body["error_code"], "DENIED");
                    assert!(body["message"].as_str().unwrap().contains(&command));
                }
            }
            // Invalid arguments demonstrate admission without writing real user files.
            let reply = manager
                .call_with_headers_raw(
                    "ced-acl-test",
                    &format!("ced.macros.{suffix}"),
                    &BTreeMap::new(),
                    "{}",
                )
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&reply.1).unwrap()["error_code"],
                "INVALID_ARGUMENT"
            );
        }
        for (command, body) in [
            ("workshop.execute", r#"{"id":"macro.put"}"#),
            ("workshop.execute", r#"{"id":"bus.call"}"#),
            (
                "workshop.call",
                r#"{"service":"ced","command":"ced.macros.put"}"#,
            ),
            ("workshop.call", r#"{}"#),
            // An empty body reaches Workshop as `{}`, a call on the selection.
            ("workshop.call", ""),
            ("workshop.call", "  "),
            ("workshop.call", "not json"),
        ] {
            let reply = caller
                .call_with_headers_raw("workshop-acl-test", command, &BTreeMap::new(), body)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&reply.1).unwrap()["error_code"],
                "DENIED"
            );
        }
        caller.close().await;
        manager.close().await;
    });
    owner.quit();
    workshop.quit();
    owner.wait_done().unwrap();
    workshop.wait_done().unwrap();
}

#[test]
fn workshop_ui_commands_stay_open_to_other_callers() {
    let broker = test_broker::Broker::start();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = std::sync::Arc::new(
            SupervisedClient::connect("workshop-ui-test", &broker.url)
                .await
                .unwrap(),
        );
        let mut jobs = citizen::macros::Jobs::default();
        for (command, body) in [
            ("workshop.ui.click", r#"{"label":"Save"}"#),
            ("workshop.ui.key", r#"{"key":"S","modifiers":["ctrl"]}"#),
            ("workshop.ui.pointer", r#"{"x":1,"y":1}"#),
            ("workshop.ui.type", r#"{"text":"x"}"#),
            (
                "workshop.call",
                r#"{"service":"workshop","command":"workshop.ui.click"}"#,
            ),
        ] {
            let incoming = bus::native_client::IncomingCommand {
                generation: 0,
                from: "unrelated-app".into(),
                command: command.into(),
                id: Some("1".into()),
                args: serde_json::Value::Null,
                body: body.into(),
                headers: BTreeMap::from([("broker_origin".into(), "local".into())]),
            };
            // false: the macro gate leaves it to Workshop's own handler.
            assert!(
                !citizen::macros::dispatch(
                    "workshop",
                    "workshop-ui-test",
                    &incoming,
                    client.clone(),
                    &mut jobs
                ),
                "{command}"
            );
        }
        jobs.finish().await;
        client.close().await;
    });
}

#[test]
fn put_has_a_separate_larger_envelope_cap() {
    let oversized = " ".repeat(macros::commands::PUT_ENVELOPE_LIMIT + 1);
    let (_, body) = citizen::macros::answer_body(
        citizen::macros::request(
            "put-limit-test",
            ::macros::commands::Caller::Workshop,
            "put",
            &oversized,
        )
        .wait(),
    );
    assert_eq!(
        body["message"],
        format!(
            "request exceeds {} bytes",
            macros::commands::PUT_ENVELOPE_LIMIT
        )
    );
    let text = serde_json::json!({"id":"missing","text":"x".repeat(9000)}).to_string();
    let (_, body) = citizen::macros::answer_body(
        citizen::macros::request(
            "put-limit-test",
            ::macros::commands::Caller::Workshop,
            "put",
            &text,
        )
        .wait(),
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
        let reply = citizen::macros::request(
            "body-limit-test",
            ::macros::commands::Caller::Workshop,
            "status",
            &body,
        );
        assert!(matches!(reply, macros::commands::Reply::Immediate(_)));
        let (rc, body) = citizen::macros::answer_body(reply.wait());
        assert_eq!(rc, 10);
        assert_eq!(body["error_code"], "INVALID_ARGUMENT");
        assert_eq!(body["message"], "request exceeds 8192 bytes");
    }
    let boundary = format!("{{}}{}", " ".repeat(8190));
    let (rc, _) = citizen::macros::answer_body(
        citizen::macros::request(
            "body-limit-test",
            ::macros::commands::Caller::Workshop,
            "status",
            &boundary,
        )
        .wait(),
    );
    assert_eq!(rc, 0);
}

#[test]
fn unauthorised_writes_are_denied_before_the_body_is_read() {
    for body in ["{".to_string(), " ".repeat(2 * 1024 * 1024)] {
        for suffix in ["put", "remove", "move", "copy", "new"] {
            let (rc, reply) = citizen::macros::answer_body(
                citizen::macros::request(
                    "body-limit-test",
                    ::macros::commands::Caller::Other,
                    suffix,
                    &body,
                )
                .wait(),
            );
            assert_eq!(rc, 10);
            assert_eq!(reply["error_code"], "DENIED", "{suffix}");
        }
    }
}

#[test]
fn malformed_macro_json_has_a_typed_refusal() {
    let (rc, body) = citizen::macros::answer_body(
        citizen::macros::request(
            "body-limit-test",
            ::macros::commands::Caller::Workshop,
            "status",
            "{",
        )
        .wait(),
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
