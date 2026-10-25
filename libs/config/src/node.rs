// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Typed node and broker configuration over strict data. NodeConfigSearch
//! supplies the same file search and read for URL, socket and account readers.
pub use crate::webd::*;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeConfig {
    /// Unowned service sections survive typed read/encode round trips.
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
    pub node: String,
    pub wg_ip: String,
    pub mesh: Option<String>,
    pub noded: NodedConfig,
    pub observe: ObserveConfig,
    pub webd: WebdConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maild: Option<MaildConfig>,
    /// Optional address overrides; absent values derive from the node IP and mail ports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jmap_listen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jmap_upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtp_inbound_listen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtps_listen: Option<String>,
}

impl NodeConfig {
    pub fn noded_listen(&self) -> String {
        let host = if self.wg_ip.is_empty() {
            "127.0.0.1"
        } else {
            &self.wg_ip
        };
        format!("{host}:{}", self.noded.port)
    }
    pub fn noded_url(&self) -> String {
        format!("ws://{}/ws", self.noded_listen())
    }

    /// JMAP listen address, derived from the node IP unless explicitly set.
    pub fn jmap_listen(&self) -> String {
        self.jmap_listen.clone().unwrap_or_else(|| {
            format!(
                "{}:{}",
                self.wg_ip,
                self.maild.as_ref().map_or(8443, |m| m.jmap_port)
            )
        })
    }

    /// JMAP reverse-proxy upstream URL.
    pub fn jmap_upstream(&self) -> String {
        self.jmap_upstream
            .clone()
            .unwrap_or_else(|| format!("https://{}", self.jmap_listen()))
    }

    /// Inbound SMTP listen address.
    pub fn smtp_inbound_listen(&self) -> String {
        self.smtp_inbound_listen.clone().unwrap_or_else(|| {
            format!(
                "{}:{}",
                self.wg_ip,
                self.maild.as_ref().map_or(25, |m| m.smtp_port)
            )
        })
    }

    /// Implicit-TLS SMTP listen address.
    pub fn smtps_listen(&self) -> String {
        self.smtps_listen.clone().unwrap_or_else(|| {
            format!(
                "{}:{}",
                self.wg_ip,
                self.maild.as_ref().map_or(465, |m| m.smtps_port)
            )
        })
    }
}

/// Optional node-wide mail service settings. Missing members use service defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MaildConfig {
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
    pub enabled: bool,
    pub jmap_port: u16,
    pub smtp_port: u16,
    pub smtps_port: u16,
    pub hostname: String,
    pub database: String,
    pub blob_dir: String,
    pub mds_dir: String,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    /// A non-empty identity list takes precedence over the legacy cert/key pair.
    pub tls: TlsConfig,
    pub spam_enabled: bool,
    pub spam_db_dir: Option<String>,
}

impl Default for MaildConfig {
    fn default() -> Self {
        let state = crate::path(crate::Dir::Var).join("maild");
        Self {
            extra: Default::default(),
            enabled: false,
            jmap_port: 8443,
            smtp_port: 25,
            smtps_port: 465,
            hostname: String::new(),
            database: state.join("mail.db").to_string_lossy().into_owned(),
            blob_dir: state.join("blobs").to_string_lossy().into_owned(),
            mds_dir: state.join("mds").to_string_lossy().into_owned(),
            tls_cert: None,
            tls_key: None,
            tls: TlsConfig::default(),
            spam_enabled: true,
            spam_db_dir: None,
        }
    }
}

/// Multi-identity TLS settings, available without the `tls` feature.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
    pub identity: Vec<TlsIdentityConfig>,
    /// Refuse both unknown SNI names and clients which omit SNI.
    pub strict_sni: bool,
}

/// One SNI identity. Name and PEM paths are required; flags default to false.
/// `default` controls mismatches and pre-TLS greetings; `no_sni_fallback`
/// independently controls clients which omit SNI in non-strict mode.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TlsIdentityConfig {
    pub server_name: String,
    pub cert: String,
    pub key: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub no_sni_fallback: bool,
}

// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

/// SPEC 13 §9a D2 broker-admission posture (slice 2-c-1). Per-node operator
/// policy, set in `node.conf.mix` `[noded] admission`. **Default `off`** — a
/// fresh node never starts challenging until told to (clients-before-broker,
/// §16). `observe` challenges + verdicts + logs but NEVER refuses (the §7.8 B3
/// decide-by-doing vehicle); `enforce` refuses a failed gated session (2-c-2,
/// Mark-gated, hub-last).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdmissionMode {
    #[default]
    Off,
    Observe,
    Enforce,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NodedConfig {
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
    pub port: u16,
    /// Absolute broker endpoint override (BUS-013); defaults to this environment's run directory.
    #[serde(deserialize_with = "absolute_unix_socket")]
    pub unix_socket: Option<std::path::PathBuf>,
    /// Operating-system account trusted by local broker clients.
    #[serde(
        deserialize_with = "broker_account_name",
        skip_serializing_if = "Option::is_none"
    )]
    pub account: Option<String>,
    /// BROKER-019 pending grants per Term; may lower the initial cap of 32.
    pub pending_grants_per_parent: usize,
    pub mesh_config: Option<String>,
    /// SPEC 13 §9a D2 admission posture (off | observe | enforce). Default off.
    pub admission: AdmissionMode,
    /// AGENTIC-FIRST posture switch (Mark, 2026-09-14). When `true` (the
    /// default), noded lets ABP flow freely between admitted WG mesh peers:
    /// the per-message principal/native-session-locality guards are relaxed and
    /// the WG /24 + signed inventory (mesh membership) is the trust boundary.
    /// This is the CLAUDE.md "default open, opt-in hard" law — the enforcement
    /// code is retained and re-armed by setting this to `false` per node once
    /// the mesh/app layer is mature. Membership trust (WG + signed inventory)
    /// is UNAFFECTED either way; only per-message authorization relaxes.
    pub mesh_open: bool,
}

fn absolute_unix_socket<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<std::path::PathBuf>, D::Error> {
    let path = Option::<std::path::PathBuf>::deserialize(deserializer)?;
    if path.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(serde::de::Error::custom(
            "noded.unix_socket must be absolute",
        ));
    }
    Ok(path)
}

