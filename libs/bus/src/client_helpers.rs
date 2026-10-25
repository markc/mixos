// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Shared broker discovery for native ABP clients. Explicit socket, node
//! config and environment runtime precede TCP ping discovery. A run root from
//! MIXOS_RUN, MIXOS or layout.mx places the process in an environment and
//! excludes the host runtime candidate. A uid tier default includes the system
//! runtime candidate after its own, deduplicated when they coincide as root.
//! An override equal to the tier default still counts as default.
//! Account configuration is independent of socket ownership.
//! A server-PID pin uses its endpoint alone. A launch handoff uses only the
//! verified issuing endpoint its parent stamped into MIXOS_BUS_SOCKET: no node
//! socket configuration, defaults or ping. A missing stamp is refused.
//! MIXOS_BUS_SOCKET is a pin under MIXOS_SESSION_FD and a skippable override
//! without it. Skipping never crosses from a session or desk to the host broker.

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
    pinned: bool,
    discover: F,
) -> Result<PathBuf, ConnectError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Option<PathBuf>>,
{
    let warn = |path: &Path, error: &std::io::Error| {
        tracing::warn!(path = %path.display(), %error, "skipping missing Unix endpoint candidate");
    };
    if let Some(path) = select_endpoint(&candidates, pinned, warn)? {
        return Ok(path);
    }
    if !pinned && let Some(path) = discover().await {
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

/// Capture candidates with a caller-resolved account. An explicit endpoint
/// overrides broken node socket settings; account settings are not relied on.
pub fn unix_connect_options(account: BrokerAccount) -> anyhow::Result<UnixConnectOptions> {
    let mut configuration = BrokerEnvironment::capture().configuration()?;
    configuration.options.broker_account = account;
    Ok(configuration.options)
}

/// Resolve the trusted account: environment, node.conf `noded.account`, noded.
/// NSS errors name both the account and the configuration source.
pub fn resolve_broker_account() -> anyhow::Result<BrokerAccount> {
    let environment = std::env::var_os("MIXOS_BROKER_ACCOUNT");
    resolve_account_from(
        environment.as_deref(),
        &config::node::NodeConfigSearch::capture(),
    )
}

fn resolve_account_from(
    environment: Option<&std::ffi::OsStr>,
    search: &config::node::NodeConfigSearch,
) -> anyhow::Result<BrokerAccount> {
    if let Some(account) = environment_account(environment)? {
        return account_selection(Some(account), None).lookup();
    }
    let source = search.read()?;
    let settings = node_setting(source.as_ref(), "account")?;
    account_selection(None, settings.account.as_deref()).lookup()
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
    account_error: Option<anyhow::Error>,
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
        if let Some(error) = self.account_error.take() {
            return Err(error).context("node config is required for broker account resolution");
        }
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
        let environment_account = environment_account(self.account.as_deref())?;
        let mut account_error = None;
        let source = match self.search.read() {
            Ok(source) => source,
            Err(error) if self.endpoint.is_some() => {
                tracing::warn!(error = %format!("{error:#}"), "ignoring node config for explicit Unix endpoint");
                if environment_account.is_none() {
                    account_error = Some(error);
                }
                None
            }
            Err(error) => return Err(error),
        };
        let configured_endpoint = match node_setting(source.as_ref(), "unix_socket") {
            Ok(settings) => settings.unix_socket,
            Err(error) if self.endpoint.is_some() => {
                tracing::warn!(error = %format!("{error:#}"), "ignoring node socket config for explicit Unix endpoint");
                None
            }
            Err(error) => return Err(error),
        };
        let configured_account = if environment_account.is_none() {
            match node_setting(source.as_ref(), "account") {
                Ok(settings) => settings.account,
                Err(error) => {
                    account_error = Some(error);
                    None
                }
            }
        } else {
            None
        };
        let account = account_selection(environment_account, configured_account.as_deref());
        let directories =
            config::Dirs::from_environment(&self.directories).map_err(anyhow::Error::msg)?;
        let mut options = UnixConnectOptions::new(BrokerAccount { uid: 0, gid: 0 });
        options.endpoint = self.endpoint;
        options.configured_endpoint = configured_endpoint;
        options.default_endpoints = default_unix_endpoints(
            &self.directories,
            &directories,
            &paths::system_default(paths::Root::Run).expect("built-in system run default"),
        )
        .map_err(anyhow::Error::msg)?;
        // A placed process never pings: the URL may name the host broker,
        // which would advertise a socket outside the process's environment.
        options.discover_by_ping =
            paths::run_root_is_tier_default(&self.directories, &directories)
                .map_err(anyhow::Error::msg)?;
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
            account_error,
        })
    }
}

