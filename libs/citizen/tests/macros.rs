// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real Bus requests exercise the shared application transport adapter.
use bus::native_client::SupervisedClient;
use std::collections::BTreeMap;
use std::time::Duration;

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("citizen-macros-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn arbitrary_bus_caller_can_manage_app_global_and_cross_app_macros() {
    const CHILD: &str = "MIXOS_MACRO_OPEN_BUS_TEST";
    if std::env::var_os(CHILD).is_none() {
        let dir = TempDir::new();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "arbitrary_bus_caller_can_manage_app_global_and_cross_app_macros",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HOME", dir.path())
            .env("XDG_CONFIG_HOME", dir.path().join("config"))
            .env("MIXOS_PREFIX", dir.path().join("prefix"))
            .output()
            .unwrap();
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let broker = test_broker::Broker::start();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (app, _) = citizen::start_with_macros("fixture", "fixture", &broker.url, &[]).unwrap();
    let (global, _) = citizen::start_with_macros("workshop", "workshop", &broker.url, &[]).unwrap();
    let (other, _) = citizen::start_with_macros("other", "other", &broker.url, &[]).unwrap();
    let roots = macros::Roots::resolve().unwrap();
    runtime.block_on(async {
        let caller = SupervisedClient::connect("agent-1", &broker.url)
            .await
            .unwrap();
        let operations = [
            ("fixture", "new", serde_json::json!({"title":"Open"})),
            (
                "fixture",
                "put",
                serde_json::json!({"id":"open","text":"-- title: Changed\n"}),
            ),
            (
                "fixture",
                "move",
                serde_json::json!({"id":"open","tier":"user-global"}),
            ),
            (
                "workshop",
                "put",
                serde_json::json!({"id":"open","text":"-- title: Global edit\n"}),
            ),
            (
                "workshop",
                "move",
                serde_json::json!({"id":"open","tier":"user-app","destination":"other"}),
            ),
            ("other", "remove", serde_json::json!({"id":"open"})),
            ("workshop", "new", serde_json::json!({"title":"Global"})),
            ("workshop", "remove", serde_json::json!({"id":"global"})),
        ];
        for (app, suffix, args) in operations {
            let (rc, body, _) = caller
                .call_with_headers_raw(
                    app,
                    &format!("{app}.macros.{suffix}"),
                    &BTreeMap::new(),
                    &args.to_string(),
                )
                .await
                .unwrap();
            assert_eq!(rc, 0, "{app}.{suffix}: {body}");
        }
        std::fs::create_dir_all(roots.builtin.join("fixture")).unwrap();
        std::fs::write(
            roots.builtin.join("fixture/builtin.mix"),
            "-- title: Builtin\n",
        )
        .unwrap();
        let (rc, body, _) = caller
            .call_with_headers_raw(
                "fixture",
                "fixture.macros.copy",
                &BTreeMap::new(),
                r#"{"id":"builtin"}"#,
            )
            .await
            .unwrap();
        assert_eq!(rc, 0, "{body}");
        assert!(roots.user.join("fixture/builtin.mix").is_file());

        // Feed the adapter a broker-stamped mesh delivery. The transport
        // test above covers real local stamping; clients cannot forge it.
        let client = std::sync::Arc::new(
            SupervisedClient::connect("mesh-recipient", &broker.url)
                .await
                .unwrap(),
        );
        let mut jobs = citizen::macros::Jobs::default();
        for (app, suffix, args) in [
            ("fixture", "new", serde_json::json!({"title":"Mesh"})),
            (
                "fixture",
                "put",
                serde_json::json!({"id":"mesh","text":"-- title: Mesh edit\n"}),
            ),
            (
                "fixture",
                "move",
                serde_json::json!({"id":"mesh","tier":"user-global"}),
            ),
            (
                "workshop",
                "move",
                serde_json::json!({"id":"mesh","tier":"user-app","destination":"other"}),
            ),
            ("other", "remove", serde_json::json!({"id":"mesh"})),
            ("fixture", "copy", serde_json::json!({"id":"builtin"})),
        ] {
            if suffix == "copy" {
                std::fs::remove_file(roots.user.join("fixture/builtin.mix")).unwrap();
            }
            let incoming = bus::native_client::IncomingCommand {
                generation: 0,
                from: String::new(),
                command: format!("{app}.macros.{suffix}"),
                id: None,
                args: serde_json::Value::Null,
                body: args.to_string(),
                headers: BTreeMap::from([("broker_origin".into(), "mesh".into())]),
            };
            assert!(citizen::macros::dispatch(
                app,
                &incoming,
                client.clone(),
                &mut jobs
            ));
            jobs.finish().await;
            match suffix {
                "new" => assert!(roots.user.join("fixture/mesh.mix").is_file()),
                "put" => assert_eq!(
                    std::fs::read_to_string(roots.user.join("fixture/mesh.mix")).unwrap(),
                    "-- title: Mesh edit\n"
                ),
                "move" if app == "fixture" => assert!(roots.user.join("mesh.mix").is_file()),
                "move" => assert!(roots.user.join("other/mesh.mix").is_file()),
                "remove" => assert!(!roots.user.join("other/mesh.mix").exists()),
                "copy" => assert!(roots.user.join("fixture/builtin.mix").is_file()),
                _ => unreachable!(),
            }
        }
        client.close().await;
        caller.close().await;
    });
    for handle in [app, global, other] {
        handle.quit();
        handle.wait_done().unwrap();
    }
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
        for origin in ["local", "mesh"] {
            for (command, body) in [
                ("workshop.execute", r#"{"id":"macro.put"}"#),
                ("workshop.execute", r#"{"id":"bus.call"}"#),
                ("workshop.call", ""),
                (
                    "workshop.call",
                    r#"{"service":"ced","command":"ced.macros.put"}"#,
                ),
                ("workshop.macros.new", r#"{"app":"global","title":"New"}"#),
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
                    from: "agent-1".into(),
                    command: command.into(),
                    id: Some("1".into()),
                    args: serde_json::Value::Null,
                    body: body.into(),
                    headers: BTreeMap::from([("broker_origin".into(), origin.into())]),
                };
                // The adapter leaves proxy and UI commands to Workshop itself.
                assert!(
                    !citizen::macros::dispatch("workshop", &incoming, client.clone(), &mut jobs),
                    "{command}"
                );
            }
        }
        jobs.finish().await;
        client.close().await;
    });
}

#[test]
fn put_has_a_separate_larger_envelope_cap() {
    let oversized = " ".repeat(macros::commands::PUT_ENVELOPE_LIMIT + 1);
    let (_, body) = citizen::macros::answer_body(
        citizen::macros::request("put-limit-test", "put", &oversized).wait(),
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
        citizen::macros::request("put-limit-test", "put", &text).wait(),
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
        let reply = citizen::macros::request("body-limit-test", "status", &body);
        assert!(matches!(reply, macros::commands::Reply::Immediate(_)));
        let (rc, body) = citizen::macros::answer_body(reply.wait());
        assert_eq!(rc, 10);
        assert_eq!(body["error_code"], "INVALID_ARGUMENT");
        assert_eq!(body["message"], "request exceeds 8192 bytes");
    }
    let boundary = format!("{{}}{}", " ".repeat(8190));
    let (rc, _) = citizen::macros::answer_body(
        citizen::macros::request("body-limit-test", "status", &boundary).wait(),
    );
    assert_eq!(rc, 0);
}

#[test]
fn open_writes_still_validate_the_body() {
    for body in ["{".to_string(), " ".repeat(2 * 1024 * 1024)] {
        for suffix in ["put", "remove", "move", "copy", "new"] {
            let (rc, reply) = citizen::macros::answer_body(
                citizen::macros::request("body-limit-test", suffix, &body).wait(),
            );
            assert_eq!(rc, 10);
            assert_eq!(reply["error_code"], "INVALID_ARGUMENT", "{suffix}");
        }
    }
}

#[test]
fn malformed_macro_json_has_a_typed_refusal() {
    let (rc, body) = citizen::macros::answer_body(
        citizen::macros::request("body-limit-test", "status", "{").wait(),
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
