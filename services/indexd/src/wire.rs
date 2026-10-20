// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Ported from the predecessor indexd's src/main.rs.

use crate::*;

#[derive(Deserialize)]
#[serde(tag = "action")]
#[serde(rename_all = "snake_case")]
pub(crate) enum Request {
    Embed(EmbedRequest),
    Store(StoreRequest),
    Search(SearchRequest),
    Update(UpdateRequest),
    Delete(DeleteRequest),
    List(ListRequest),
    Feedback(FeedbackRequest),
    Supersede(SupersedeRequest),
    Stale(StaleRequest),
    IndexFile(IndexFileRequest),
    Stats,
}

#[derive(Deserialize)]
pub(crate) struct EmbedRequest {
    pub(crate) texts: Vec<String>,
    #[serde(default = "default_doc_prefix")]
    pub(crate) prefix: String,
}

#[derive(Deserialize)]
pub(crate) struct StoreRequest {
    pub(crate) texts: Vec<String>,
    #[serde(default)]
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) metadata: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct SearchRequest {
    pub(crate) query: String,
    #[serde(default = "default_limit")]
    pub(crate) limit: usize,
    #[serde(default)]
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) metadata_filter: Vec<MetadataFilter>,
}

#[derive(Deserialize)]
pub(crate) struct MetadataFilter {
    pub(crate) field: String,
    pub(crate) op: FilterOp,
    pub(crate) value: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FilterOp {
    Eq,
    Gt,
    Lt,
    Gte,
    Lte,
    Contains,
}

#[derive(Deserialize)]
pub(crate) struct UpdateRequest {
    pub(crate) id: i64,
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(default)]
    pub(crate) metadata: Option<String>,
    #[serde(default)]
    pub(crate) source: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct DeleteRequest {
    #[serde(default)]
    pub(crate) ids: Vec<i64>,
    #[serde(default)]
    pub(crate) filter: Vec<MetadataFilter>,
}

#[derive(Deserialize)]
pub(crate) struct ListRequest {
    #[serde(default)]
    pub(crate) source: String,
    #[serde(default = "default_limit")]
    pub(crate) limit: usize,
    #[serde(default)]
    pub(crate) offset: usize,
}

#[derive(Deserialize)]
pub(crate) struct FeedbackRequest {
    pub(crate) id: i64,
    pub(crate) useful: bool,
}

#[derive(Deserialize)]
pub(crate) struct SupersedeRequest {
    /// ID of the older chunk being superseded (will be hidden from context_search).
    pub(crate) old_id: i64,
    /// ID of the newer chunk that replaces it.
    pub(crate) new_id: i64,
    /// Optional human-readable reason (logged but not persisted in this version).
    #[serde(default)]
    pub(crate) reason: String,
}

#[derive(Deserialize)]
pub(crate) struct StaleRequest {
    /// Only include chunks of this source (empty = all sources).
    #[serde(default)]
    pub(crate) source: String,
    /// "Never retrieved & old" bucket: age > this many days (default 90).
    #[serde(default = "default_stale_age_days")]
    pub(crate) never_retrieved_age_days: i64,
    /// "Low value" bucket: retrieval_count > this AND feedback_score <= 0 (default 3).
    #[serde(default = "default_low_value_retrievals")]
    pub(crate) low_value_min_retrievals: i64,
    /// "Long dormant" bucket: last_retrieved older than this many days (default 180).
    #[serde(default = "default_dormant_days")]
    pub(crate) long_dormant_days: i64,
    /// Max chunks per bucket (default 50).
    #[serde(default = "default_stale_limit")]
    pub(crate) per_bucket_limit: usize,
}

#[derive(Deserialize, Clone)]
pub(crate) struct IndexFileRequest {
    /// Absolute path to the markdown file to index. Used for metadata
    /// (path/filename/date) and source/domain auto-detection. When
    /// `content` is supplied the file itself is never read, so the path
    /// need not be readable by the daemon — only well-formed.
    pub(crate) path: String,
    /// Optional file body. When present, the daemon indexes this text
    /// instead of reading `path` from disk. This is the only way to index
    /// files the daemon cannot read itself: under `ProtectSystem=strict`
    /// with `ProtectHome=yes` the `/home/<user>/…` tree is invisible to
    /// the indexd sandbox, so a git post-commit hook (running as the user,
    /// who *can* read the file) reads it and passes the bytes here.
    #[serde(default)]
    pub(crate) content: Option<String>,
    /// Source type: "doc" or "journal". Auto-detected from path if omitted.
    #[serde(default)]
    pub(crate) source: String,
    /// Domain name (e.g. "mixos"). Auto-detected from path if omitted.
    #[serde(default)]
    pub(crate) domain: String,
    /// Opt-in fire-and-forget mode. When `true`, `index_file` enqueues the
    /// request onto the background job queue and returns an immediate ack
    /// instead of indexing synchronously — so a large doc whose sequential
    /// per-section embed exceeds the caller's Bus `send` timeout no longer
    /// produces a false `rc=10` while indexd quietly finishes. Defaults to
    /// `false`, preserving the synchronous path byte-for-byte for existing
    /// callers (MCP, interactive). The field is named `background` rather
    /// than `async` because `async` is a Rust keyword.
    #[serde(default)]
    pub(crate) background: bool,
}

pub(crate) fn default_stale_age_days() -> i64 {
    90
}
pub(crate) fn default_low_value_retrievals() -> i64 {
    3
}
pub(crate) fn default_dormant_days() -> i64 {
    180
}
pub(crate) fn default_stale_limit() -> usize {
    50
}

#[derive(Serialize)]
pub(crate) struct StaleChunk {
    pub(crate) id: i64,
    pub(crate) source: String,
    pub(crate) preview: String,
    pub(crate) retrieval_count: i64,
    pub(crate) feedback_score: i64,
    pub(crate) last_retrieved: Option<String>,
    pub(crate) created: String,
    pub(crate) path: Option<String>,
    pub(crate) filename: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct StaleResponse {
    pub(crate) never_retrieved_old: Vec<StaleChunk>,
    pub(crate) low_value: Vec<StaleChunk>,
    pub(crate) long_dormant: Vec<StaleChunk>,
    pub(crate) total_chunks: usize,
}

pub(crate) fn default_doc_prefix() -> String {
    "search_document: ".into()
}

pub(crate) fn default_limit() -> usize {
    10
}

#[derive(Serialize)]
pub(crate) struct EmbedResponse {
    pub(crate) embeddings: Vec<Vec<f32>>,
}

#[derive(Serialize)]
pub(crate) struct StoreResponse {
    pub(crate) stored: usize,
    pub(crate) duplicates: usize,
    pub(crate) ids: Vec<i64>,
}

#[derive(Serialize)]
pub(crate) struct SearchResult {
    pub(crate) id: i64,
    pub(crate) content: String,
    pub(crate) source: String,
    pub(crate) metadata: String,
    pub(crate) distance: f64,
    pub(crate) feedback_score: i64,
    pub(crate) retrieval_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_retrieved: Option<String>,
    pub(crate) created: String,
}

#[derive(Serialize)]
pub(crate) struct SearchResponse {
    pub(crate) results: Vec<SearchResult>,
}

#[derive(Serialize)]
pub(crate) struct UpdateResponse {
    pub(crate) updated: bool,
    pub(crate) re_embedded: bool,
}

#[derive(Serialize)]
pub(crate) struct DeleteResponse {
    pub(crate) deleted: usize,
}

#[derive(Serialize)]
pub(crate) struct ListItem {
    pub(crate) id: i64,
    pub(crate) content: String,
    pub(crate) source: String,
    pub(crate) metadata: String,
    pub(crate) created: String,
}

#[derive(Serialize)]
pub(crate) struct ListResponse {
    pub(crate) items: Vec<ListItem>,
    pub(crate) total: usize,
}

#[derive(Serialize)]
pub(crate) struct StatsResponse {
    pub(crate) total_vectors: usize,
    pub(crate) db_size_bytes: u64,
    pub(crate) model_loaded: bool,
    pub(crate) model_circuit: String,
    pub(crate) embed_circuit: String,
    pub(crate) embed_cache_entries: usize,
    pub(crate) embed_cache_hits: u64,
    pub(crate) embed_cache_misses: u64,
    pub(crate) queue_depth: usize,
    pub(crate) queued_bytes: u64,
    pub(crate) inference_in_flight: usize,
    pub(crate) timed_out_still_running: usize,
    pub(crate) model_generation: u64,
    pub(crate) rss_before_load_bytes: u64,
    pub(crate) rss_after_load_bytes: u64,
    pub(crate) rss_after_unload_bytes: u64,
    #[serde(default)]
    pub(crate) by_source: Vec<SourceCount>,
}

#[derive(Serialize)]
pub(crate) struct SourceCount {
    pub(crate) source: String,
    pub(crate) count: usize,
}

#[derive(Serialize)]
pub(crate) struct ErrorResponse {
    pub(crate) error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) code: Option<String>,
}