fn broker_account_name<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let account = Option::<String>::deserialize(deserializer).map_err(|error| {
        <D::Error as serde::de::Error>::custom(format!("noded.account: {error}"))
    })?;
    if account.as_ref().is_some_and(|name| {
        name.is_empty()
            || name
                .chars()
                .any(|c| c.is_whitespace() || c == '\0' || c == ':')
    }) {
        return Err(serde::de::Error::custom(
            "noded.account must be an account name",
        ));
    }
    Ok(account)
}

impl Default for NodedConfig {
    fn default() -> Self {
        Self {
            extra: Default::default(),
            port: 4200,
            unix_socket: None,
            account: None,
            pending_grants_per_parent: 32,
            mesh_config: None,
            admission: AdmissionMode::Off,
            mesh_open: true,
        }
    }
}

impl NodedConfig {
    /// Compatibility alias for the broker's environment-relative endpoint.
    pub fn unix_endpoint(&self) -> std::path::PathBuf {
        self.broker_unix_endpoint()
    }

    /// A broker publishes its resolved endpoint through noded.ping. System
    /// units pin MIXOS_RUN; development installs retain the common path rules.
    pub fn broker_unix_endpoint(&self) -> std::path::PathBuf {
        self.unix_socket
            .clone()
            .unwrap_or_else(|| crate::path(crate::Dir::Run).join("noded/bus.sock"))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ObserveConfig {
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
    /// Anchored service-name globs admitted to `noded.observe.start`.
    /// The broker validates pattern syntax and ignores invalid entries.
    pub allowed_services: Vec<String>,
}
// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

const ENV_VAR: &str = "MIXOS_NODE_CONFIG";

#[cfg(test)]
fn has_override(value: Option<std::ffi::OsString>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// User/layout etc precedes system etc. Explicit MIXOS_ETC or MIXOS isolates
/// the search; root's primary system directory deduplicates naturally.
fn build_search_paths(primary_dir: &Path, mixos_etc_set: bool) -> Vec<PathBuf> {
    let mut dirs = vec![primary_dir.to_path_buf()];
    if !mixos_etc_set {
        let system =
            ::paths::system_default(::paths::Root::Etc).expect("built-in system etc default");
        if !dirs.contains(&system) {
            dirs.push(system);
        }
    }
    dirs.into_iter()
        .map(|dir| dir.join("node.conf.mix"))
        .collect()
}

/// Load node config from the given path (strict-data `.conf.mix`).
///
/// A read or parse failure is a hard error with the path in the chain.
/// C11 removed the legacy `.toml` parse + upgrade-write; a `.toml` path
/// now simply fails to parse as strict-data.
pub fn load_from(path: &Path) -> Result<NodeConfig> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let config: NodeConfig = strict::from_str(&contents)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
    Ok(config)
}

/// Load node config using the standard search order.
///
/// Returns `Ok(Some(config))` if found, `Ok(None)` if no file exists
/// at any search path.
pub fn load_node_config() -> Result<Option<NodeConfig>> {
    NodeConfigSearch::capture().load()
}

/// One read-only search for URL, socket and account consumers. Capturing only
/// environment inputs lets resident clients defer filesystem reads safely.
/// MIXOS_NODE_CONFIG is authoritative even when missing. Otherwise the
/// environment/layout etc directory precedes system etc, with system fallback
/// suppressed by MIXOS_ETC or MIXOS.
pub struct NodeConfigSearch {
    explicit: Option<PathBuf>,
    environment: crate::Environment,
}

impl NodeConfigSearch {
    pub fn capture() -> Self {
        Self::from_environment(
            std::env::var_os(ENV_VAR)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            crate::Environment::current(),
        )
    }

    pub fn from_environment(explicit: Option<PathBuf>, environment: crate::Environment) -> Self {
        Self {
            explicit,
            environment,
        }
    }

    pub fn candidate_paths(&self) -> Result<Vec<PathBuf>> {
        if let Some(path) = &self.explicit {
            return Ok(vec![path.clone()]);
        }
        let dirs = crate::Dirs::from_environment(&self.environment).map_err(anyhow::Error::msg)?;
        self.candidate_paths_with_dirs(&dirs)
    }

    /// Pure search order for an already resolved layout, without filesystem reads.
    pub fn candidate_paths_with_dirs(&self, dirs: &crate::Dirs) -> Result<Vec<PathBuf>> {
        if let Some(path) = &self.explicit {
            return Ok(vec![path.clone()]);
        }
        let isolated = self.environment.etc.is_some() || self.environment.root.is_some();
        // Without a user home, retain URL discovery's system-only default.
        // A layout-provided etc directory still takes precedence.
        if !isolated
            && self.environment.uid != 0
            && self.environment.home.is_none()
            && self
                .environment
                .xdg_config_home
                .as_ref()
                .is_none_or(|dir| !dir.is_absolute())
        {
            let defaults =
                crate::Dirs::from_source(&self.environment, "").map_err(anyhow::Error::msg)?;
            if dirs.get(crate::Dir::Etc) == defaults.get(crate::Dir::Etc) {
                return Ok(build_search_paths(
                    &paths::system_default(paths::Root::Etc).expect("built-in system etc default"),
                    false,
                ));
            }
        }
        Ok(build_search_paths(dirs.get(crate::Dir::Etc), isolated))
    }

    pub fn path(&self) -> Result<Option<PathBuf>> {
        if let Some(path) = &self.explicit {
            return Ok(Some(path.clone()));
        }
        first_config_file(self.candidate_paths()?)
    }

    /// Preserve the source for tolerant URL parsing and typed native settings.
    pub fn read(&self) -> Result<Option<(PathBuf, String)>> {
        self.path()?
            .map(|path| {
                let source = std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                Ok((path, source))
            })
            .transpose()
    }

    pub fn load(&self) -> Result<Option<NodeConfig>> {
        self.read()?
            .map(|(path, source)| {
                strict::from_str(&source)
                    .map_err(|error| anyhow::anyhow!("failed to parse {}: {error}", path.display()))
            })
            .transpose()
    }
}

fn first_config_file(candidates: Vec<PathBuf>) -> Result<Option<PathBuf>> {
    for path in candidates {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => return Ok(Some(path)),
            Ok(_) => continue,
            Err(error) if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR)) => {
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("node config {}", path.display()));
            }
        }
    }
    Ok(None)
}

