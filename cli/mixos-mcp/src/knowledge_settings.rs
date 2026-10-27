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
    fn knowledge_loader_tolerates_retired_keys_and_ignores_retired_files() {
        let dir = tempfile::tempdir().unwrap();
        let content = r#"
indexd: {target: "knowledge-fixture"}
trust_weight_doc: 0.12
trust_weight_graduated: 0.06
trust_weight_skill: 0.03
skills: {min_confidence: 0.3, max_skills: 3}
llm: {default: "retired", backends: {retired: {provider: "ollama"}}}
"#;
        std::fs::write(dir.path().join("knowledge.conf.mix"), content).unwrap();
        // Invalid files prove that loading knowledge does not consult these.
        for name in ["skills.conf.mix", "llm.conf.mix"] {
            std::fs::write(dir.path().join(name), "this is not strict-data config {").unwrap();
        }
        // load_service delegates to this directory-explicit core. Avoid process
        // env changes: the shared paths resolver caches configuration locations.
        let loaded =
            config::store::load_service_in::<KnowledgeSettings>(dir.path(), "knowledge").unwrap();
        assert_eq!(loaded.indexd.target, "knowledge-fixture");
        assert_eq!(loaded.trust_weight_doc, 0.12);
        let saved = serde_json::to_value(&loaded).unwrap();
        for key in [
            "trust_weight_graduated",
            "trust_weight_skill",
            "skills",
            "llm",
        ] {
            assert!(saved.get(key).is_none(), "retired key {key} retained");
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("knowledge.conf.mix")).unwrap(),
            content
        );
        let defaults = tempfile::tempdir().unwrap();
        config::store::load_service_in::<KnowledgeSettings>(defaults.path(), "knowledge").unwrap();
        let generated =
            std::fs::read_to_string(defaults.path().join("knowledge.conf.mix")).unwrap();
        for key in [
            "trust_weight_graduated",
            "trust_weight_skill",
            "skills",
            "llm",
        ] {
            assert!(
                !generated.contains(key),
                "default config still contains {key}"
            );
        }
        assert!(!defaults.path().join("skills.conf.mix").exists());
        assert!(!defaults.path().join("llm.conf.mix").exists());
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
