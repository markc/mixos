// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Native Bus registration, bounded concurrent dispatch and reconnect.
use crate::Engine;
use ::bus::{RegisterProvenance, CommandDescriptor, native_client::NodedClient};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

pub const BUS_SERVICE: &str = "indexd";

pub fn command_manifest() -> &'static [CommandDescriptor] {
    static COMMANDS: std::sync::LazyLock<Vec<CommandDescriptor>> = std::sync::LazyLock::new(|| {
        [
            (
                "indexd.embed",
                vec!["texts", "prefix"],
                "Embed text locally",
                true,
            ),
            (
                "indexd.store",
                vec!["texts", "source", "metadata"],
                "Store semantic chunks",
                false,
            ),
            (
                "indexd.search",
                vec!["query", "limit", "source", "metadata_filter"],
                "Search active chunks",
                true,
            ),
            (
                "indexd.update",
                vec!["id", "content", "metadata"],
                "Update a chunk",
                false,
            ),
            (
                "indexd.delete",
                vec!["ids", "filter"],
                "Delete chunks: optional ids and filter, at least one non-empty",
                false,
            ),
            (
                "indexd.list",
                vec!["source", "limit", "offset"],
                "List chunks",
                true,
            ),
            (
                "indexd.feedback",
                vec!["id", "useful"],
                "Record retrieval feedback",
                false,
            ),
            (
                "indexd.supersede",
                vec!["old_id", "new_id", "reason"],
                "Supersede a chunk",
                false,
            ),
            (
                "indexd.stale",
                vec![
                    "never_retrieved_age_days",
                    "low_value_min_retrievals",
                    "long_dormant_days",
                    "per_bucket_limit",
                    "source",
                ],
                "Find stale chunks",
                true,
            ),
            ("indexd.stats", vec![], "Read index statistics", true),
            (
                "indexd.index_file",
                vec!["path", "content", "source", "domain", "background"],
                "Index markdown sections",
                false,
            ),
        ]
        .into_iter()
        .map(|(name, args, help, read_only)| CommandDescriptor::new(name, &args, help, read_only))
        .collect()
    });
    &COMMANDS
}

pub async fn run(engine: Arc<Engine>, mut stop: watch::Receiver<bool>) {
    let bi = buildinfo::build_info!();
    let prov = RegisterProvenance::from_parts(
        bi.pkg,
        bi.version,
        bi.git_sha,
        bi.git_dirty,
        bi.build_time,
        buildinfo::now_rfc3339(),
    );
    let mut delay = Duration::from_secs(1);
    loop {
        if *stop.borrow() {
            break;
        }
        let connected = tokio::select! {
            _ = stop.changed() => break,
            result = ::bus::client_helpers::connect_default_with_provenance(BUS_SERVICE, Some(prov.clone())) => result,
        };
        match connected {
            Ok(client) => {
                let started = Instant::now();
                let client = Arc::new(client.with_commands(command_manifest().to_vec()));
                tracing::info!(service = BUS_SERVICE, "registered on node Bus");
                serve_client(engine.clone(), client.clone(), &mut stop).await;
                client.close().await;
                if started.elapsed() >= Duration::from_secs(30) {
                    delay = Duration::from_secs(1);
                }
            }
            Err(error) => {
                tracing::warn!(%error, retry_secs = delay.as_secs(), "indexd Bus unavailable")
            }
        }
        if *stop.borrow() {
            break;
        }
        tokio::select! { _ = stop.changed() => break, _ = tokio::time::sleep(delay) => {} }
        delay = (delay * 2).min(Duration::from_secs(60));
    }
}

