// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Shared broker discovery for native ABP clients. Explicit socket, node
//! config, environment runtime and system runtime precede TCP ping discovery.
//! Account configuration is independent of socket ownership.

use crate::native_client::ConnectError;
use crate::native_client::{BrokerAccount, NodedClient, UnixConnectOptions};
use anyhow::Context;
use std::path::{Path, PathBuf};

pub(crate) fn at_endpoint(path: &Path, error: ConnectError) -> ConnectError {
    ConnectError::Endpoint {
        path: path.to_owned(),
        error: Box::new(error),
    }
}

/// Probe every component without following symlinks. Only a kernel ENOENT or
/// ENOTDIR permits skipping; verification and connection failures never do.
fn missing_component(path: &Path) -> Result<Option<std::io::Error>, ConnectError> {
    if !path.is_absolute()
        || path
            .as_os_str()
            .as_encoded_bytes()
            .split(|b| *b == b'/')
            .any(|part| part == b"." || part == b"..")
    {
        return Err(ConnectError::InvalidEndpoint);
    }
    let mut component_path = PathBuf::new();
    for component in path.components() {
        component_path.push(component);
        match std::fs::symlink_metadata(&component_path) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(ConnectError::InvalidEndpoint),
            Ok(_) => {}
            Err(error) if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR)) => {
                return Ok(Some(error));
            }
            Err(error) => return Err(ConnectError::Io(error)),
        }
    }
    Ok(None)
}

fn select_endpoint(
    candidates: &[PathBuf],
    pinned: bool,
    mut warn: impl FnMut(&Path, &std::io::Error),
) -> Result<Option<PathBuf>, ConnectError> {
    for path in candidates {
        match missing_component(path).map_err(|error| at_endpoint(path, error))? {
            Some(error) if !pinned => warn(path, &error),
            Some(error) => return Err(at_endpoint(path, ConnectError::Io(error))),
            None => return Ok(Some(path.clone())),
        }
    }
    Ok(None)
}

pub(crate) async fn resolve_endpoint<F, Fut>(
    mut candidates: Vec<PathBuf>,
    expected_server_pid: Option<u32>,
    discover: F,
) -> Result<PathBuf, ConnectError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Option<PathBuf>>,
{
    let warn = |path: &Path, error: &std::io::Error| {
        tracing::warn!(path = %path.display(), %error, "skipping missing Unix endpoint candidate");
    };
    if let Some(path) = select_endpoint(&candidates, expected_server_pid.is_some(), warn)? {
        return Ok(path);
    }
    if expected_server_pid.is_none()
        && let Some(path) = discover().await
    {
        candidates.push(path.clone());
        if let Some(path) = select_endpoint(&[path], false, warn)? {
            return Ok(path);
        }
    }
    Err(ConnectError::CandidatesMissing(candidates))
}

pub fn resolve_noded_url() -> String {
    crate::noded_url()
}

/// Configuration errors propagate for authenticated ingress. Missing socket
/// candidates may fall through; existing candidates must pass verification.
pub fn unix_connect_options(account: BrokerAccount) -> anyhow::Result<UnixConnectOptions> {
    let mut configuration = BrokerEnvironment::capture().configuration()?;
    configuration.options.broker_account = account;
    Ok(configuration.options)
}

/// Resolve the trusted account: environment, node.conf `noded.account`, noded.
/// NSS errors name both the account and the configuration source.
pub fn resolve_broker_account() -> anyhow::Result<BrokerAccount> {
    let configuration = BrokerEnvironment::capture().configuration()?;
    configuration.account_selection().lookup()
}

struct AccountSelection<'a> {
    name: &'a str,
    source: &'static str,
}

fn account_selection<'a>(
    environment: Option<&'a str>,
    configured: Option<&'a str>,
) -> AccountSelection<'a> {
    match (environment, configured) {
        (Some(name), _) => AccountSelection {
            name,
            source: "MIXOS_BROKER_ACCOUNT",
        },
        (_, Some(name)) => AccountSelection {
            name,
            source: "node.conf noded.account",
        },
        _ => AccountSelection {
            name: "noded",
            source: "default",
        },
    }
}

impl AccountSelection<'_> {
    fn lookup(&self) -> anyhow::Result<BrokerAccount> {
        self.lookup_inner().with_context(|| {
            format!(
                "broker account {:?} from {} is unavailable",
                self.name, self.source
            )
        })
    }

    fn lookup_inner(&self) -> anyhow::Result<BrokerAccount> {
        let name = std::ffi::CString::new(self.name)?;
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut buffer = vec![0_u8; 65536];
        let mut result = std::ptr::null_mut();
        // SAFETY: entry and buffer remain live until uid/gid have been copied.
        let rc = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        anyhow::ensure!(rc == 0 && !result.is_null(), "getpwnam_r failed ({rc})");
        // SAFETY: getpwnam_r succeeded and returned a non-null result.
        let entry = unsafe { entry.assume_init() };
        Ok(BrokerAccount {
            uid: entry.pw_uid,
            gid: entry.pw_gid,
        })
    }
}

