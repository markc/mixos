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
/// ```text
/// map: { "~/Projects/mixos": "mixos" }
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
            ("~/.mix".into(), "mix".into()),
            ("~/.ns".into(), "ns".into()),
            ("~/.mc".into(), "mc".into()),
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
        self.resolve_with_home(path, &home)
    }

    fn resolve_with_home(&self, path: &Path, home: &Path) -> Option<String> {
        let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());

        let mut best: Option<(usize, &str)> = None;
        for (prefix, domain) in &self.map {
            let expanded = if let Some(rest) = prefix.strip_prefix("~/") {
                home.join(rest)
            } else if prefix == "~" {
                home.clone()
            } else {
                std::path::PathBuf::from(prefix)
            };
            let expanded = expanded.canonicalize().unwrap_or(expanded);
            if path.starts_with(&expanded) {
                let len = expanded.as_os_str().len();
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
    pub indexd: IndexdSettings,
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
            indexd: IndexdSettings::default(),
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

/// Client-side indexd connection. SSH credentials and host names belong in
/// the user's knowledge.conf.mix, never in component defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexdSettings {
    pub transport: IndexdTransport,
    pub target: String,
    pub ssh_host: String,
    pub ssh_command: Vec<String>,
    pub call_timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexdTransport {
    #[default]
    Bus,
    Ssh,
}

impl Default for IndexdSettings {
    fn default() -> Self {
        Self {
            transport: IndexdTransport::Bus,
            target: "indexd".into(),
            ssh_host: String::new(),
            ssh_command: vec!["mixos:prefix/bin/indexd".into(), "bridge".into()],
            call_timeout_secs: 120,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_match_canonical_home_and_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("real-home");
        std::fs::create_dir_all(home.join("Projects/repo")).unwrap();
        let alias = dir.path().join("home-alias");
        std::os::unix::fs::symlink(&home, &alias).unwrap();
        let domains = DomainsSettings {
            map: BTreeMap::from([
                ("~".into(), "home".into()),
                ("~/Projects".into(), "projects".into()),
                (
                    alias.join("Projects/repo").to_string_lossy().into_owned(),
                    "repo".into(),
                ),
            ]),
        };
        assert_eq!(
            domains
                .resolve_with_home(&home.join("Projects/repo/_doc/a.md"), &alias)
                .as_deref(),
            Some("repo")
        );
        assert_eq!(
            domains
                .resolve_with_home(&home.join("Projects/other"), &alias)
                .as_deref(),
            Some("projects")
        );
        assert_eq!(
            domains
                .resolve_with_home(&home.join("Projects-other"), &alias)
                .as_deref(),
            Some("home")
        );
    }

    #[test]
    fn knowledge_transport_defaults_and_partial_config() {
        let dir = tempfile::tempdir().unwrap();
        let absent =
            config::store::load_service_in::<KnowledgeSettings>(dir.path(), "knowledge").unwrap();
        assert_eq!(absent.indexd, IndexdSettings::default());
        std::fs::write(
            dir.path().join("knowledge.conf.mix"),
            "indexd: {transport: \"ssh\", ssh_host: \"user@host.example\"}\n",
        )
        .unwrap();
        let partial =
            config::store::load_service_in::<KnowledgeSettings>(dir.path(), "knowledge").unwrap();
        assert_eq!(partial.indexd.transport, IndexdTransport::Ssh);
        assert_eq!(partial.indexd.target, "indexd");
        assert_eq!(partial.indexd.call_timeout_secs, 120);
        assert_eq!(
            partial.indexd.ssh_command,
            ["mixos:prefix/bin/indexd", "bridge"]
        );
        std::fs::write(
            dir.path().join("knowledge.conf.mix"),
            "indexd: {transport: \"invalid\"}\n",
        )
        .unwrap();
        assert!(
            config::store::load_service_in::<KnowledgeSettings>(dir.path(), "knowledge").is_err()
        );
    }
}
