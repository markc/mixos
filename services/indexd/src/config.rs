// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Strict-data configuration and filesystem domain policy.
use crate::*;




#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub service: ServiceConfig,
    pub source_types: BTreeMap<String, SourceTypeSpec>,
    pub domains: Domains,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            service: ServiceConfig::default(),
            source_types: default_source_types(),
            domains: Domains::default(),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceTypeSpec {
    pub required: Vec<String>,
    pub date_field: Option<String>,
}

fn default_source_types() -> BTreeMap<String, SourceTypeSpec> {
    // Preserve the deployed source policy, including legacy corpus types.
    [
        ("doc", vec!["path", "domain"], None),
        ("journal", vec!["path", "domain", "date"], Some("date")),
        (
            "memory",
            vec!["path", "domain", "date", "generator"],
            Some("date"),
        ),
        ("mix-script", vec!["file", "domain"], None),
        ("notes", vec!["path", "domain"], None),
        (
            "observation",
            vec!["observer", "tool", "observed_at", "domain"],
            None,
        ),
        ("plan", vec!["path", "domain"], None),
        ("rust-doc", vec!["file", "kind", "domain"], None),
        ("skill", vec!["name", "trigger", "approach"], None),
        ("spec", vec!["path", "domain"], None),
    ]
    .into_iter()
    .map(|(name, required, date_field)| {
        (
            name.into(),
            SourceTypeSpec {
                required: required.into_iter().map(str::to_owned).collect(),
                date_field: date_field.map(str::to_owned),
            },
        )
    })
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceConfig {
    pub vectors_db: String,
    pub model_dir: String,
    pub idle_timeout_secs: u64,
    pub dtype: String,
    pub vindex_dtype: String,
    pub max_sequence_tokens: usize,
    pub max_forward_tokens: usize,
    pub max_request_text_bytes: usize,
    pub max_prefix_bytes: usize,
    pub max_request_line_bytes: usize,
    pub max_response_bytes: usize,
    pub inference_admission_timeout_secs: u64,
    pub background_queue_max_bytes: usize,
    pub background_queue_max_jobs: usize,
    pub background_retry_max_attempts: usize,
    pub background_retry_initial_backoff_secs: u64,
    pub max_file_bytes: usize,
    pub journal_size_limit_bytes: u64,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            vectors_db: ::paths::root(::paths::Root::State).join("indexd/vectors.db").to_string_lossy().into_owned(),
            model_dir: resolve_snapshot(&::paths::root(::paths::Root::Cache).join("indexd/huggingface/hub/models--nomic-ai--nomic-embed-text-v1.5/snapshots"))
                .to_string_lossy()
                .into_owned(),
            idle_timeout_secs: 300,
            dtype: "f16".into(),
            vindex_dtype: "f32".into(),
            max_sequence_tokens: 2048,
            max_forward_tokens: 4096,
            max_request_text_bytes: (8 * 1024 * 1024).min(BUS_BODY_LIMIT),
            max_prefix_bytes: 4 * 1024,
            max_request_line_bytes: BUS_BODY_LIMIT,
            max_response_bytes: BUS_BODY_LIMIT,
            inference_admission_timeout_secs: 30,
            background_queue_max_bytes: 128 * 1024 * 1024,
            background_queue_max_jobs: 512,
            background_retry_max_attempts: 5,
            background_retry_initial_backoff_secs: 2,
            max_file_bytes: 16 * 1024 * 1024,
            journal_size_limit_bytes: 64 * 1024 * 1024,
        }
    }
}

fn resolve_snapshot(root: &Path) -> PathBuf {
    let dirs: Vec<_> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    if dirs.len() == 1 {
        dirs[0].clone()
    } else {
        root.to_path_buf()
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            (
                "max_request_text_bytes",
                self.service.max_request_text_bytes,
            ),
            (
                "max_request_line_bytes",
                self.service.max_request_line_bytes,
            ),
            ("max_response_bytes", self.service.max_response_bytes),
        ] {
            anyhow::ensure!(
                value > 0 && value <= BUS_BODY_LIMIT,
                "{name} must be between 1 and {BUS_BODY_LIMIT} bytes (Bus frame budget)"
            );
        }
        anyhow::ensure!(
            matches!(self.service.dtype.as_str(), "f16" | "f32"),
            "dtype must be f16 or f32"
        );
        vindex::VectorDtype::parse(&self.service.vindex_dtype).map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            self.service.max_forward_tokens > 0,
            "max_forward_tokens must be positive"
        );
        anyhow::ensure!(
            self.service.background_queue_max_jobs > 0,
            "background_queue_max_jobs must be positive"
        );
        anyhow::ensure!(
            !self.service.model_dir.is_empty(),
            "model_dir must name a local model directory"
        );
        for name in ["config.json", "tokenizer.json", "model.safetensors"] {
            let path = Path::new(&self.service.model_dir).join(name);
            anyhow::ensure!(
                path.is_file(),
                "missing local model file {}",
                path.display()
            );
        }
        for prefix in self.domains.map.keys() {
            anyhow::ensure!(
                Path::new(prefix).is_absolute(),
                "domains.map prefixes must be absolute: {prefix}"
            );
        }
        Ok(())
    }
}