/// Snapshot environment strings before a shell can mutate them. Resolution
/// reads one node file for URL, socket and account; no environment is reread.
pub struct BrokerEnvironment {
    search: config::node::NodeConfigSearch,
    directories: config::Environment,
    endpoint: Option<PathBuf>,
    account: Option<std::ffi::OsString>,
    url: Option<String>,
}

/// One node-file read, with NSS deferred until a Unix candidate is available.
pub struct BrokerConfiguration {
    options: UnixConnectOptions,
    pub url: String,
    account: String,
    account_source: &'static str,
}

impl BrokerConfiguration {
    fn account_selection(&self) -> AccountSelection<'_> {
        AccountSelection {
            name: &self.account,
            source: self.account_source,
        }
    }

    pub fn endpoint_candidates(&self) -> anyhow::Result<Vec<PathBuf>> {
        self.options.endpoint_candidates()
    }

    /// Locate before NSS; this operation establishes no broker authority.
    pub async fn resolve_endpoint(&self) -> Result<PathBuf, ConnectError> {
        resolve_unix_endpoint(&self.options, &self.url).await
    }

    pub fn into_options(mut self) -> anyhow::Result<UnixConnectOptions> {
        self.options.broker_account = self.account_selection().lookup()?;
        Ok(self.options)
    }
}

impl BrokerEnvironment {
    pub fn capture() -> Self {
        let directories = config::Environment::current();
        Self {
            search: config::node::NodeConfigSearch::from_environment(
                std::env::var_os("MIXOS_NODE_CONFIG")
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from),
                directories.clone(),
            ),
            directories,
            endpoint: std::env::var_os("MIXOS_BUS_SOCKET")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            account: std::env::var_os("MIXOS_BROKER_ACCOUNT"),
            url: std::env::var("MIXOS_NODED_URL")
                .ok()
                .filter(|url| !url.trim().is_empty()),
        }
    }

    pub fn resolve(self) -> anyhow::Result<(UnixConnectOptions, String)> {
        let configuration = self.configuration()?;
        let url = configuration.url.clone();
        Ok((configuration.into_options()?, url))
    }

    pub fn configuration(self) -> anyhow::Result<BrokerConfiguration> {
        let source = self.search.read()?;
        #[derive(Default, serde::Deserialize)]
        #[serde(default)]
        struct NativeConfig {
            noded: config::node::NodedConfig,
        }
        let native: NativeConfig = match &source {
            Some((path, source)) => config::from_conf_mix_str(source)
                .with_context(|| format!("invalid node config {}", path.display()))?,
            None => NativeConfig::default(),
        };
        let environment_account =
            self.account
                .as_ref()
                .map(|account| {
                    account.to_str().with_context(|| format!(
                "broker account {account:?} from MIXOS_BROKER_ACCOUNT is not valid Unicode"
            ))
                })
                .transpose()?;
        let account = account_selection(environment_account, native.noded.account.as_deref());
        let directories =
            config::Dirs::from_environment(&self.directories).map_err(anyhow::Error::msg)?;
        let mut options = UnixConnectOptions::new(BrokerAccount { uid: 0, gid: 0 });
        options.endpoint = self.endpoint;
        options.configured_endpoint = native.noded.unix_socket;
        options.default_endpoints = Some(vec![
            directories.get(config::Dir::Run).join("noded/bus.sock"),
            paths::system_default(paths::Root::Run)
                .expect("built-in system run default")
                .join("noded/bus.sock"),
        ]);
        let url = self
            .url
            .or_else(|| {
                source
                    .as_ref()
                    .and_then(|(_, source)| crate::client::url_from_node_config(source))
            })
            .unwrap_or_else(|| crate::DEFAULT_NODED_URL.to_owned());
        Ok(BrokerConfiguration {
            options,
            url,
            account: account.name.to_owned(),
            account_source: account.source,
        })
    }
}