fn environment_account(account: Option<&std::ffi::OsStr>) -> anyhow::Result<Option<&str>> {
    account
        .map(|account| {
            account.to_str().with_context(|| {
                format!("broker account {account:?} from MIXOS_BROKER_ACCOUNT is not valid Unicode")
            })
        })
        .transpose()
}

/// Decode only the field being relied on, so an overridden field cannot veto it.
fn node_setting(
    source: Option<&(PathBuf, String)>,
    field: &str,
) -> anyhow::Result<config::node::NodedConfig> {
    let Some((path, source)) = source else {
        return Ok(config::node::NodedConfig::default());
    };
    let decode = || -> anyhow::Result<config::node::NodedConfig> {
        let value = strict::parse(source)?;
        anyhow::ensure!(
            matches!(value, strict::Value::Map(_)),
            "node config must be a map"
        );
        let Some(noded) = value.get("noded") else {
            return Ok(config::node::NodedConfig::default());
        };
        anyhow::ensure!(
            matches!(noded, strict::Value::Map(_)),
            "noded must be a map"
        );
        let mut fields = strict::Map::new();
        if let Some(value) = noded.get(field) {
            fields.insert(field.to_owned(), value.clone());
        }
        Ok(strict::from_value(&strict::Value::Map(fields))?)
    };
    decode().with_context(|| format!("invalid node config {} noded.{field}", path.display()))
}

/// Build the ordered list without testing existence or connecting. Callers
/// may override the first two entries; shared defaults remain in order.
pub fn unix_endpoint_candidates(options: &UnixConnectOptions) -> anyhow::Result<Vec<PathBuf>> {
    if options.endpoint_only {
        let endpoint = options
            .endpoint
            .as_ref()
            .context("a launch handoff requires the parent's broker endpoint")?;
        return Ok(vec![endpoint.clone()]);
    }
    let mut candidates = Vec::new();
    for path in options
        .endpoint
        .iter()
        .chain(options.configured_endpoint.iter())
        .chain(&options.default_endpoints)
    {
        if !candidates.contains(path) {
            candidates.push(path.clone());
        }
    }
    Ok(candidates)
}

pub(crate) fn default_unix_endpoints(
    environment: &config::Environment,
    directories: &config::Dirs,
    system_run: &Path,
) -> paths::Result<Vec<PathBuf>> {
    let mut endpoints = vec![directories.get(config::Dir::Run).join("noded/bus.sock")];
    if paths::run_root_is_tier_default(environment, directories)? {
        let system = system_run.join("noded/bus.sock");
        if !endpoints.contains(&system) {
            endpoints.push(system);
        }
    }
    Ok(endpoints)
}

/// Locate a Unix endpoint without connecting or deriving authority from it.
/// Only missing candidates are skipped; PID and handoff pins forbid skipping and ping.
pub async fn resolve_unix_endpoint(
    options: &UnixConnectOptions,
    tcp_url: &str,
) -> Result<PathBuf, ConnectError> {
    let candidates = options
        .endpoint_candidates()
        .map_err(ConnectError::Protocol)?;
    resolve_endpoint(candidates, options.endpoint_is_pinned(), || async {
        if options.discover_by_ping {
            discover_endpoint(tcp_url).await
        } else {
            None
        }
    })
    .await
}

