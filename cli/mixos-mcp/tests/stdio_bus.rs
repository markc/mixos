// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

use bus::native_client::NodedClient;
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use tokio::process::Command;

fn result(result: rmcp::model::CallToolResult) -> Value {
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    result.structured_content.unwrap()
}

#[tokio::test]
async fn application_tools_round_trip_the_registry_contract() {
    let broker = test_broker::Broker::start();
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("node.conf.mix");
    std::fs::write(&config, "node: \"fixture\"\nnoded: {port: 4200}\n").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_mixos-mcp"));
    command
        .env("MIXOS_NODED_URL", &broker.url)
        .env("MIXOS_BUS_SOCKET", &broker.endpoint)
        .env("MIXOS_BROKER_ACCOUNT", std::env::var("USER").unwrap())
        .env("MIXOS_NODE_CONFIG", &config)
        .env("MIXOS_ETC", dir.path())
        .env("HOME", dir.path())
        .env("MIXOS_LOG", dir.path().join("logs"))
        .current_dir(dir.path());
    let service = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = service.list_tools(None).await.unwrap().tools;
    for name in ["app_commands", "app_execute"] {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["service"]["type"], "string");
        assert_eq!(schema["additionalProperties"], false);
        if name == "app_execute" {
            assert_eq!(schema["properties"]["id"]["type"], "string");
            assert_eq!(schema["properties"]["params"]["type"], "object");
            assert_eq!(schema["required"], json!(["service", "id"]));
        } else {
            assert_eq!(schema["required"], json!(["service"]));
        }
    }

    let rows = json!([
        {"id":"file.refresh", "label":"Refresh", "menu":"File", "shortcut":"F5",
         "more":["Ctrl+R"], "enabled":true, "checked":null},
        {"id":"view.filter", "label":"Filter", "menu":"View", "shortcut":null,
         "enabled":false, "checked":true, "schema":{"type":"object"}}
    ]);
    let listed = rows.clone();
    let owner = NodedClient::connect("registry-fixture", &broker.url)
        .await
        .unwrap();
    let mut incoming = owner.incoming_async().await.unwrap();
    let deliveries = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = deliveries.clone();
    let task = tokio::spawn(async move {
        while let Some(command) = incoming.recv().await {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let args: Value = serde_json::from_str(&command.body).unwrap();
            let (rc, reply) = match command.command.as_str() {
                "registry-fixture.commands" => {
                    assert_eq!(args, json!({}));
                    (0, listed.to_string())
                }
                "registry-fixture.execute" if args["id"] == "missing.command" => {
                    (10, "unknown command: missing.command".to_string())
                }
                "registry-fixture.execute" if args["id"] == "missing.structured" => (
                    10,
                    json!({
                        "error_code":"UNKNOWN_COMMAND", "message":"unknown command: missing.structured"
                    })
                    .to_string(),
                ),
                "registry-fixture.execute" => (0, args.to_string()),
                "app.describe" => (10, "fixture describe refusal".to_string()),
                _ => panic!("unexpected application request: {}", command.command),
            };
            owner.respond(&command, rc, &reply).await.unwrap();
        }
    });
    let call = |name, args: Value| {
        CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone())
    };
    for target in ["busviewer@mesh.example", "a.b"] {
        for name in ["app_commands", "app_execute"] {
            let mut args = json!({"service":target});
            if name == "app_execute" {
                args["id"] = json!("file.refresh");
            }
            let refused = service.call_tool(call(name, args)).await.unwrap();
            assert_eq!(refused.is_error, Some(true));
            assert_eq!(refused.content.len(), 1);
            assert_eq!(
                refused.content[0].as_text().unwrap().text,
                format!("service must be a plain Bus service name (no '@' or '.'): {target}")
            );
            assert_eq!(deliveries.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
    }
    let rejected = service
        .call_tool(call(
            "app_commands",
            json!({"service":"registry-fixture", "id":"file.refresh"}),
        ))
        .await
        .unwrap();
    assert_eq!(rejected.is_error, Some(true));
    assert!(
        rejected.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("unknown field `id`"),
        "{rejected:?}"
    );
    assert_eq!(deliveries.load(std::sync::atomic::Ordering::SeqCst), 0);
    // No native connection has even been opened by the refused requests.
    let status = result(
        service
            .call_tool(call("mcp_status", json!({})))
            .await
            .unwrap(),
    );
    assert_eq!(status["broker_connected"], false);
    let observed = result(
        service
            .call_tool(call("app_commands", json!({"service":"registry-fixture"})))
            .await
            .unwrap(),
    );
    assert_eq!(
        observed,
        json!({
            "service":"registry-fixture", "command":"registry-fixture.commands", "result":rows
        })
    );
    for body in [
        json!({"id":"file.refresh"}),
        json!({"id":"view.filter", "params":{}}),
        json!({"id":"view.filter", "params":{
            "id":"nested.id", "service":"nested-service", "query":"errors",
            "options":{"enabled":false, "values":[1, null, "two"]}
        }}),
    ] {
        let mut args = body.clone();
        args["service"] = json!("registry-fixture");
        let observed = result(service.call_tool(call("app_execute", args)).await.unwrap());
        assert_eq!(
            observed,
            json!({
                "service":"registry-fixture", "command":"registry-fixture.execute", "result":body
            })
        );
    }
    let observed = result(
        service
            .call_tool(call(
                "app_execute",
                json!({"service":"registry-fixture", "id":"file.refresh", "params":null}),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(observed["result"], json!({"id":"file.refresh"}));
    for (name, args, message) in [
        (
            "app_execute",
            json!({"service":"registry-fixture", "id":"missing.command"}),
            "unknown command: missing.command",
        ),
        (
            "app_execute",
            json!({"service":"registry-fixture", "id":"missing.structured"}),
            "UNKNOWN_COMMAND: unknown command: missing.structured",
        ),
        (
            "app_describe",
            json!({"service":"registry-fixture"}),
            "fixture describe refusal",
        ),
    ] {
        let refused = service.call_tool(call(name, args)).await.unwrap();
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(refused.content.len(), 1);
        assert_eq!(refused.content[0].as_text().unwrap().text, message);
    }
    task.abort();
    service.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_tools_use_the_real_fixture_bus_and_reconnect_without_replay() {
    let mut broker = test_broker::Broker::start();
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("node.conf.mix");
    std::fs::write(&config, "node: \"fixture\"\nnoded: {port: 4200}\n").unwrap();
    // A different service name proves the client probes its target rather
    // than requiring a locally registered service named indexd.
    std::fs::write(
        dir.path().join("knowledge.conf.mix"),
        "indexd: {target: \"knowledge-fixture\"}\n",
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_mixos-mcp"));
    command
        .env("MIXOS_NODED_URL", &broker.url)
        .env("MIXOS_BUS_SOCKET", &broker.endpoint)
        .env("MIXOS_BROKER_ACCOUNT", std::env::var("USER").unwrap())
        .env("MIXOS_NODE_CONFIG", &config)
        .env("MIXOS_ETC", dir.path())
        .env("HOME", dir.path())
        .env("MIXOS_LOG", dir.path().join("logs"))
        .current_dir(dir.path());
    let service = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = service.list_tools(None).await.unwrap().tools;
    let mut names = tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "app_commands",
            "app_describe",
            "app_execute",
            "bus_call",
            "bus_list_peers",
            "bus_list_services",
            "bus_node_info",
            "context_search",
            "desktop_window_wait",
            "desktop_windows",
            "docs_feedback",
            "index_workspace",
            "journal_feedback",
            "journal_supersede",
            "knowledge_brief",
            "knowledge_digest",
            "log_search",
            "log_tail",
            "mcp_status",
            "memory_feedback",
            "mix_execute",
            "noded_ping",
            "skills_delete",
            "skills_graduate",
            "skills_list",
            "skills_refine",
            "skills_retrieve",
            "skills_store",
            "term_list",
            "term_pane",
            "term_snapshot",
            "term_tab",
            "term_type",
        ]
    );
    for tool in &tools {
        assert!(tool.output_schema.is_some(), "{}", tool.name);
        assert!(tool.annotations.is_some(), "{}", tool.name);
    }
    let call = |name, args: Value| {
        CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone())
    };
    let initial = result(
        service
            .call_tool(call("mcp_status", json!({})))
            .await
            .unwrap(),
    );
    assert_eq!(initial["broker_connected"], false);
    let owner = NodedClient::connect("email", &broker.url).await.unwrap();
    let mut incoming = owner.incoming_async().await.unwrap();
    let deliveries = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = deliveries.clone();
    let task = tokio::spawn(async move {
        while let Some(command) = incoming.recv().await {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (rc, reply) = match command.command.as_str() {
                "app.describe" => (0, json!({"app":"Email"})),
                _ => (10, json!({"message":"fixture refusal"})),
            };
            owner
                .respond(&command, rc, &reply.to_string())
                .await
                .unwrap();
        }
    });
    let services = result(
        service
            .call_tool(call("bus_list_services", json!({})))
            .await
            .unwrap(),
    );
    assert!(
        services["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "email")
    );
    let peers = result(
        service
            .call_tool(call("bus_list_peers", json!({})))
            .await
            .unwrap(),
    );
    assert!(peers.get("result").is_some());
    let observed = result(
        service
            .call_tool(call("app_describe", json!({"service":"email"})))
            .await
            .unwrap(),
    );
    assert_eq!(observed["result"]["app"], "Email");
    let rejected = service
        .call_tool(call(
            "bus_call",
            json!({"to":"email", "command":"bad", "args":"{"}),
        ))
        .await
        .unwrap();
    assert!(rejected.is_error.unwrap());
    assert_eq!(deliveries.load(std::sync::atomic::Ordering::SeqCst), 1);
    let refused = service
        .call_tool(call("bus_call", json!({"to":"email", "command":"bad"})))
        .await
        .unwrap();
    assert!(refused.is_error.unwrap());
    assert_eq!(refused.structured_content.unwrap()["rc"], 10);
    let output = result(
        service
            .call_tool(call("mix_execute", json!({"script":"print(6 * 7)"})))
            .await
            .unwrap(),
    );
    assert_eq!(output["stdout"], "42\n");
    let native = result(
        service
            .call_tool(call(
                "mix_execute",
                json!({"script":"send email app.describe\nprint($reply)"}),
            ))
            .await
            .unwrap(),
    );
    assert!(native["stdout"].as_str().unwrap().contains("Email"));
    let absent = service
        .call_tool(call("skills_list", json!({})))
        .await
        .unwrap();
    assert!(absent.is_error.unwrap_or(false));
    assert!(
        format!("{absent:?}").contains("native indexd service is not registered on the MixOS Bus")
    );

    let workspace = dir.path().join("work~space");
    std::fs::create_dir_all(workspace.join("_doc")).unwrap();
    let file = workspace.join("_doc/2026-10-20-fixture.md");
    std::fs::write(&file, "# Fixture\n\nA document long enough for indexing, with one section to replace stale chunks.\n").unwrap();
    let expected_path = file.canonicalize().unwrap().to_string_lossy().into_owned();
    let rows = std::sync::Arc::new(std::sync::Mutex::new((0..2107).map(|id| json!({
        "id":id, "content":"old chunk", "source":"doc", "created":"2026-10-20",
        "metadata":json!({"path":if [0, 1000, 2106].contains(&id) { expected_path.as_str() } else { "/other/document.md" }}).to_string(),
    })).collect::<Vec<_>>()));
    let index_rows = rows.clone();
    let offsets = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let index_offsets = offsets.clone();
    let stored_paths = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let index_paths = stored_paths.clone();
    let deleted_ids = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let index_deleted = deleted_ids.clone();
    let index = NodedClient::connect("knowledge-fixture", &broker.url)
        .await
        .unwrap();
    let mut requests = index.incoming_async().await.unwrap();
    let indexing = tokio::spawn(async move {
        while let Some(command) = requests.recv().await {
            let request: Value = serde_json::from_str(&command.body).unwrap();
            assert_eq!(
                command.command,
                format!("indexd.{}", request["action"].as_str().unwrap())
            );
            let (rc, body) = match command.command.as_str() {
                "indexd.stats" => (0, json!({"total_vectors":2107})),
                "indexd.list" => {
                    if request["source"] == "skill" {
                        (0, json!({"items":[], "total":0}))
                    } else {
                        let offset = request["offset"].as_u64().unwrap() as usize;
                        let limit = request["limit"].as_u64().unwrap() as usize;
                        index_offsets.lock().unwrap().push(offset);
                        let rows = index_rows.lock().unwrap();
                        (
                            0,
                            json!({"items":rows.iter().skip(offset).take(limit).collect::<Vec<_>>(), "total":rows.len()}),
                        )
                    }
                }
                "indexd.delete" => {
                    assert!(request.get("ids").is_none());
                    assert_eq!(request["filter"][0]["field"], "path");
                    assert_eq!(request["filter"][0]["op"], "eq");
                    let mut rows = index_rows.lock().unwrap();
                    let before = rows.len();
                    rows.retain(|row| {
                        let meta: Value =
                            serde_json::from_str(row["metadata"].as_str().unwrap()).unwrap();
                        let keep = meta["path"] != request["filter"][0]["value"];
                        if !keep {
                            index_deleted
                                .lock()
                                .unwrap()
                                .push(row["id"].as_i64().unwrap());
                        }
                        keep
                    });
                    (0, json!({"deleted":before - rows.len()}))
                }
                "indexd.store" => {
                    let metadata = request["metadata"][0].as_str().unwrap();
                    let meta: Value = serde_json::from_str(metadata).unwrap();
                    index_paths
                        .lock()
                        .unwrap()
                        .push(meta["path"].as_str().unwrap().to_string());
                    index_rows
                        .lock()
                        .unwrap()
                        .push(json!({"id":9999,"metadata":metadata}));
                    (0, json!({"stored":1,"ids":[9999]}))
                }
                "indexd.feedback" if request["id"] == -1 => {
                    (10, json!({"error":"fixture refusal"}))
                }
                "indexd.feedback" if request["id"] == -2 => {
                    (0, json!({"error":"fixture body error"}))
                }
                "indexd.search" => (0, json!({"results":[]})),
                "indexd.feedback" => (0, json!({"updated":1})),
                _ => panic!("unexpected index request: {}", command.command),
            };
            index
                .respond(&command, rc, &body.to_string())
                .await
                .unwrap();
        }
    });
    let skills = result(
        service
            .call_tool(call("skills_list", json!({})))
            .await
            .unwrap(),
    );
    assert!(skills.get("result").is_some());
    let search = result(
        service
            .call_tool(call(
                "context_search",
                json!({"query":"fixture", "domain":"all"}),
            ))
            .await
            .unwrap(),
    );
    assert!(search["result"]["skills"].as_array().unwrap().is_empty());
    let feedback = result(
        service
            .call_tool(call("docs_feedback", json!({"id":1, "useful":true})))
            .await
            .unwrap(),
    );
    assert!(feedback.get("result").is_some());
    for (id, message) in [
        (-1, "fixture refusal"),
        (-2, "indexd error: fixture body error"),
    ] {
        let rejected = service
            .call_tool(call("docs_feedback", json!({"id":id,"useful":true})))
            .await
            .unwrap();
        assert!(rejected.is_error.unwrap_or(false), "{rejected:?}");
        assert!(format!("{rejected:?}").contains(message), "{rejected:?}");
    }
    for path in ["work~space", "~/work~space"] {
        let indexed = result(
            service
                .call_tool(call("index_workspace", json!({"path":path})))
                .await
                .unwrap(),
        );
        assert!(indexed["result"].get("errors").is_none(), "{indexed}");
        assert_eq!(indexed["result"]["sections"], 1);
    }
    assert_eq!(
        *stored_paths.lock().unwrap(),
        [expected_path.clone(), expected_path]
    );
    assert!(
        offsets.lock().unwrap().is_empty(),
        "deletion must not list the corpus"
    );
    assert_eq!(*deleted_ids.lock().unwrap(), [0, 1000, 2106, 9999]);
    let remaining = rows
        .lock()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_i64().unwrap())
        .collect::<Vec<_>>();
    assert!(!remaining.contains(&0) && !remaining.contains(&1000) && !remaining.contains(&2106));
    assert_eq!(remaining.iter().filter(|&&id| id == 9999).count(), 1);
    indexing.abort();
    task.abort();
    broker.bounce();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    result(
        service
            .call_tool(call("bus_list_services", json!({})))
            .await
            .unwrap(),
    );
    assert_eq!(deliveries.load(std::sync::atomic::Ordering::SeqCst), 3);
    service.cancel().await.unwrap();
}