/// Build the ordered list without testing existence or connecting. Callers
/// may override the first two entries; shared defaults remain in order.
pub fn unix_endpoint_candidates(options: &UnixConnectOptions) -> anyhow::Result<Vec<PathBuf>> {
    let defaults;
    let configured = if options.default_endpoints.is_none() {
        defaults = unix_connect_options(options.broker_account)?;
        &defaults
    } else {
        options
    };
    let mut candidates = Vec::new();
    for path in options
        .endpoint
        .as_ref()
        .or(configured.endpoint.as_ref())
        .into_iter()
        .chain(
            options
                .configured_endpoint
                .as_ref()
                .or(configured.configured_endpoint.as_ref()),
        )
        .chain(configured.default_endpoints.iter().flatten())
    {
        if !candidates.contains(path) {
            candidates.push(path.clone());
        }
    }
    Ok(candidates)
}

/// Locate a Unix endpoint without connecting or deriving authority from it.
/// Only missing candidates are skipped; a PID pin forbids skipping and ping.
pub async fn resolve_unix_endpoint(
    options: &UnixConnectOptions,
    tcp_url: &str,
) -> Result<PathBuf, ConnectError> {
    let candidates = options
        .endpoint_candidates()
        .map_err(ConnectError::Protocol)?;
    resolve_endpoint(candidates, options.expected_server_pid, || {
        discover_endpoint(tcp_url)
    })
    .await
}

/// TCP discovery supplies a locator only, never authority. Every discovered
/// endpoint undergoes the same ownership/peer-credential/version verification.
/// This read does not register a service or send any application/grant traffic.
async fn discover_endpoint(tcp_url: &str) -> Option<PathBuf> {
    let timeout = std::time::Duration::from_secs(2);
    let client = tokio::time::timeout(timeout, NodedClient::connect_anonymous(tcp_url))
        .await
        .ok()?
        .ok()?;
    let ping = tokio::time::timeout(
        timeout,
        client.call("noded", "noded.ping", serde_json::Value::Null),
    )
    .await;
    client.close().await;
    let value = ping.ok()?.ok()?;
    if value["extensions"]["native-session"].as_str() != Some("1") {
        return None;
    }
    value["extensions"]["native-session-endpoint"]
        .as_str()
        .map(PathBuf::from)
}

pub async fn connect_default(service: &str) -> anyhow::Result<NodedClient> {
    NodedClient::connect(service, &resolve_noded_url()).await
}

pub async fn connect_anonymous_default() -> anyhow::Result<NodedClient> {
    NodedClient::connect_anonymous(&resolve_noded_url()).await
}