/// Re-resolve once after a selected endpoint disappears. Other failures remain
/// fatal, and PID or handoff pins forbid both skipping and this retry.
pub(crate) async fn connect_unix_candidates<T, F, Fut>(
    options: &UnixConnectOptions,
    tcp_url: &str,
    mut connect: F,
) -> Result<T, ConnectError>
where
    F: FnMut(PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<T, ConnectError>>,
{
    let mut candidates = options
        .endpoint_candidates()
        .map_err(ConnectError::Protocol)?;
    let mut tried = candidates.clone();
    let mut ping_available = options.discover_by_ping;
    let mut retried = false;
    loop {
        let mut ping_used = false;
        let path = resolve_endpoint(candidates.clone(), options.endpoint_is_pinned(), || {
            ping_used = ping_available;
            async {
                if ping_available {
                    discover_endpoint(tcp_url).await
                } else {
                    None
                }
            }
        })
        .await
        .map_err(|error| match error {
            ConnectError::CandidatesMissing(paths) => {
                for path in paths {
                    if !tried.contains(&path) {
                        tried.push(path);
                    }
                }
                ConnectError::CandidatesMissing(tried.clone())
            }
            error => error,
        })?;
        if ping_used {
            ping_available = false;
        }
        if !tried.contains(&path) {
            tried.push(path.clone());
        }
        match connect(path.clone()).await {
            Ok(connection) => return Ok(connection),
            Err(error)
                if !options.endpoint_is_pinned()
                    && matches!(error.endpoint_cause(), ConnectError::Io(error)
                    if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))) =>
            {
                tracing::warn!(path = %path.display(), %error, "selected Unix endpoint disappeared");
                if retried {
                    return Err(ConnectError::CandidatesMissing(tried));
                }
                retried = true;
                candidates = candidates
                    .iter()
                    .position(|candidate| candidate == &path)
                    .map_or_else(Vec::new, |index| candidates[index + 1..].to_vec());
            }
            Err(error) => return Err(at_endpoint(&path, error)),
        }
    }
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

    fn captured_environment(dir: &TestDir) -> BrokerEnvironment {
        let directories = config::Environment {
            prefix: Some(dir.0.clone()),
            run: Some(dir.0.join("run")),
            ..Default::default()
        };
        BrokerEnvironment {
            search: config::node::NodeConfigSearch::from_environment(
                Some(dir.0.join("node.conf.mix")),
                directories.clone(),
            ),
            directories,
            endpoint: None,
            account: None,
            url: None,
        }
    }

    #[test]
    fn captured_runtime_defaults_distinguish_host_logins_from_placed_environments() {
        let dir = TestDir::new();
        std::fs::write(dir.0.join("node.conf.mix"), "noded: {}").unwrap();
        let user = config::Environment {
            uid: 1000,
            home: Some(dir.0.join("home")),
            prefix: Some(dir.0.clone()),
            xdg_runtime_dir: Some(dir.0.join("xdg")),
            ..Default::default()
        };
        let system = paths::system_default(paths::Root::Run)
            .unwrap()
            .join("noded/bus.sock");
        for (directories, expected, pings) in [
            (
                user.clone(),
                vec![dir.0.join("xdg/mixos/noded/bus.sock"), system.clone()],
                true,
            ),
            (
                config::Environment {
                    run: Some(dir.0.join("session-run")),
                    ..user.clone()
                },
                vec![dir.0.join("session-run/noded/bus.sock")],
                false,
            ),
            (
                config::Environment {
                    root: Some(dir.0.join("desk")),
                    ..user.clone()
                },
                vec![dir.0.join("desk/run/noded/bus.sock")],
                false,
            ),
            (
                config::Environment {
                    uid: 0,
                    ..user.clone()
                },
                vec![system],
                true,
            ),
        ] {
            let environment = BrokerEnvironment {
                search: config::node::NodeConfigSearch::from_environment(
                    Some(dir.0.join("node.conf.mix")),
                    directories.clone(),
                ),
                directories,
                endpoint: None,
                account: None,
                url: None,
            };
            let configuration = environment.configuration().unwrap();
            assert_eq!(configuration.options.default_endpoints, expected);
            assert_eq!(configuration.options.discover_by_ping, pings);
            assert_eq!(configuration.endpoint_candidates().unwrap(), expected);
        }
        std::fs::write(
            dir.0.join("layout.mx"),
            format!("run: '{}'", dir.0.join("layout-run").display()),
        )
        .unwrap();
        let mut environment = captured_environment(&dir);
        environment.directories = user;
        let options = environment.configuration().unwrap().options;
        assert_eq!(
            options.default_endpoints,
            vec![dir.0.join("layout-run/noded/bus.sock")]
        );
        assert!(!options.discover_by_ping, "a layout-placed process must not ping");
    }

    #[tokio::test]
    async fn explicit_endpoint_and_account_override_broken_node_config() {
        let dir = TestDir::new();
        let endpoint = dir.0.join("explicit.sock");
        let _socket = UnixListener::bind(&endpoint).unwrap();
        for contents in ["noded: {", "noded: { unix_socket: 7, account: 7 }"] {
            std::fs::write(dir.0.join("node.conf.mix"), contents).unwrap();
            let mut environment = captured_environment(&dir);
            environment.endpoint = Some(endpoint.clone());
            environment.account = Some("env-account".into());
            let configuration = environment.configuration().unwrap();
            assert_eq!(configuration.account, "env-account");
            assert!(configuration.account_error.is_none());
            assert_eq!(configuration.resolve_endpoint().await.unwrap(), endpoint);
            let mut environment = captured_environment(&dir);
            environment.account = Some("env-account".into());
            let error = environment.configuration().err().unwrap();
            assert!(format!("{error:#}").contains("node.conf.mix"));
        }
        // Reading a directory fails even when the test runs as root.
        std::fs::remove_file(dir.0.join("node.conf.mix")).unwrap();
        std::fs::create_dir(dir.0.join("node.conf.mix")).unwrap();
        let mut environment = captured_environment(&dir);
        environment.endpoint = Some(endpoint.clone());
        environment.account = Some("env-account".into());
        assert_eq!(
            environment
                .configuration()
                .unwrap()
                .resolve_endpoint()
                .await
                .unwrap(),
            endpoint
        );
        assert!(captured_environment(&dir).configuration().is_err());
    }

    #[test]
    fn only_the_config_field_being_relied_on_can_fail_resolution() {
        let dir = TestDir::new();
        std::fs::write(
            dir.0.join("node.conf.mix"),
            "noded: { unix_socket: 7, account: 'config-account' }",
        )
        .unwrap();
        let mut environment = captured_environment(&dir);
        environment.endpoint = Some(dir.0.join("explicit.sock"));
        let configuration = environment.configuration().unwrap();
        assert_eq!(configuration.account, "config-account");
        assert!(configuration.account_error.is_none());
        std::fs::write(
            dir.0.join("node.conf.mix"),
            "noded: { unix_socket: '/tmp/example.sock', account: 7 }",
        )
        .unwrap();
        let mut environment = captured_environment(&dir);
        environment.account = Some("env-account".into());
        let configuration = environment.configuration().unwrap();
        assert_eq!(
            configuration.options.configured_endpoint,
            Some("/tmp/example.sock".into())
        );
        assert_eq!(configuration.account, "env-account");
        // Account-only lookup must not read the broken config before the override.
        let error = resolve_account_from(
            Some(std::ffi::OsStr::new("invalid\0account")),
            &captured_environment(&dir).search,
        )
        .unwrap_err();
        assert!(error.to_string().contains("MIXOS_BROKER_ACCOUNT"));
        assert!(!format!("{error:#}").contains("node.conf"));
        std::fs::write(dir.0.join("node.conf.mix"), "noded: {").unwrap();
        let mut environment = captured_environment(&dir);
        environment.endpoint = Some(dir.0.join("explicit.sock"));
        let error = environment
            .configuration()
            .unwrap()
            .into_options()
            .unwrap_err();
        assert!(format!("{error:#}").contains("broker account resolution"));
    }

    #[test]
    fn captured_explicit_socket_precedes_the_full_configured_order() {
        let dir = TestDir::new();
        let explicit = dir.0.join("explicit.sock");
        let configured = dir.0.join("configured.sock");
        std::fs::write(
            dir.0.join("node.conf.mix"),
            format!("noded: {{ unix_socket: '{}' }}", configured.display()),
        )
        .unwrap();
        let mut environment = captured_environment(&dir);
        environment.endpoint = Some(explicit.clone());
        let configuration = environment.configuration().unwrap();
        assert_eq!(
            configuration.endpoint_candidates().unwrap(),
            vec![explicit, configured, dir.0.join("run/noded/bus.sock")]
        );
        let mut supplied = UnixConnectOptions::new(BrokerAccount { uid: 123, gid: 123 });
        supplied.endpoint = Some(dir.0.join("caller.sock"));
        assert_eq!(
            supplied.endpoint_candidates().unwrap(),
            vec![dir.0.join("caller.sock")]
        );
        assert!(supplied.default_endpoints.is_empty());
    }

    #[test]
    fn non_missing_component_errors_are_fatal_and_name_the_candidate() {
        let dir = TestDir::new();
        let path = dir.0.join("x".repeat(256)).join("socket");
        let error = select_endpoint(&[path.clone(), dir.0.join("later")], false, |_, _| {
            panic!("must not skip a non-missing error")
        })
        .unwrap_err();
        assert!(
            matches!(error.endpoint_cause(), ConnectError::Io(error) if error.raw_os_error() == Some(libc::ENAMETOOLONG))
        );
        assert!(error.to_string().contains(&path.display().to_string()));
    }

    #[tokio::test]
    async fn disappearing_endpoint_retries_remaining_candidates_once() {
        let dir = TestDir::new();
        let paths = vec![
            dir.0.join("first"),
            dir.0.join("second"),
            dir.0.join("third"),
        ];
        let _sockets: Vec<_> = paths
            .iter()
            .map(|path| UnixListener::bind(path).unwrap())
            .collect();
        let mut options = options(&paths);
        options.discover_by_ping = false;
        let mut attempts = Vec::new();
        let selected = connect_unix_candidates(&options, "ws://192.0.2.1:4200/ws", |path| {
            attempts.push(path.clone());
            if attempts.len() == 1 {
                std::fs::remove_file(&path).unwrap();
            }
            std::future::ready(
                std::fs::symlink_metadata(&path)
                    .map(|_| path)
                    .map_err(ConnectError::Io),
            )
        })
        .await
        .unwrap();
        assert_eq!(selected, paths[1]);
        assert_eq!(attempts, paths[..2]);
        let _first = UnixListener::bind(&paths[0]).unwrap();
        for errno in [libc::ENOENT, libc::ENOTDIR] {
            let mut attempts = Vec::new();
            let error =
                connect_unix_candidates::<(), _, _>(&options, "ws://192.0.2.1:4200/ws", |path| {
                    attempts.push(path);
                    std::future::ready(Err(ConnectError::Io(std::io::Error::from_raw_os_error(
                        errno,
                    ))))
                })
                .await
                .unwrap_err();
            assert_eq!(attempts, paths[..2]);
            assert!(matches!(error, ConnectError::CandidatesMissing(ref tried) if tried == &paths));
        }
        options.expected_server_pid = Some(42);
        let mut attempts = 0;
        let error = connect_unix_candidates::<(), _, _>(&options, "ws://192.0.2.1:4200/ws", |_| {
            attempts += 1;
            std::future::ready(Err(ConnectError::Io(std::io::Error::from_raw_os_error(
                libc::ENOENT,
            ))))
        })
        .await
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert!(matches!(error.endpoint_cause(), ConnectError::Io(_)));
    }

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
        options.default_endpoints = paths.iter().skip(2).cloned().collect();
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
            let path = resolve_endpoint(candidates, false, || async {
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
            resolve_endpoint(paths.clone(), false, || std::future::ready(Some(
                advertised.clone()
            )))
            .await
            .unwrap(),
            advertised
        );
        drop(socket);
        std::fs::remove_file(&advertised).unwrap();
        let error = resolve_endpoint(paths.clone(), false, || {
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
        let error = resolve_endpoint(paths.clone(), true, || async {
            panic!("PID pin must disable ping")
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains(&paths[0].display().to_string()));
        assert!(
            matches!(error.endpoint_cause(), ConnectError::Io(error) if error.raw_os_error() == Some(libc::ENOENT))
        );
        let path = resolve_endpoint(vec![paths[1].clone()], true, || async {
            panic!("PID pin must disable ping")
        })
        .await
        .unwrap();
        assert_eq!(path, paths[1]);
        assert!(
            resolve_endpoint(Vec::new(), true, || async {
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
