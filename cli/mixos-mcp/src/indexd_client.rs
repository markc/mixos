// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Clone)]
pub struct IndexdClient {
    transport: super::indexd_transport::Transport,
}

impl IndexdClient {
    pub async fn from_config() -> Result<Self> {
        let settings = config::store::load_service::<crate::knowledge_settings::KnowledgeSettings>(
            "knowledge",
        )?;
        let mut client = Self {
            transport: super::indexd_transport::Transport::connect(settings.indexd).await?,
        };
        // Probe the configured address, which may be a routed Bus service.
        let _: serde_json::Value = client.request(r#"{"action":"stats"}"#).await?;
        Ok(client)
    }

    pub fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }

    pub async fn shutdown(&self) {
        self.transport.shutdown().await;
    }

    async fn request<T: for<'de> Deserialize<'de>>(&mut self, json: &str) -> Result<T> {
        let request: serde_json::Value = serde_json::from_str(json)?;
        let action = request["action"].as_str().context("missing index action")?;
        let response = self
            .transport
            .call(&format!("indexd.{action}"), &request)
            .await?;
        if let Some(error) = response.get("error").and_then(serde_json::Value::as_str) {
            anyhow::bail!("indexd error: {error}");
        }
        serde_json::from_value(response).context("parsing indexd response")
    }

    /// Generic raw JSON request — send any valid indexd action and get the response.
    /// Used for context retrieval, feedback and workspace indexing.
    pub async fn raw_request(&mut self, req: &serde_json::Value) -> Result<serde_json::Value> {
        self.request(&req.to_string()).await
    }
}