pub(crate) async fn serve_client(
    engine: Arc<Engine>,
    client: Arc<NodedClient>,
    stop: &mut watch::Receiver<bool>,
) {
    let Some(mut incoming) = client.incoming_async().await else {
        return;
    };
    let mut requests = tokio::task::JoinSet::new();
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            _ = stop.changed() => break,
            Some(result) = requests.join_next(), if !requests.is_empty() => {
                if let Err(error) = result { tracing::error!(%error, "Bus request task failed"); }
            }
            command = incoming.recv() => {
                let Some(cmd) = command else { break; };
                if cmd.body.len() > engine.max_request_bytes() {
                    let _ = client.respond(&cmd, 10, r#"{"error":"request exceeds configured byte limit"}"#).await;
                    continue;
                }
                if requests.len() >= 64 {
                    let _ = client.respond(&cmd, 10, r#"{"error":"indexd busy"}"#).await;
                    continue;
                }
                let engine = engine.clone();
                let client = client.clone();
                requests.spawn(async move {
                    let body = if cmd.body.is_empty() { serde_json::json!({}) } else {
                        match serde_json::from_str(&cmd.body) {
                            Ok(body) => body,
                            Err(error) => {
                                let _ = client.respond(&cmd, 10, &serde_json::json!({"error":format!("invalid JSON: {error}")}).to_string()).await;
                                return;
                            }
                        }
                    };
                    let reply = engine.call_sized(&cmd.command, body, cmd.body.len()).await;
                    let rc = if reply.get("error").is_some() { 10 } else { 0 };
                    if let Err(error) = client.respond(&cmd, rc, &reply.to_string()).await {
                        tracing::warn!(%error, "Bus reply failed");
                    }
                });
            }
        }
    }
    // Complete accepted calls before closing the connection on SIGTERM.
    while let Some(result) = requests.join_next().await {
        if let Err(error) = result {
            tracing::error!(%error, "Bus request task failed during shutdown");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_reply_is_rc10_and_connection_survives() {
        let broker = test_broker::Broker::start();
        let root = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.service.vectors_db = root
            .path()
            .join("vectors.db")
            .to_string_lossy()
            .into_owned();
        config.service.model_dir = root.path().join("model").to_string_lossy().into_owned();
        config.service.max_response_bytes = 128;
        crate::test_support::model_files(&config);
        let engine = Arc::new(Engine::open(config).unwrap());
        let client = Arc::new(
            NodedClient::connect(BUS_SERVICE, &broker.url)
                .await
                .unwrap(),
        );
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            serve_client(engine, client.clone(), &mut stop_rx).await;
            client.close().await;
        });
        let caller = NodedClient::connect_anonymous(&broker.url).await.unwrap();
        for _ in 0..2 {
            let (rc, body, _) = caller
                .call_with_headers_raw(BUS_SERVICE, "indexd.stats", &Default::default(), "{}")
                .await
                .unwrap();
            assert_eq!(rc, 10);
            assert!(body.contains("response_too_large"), "{body}");
        }
        caller.close().await;
        let _ = stop_tx.send(true);
        task.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_contract() {
        let broker = test_broker::Broker::start();
        let root = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.service.vectors_db = root
            .path()
            .join("vectors.db")
            .to_string_lossy()
            .into_owned();
        config.service.model_dir = root.path().join("model").to_string_lossy().into_owned();
        crate::test_support::model_files(&config);
        let engine = Arc::new(Engine::open(config).unwrap());
        {
            let mut guard = engine.state.lock().await;
            for (text, value) in [("alpha", 0.0), ("beta", 0.5), ("changed", 0.2)] {
                guard.embed_cache.store(
                    text,
                    "search_document: ",
                    vec![value; crate::EMBEDDING_DIM],
                );
            }
            guard
                .embed_cache
                .store("query", "search_query: ", vec![0.0; crate::EMBEDDING_DIM]);
        }
        let client = Arc::new(
            NodedClient::connect(BUS_SERVICE, &broker.url)
                .await
                .unwrap()
                .with_commands(command_manifest().to_vec()),
        );
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            serve_client(engine, client.clone(), &mut stop_rx).await;
            client.close().await;
        });
        let mix = std::env::var("MIXOS_TEST_MIX").unwrap_or_else(|_| "/opt/mixos/bin/mix".into());
        let output = tokio::process::Command::new(mix)
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/bus_test.mix"))
            .env("MIXOS_NODED_URL", &broker.url)
            .env("INDEXD_TEST_BROKER", "1")
            .output()
            .await
            .unwrap();
        let _ = stop_tx.send(true);
        task.await.unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        println!("{}", String::from_utf8_lossy(&output.stdout));
    }
}