/// Load node config, returning an error if not found.
pub fn require_node_config() -> Result<NodeConfig> {
    let search = NodeConfigSearch::capture();
    let candidates = search.candidate_paths()?;
    search.load()?.with_context(|| {
        let paths: Vec<_> = candidates.iter().map(|p| p.display().to_string()).collect();
        format!(
            "node config not found — set {ENV_VAR} or place node.conf.mix at: {}",
            paths.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn non_files_in_user_search_do_not_mask_the_next_config() {
        let directory = tempfile::tempdir().unwrap();
        let dangling = directory.path().join("dangling");
        std::os::unix::fs::symlink(directory.path().join("missing"), &dangling).unwrap();
        let regular = directory.path().join("node.conf.mix");
        std::fs::write(&regular, "node: example").unwrap();
        let alias = directory.path().join("alias");
        std::os::unix::fs::symlink(&regular, &alias).unwrap();
        let candidates = vec![
            dangling,
            alias,
            directory.path().to_path_buf(),
            regular.join("child"),
            regular.clone(),
        ];
        assert_eq!(first_config_file(candidates).unwrap(), Some(regular));
    }
    #[test]
    fn broker_account_is_optional_and_validated() {
        let config: NodeConfig = strict::from_str("noded: { account: 'broker-user' }").unwrap();
        assert_eq!(config.noded.account.as_deref(), Some("broker-user"));
        assert!(NodedConfig::default().account.is_none());
        assert!(
            serde_json::to_value(NodedConfig::default())
                .unwrap()
                .get("account")
                .is_none()
        );
        for source in [
            "noded: { account: 7 }",
            "noded: { account: '' }",
            "noded: { account: 'bad name' }",
            "noded: { account: 'bad:name' }",
        ] {
            let error = strict::from_str::<NodeConfig>(source).unwrap_err();
            assert!(error.to_string().contains("account"));
        }
    }

    #[test]
    fn shared_search_reads_one_file_and_keeps_explicit_errors() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let explicit = root.join("explicit.conf.mix");
        let search = NodeConfigSearch::from_environment(
            Some(explicit.clone()),
            crate::Environment::default(),
        );
        assert_eq!(search.path().unwrap(), Some(explicit.clone()));
        assert!(
            search
                .load()
                .unwrap_err()
                .to_string()
                .contains(&explicit.display().to_string())
        );
        std::fs::write(&explicit, "noded: { account: 'broker-user' }").unwrap();
        assert_eq!(
            search.load().unwrap().unwrap().noded.account.as_deref(),
            Some("broker-user")
        );
        let search = NodeConfigSearch::from_environment(
            None,
            crate::Environment {
                etc: Some(root.clone()),
                prefix: Some(root.clone()),
                ..Default::default()
            },
        );
        assert!(search.read().unwrap().is_none());
        std::fs::write(
            root.join("node.conf.mix"),
            "noded: { account: 'isolated-user' }",
        )
        .unwrap();
        assert_eq!(
            search.load().unwrap().unwrap().noded.account.as_deref(),
            Some("isolated-user")
        );
    }
    #[test]
    fn broker_defaults_and_explicit_native_endpoint() {
        let config: NodeConfig = strict::from_str("node: alpha\n").unwrap();
        assert_eq!(config.noded_listen(), "127.0.0.1:4200");
        assert!(config.noded.mesh_open);
        assert_eq!(config.noded.admission, AdmissionMode::Off);
        assert_eq!(config.noded.pending_grants_per_parent, 32);
        let config: NodeConfig = strict::from_str(
            "node: alpha\nnoded: { unix_socket: '/tmp/alpha/bus.sock', admission: enforce }\n",
        )
        .unwrap();
        assert_eq!(
            config.noded.unix_endpoint(),
            PathBuf::from("/tmp/alpha/bus.sock")
        );
        assert_eq!(
            config.noded.broker_unix_endpoint(),
            config.noded.unix_endpoint()
        );
        assert_eq!(config.noded.admission, AdmissionMode::Enforce);
        assert!(strict::from_str::<NodeConfig>("noded: { unix_socket: relative }\n").is_err());
    }
    #[test]
    fn explicit_etc_isolates_and_system_path_deduplicates() {
        assert!(!has_override(None));
        assert!(!has_override(Some("".into())));
        assert!(has_override(Some("/sandbox".into())));
        assert_eq!(
            build_search_paths(
                Path::new("/home/user/.config/mixos"),
                has_override(Some("".into()))
            ),
            vec![
                PathBuf::from("/home/user/.config/mixos/node.conf.mix"),
                PathBuf::from("/etc/mixos/node.conf.mix")
            ]
        );
        assert_eq!(
            build_search_paths(Path::new("/sandbox"), true),
            vec![PathBuf::from("/sandbox/node.conf.mix")]
        );
        assert_eq!(
            build_search_paths(Path::new("/etc/mixos"), false),
            vec![PathBuf::from("/etc/mixos/node.conf.mix")]
        );
    }
    #[test]
    fn unrelated_service_fields_are_forward_compatible() {
        let config: NodeConfig = strict::from_str("node: beta\nwg_ip: '192.0.2.2'\nwebd: { port: 443 }\nobserve: { allowed_services: [tower] }\n").unwrap();
        assert_eq!(config.noded_url(), "ws://192.0.2.2:4200/ws");
        assert_eq!(config.observe.allowed_services, vec!["tower"]);
    }

    #[test]
    fn typed_node_round_trip_preserves_unowned_sections() {
        let source = "node: alpha\nwebd: { port: 443, tls: true }\nnoded: { future_option: [a, b] }\nobserve: { future_policy: { enabled: true } }\n";
        let config: NodeConfig = strict::from_str(source).unwrap();
        let encoded = strict::to_string_pretty(&config).unwrap();
        let decoded: NodeConfig = strict::from_str(&encoded).unwrap();
        assert_eq!(decoded.extra, config.extra);
        assert_eq!(decoded.webd.extra, config.webd.extra);
        assert_eq!(decoded.webd.extra["tls"], serde_json::json!(true));
        assert_eq!(decoded.noded.extra, config.noded.extra);
        assert_eq!(decoded.observe.extra, config.observe.extra);
        assert_eq!(decoded.webd.port, 443);
    }
}

impl NodeConfig {
    /// Web server listen address: `{wg_ip}:{webd.port}`.
    pub fn web_listen(&self) -> String {
        format!("{}:{}", self.wg_ip, self.webd.port)
    }

    /// Resolve the webd listener set, validating per-interface listeners
    /// against the full vhost host-set.
    ///
    /// `all_hosts` is every vhost FQDN webd intends to serve — the union
    /// of `[[webd.vhost]]` primaries + aliases and the legacy
    /// `tls_server_name` entries (caller-deduplicated, host-parsed).
    ///
    /// **Empty `[webd] listener` ⇒ back-compat:** one implicit `wg`
    /// listener at `web_listen()` (`wg_ip:port`), internal, serving every
    /// host — byte-identical to the pre-listener single bind.
    ///
    /// **Non-empty ⇒** the array is used and validated:
    /// - non-empty unique ids; non-empty unique binds (each a parseable
    ///   `ip:port`);
    /// - no wildcard (`0.0.0.0` / `::`) sharing a port with another bind
    ///   (would `EADDRINUSE` or silently shadow);
    /// - every host named in a listener's `vhosts` is a known host
    ///   (in `all_hosts`);
    /// - every host in `all_hosts` is claimed by at least one **enabled**
    ///   listener (a host on no listener or only disabled listeners is a
    ///   hard error); a host may occur on several listeners, but only
    ///   once within each listener's allowlist.
    ///
    /// **`disabled_hosts` (B1 fail-soft)** is every vhost name webd
    /// dropped at resolve time (bad cert / missing www_dir / unopenable
    /// CMS DB). Such a host is already absent from `all_hosts` (it left
    /// no routing surface), but it may still be *named* in a listener's
    /// `vhosts` allowlist in the config. A name in `disabled_hosts` is
    /// **skipped** from the allowlist rather than rejected as an unknown
    /// vhost — so one bad cert disables that vhost without failing the
    /// whole listener set. A name that is in neither `all_hosts` nor
    /// `disabled_hosts` is still a hard "unknown vhost" error (catches
    /// config typos). Pass an empty set for the strict pre-B1 behaviour.
    pub fn synthesize_listeners(
        &self,
        all_hosts: &[String],
        disabled_hosts: &std::collections::HashSet<String>,
    ) -> Result<Vec<ResolvedWebdListener>> {
        use std::collections::{HashMap, HashSet};
        use std::net::SocketAddr;

        let listeners = &self.webd.listener;
        if listeners.is_empty() {
            return Ok(vec![ResolvedWebdListener {
                id: "wg".to_string(),
                bind: self.web_listen(),
                external: false,
                enabled: true,
                hosts: all_hosts.to_vec(),
            }]);
        }

        // ── ids + binds ──────────────────────────────────────────────
        // Dedup binds on the *parsed* SocketAddr, not the raw string, so
        // text variants of one address (`[::]:443` vs
        // `[0:0:0:0:0:0:0:0]:443`) can't slip two listeners onto the same
        // socket and have the runtime silently bring up one and drop the
        // other. Per port we track whether any bind is a wildcard
        // (`0.0.0.0` / `::`) and the total count: a wildcard listener
        // must be the *only* bind on its port. This is the conservative
        // family-agnostic policy — `0.0.0.0:P` and `[::]:P` would
        // `EADDRINUSE` on a dual-stack default, and even a non-overlapping
        // v4-wildcard + v6-specific pair is rejected rather than reasoned
        // about. Wildcards are discouraged here anyway (the substrate
        // posture is bind a *specific* interface — `feedback_wg_only_binding`).
        let mut seen_ids: HashSet<&str> = HashSet::new();
        let mut seen_binds: HashSet<SocketAddr> = HashSet::new();
        let mut port_binds: HashMap<u16, (bool, usize)> = HashMap::new();
        for l in listeners {
            if l.id.trim().is_empty() {
                anyhow::bail!("[webd] listener with empty id");
            }
            if !seen_ids.insert(l.id.as_str()) {
                anyhow::bail!("[webd] duplicate listener id {:?}", l.id);
            }
            if l.bind.trim().is_empty() {
                anyhow::bail!("[webd] listener {:?} has empty bind", l.id);
            }
            let addr: SocketAddr = l.bind.parse().with_context(|| {
                format!(
                    "[webd] listener {:?}: bind {:?} is not a valid ip:port",
                    l.id, l.bind
                )
            })?;
            if !seen_binds.insert(addr) {
                anyhow::bail!(
                    "[webd] duplicate listener bind {:?} (resolves to {addr})",
                    l.bind
                );
            }
            let entry = port_binds.entry(addr.port()).or_insert((false, 0));
            entry.0 |= addr.ip().is_unspecified();
            entry.1 += 1;
        }
        for (port, (has_wildcard, count)) in &port_binds {
            if *has_wildcard && *count > 1 {
                anyhow::bail!(
                    "[webd] port {port} has a wildcard (0.0.0.0 / ::) bind plus another bind \
                     — a wildcard listener must be the only bind on its port"
                );
            }
        }

        // ── host coverage + uniqueness within each listener ──────────
        let known: HashSet<&str> = all_hosts.iter().map(String::as_str).collect();
        let mut owner: HashMap<&str, &WebdListenerConfig> = HashMap::new();
        for l in listeners {
            let mut seen = HashSet::new();
            for h in &l.vhosts {
                if !seen.insert(h.as_str()) {
                    anyhow::bail!(
                        "[webd] vhost {:?} is listed twice in listener {:?}'s `vhosts`",
                        h,
                        l.id
                    );
                }
                // B1 fail-soft: a host webd disabled at resolve time
                // (bad cert / missing www_dir / unopenable CMS DB) is
                // absent from `all_hosts` but may still be named here.
                // Skip it — it owns no listener and serves nothing —
                // rather than rejecting the whole set as "unknown vhost".
                if disabled_hosts.contains(h.as_str()) {
                    continue;
                }
                if !known.contains(h.as_str()) {
                    anyhow::bail!(
                        "[webd] listener {:?} references unknown vhost {:?} \
                         (not a configured webd.vhost host/alias or tls_server_name)",
                        l.id,
                        h
                    );
                }
                // Prefer an enabled claim, regardless of declaration order.
                if l.enabled || !owner.contains_key(h.as_str()) {
                    owner.insert(h.as_str(), l);
                }
            }
        }
        // Every served host must be claimed, by an *enabled* listener.
        for h in all_hosts {
            match owner.get(h.as_str()) {
                None => anyhow::bail!(
                    "[webd] vhost {:?} is not served by any listener \
                     — add it to a listener's `vhosts` allowlist",
                    h
                ),
                Some(l) if !l.enabled => anyhow::bail!(
                    "[webd] vhost {:?} is served only by the disabled listener {:?} \
                     — it would never be reachable (move it to an enabled listener)",
                    h,
                    l.id
                ),
                Some(_) => {}
            }
        }

        Ok(listeners
            .iter()
            .map(|l| ResolvedWebdListener {
                id: l.id.clone(),
                bind: l.bind.clone(),
                external: l.external,
                enabled: l.enabled,
                hosts: l.vhosts.clone(),
            })
            .collect())
    }
}

#[cfg(test)]
mod transplanted_tests {
    #[test]
    fn native_endpoint_uses_the_broker_runtime() {
        assert!(
            serde_json::from_str::<super::NodedConfig>(r#"{"unix_socket":"relative.sock"}"#)
                .is_err()
        );
        let config: super::NodedConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(
            config.broker_unix_endpoint(),
            crate::path(crate::Dir::Run).join("noded/bus.sock")
        );
        assert_eq!(config.unix_endpoint(), config.broker_unix_endpoint());
        let config: super::NodedConfig =
            serde_json::from_str(r#"{"unix_socket":"/run/example/bus.sock"}"#).unwrap();
        assert_eq!(
            config.unix_endpoint(),
            std::path::PathBuf::from("/run/example/bus.sock")
        );
        assert_eq!(config.broker_unix_endpoint(), config.unix_endpoint());
        assert_eq!(
            serde_json::to_value(config).unwrap()["unix_socket"],
            "/run/example/bus.sock"
        );
    }
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn shared_assets_publication_is_explicitly_opt_in() {
        let defaults: WebdConfig = crate::from_conf_mix_str("{}").unwrap();
        assert!(defaults.shared_assets_dir.is_none());
        assert!(!defaults.shared_assets_cross_origin);
        let configured: WebdConfig = crate::from_conf_mix_str(
            r#"{ shared_assets_dir: "/srv/mixos-assets", shared_assets_cross_origin: true }"#,
        )
        .unwrap();
        assert_eq!(
            configured.shared_assets_dir.as_deref(),
            Some("/srv/mixos-assets")
        );
        assert!(configured.shared_assets_cross_origin);
    }
    #[test]
    fn webd_vhost_tolerates_unknown_fields_core_structs_still_strict() {
        // Forward-compat (drift fix): `WebdVhostConfig` no longer denies unknown
        // fields, so a node.conf.mix carrying a newer webd-vhost field (e.g.
        // `public_read_email`, added on another branch) still parses — any daemon
        // built from an older branch can still resolve the broker URL from the
        // shared config instead of failing the whole parse and dropping to loopback.
        let v: WebdVhostConfig = crate::from_conf_mix_str(
            r#"{ "host": "x.example.org", "www_dir": "/srv/www", "public_read_email_future": "a@b.org" }"#,
        )
        .expect("an unknown webd-vhost field must not break parsing");
        assert_eq!(v.host, "x.example.org");

        // A core struct that keeps `deny_unknown_fields` (it has `default`, so the
        // failure is the unknown field, not a missing one) still rejects typos.
        let core: Result<WebdListenerConfig, _> =
            crate::from_conf_mix_str(r#"{ "id": "x", "bogus_unknown_field": true }"#);
        assert!(
            core.is_err(),
            "a strict core struct must still reject unknown fields"
        );
    }

    #[test]
    fn noded_url_empty_wg_ip_falls_back_to_loopback() {
        // A standalone / non-mesh node omits wg_ip; the broker URL must resolve
        // to loopback, not a hostless ws://:PORT/ws no client can reach (mix#1).
        let mut cfg = NodeConfig {
            wg_ip: String::new(),
            ..Default::default()
        };
        assert_eq!(cfg.noded_url(), "ws://127.0.0.1:4200/ws");
        cfg.wg_ip = "192.0.2.9".into();
        assert_eq!(cfg.noded_url(), "ws://192.0.2.9:4200/ws");
    }

    // Tests exercise the pure `build_search_paths()` helper with
    // explicit inputs, so they do not depend on (and do not mutate)
    // the inherited `MIXOS_ETC` env var or the `OnceLock`-cached
    // path resolver in `paths.rs`. This keeps them hermetic regardless
    // of the test harness's environment.

    #[test]
    fn build_search_paths_user_mode_no_env_appends_system_fallback() {
        let dir = PathBuf::from("/home/alice/.config/mixos");
        let paths = build_search_paths(&dir, false);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/alice/.config/mixos/node.conf.mix"),
                PathBuf::from("/etc/mixos/node.conf.mix"),
            ],
            "user-mode (no MIXOS_ETC) must search user dir then /etc/mixos"
        );
    }

    #[test]
    fn build_search_paths_mixos_etc_set_isolates() {
        // The MIXOS_ETC isolation case — the original Medium finding.
        // When the caller has explicitly overridden the etc dir, we
        // do NOT silently fall through to the host's <system etc>.
        let dir = PathBuf::from("/sandbox/etc/mixos");
        let paths = build_search_paths(&dir, true);
        assert_eq!(
            paths,
            vec![PathBuf::from("/sandbox/etc/mixos/node.conf.mix")],
            "MIXOS_ETC set must suppress /etc/mixos fallback"
        );
    }

    #[test]
    fn build_search_paths_root_mode_dedupes() {
        let dir = ::paths::system_default(::paths::Root::Etc).expect("built-in system etc default");
        let paths = build_search_paths(&dir, false);
        assert_eq!(
            paths,
            vec![PathBuf::from("/etc/mixos/node.conf.mix")],
            "root mode (primary dir already == system) must not duplicate"
        );
    }

    // The maild-TLS config-shape tests now exercise the `.conf.mix`
    // strict-data path (the real load format) via the serde bridge.
    // `.conf.mix` map literals separate entries with `,` (newlines are
    // suppressed inside `{ }`); the top-level body uses newlines.

    #[test]
    fn maild_tls_identities_parse_multi() {
        let src = r#"
            node: "alpha"
            wg_ip: "192.0.2.5"
            maild: {
              hostname: "alpha.example.org",
              tls: {
                identity: [
                  { server_name: "alpha.example.org", cert: "/etc/mixos/maild/tls/lan.crt", key: "/etc/mixos/maild/tls/lan.key", default: true, no_sni_fallback: true },
                  { server_name: "alpha.bus.example.org", cert: "/etc/mixos/maild/tls/wg.crt", key: "/etc/mixos/maild/tls/wg.key" }
                ]
              }
            }
        "#;
        let nc: NodeConfig = crate::from_conf_mix_str(src).expect("parse");
        assert_eq!(nc.maild.as_ref().unwrap().tls.identity.len(), 2);
        assert_eq!(
            nc.maild.as_ref().unwrap().tls.identity[0].server_name,
            "alpha.example.org"
        );
        assert!(nc.maild.as_ref().unwrap().tls.identity[0].default);
        assert!(nc.maild.as_ref().unwrap().tls.identity[0].no_sni_fallback);
        assert_eq!(
            nc.maild.as_ref().unwrap().tls.identity[1].server_name,
            "alpha.bus.example.org"
        );
        assert!(!nc.maild.as_ref().unwrap().tls.identity[1].default);
        assert!(!nc.maild.as_ref().unwrap().tls.identity[1].no_sni_fallback);
        assert!(!nc.maild.as_ref().unwrap().tls.strict_sni);
    }

    #[test]
    fn maild_tls_strict_sni_toggle() {
        let src = "maild: { tls: { strict_sni: true } }\n";
        let nc: NodeConfig = crate::from_conf_mix_str(src).expect("parse");
        assert!(nc.maild.as_ref().unwrap().tls.strict_sni);
        assert!(nc.maild.as_ref().unwrap().tls.identity.is_empty());
    }

    #[test]
    fn maild_tls_legacy_only_leaves_identity_empty() {
        let src = "maild: { tls_cert: \"/etc/mixos/maild/tls/legacy.crt\", \
                   tls_key: \"/etc/mixos/maild/tls/legacy.key\" }\n";
        let nc: NodeConfig = crate::from_conf_mix_str(src).expect("parse");
        assert!(nc.maild.as_ref().unwrap().tls.identity.is_empty());
        assert_eq!(
            nc.maild.as_ref().unwrap().tls_cert.as_deref(),
            Some("/etc/mixos/maild/tls/legacy.crt"),
            "legacy pair must still parse unchanged"
        );
    }

    #[test]
    fn noded_admission_parses_lowercase_and_defaults_off() {
        // Absent → off (a fresh node never starts challenging, §16).
        let nc: NodeConfig = crate::from_conf_mix_str("node: \"alpha\"\n").expect("parse");
        assert_eq!(nc.noded.admission, AdmissionMode::Off);
        // Explicit lowercase values round-trip.
        for (s, want) in [
            ("off", AdmissionMode::Off),
            ("observe", AdmissionMode::Observe),
            ("enforce", AdmissionMode::Enforce),
        ] {
            let src = format!("noded: {{ admission: \"{s}\" }}\n");
            let nc: NodeConfig = crate::from_conf_mix_str(&src).expect("parse");
            assert_eq!(nc.noded.admission, want, "admission={s}");
        }
    }

    #[test]
    fn observe_allowlist_parses_and_defaults_fail_closed() {
        let empty: NodeConfig =
            crate::from_conf_mix_str("node: \"alpha\"\n").expect("parse defaults");
        assert!(empty.observe.allowed_services.is_empty());

        let configured: NodeConfig = crate::from_conf_mix_str(
            "observe: { allowed_services: [\"tower-bevy-*\", \"log-observe-*\"] }\n",
        )
        .expect("parse observe allowlist");
        assert_eq!(
            configured.observe.allowed_services,
            ["tower-bevy-*", "log-observe-*"]
        );
    }

    #[test]
    fn maild_tls_identity_row_missing_required_field_fails_parse() {
        // An operator writing a tls identity row but forgetting `cert`
        // (or `key`, or `server_name`) must hit a hard parse error at
        // startup, not silently inherit empty strings that would later
        // shadow the legacy `tls_cert` / `tls_key` pair with an
        // unservable identity. `TlsIdentityConfig` has no
        // `#[serde(default)]`, so the three string fields are required.
        let cases = [
            // missing cert
            "maild: { tls_cert: \"/etc/legacy.crt\", tls_key: \"/etc/legacy.key\", \
             tls: { identity: [ { server_name: \"host.example\", key: \"/etc/host.key\" } ] } }\n",
            // missing key
            "maild: { tls: { identity: [ { server_name: \"host.example\", cert: \"/etc/host.crt\" } ] } }\n",
            // missing server_name
            "maild: { tls: { identity: [ { cert: \"/etc/host.crt\", key: \"/etc/host.key\" } ] } }\n",
        ];
        for (i, src) in cases.iter().enumerate() {
            let res: Result<NodeConfig, _> = crate::from_conf_mix_str(src);
            assert!(
                res.is_err(),
                "case {i}: malformed identity row must fail parse, got Ok"
            );
        }
    }

    #[test]
    fn load_from_reads_conf_mix() {
        // C11: `.conf.mix` is the only format; load_from reads it
        // directly with no upgrade-write.
        let dir = std::env::temp_dir().join(format!("mixos-node-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conf_mix_path = dir.join("node.conf.mix");
        std::fs::write(&conf_mix_path, "node: \"alpha\"\nwg_ip: \"192.0.2.5\"\n").unwrap();

        let cfg = load_from(&conf_mix_path).expect("conf.mix load");
        assert_eq!(cfg.node, "alpha");
        assert_eq!(cfg.wg_ip, "192.0.2.5");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_toml_is_hard_error() {
        // A `.toml` path now fails to parse as strict-data (no fallback).
        let dir = std::env::temp_dir().join(format!("mixos-node-toml-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let toml_path = dir.join("node.toml");
        std::fs::write(&toml_path, "node = \"alpha\"\nwg_ip = \"192.0.2.5\"\n").unwrap();
        assert!(load_from(&toml_path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn webd_acme_provider_wire_form_matches_docs() {
        // The operator-facing form is `letsencrypt_prod` /
        // `letsencrypt_staging` (no underscore between *lets* and
        // *encrypt*) per every reference in
        // `_doc/planned/webd-vhosts*.md`, every in-source comment in
        // `mixos-lib-daemon::tls`, and the on-disk meta-file names
        // (`account.letsencrypt_prod.meta.json`) the Phase 2 plan
        // reserves. Pinned here so a future refactor that swaps
        // the per-variant `#[serde(rename = ...)]` back to a
        // `rename_all = "snake_case"` (which would derive
        // `lets_encrypt_*`) regresses loudly in CI rather than
        // silently breaking every operator config that uses the
        // documented spelling.
        let prod = serde_json::to_string(&WebdAcmeProvider::LetsEncryptProd).expect("serialise");
        let stg = serde_json::to_string(&WebdAcmeProvider::LetsEncryptStaging).expect("serialise");
        assert_eq!(prod, "\"letsencrypt_prod\"");
        assert_eq!(stg, "\"letsencrypt_staging\"");

        let parsed_prod: WebdAcmeProvider =
            serde_json::from_str("\"letsencrypt_prod\"").expect("deserialise prod");
        let parsed_stg: WebdAcmeProvider =
            serde_json::from_str("\"letsencrypt_staging\"").expect("deserialise staging");
        assert_eq!(parsed_prod, WebdAcmeProvider::LetsEncryptProd);
        assert_eq!(parsed_stg, WebdAcmeProvider::LetsEncryptStaging);

        // Negative: the `rename_all = "snake_case"` derivation
        // (`lets_encrypt_*`) must NOT parse. Catches the exact
        // refactor we're guarding against — a maintainer dropping
        // the per-variant renames thinking they're redundant.
        let bad_prod = serde_json::from_str::<WebdAcmeProvider>("\"lets_encrypt_prod\"");
        let bad_stg = serde_json::from_str::<WebdAcmeProvider>("\"lets_encrypt_staging\"");
        assert!(bad_prod.is_err(), "lets_encrypt_prod must not parse");
        assert!(bad_stg.is_err(), "lets_encrypt_staging must not parse");
    }

    // ── synthesize_listeners ──────────────────────────────────────────

    fn node_with_listeners(src: &str) -> NodeConfig {
        crate::from_conf_mix_str(src).expect("parse listener config")
    }

    #[test]
    fn synthesize_listeners_empty_is_backcompat_single_wg() {
        // No `[webd] listener` array ⇒ one implicit internal `wg`
        // listener at wg_ip:port serving every host (byte-identical to
        // the pre-listener single bind).
        let nc =
            node_with_listeners("node: \"alpha\"\nwg_ip: \"192.0.2.5\"\nwebd: { port: 443 }\n");
        let hosts = vec!["a.example.org".to_string(), "b.example.org".to_string()];
        let out = nc
            .synthesize_listeners(&hosts, &HashSet::new())
            .expect("synthesize");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "wg");
        assert_eq!(out[0].bind, "192.0.2.5:443");
        assert!(!out[0].external);
        assert!(out[0].enabled);
        assert_eq!(out[0].hosts, hosts);
    }

    #[test]
    fn synthesize_listeners_enabled_defaults_true_when_omitted() {
        // A row omitting `enabled` must be on (serde(default) → true).
        let nc = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"wg\", bind: \"192.0.2.5:443\", vhosts: [\"a.example.org\"] } ] }\n",
        );
        let hosts = vec!["a.example.org".to_string()];
        let out = nc
            .synthesize_listeners(&hosts, &HashSet::new())
            .expect("synthesize");
        assert_eq!(out.len(), 1);
        assert!(out[0].enabled, "omitted enabled must default true");
    }

    #[test]
    fn synthesize_listeners_two_interface_split_ok() {
        let nc = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"wg\", bind: \"192.0.2.5:443\", vhosts: [\"int.example.org\"] }, \
             { id: \"pub\", bind: \"203.0.113.5:443\", external: true, vhosts: [\"ext.example.org\"] } ] }\n",
        );
        let hosts = vec!["int.example.org".to_string(), "ext.example.org".to_string()];
        let out = nc
            .synthesize_listeners(&hosts, &HashSet::new())
            .expect("synthesize");
        assert_eq!(out.len(), 2);
        let publ = out.iter().find(|l| l.id == "pub").unwrap();
        assert!(publ.external);
        assert_eq!(publ.bind, "203.0.113.5:443");
        assert_eq!(publ.hosts, vec!["ext.example.org".to_string()]);
    }

    #[test]
    fn synthesize_listeners_rejects_dup_id_and_bind() {
        let dup_id = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"x\", bind: \"192.0.2.5:443\", vhosts: [\"a.example.org\"] }, \
             { id: \"x\", bind: \"203.0.113.5:443\", vhosts: [\"b.example.org\"] } ] }\n",
        );
        assert!(
            dup_id
                .synthesize_listeners(
                    &["a.example.org".into(), "b.example.org".into()],
                    &HashSet::new()
                )
                .is_err()
        );

        let dup_bind = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"x\", bind: \"192.0.2.5:443\", vhosts: [\"a.example.org\"] }, \
             { id: \"y\", bind: \"192.0.2.5:443\", vhosts: [\"b.example.org\"] } ] }\n",
        );
        assert!(
            dup_bind
                .synthesize_listeners(
                    &["a.example.org".into(), "b.example.org".into()],
                    &HashSet::new()
                )
                .is_err()
        );
    }

    #[test]
    fn synthesize_listeners_rejects_wildcard_specific_same_port() {
        let nc = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"wg\", bind: \"192.0.2.5:443\", vhosts: [\"a.example.org\"] }, \
             { id: \"any\", bind: \"0.0.0.0:443\", external: true, vhosts: [\"b.example.org\"] } ] }\n",
        );
        let err = nc
            .synthesize_listeners(
                &["a.example.org".into(), "b.example.org".into()],
                &HashSet::new(),
            )
            .unwrap_err();
        assert!(format!("{err}").contains("wildcard"), "got: {err}");

        // Two wildcards on the same port (v4 + v6) also conflict — on a
        // dual-stack default `[::]:443` already covers v4, so the pair
        // would EADDRINUSE. A wildcard must be the only bind on its port.
        let dual_wild = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"v4\", bind: \"0.0.0.0:443\", external: true, vhosts: [\"a.example.org\"] }, \
             { id: \"v6\", bind: \"[::]:443\", external: true, vhosts: [\"b.example.org\"] } ] }\n",
        );
        assert!(
            dual_wild
                .synthesize_listeners(
                    &["a.example.org".into(), "b.example.org".into()],
                    &HashSet::new()
                )
                .is_err()
        );

        // Two specific IPs on the same port are fine — distinct
        // interfaces, no kernel conflict (the WG + public split).
        let two_specific = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"wg\", bind: \"192.0.2.5:443\", vhosts: [\"a.example.org\"] }, \
             { id: \"pub\", bind: \"203.0.113.5:443\", external: true, vhosts: [\"b.example.org\"] } ] }\n",
        );
        assert!(
            two_specific
                .synthesize_listeners(
                    &["a.example.org".into(), "b.example.org".into()],
                    &HashSet::new()
                )
                .is_ok()
        );
    }

    #[test]
    fn synthesize_listeners_dedups_bind_on_parsed_addr() {
        // Text variants of one IPv6 address resolve to the same socket;
        // must be caught as a duplicate, not slip two listeners onto it.
        let nc = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"a\", bind: \"[::1]:443\", vhosts: [\"a.example.org\"] }, \
             { id: \"b\", bind: \"[0:0:0:0:0:0:0:1]:443\", vhosts: [\"b.example.org\"] } ] }\n",
        );
        let err = nc
            .synthesize_listeners(
                &["a.example.org".into(), "b.example.org".into()],
                &HashSet::new(),
            )
            .unwrap_err();
        assert!(
            format!("{err}").contains("duplicate listener bind"),
            "got: {err}"
        );
    }

    #[test]
    fn synthesize_listeners_rejects_unknown_and_uncovered_hosts() {
        // A listener naming a host that is not a known vhost.
        let unknown = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"wg\", bind: \"192.0.2.5:443\", vhosts: [\"ghost.example.org\"] } ] }\n",
        );
        assert!(
            unknown
                .synthesize_listeners(&["a.example.org".into()], &HashSet::new())
                .is_err()
        );

        // A served host claimed by no listener (silent-drop guard).
        let uncovered = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"wg\", bind: \"192.0.2.5:443\", vhosts: [\"a.example.org\"] } ] }\n",
        );
        assert!(
            uncovered
                .synthesize_listeners(
                    &["a.example.org".into(), "b.example.org".into()],
                    &HashSet::new()
                )
                .is_err()
        );
    }

    #[test]
    fn synthesize_listeners_rejects_disabled_only_even_with_double_claim() {
        // Host served only by a disabled listener ⇒ unreachable ⇒ error.
        let disabled_only = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"wg\", bind: \"192.0.2.5:443\", enabled: false, vhosts: [\"a.example.org\"] } ] }\n",
        );
        assert!(
            disabled_only
                .synthesize_listeners(&["a.example.org".into()], &HashSet::new())
                .is_err()
        );

        // Two claims still fail when neither listener is enabled.
        let double = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"wg\", bind: \"192.0.2.5:443\", enabled: false, vhosts: [\"a.example.org\"] }, \
             { id: \"pub\", bind: \"203.0.113.5:443\", enabled: false, external: true, vhosts: [\"a.example.org\"] } ] }\n",
        );
        assert!(
            double
                .synthesize_listeners(&["a.example.org".into()], &HashSet::new())
                .is_err()
        );
    }

    #[test]
    fn synthesize_listeners_shared_host_keeps_binds_and_enabled_coverage() {
        for enabled in [(true, true), (true, false), (false, true)] {
            let nc = node_with_listeners(&format!(
                "webd: {{ listener: [ \
                 {{ id: first, bind: '192.0.2.1:443', enabled: {}, vhosts: ['a.example.org'] }}, \
                 {{ id: second, bind: '198.51.100.1:443', enabled: {}, vhosts: ['a.example.org'] }} ] }}",
                enabled.0, enabled.1
            ));
            let hosts = vec!["a.example.org".to_string()];
            let out = nc.synthesize_listeners(&hosts, &HashSet::new()).unwrap();
            assert_eq!(out.len(), 2);
            assert_eq!(out[0].bind, "192.0.2.1:443");
            assert_eq!(out[1].bind, "198.51.100.1:443");
            assert_eq!(out[0].hosts, hosts);
            assert_eq!(out[1].hosts, hosts);
            assert_eq!((out[0].enabled, out[1].enabled), enabled);
        }
    }

    #[test]
    fn synthesize_listeners_rejects_duplicate_even_when_host_is_disabled() {
        let nc = node_with_listeners(
            "webd: { listener: [ \
             { id: first, bind: '192.0.2.1:443', vhosts: ['a.example.org', 'a.example.org'] } ] }",
        );
        for disabled in [HashSet::new(), HashSet::from(["a.example.org".to_string()])] {
            let err = nc
                .synthesize_listeners(&["a.example.org".into()], &disabled)
                .unwrap_err();
            assert!(err.to_string().contains("listed twice"), "{err}");
        }
    }

    #[test]
    fn synthesize_listeners_failsoft_skips_disabled_host() {
        // B1 fail-soft: `ext.example.org` had a bad cert at resolve, so
        // it is absent from `all_hosts` but still named in the `pub`
        // listener's allowlist. With it in `disabled_hosts` the set
        // resolves — `pub` simply serves nothing — instead of failing
        // the whole node with "unknown vhost".
        let nc = node_with_listeners(
            "wg_ip: \"192.0.2.5\"\nwebd: { listener: [ \
             { id: \"wg\", bind: \"192.0.2.5:443\", vhosts: [\"int.example.org\"] }, \
             { id: \"pub\", bind: \"203.0.113.5:443\", external: true, vhosts: [\"ext.example.org\"] } ] }\n",
        );
        // Healthy host set after fail-soft drop: only the WG vhost.
        let healthy = vec!["int.example.org".to_string()];
        let mut disabled = HashSet::new();
        disabled.insert("ext.example.org".to_string());

        let out = nc
            .synthesize_listeners(&healthy, &disabled)
            .expect("disabled host must be skipped, not rejected");
        // Both listeners still resolve (the public one binds but serves
        // no vhost until the cert is repaired).
        assert_eq!(out.len(), 2);
        let publ = out.iter().find(|l| l.id == "pub").unwrap();
        assert!(publ.external);

        // Without the disabled set, the same config is a hard error —
        // the dangling allowlist entry is treated as a config typo.
        assert!(
            nc.synthesize_listeners(&healthy, &HashSet::new()).is_err(),
            "an un-disabled unknown vhost must still be rejected"
        );
    }

    #[test]
    fn synthesize_listeners_rejects_unknown_field() {
        // deny_unknown_fields closes the typo-silently-parses trap.
        let res: Result<NodeConfig, _> = crate::from_conf_mix_str(
            "webd: { listener: [ { id: \"wg\", bind: \"192.0.2.5:443\", \
             vhost: [\"a.example.org\"] } ] }\n",
        );
        assert!(
            res.is_err(),
            "misspelled `vhost` (should be `vhosts`) must fail parse"
        );
    }
}
