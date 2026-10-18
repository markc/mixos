// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Typed configuration structs — one struct per service.
//!
//! Every struct derives `Default` with values matching what apps currently
//! hardcode, so a freshly-materialised `~/.config/mixos/<service>.conf.mix`
//! is immediately usable.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Path-prefix → domain name mapping.
///
/// Used by the knowledge base to map workspace paths to domain keys.
/// Entries are checked longest-prefix-first.
///
/// ```toml
/// [domains.map]
/// "~/Projects/mixos" = "mixos"


/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DomainsSettings {
    pub map: BTreeMap<String, String>,
}

impl Default for DomainsSettings {
    fn default() -> Self {
        let map = BTreeMap::from([
            ("~/Projects/mixos".into(), "mixos".into()),
            
            
        ]);
        Self { map }
    }
}

impl DomainsSettings {
    /// Resolve a filesystem path to a domain name via prefix matching.
    ///
    /// Expands `~` in map keys to `$HOME`. Returns `None` if no prefix matches.
    /// Longest prefix wins when multiple entries match.
    pub fn resolve(&self, path: &Path) -> Option<String> {
        let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
        let path_str = path.to_string_lossy();

        let mut best: Option<(usize, &str)> = None;
        for (prefix, domain) in &self.map {
            let expanded = if let Some(rest) = prefix.strip_prefix("~/") {
                format!("{}/{rest}", home.display())
            } else {
                prefix.clone()
            };
            if path_str.starts_with(&expanded) {
                let len = expanded.len();
                if best.is_none() || len > best.unwrap().0 {
                    best = Some((len, domain));
                }
            }
        }
        best.map(|(_, d)| d.to_string())
    }
}

/// LLM backend configuration — supports multiple named backends.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmSettings {
    /// Which backend to use by default (key into `backends` table).
    pub default: String,
    /// Named backend configurations.
    pub backends: std::collections::BTreeMap<String, LlmBackendConfig>,
}

impl Default for LlmSettings {
    fn default() -> Self {
        let mut backends = std::collections::BTreeMap::new();
        backends.insert(
            "ollama".into(),
            LlmBackendConfig {
                provider: "ollama".into(),
                model: "qwen3:30b-a3b-nt".into(),
                base_url: "http://localhost:11434".into(),
                api_key_env: String::new(),
                api_key_cmd: String::new(),
                port: String::new(),
                command: String::new(),
            },
        );
        // Default: routes through the Claude Code CLI's OAuth session (MAX
        // plan) via `claude -p`. Zero incremental cost. Text-only — no tool
        // use, so agent loops must use `claude-api` or `ollama` instead.
        backends.insert(
            "claude-cli".into(),
            LlmBackendConfig {
                provider: "claude-cli".into(),
                model: "haiku".into(),
                base_url: String::new(),
                api_key_env: String::new(),
                api_key_cmd: String::new(),
                port: String::new(),
                command: String::new(),
            },
        );
        // Paid Anthropic Messages API. Required for tool-calling workloads
        // (multi-turn agent loops). Keyed by MIXOS_ANTHROPIC_KEY to avoid collision
        // with ANTHROPIC_API_KEY, which Claude Code picks up and uses to
        // bypass the MAX subscription.
        backends.insert(
            "claude-api".into(),
            LlmBackendConfig {
                provider: "anthropic".into(),
                model: "claude-haiku-4-5-20251001".into(),
                base_url: "https://api.anthropic.com".into(),
                api_key_env: "MIXOS_ANTHROPIC_KEY".into(),
                api_key_cmd: String::new(),
                port: String::new(),
                command: String::new(),
            },
        );
        backends.insert(
            "claud".into(),
            LlmBackendConfig {
                provider: "bus".into(),
                model: String::new(),
                base_url: String::new(),
                api_key_env: String::new(),
                api_key_cmd: String::new(),
                port: "claud".into(),
                command: "ask".into(),
            },
        );
        Self {
            default: "claude-cli".into(),
            backends,
        }
    }
}

/// Configuration for a single LLM backend.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LlmBackendConfig {
    /// Provider type: "anthropic", "openai", "ollama", "bus".
    pub provider: String,
    /// Model identifier (e.g. "claude-haiku-4-5-20251001", "gpt-4o-mini", "qwen3:30b-a3b-nt").
    pub model: String,
    /// Base URL for HTTP-based providers.
    pub base_url: String,
    /// Environment variable name containing the API key.
    pub api_key_env: String,
    /// Shell command that outputs the API key (alternative to env var).
    pub api_key_cmd: String,
    /// Bus port name (for "bus" provider only).
    pub port: String,
    /// Bus command name (for "bus" provider, default "ask").
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsSettings {
    /// Minimum confidence threshold for skill retrieval (0.0–1.0).
    pub min_confidence: f64,
    /// Maximum skills injected into agent prompts.
    pub max_skills: u32,
    /// LLM backend name to use (key in `[llm.backends]`, empty = use `[llm].default`).
    pub llm_backend: String,
    /// Minimum confidence for skill graduation to CLAUDE.md (0.0–1.0).
    pub graduation_confidence: f64,
    /// Minimum use count for skill graduation.
    pub graduation_min_uses: u32,
    /// Minimum success count for skill graduation.
    pub graduation_min_successes: u32,
}

impl Default for SkillsSettings {
    fn default() -> Self {
        Self {
            min_confidence: 0.3,
            max_skills: 3,
            llm_backend: String::new(),
            graduation_confidence: 0.9,
            graduation_min_uses: 5,
            graduation_min_successes: 4,
        }
    }
}

/// Knowledge base ranking: trust weights per source type + temporal decay.
/// Applied in MCP context_search after indexd returns raw vector-similarity results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeSettings {
    /// Distance bonus for `_spec/` chunks (normative contracts, highest authority).
    pub trust_weight_spec: f64,
    /// Distance bonus for `_doc/` chunks (curated design truth). Subtracted from distance.
    pub trust_weight_doc: f64,
    /// Distance bonus for `_notes.md` files (persistent operational facts).
    pub trust_weight_notes: f64,
    /// Distance bonus for `_plan/` chunks (provisional implementation plans).
    pub trust_weight_plan: f64,
    /// Distance bonus for skills that have graduated to CLAUDE.md.
    pub trust_weight_graduated: f64,
    /// Distance bonus for ungraduated (provisional) skills.
    pub trust_weight_skill: f64,
    /// Distance bonus for journal entries (operational residue, usually 0.0).
    pub trust_weight_journal: f64,
    /// Distance bonus for _memory/ chunks (CMM-generated observations).
    pub trust_weight_memory: f64,
    /// Journal decay: distance penalty per month of age.
    pub journal_decay_per_month: f64,
    /// Journals older than this many days are excluded from results entirely.
    pub journal_max_age_days: u32,
}

impl Default for KnowledgeSettings {
    fn default() -> Self {
        Self {
            trust_weight_spec: 0.10,
            trust_weight_doc: 0.08,
            trust_weight_notes: 0.06,
            trust_weight_plan: 0.04,
            trust_weight_graduated: 0.06,
            trust_weight_skill: 0.03,
            trust_weight_journal: 0.0,
            trust_weight_memory: 0.01,
            journal_decay_per_month: 0.02,
            journal_max_age_days: 180,
        }
    }
}