pub async fn connect_default_with_provenance(
    service: &str,
    provenance: Option<crate::RegisterProvenance>,
) -> anyhow::Result<NodedClient> {
    NodedClient::connect_with_provenance(service, &resolve_noded_url(), provenance).await
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    pub(crate) struct TestDir(pub(crate) PathBuf);

    impl TestDir {
        pub(crate) fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "bus-discovery-{}-{nonce:x}-{next}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn options(paths: &[PathBuf]) -> UnixConnectOptions {
        // SAFETY: reading process credentials has no preconditions.
        let account = unsafe {
            BrokerAccount {
                uid: libc::geteuid(),
                gid: libc::getegid(),
            }
        };
        let mut options = UnixConnectOptions::new(account);
        options.endpoint = paths.first().cloned();
        options.configured_endpoint = paths.get(1).cloned();
        options.default_endpoints = Some(paths.iter().skip(2).cloned().collect());
        options.require_native_session = true;
        options
    }

    #[tokio::test]
    async fn each_local_candidate_precedes_later_candidates_and_ping() {
        let dir = TestDir::new();
        let paths: Vec<_> = ["explicit", "configured", "environment", "system"]
            .into_iter()
            .map(|name| dir.0.join(name))
            .collect();
        for first in 0..paths.len() {
            let listeners: Vec<_> = paths[first..]
                .iter()
                .map(|path| UnixListener::bind(path).unwrap())
                .collect();
            let candidates = options(&paths).endpoint_candidates().unwrap();
            assert_eq!(candidates, paths);
            let path = resolve_endpoint(candidates, None, || async {
                panic!("local candidates must precede ping")
            })
            .await
            .unwrap();
            assert_eq!(path, paths[first]);
            drop(listeners);
            for path in &paths[first..] {
                std::fs::remove_file(path).unwrap();
            }
        }
    }

    #[test]
    fn missing_components_warn_with_path_and_reason() {
        let dir = TestDir::new();
        let absent = dir.0.join("absent/socket");
        let file = dir.0.join("file");
        std::fs::write(&file, "not a directory").unwrap();
        let not_directory = file.join("socket");
        let present = dir.0.join("present");
        let _socket = UnixListener::bind(&present).unwrap();
        let mut warnings = Vec::new();
        let selected = select_endpoint(
            &[absent.clone(), not_directory.clone(), present.clone()],
            false,
            |path, error| warnings.push((path.to_owned(), error.raw_os_error(), error.to_string())),
        )
        .unwrap();
        assert_eq!(selected, Some(present));
        assert_eq!(warnings.len(), 2);
        for (warning, path, errno) in [
            (&warnings[0], absent, libc::ENOENT),
            (&warnings[1], not_directory, libc::ENOTDIR),
        ] {
            assert_eq!(warning.0, path);
            assert_eq!(warning.1, Some(errno));
            assert!(!warning.2.is_empty());
        }
    }

    #[tokio::test]
    async fn ping_is_last_and_missing_error_lists_all_attempts() {
        let dir = TestDir::new();
        let paths: Vec<_> = ["explicit", "configured", "environment", "system"]
            .into_iter()
            .map(|name| dir.0.join(name))
            .collect();
        let advertised = dir.0.join("advertised");
        let socket = UnixListener::bind(&advertised).unwrap();
        assert_eq!(
            resolve_endpoint(paths.clone(), None, || std::future::ready(Some(
                advertised.clone()
            )))
            .await
            .unwrap(),
            advertised
        );
        drop(socket);
        std::fs::remove_file(&advertised).unwrap();
        let error = resolve_endpoint(paths.clone(), None, || {
            std::future::ready(Some(advertised.clone()))
        })
        .await
        .unwrap_err();
        for path in paths.iter().chain([&advertised]) {
            assert!(error.to_string().contains(&path.display().to_string()));
        }
        assert!(matches!(error, ConnectError::CandidatesMissing(_)));
    }

    #[tokio::test]
    async fn pid_pin_disables_skipping_and_ping() {
        let dir = TestDir::new();
        let paths = vec![dir.0.join("missing"), dir.0.join("existing")];
        let _socket = UnixListener::bind(&paths[1]).unwrap();
        let error = resolve_endpoint(paths.clone(), Some(42), || async {
            panic!("PID pin must disable ping")
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains(&paths[0].display().to_string()));
        assert!(
            matches!(error.endpoint_cause(), ConnectError::Io(error) if error.raw_os_error() == Some(libc::ENOENT))
        );
        let path = resolve_endpoint(vec![paths[1].clone()], Some(42), || async {
            panic!("PID pin must disable ping")
        })
        .await
        .unwrap();
        assert_eq!(path, paths[1]);
        assert!(
            resolve_endpoint(Vec::new(), Some(42), || async {
                panic!("PID pin must disable ping")
            })
            .await
            .is_err()
        );
    }

    #[test]
    fn account_precedence_and_unavailable_source_are_explicit() {
        for (environment, configured, name, source) in [
            (
                Some("env-user"),
                Some("config-user"),
                "env-user",
                "MIXOS_BROKER_ACCOUNT",
            ),
            (
                None,
                Some("config-user"),
                "config-user",
                "node.conf noded.account",
            ),
            (None, None, "noded", "default"),
        ] {
            let selected = account_selection(environment, configured);
            assert_eq!(selected.name, name);
            assert_eq!(selected.source, source);
            let error = AccountSelection {
                name: "invalid\0account",
                source: selected.source,
            }
            .lookup()
            .unwrap_err();
            assert!(error.to_string().contains("invalid\\0account"));
            assert!(error.to_string().contains(source));
            let error = AccountSelection {
                name: "mixos-no-such-account",
                source,
            }
            .lookup()
            .unwrap_err();
            assert!(error.to_string().contains("mixos-no-such-account"));
            assert!(error.to_string().contains(source));
        }
    }

    #[test]
    fn one_node_file_supplies_socket_account_and_url() {
        let dir = TestDir::new();
        let path = dir.0.join("node.conf.mix");
        let socket = dir.0.join("configured.sock");
        std::fs::write(&path, format!("wg_ip: '192.0.2.10'\nnoded: {{ port: 4300, unix_socket: '{}', account: 'config-user' }}", socket.display())).unwrap();
        let directories = config::Environment {
            run: Some(dir.0.join("run")),
            prefix: Some(dir.0.clone()),
            ..Default::default()
        };
        let configuration = BrokerEnvironment {
            search: config::node::NodeConfigSearch::from_environment(
                Some(path),
                directories.clone(),
            ),
            directories,
            endpoint: None,
            account: None,
            url: None,
        }
        .configuration()
        .unwrap();
        assert_eq!(configuration.url, "ws://192.0.2.10:4300/ws");
        assert_eq!(configuration.options.configured_endpoint, Some(socket));
        assert_eq!(
            configuration.options.endpoint_candidates().unwrap()[1],
            dir.0.join("run/noded/bus.sock")
        );
        assert_eq!(configuration.account, "config-user");
        assert_eq!(configuration.account_source, "node.conf noded.account");
    }
}