pub fn load_indexd_config(cli_path: Option<&Path>) -> Result<(PathBuf, Config)> {
    if let Some(path) = cli_path {
        let path = path.to_path_buf();
        return Ok((path.clone(), ::config::load_conf_mix_path(&path)?));
    }
    if let Some(found) = load_system_config(&::paths::root(::paths::Root::Config).join("indexd"))? {
        return Ok(found);
    }
    let user_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    if let Some(dir) = user_dir {
        let path = dir.join("mixos/indexd.conf.mix");
        if let Some(cfg) = load_optional(&path)? {
            return Ok((path, cfg));
        }
    }
    Ok((
        ::paths::root(::paths::Root::Config).join("indexd").join("config.conf.mix"),
        Config::default(),
    ))
}

fn load_optional(path: &Path) -> Result<Option<Config>> {
    match std::fs::metadata(path) {
        Ok(_) => ::config::load_conf_mix_path(path)
            .map(Some)
            .with_context(|| format!("loading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn load_system_config(dir: &Path) -> Result<Option<(PathBuf, Config)>> {
    let path = dir.join("config.conf.mix");
    Ok(load_optional(&path)?.map(|cfg| (path, cfg)))
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Domains {
    pub map: BTreeMap<String, String>,
}

impl Domains {
    pub fn resolve(&self, path: &Path) -> Option<String> {
        self.map
            .iter()
            .filter_map(|(prefix, domain)| {
                let prefix = PathBuf::from(prefix);
                path.starts_with(&prefix)
                    .then_some((prefix.as_os_str().len(), domain))
            })
            .max_by_key(|(len, _)| *len)
            .map(|(_, domain)| domain.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_dir;
    #[test]
    fn system_none_when_dir_empty() {
        let dir = temp_dir("empty");
        assert!(load_system_config(&dir).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn system_reads_conf_mix() {
        let dir = temp_dir("prefer");
        std::fs::write(
            dir.join("config.conf.mix"),
            "schema_version: 7\nservice: { model_dir: \"/srv/model\" }\n",
        )
        .unwrap();
        let (path, cfg) = load_system_config(&dir).unwrap().expect("some");
        assert_eq!(path, dir.join("config.conf.mix"));
        assert_eq!(cfg.schema_version, 7);
        assert_eq!(cfg.service.model_dir, "/srv/model");
        assert_eq!(cfg.service.max_sequence_tokens, 2048);
        assert_eq!(cfg.service.max_forward_tokens, 4096);
        assert_eq!(cfg.service.vindex_dtype, "f32");
        assert_eq!(cfg.service.inference_admission_timeout_secs, 30);
        assert_eq!(cfg.service.max_prefix_bytes, 4096);
        assert_eq!(cfg.service.max_response_bytes, BUS_BODY_LIMIT);
        assert_eq!(cfg.service.background_retry_max_attempts, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn configured_memory_bounds_override_individually() {
        let cfg: Config = ::config::from_conf_mix_str(
            "service: { max_sequence_tokens: 1024, max_forward_tokens: 4096, \
             max_request_text_bytes: 12345 }\n",
        )
        .unwrap();
        assert_eq!(cfg.service.max_sequence_tokens, 1024);
        assert_eq!(cfg.service.max_forward_tokens, 4096);
        assert_eq!(cfg.service.max_request_text_bytes, 12_345);
        assert_eq!(cfg.service.max_file_bytes, 16 * 1024 * 1024);
    }
    #[test]
    fn system_legacy_toml_is_ignored() {
        // C11: the TOML fallback is gone. A lone legacy config.toml is
        // never read — resolution reports "no config" (None).
        let dir = temp_dir("legacy");
        std::fs::write(
            dir.join("config.toml"),
            "schema_version = 3\n[service]\nmodel_dir = \"/run/legacy.sock\"\n",
        )
        .unwrap();
        assert!(load_system_config(&dir).unwrap().is_none());
        assert!(!dir.join("config.conf.mix").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn system_bad_conf_mix_is_hard_error() {
        let dir = temp_dir("badparse");
        // An unparseable .conf.mix must hard-fail.
        std::fs::write(dir.join("config.conf.mix"), "schema_version: $oops\n").unwrap();
        assert!(load_system_config(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;
    use crate::test_support::temp_dir;

    #[test]
    fn byte_limits_cannot_exceed_bus_frame_budget() {
        for field in [
            "max_request_text_bytes",
            "max_request_line_bytes",
            "max_response_bytes",
        ] {
            let mut cfg = Config::default();
            match field {
                "max_request_text_bytes" => cfg.service.max_request_text_bytes = BUS_BODY_LIMIT + 1,
                "max_request_line_bytes" => cfg.service.max_request_line_bytes = BUS_BODY_LIMIT + 1,
                _ => cfg.service.max_response_bytes = BUS_BODY_LIMIT + 1,
            }
            assert!(
                cfg.validate()
                    .unwrap_err()
                    .to_string()
                    .contains("Bus frame budget")
            );
        }
    }

    #[test]
    fn validation_follows_model_symlinks_and_rejects_relative_domains() {
        let dir = temp_dir("validate");
        let mut cfg = Config::default();
        cfg.service.model_dir = dir.join("snapshot").to_string_lossy().into_owned();
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("config.json")
        );
        std::fs::create_dir(dir.join("snapshot")).unwrap();
        std::fs::write(dir.join("blob"), "fixture").unwrap();
        for name in ["config.json", "tokenizer.json", "model.safetensors"] {
            std::os::unix::fs::symlink("../blob", dir.join("snapshot").join(name)).unwrap();
        }
        cfg.validate().unwrap();
        cfg.domains.map.insert("~/project".into(), "project".into());
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("must be absolute")
        );
        std::fs::remove_file(dir.join("blob")).unwrap();
        assert!(cfg.validate().is_err(), "broken symlinks must fail");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn default_source_policy_matches_deployed_config() {
        let deployed: Config =
            ::config::from_conf_mix_str(include_str!("../tests/source_types.conf.mix")).unwrap();
        assert_eq!(Config::default().source_types, deployed.source_types);
        assert!(
            validate_store_entry(
                "journal",
                r#"{"path":"/srv/a.md","domain":"mixos","date":"2026-10-20"}"#,
                &deployed
            )
            .is_ok()
        );
        assert!(
            validate_store_entry(
                "journal",
                r#"{"path":"/srv/a.md","domain":"mixos"}"#,
                &deployed
            )
            .is_err()
        );
        assert!(
            validate_store_entry(
                "skill",
                r#"{"name":"name","trigger":"trigger","approach":"approach"}"#,
                &deployed
            )
            .is_ok()
        );
    }

    #[test]
    fn explicit_config_path_is_literal() {
        let dir = temp_dir("literal-config");
        std::fs::create_dir(dir.join("~")).unwrap();
        let path = dir.join("~/config.conf.mix");
        std::fs::write(&path, "schema_version: 7\n").unwrap();
        let (loaded, cfg) = load_indexd_config(Some(&path)).unwrap();
        assert_eq!(loaded, path);
        assert_eq!(cfg.schema_version, 7);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snapshot_resolution_requires_one_directory() {
        let dir = temp_dir("snapshots");
        assert_eq!(resolve_snapshot(&dir), dir);
        std::fs::create_dir(dir.join("rev-a")).unwrap();
        std::fs::write(dir.join("not-a-snapshot"), "x").unwrap();
        assert_eq!(resolve_snapshot(&dir), dir.join("rev-a"));
        std::fs::create_dir(dir.join("rev-b")).unwrap();
        assert_eq!(
            resolve_snapshot(&dir),
            dir,
            "ambiguous snapshots require explicit model_dir"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn domains_use_longest_component_prefix() {
        let domains = Domains {
            map: [("/srv/project", "project"), ("/srv/project/deep", "deep")]
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        };
        assert_eq!(
            domains
                .resolve(Path::new("/srv/project/deep/a.md"))
                .as_deref(),
            Some("deep")
        );
        assert_eq!(
            domains.resolve(Path::new("/srv/project/a.md")).as_deref(),
            Some("project")
        );
        assert_eq!(domains.resolve(Path::new("/srv/project-other/a.md")), None);
        let defaults = Domains::default();
        assert!(defaults.map.is_empty());
    }
}
