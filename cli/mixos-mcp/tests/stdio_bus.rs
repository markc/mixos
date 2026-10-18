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
async fn stdio_tools_use_the_real_fixture_bus_and_reconnect_without_replay() {
    let mut broker = test_broker::Broker::start();
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("node.conf.mix");
    std::fs::write(&config, "node: \"fixture\"\nnoded: {port: 4200}\n").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_mixos-mcp"));
    command.env("MIXOS_NODED_URL", &broker.url)
        .env("MIXOS_BUS_SOCKET", &broker.endpoint)
        .env("MIXOS_BROKER_ACCOUNT", std::env::var("USER").unwrap())
        .env("MIXOS_NODE_CONFIG", &config)
        .env("HOME", dir.path())
        .env("MIXOS_LOG", dir.path().join("logs"))
        .current_dir(dir.path());
    let service = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = service.list_tools(None).await.unwrap().tools;
    let mut names = tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, [
        "app_action_invoke", "app_actions_list", "app_control_get", "app_control_set",
        "app_control_wait", "app_controls_list", "app_describe", "bus_call",
        "bus_list_peers", "bus_list_services", "bus_node_info", "context_search",
        "desktop_window_wait", "desktop_windows", "docs_feedback", "index_workspace",
        "journal_feedback", "journal_supersede", "knowledge_brief", "knowledge_digest",
        "log_search", "log_tail", "mcp_status", "memory_feedback", "mix_execute",
        "noded_ping", "skills_delete", "skills_graduate", "skills_list", "skills_refine",
        "skills_retrieve", "skills_store", "term_list", "term_pane", "term_snapshot",
        "term_tab", "term_type",
    ]);
    for tool in &tools {
        assert!(tool.output_schema.is_some(), "{}", tool.name);
        assert!(tool.annotations.is_some(), "{}", tool.name);
    }
    let call = |name, args: Value| CallToolRequestParams::new(name)
        .with_arguments(args.as_object().unwrap().clone());
    let initial = result(service.call_tool(call("mcp_status", json!({}))).await.unwrap());
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
            owner.respond(&command, rc, &reply.to_string()).await.unwrap();
        }
    });
    let services = result(service.call_tool(call("bus_list_services", json!({}))).await.unwrap());
    assert!(services["result"].as_array().unwrap().iter().any(|s| s["name"] == "email"));
    let peers = result(service.call_tool(call("bus_list_peers", json!({}))).await.unwrap());
    assert!(peers.get("result").is_some());
    let observed = result(service.call_tool(call("app_describe", json!({"service":"email"}))).await.unwrap());
    assert_eq!(observed["result"]["app"], "Email");
    let rejected = service.call_tool(call("bus_call", json!({"to":"email", "command":"bad", "args":"{"}))).await.unwrap();
    assert!(rejected.is_error.unwrap());
    assert_eq!(deliveries.load(std::sync::atomic::Ordering::SeqCst), 1);
    let refused = service.call_tool(call("bus_call", json!({"to":"email", "command":"bad"}))).await.unwrap();
    assert!(refused.is_error.unwrap());
    assert_eq!(refused.structured_content.unwrap()["rc"], 10);
    let output = result(service.call_tool(call("mix_execute", json!({"script":"print(6 * 7)"}))).await.unwrap());
    assert_eq!(output["stdout"], "42\n");
    let native = result(service.call_tool(call("mix_execute", json!({"script":"send email app.describe\nprint($reply)"}))).await.unwrap());
    assert!(native["stdout"].as_str().unwrap().contains("Email"));
    let index = NodedClient::connect("indexd", &broker.url).await.unwrap();
    let mut requests = index.incoming_async().await.unwrap();
    let indexing = tokio::spawn(async move {
        while let Some(command) = requests.recv().await {
            let body = match command.command.as_str() {
                "indexd.list" => json!({"items":[], "total":0}),
                "indexd.search" => json!({"results":[]}),
                "indexd.feedback" => json!({"updated":1}),
                _ => panic!("unexpected index request: {}", command.command),
            };
            index.respond(&command, 0, &body.to_string()).await.unwrap();
        }
    });
    let skills = result(service.call_tool(call("skills_list", json!({}))).await.unwrap());
    assert!(skills.get("result").is_some());
    let search = result(service.call_tool(call("context_search", json!({"query":"fixture", "domain":"all"}))).await.unwrap());
    assert!(search["result"]["skills"].as_array().unwrap().is_empty());
    let feedback = result(service.call_tool(call("docs_feedback", json!({"id":1, "useful":true}))).await.unwrap());
    assert!(feedback.get("result").is_some());
    indexing.abort();
    task.abort();
    broker.bounce();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    result(service.call_tool(call("bus_list_services", json!({}))).await.unwrap());
    assert_eq!(deliveries.load(std::sync::atomic::Ordering::SeqCst), 3);
    service.cancel().await.unwrap();
}
