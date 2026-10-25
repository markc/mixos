// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Verified node-local BUS-013 transport with shared environment discovery.
//! Missing candidates may be skipped before connecting. An unpinned client
//! retries once if its selected endpoint disappears; other failures on an
//! existing endpoint are fatal. Ordinary `NodedClient::connect` remains TCP.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Component, Path, PathBuf};

use crate::client_helpers::connect_unix_candidates;
use crate::native_session::BrokerPrincipal;
use tokio::sync::mpsc;

use crate::native_client::{IncomingCommand, NodedClient};

/// Trusted configuration, resolved by the application from its broker account.
/// No fixed numeric UID or caller-UID default is assumed.
#[derive(Debug, Clone, Copy)]
pub struct BrokerAccount {
    pub uid: u32,
    pub gid: u32,
}

/// Kernel credentials captured from the Unix server at connect time.
/// They are a snapshot, not a claim about the process at a later instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerPeerCredentials {
    /// None when the kernel API supplies no usable positive PID. An explicit
    /// PID pin rejects that case after Unix connect and before WebSocket setup.
    pub pid: Option<u32>,
    pub uid: u32,
    pub gid: u32,
}

/// Unix connection policy. Shared discovery supplies node-config and runtime
/// candidates; explicit fields retain precedence for existing callers.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct UnixConnectOptions {
    pub broker_account: BrokerAccount,
    pub endpoint: Option<PathBuf>,
    pub configured_endpoint: Option<PathBuf>,
    /// A launch handoff is bound to its issuing endpoint. Use only the
    /// parent-stamped endpoint, without config, defaults, skipping, ping or TCP fallback.
    pub endpoint_only: bool,
    /// Runtime candidates for this environment, captured before connecting. Empty means none.
    pub default_endpoints: Vec<PathBuf>,
    /// Permit TCP ping discovery after local candidates are missing. Endpoint
    /// and PID pins override this option.
    pub discover_by_ping: bool,
    /// Hard requirement. Overrides `allow_unverified_tcp_fallback` on every
    /// failure, including permissions, ownership, peer credentials and version.
    pub require_native_session: bool,
    /// Explicitly allow a fresh TCP connection when all Unix candidates are
    /// missing. It never carries trusted context, even if TCP advertises native-session.
    pub allow_unverified_tcp_fallback: bool,
    /// Require this kernel-reported server PID on every Unix connection.
    /// A pin disables TCP endpoint discovery and all TCP fallback.
    pub expected_server_pid: Option<u32>,
    /// Opt-in bounded verified command lane (1..=1024). Excess requests receive a uniform refusal; lifecycle notices
    /// use backpressure rather than being dropped.
    /// Individual retained commands are limited to 64 KiB of envelope/body.
    pub incoming_capacity: Option<usize>,
}

impl UnixConnectOptions {
    /// Construct without reading configuration or environment. Capture shared
    /// defaults through BrokerEnvironment before passing options to connect.
    pub fn new(broker_account: BrokerAccount) -> Self {
        Self {
            broker_account,
            endpoint: None,
            configured_endpoint: None,
            endpoint_only: false,
            default_endpoints: Vec::new(),
            discover_by_ping: true,
            require_native_session: false,
            allow_unverified_tcp_fallback: false,
            expected_server_pid: None,
            incoming_capacity: None,
        }
    }

    /// Ordered explicit/config/environment/system candidates, before ping.
    pub fn endpoint_candidates(&self) -> anyhow::Result<Vec<PathBuf>> {
        crate::client_helpers::unix_endpoint_candidates(self)
    }

    pub(crate) fn endpoint_is_pinned(&self) -> bool {
        self.endpoint_only || self.expected_server_pid.is_some()
    }
}

/// Structured connection failure. Diagnostics never include wire payloads.
#[derive(Debug)]
pub enum ConnectError {
    Endpoint {
        path: PathBuf,
        error: Box<ConnectError>,
    },
    CandidatesMissing(Vec<PathBuf>),
    InvalidEndpoint,
    EndpointOwnership,
    EndpointChanged,
    PeerCredentials,
    ServerPidMismatch,
    UnsupportedVersion,
    Io(std::io::Error),
    Protocol(anyhow::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Endpoint { path, error } => {
                return write!(f, "Unix endpoint {}: {error}", path.display());
            }
            Self::CandidatesMissing(paths) => {
                return write!(
                    f,
                    "no Unix endpoint exists; candidates tried: {}; noded.ping discovery unavailable or disabled",
                    paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Self::Io(error) => return write!(f, "Unix endpoint I/O failed: {error}"),
            Self::Protocol(error) => return write!(f, "broker connection setup failed: {error:#}"),
            _ => {}
        }
        f.write_str(match self {
            Self::Endpoint { .. } | Self::CandidatesMissing(_) => unreachable!(),
            Self::InvalidEndpoint => {
                "Unix endpoint must be absolute without symlinks or dot components"
            }
            Self::EndpointOwnership => "Unix endpoint or ancestor ownership/mode is not protected",
            Self::EndpointChanged => "Unix endpoint changed during connection verification",
            Self::PeerCredentials => {
                "Unix server credentials do not match configured broker account"
            }
            Self::ServerPidMismatch => "Unix server PID is missing or does not match the pin",
            Self::UnsupportedVersion => "broker does not support native-session version 1",
            Self::Io(_) => "Unix endpoint I/O failed",
            Self::Protocol(_) => "broker connection setup failed",
        })
    }
}
impl std::error::Error for ConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Endpoint { error, .. } => Some(error.as_ref()),
            Self::Io(error) => Some(error),
            Self::Protocol(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl ConnectError {
    /// The failure beneath its endpoint diagnostic, for existing policy checks.
    pub fn endpoint_cause(&self) -> &Self {
        match self {
            Self::Endpoint { error, .. } => error.endpoint_cause(),
            error => error,
        }
    }
}

/// The fallback is deliberately a distinct, explicitly unverified variant.
/// Neither raw headers nor an extension advertisement can upgrade it.
pub enum UnixConnectOutcome {
    VerifiedUnix(VerifiedConnection),
    UnverifiedTcp {
        client: NodedClient,
        unix_error: ConnectError,
    },
}

/// Only endpoint verification plus profile negotiation can construct this.
pub struct VerifiedConnection {
    client: NodedClient,
    endpoint: PathBuf,
    server_peer_credentials: ServerPeerCredentials,
    incoming: tokio::sync::Mutex<VerifiedIncoming>,
    pub(crate) session_lock: tokio::sync::Mutex<()>,
    /// One hello per connection: see [`VerifiedConnection::session_context`].
    pub(crate) session_context: tokio::sync::OnceCell<crate::native_client::session::Hello>,
}
impl VerifiedConnection {
    /// Requests/replies use the existing ABP client API. Its raw receive lane
    /// is absent on this handle; trusted deliveries come only from `recv`.
    pub fn client(&self) -> &NodedClient {
        &self.client
    }

    /// The verified endpoint that issued grants on this connection.
    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    /// Connect-time SO_PEERCRED, retained for a caller's authority check.
    pub fn server_peer_credentials(&self) -> ServerPeerCredentials {
        self.server_peer_credentials
    }

    pub async fn recv(&mut self) -> Option<VerifiedCommand> {
        self.recv_shared().await
    }

    /// Single receive owner may share this connection with bounded RPC tasks.
    pub async fn recv_shared(&self) -> Option<VerifiedCommand> {
        match &mut *self.incoming.lock().await {
            VerifiedIncoming::Unbounded(receiver) => receiver.recv().await,
            VerifiedIncoming::Bounded {
                commands,
                refusals,
                gap,
            } => {
                if gap.swap(false, std::sync::atomic::Ordering::AcqRel) {
                    return Some(VerifiedCommand::gap());
                }
                // Refusals first: a request the lane dropped is already waiting
                // on its caller's deadline, and answering it frees that caller.
                tokio::select! {
                    biased;
                    refused = refusals.recv() => match refused {
                        Some(refused) => Some(refused),
                        None => commands.recv().await,
                    },
                    command = commands.recv() => command,
                }
            }
        }
    }
}

pub(crate) enum VerifiedIncoming {
    Unbounded(mpsc::UnboundedReceiver<VerifiedCommand>),
    Bounded {
        commands: mpsc::Receiver<VerifiedCommand>,
        refusals: mpsc::Receiver<VerifiedCommand>,
        gap: std::sync::Arc<std::sync::atomic::AtomicBool>,
    },
}

/// What the receive owner owes this delivery. The reader task never writes to
/// the shared sink and never waits on a full lane, so both of the non-ordinary
/// outcomes it can reach are reported here and settled by the owner instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// An ordinary broker delivery to act on.
    Command,
    /// The bounded lane was full; the owner owes this request one refusal.
    Refuse,
    /// One or more id-less broker notices were dropped. Indistinguishable from
    /// a missed lifecycle change: treat it exactly as a broker lifecycle gap.
    Gap,
}

/// Immutable delivery paired with context parsed on its verified transport.
/// No public constructor, deserialiser or mutable command accessor exists.
/// Context authenticates the stamp, not a current session lease: cached and
/// retained deliveries still require BROKER-020/PROP-025 lease enforcement.
///
/// ```compile_fail,E0624
/// fn forge(command: bus::native_client::IncomingCommand) {
///     let _ = bus::native_client::VerifiedCommand::new(command, None);
/// }
/// ```
/// ```compile_fail,E0599
/// fn trust_raw(command: bus::native_client::IncomingCommand) {
///     let _ = command.trusted_context();
/// }
/// ```
pub struct VerifiedCommand {
    command: IncomingCommand,
    principal: Option<BrokerPrincipal>,
    delivery: Delivery,
}
impl VerifiedCommand {
    pub(crate) fn new(command: IncomingCommand, principal: Option<BrokerPrincipal>) -> Self {
        Self {
            command,
            principal,
            delivery: Delivery::Command,
        }
    }
    /// Keeps the correlation the reply needs and nothing that could be mistaken
    /// for an admitted request: the delivery itself says it must be refused.
    pub(crate) fn refusal(self) -> Self {
        Self {
            delivery: Delivery::Refuse,
            ..self
        }
    }
    /// Carries no broker content and no principal: a gap is the absence of a
    /// delivery, never a delivery to act on.
    pub(crate) fn gap() -> Self {
        Self {
            command: IncomingCommand {
                generation: 0,
                from: String::new(),
                command: String::new(),
                id: None,
                args: serde_json::Value::Null,
                body: String::new(),
                headers: Default::default(),
            },
            principal: None,
            delivery: Delivery::Gap,
        }
    }
    /// What the receive owner owes this delivery. Check this before the command:
    /// a refusal or a gap carries no admissible request.
    pub fn delivery(&self) -> Delivery {
        self.delivery
    }
    pub fn command(&self) -> &IncomingCommand {
        &self.command
    }
    /// None for unverified senders and direct broker control messages.
    pub fn trusted_context(&self) -> Option<&BrokerPrincipal> {
        self.principal.as_ref()
    }
}

impl NodedClient {
    /// Explicit Unix opt-in; ordinary `connect` remains unchanged. Native
    /// session clients MUST set `require_native_session`. Resolution is explicit
    /// endpoint, config, environment runtime, then ping. The system runtime
    /// candidate follows a uid tier default; placed run roots exclude it. Every
    /// candidate is verified; discovery never establishes authority. Unix is node-local
    /// in v1: mesh-destined traffic is refused, with no transparent TCP retry.
    /// One disappearing endpoint permits re-resolution unless its endpoint or server PID is pinned.
    pub async fn connect_unix(
        service_name: &str,
        tcp_url: &str,
        options: &UnixConnectOptions,
        provenance: Option<crate::RegisterProvenance>,
    ) -> Result<UnixConnectOutcome, ConnectError> {
        let result = connect_unix_candidates(options, tcp_url, |path| {
            let provenance = provenance.clone();
            async move { connect_verified(service_name, &path, options, provenance).await }
        })
        .await;
        match result {
            Ok(connection) => Ok(UnixConnectOutcome::VerifiedUnix(connection)),
            Err(unix_error)
                if !options.require_native_session
                    && !options.endpoint_is_pinned()
                    && options.allow_unverified_tcp_fallback
                    && matches!(unix_error, ConnectError::CandidatesMissing(_)) =>
            {
                let client = Self::connect_with_provenance(service_name, tcp_url, provenance)
                    .await
                    .map_err(ConnectError::Protocol)?;
                Ok(UnixConnectOutcome::UnverifiedTcp { client, unix_error })
            }
            Err(error) => Err(error),
        }
    }
}

async fn connect_verified(
    service_name: &str,
    path: &Path,
    options: &UnixConnectOptions,
    provenance: Option<crate::RegisterProvenance>,
) -> Result<VerifiedConnection, ConnectError> {
    let before = verify_path(path, options.broker_account)?;
    let socket = tokio::net::UnixStream::connect(path)
        .await
        .map_err(ConnectError::Io)?;
    // SO_PEERCRED describes the server at connect time, not later setuid.
    let peer = socket.peer_cred().map_err(ConnectError::Io)?;
    if peer.uid() != options.broker_account.uid || peer.gid() != options.broker_account.gid {
        return Err(ConnectError::PeerCredentials);
    }
    let pid = verified_server_pid(peer.pid(), options.expected_server_pid)?;
    if verify_path(path, options.broker_account)? != before {
        return Err(ConnectError::EndpointChanged);
    }
    let (ws, _) = tokio_tungstenite::client_async("ws://localhost/ws", socket)
        .await
        .map_err(|error| ConnectError::Protocol(error.into()))?;
    let (client, incoming) =
        NodedClient::from_verified_unix(ws, service_name, provenance, options.incoming_capacity)
            .await
            .map_err(|error| match error.downcast::<ConnectError>() {
                Ok(error) => error,
                Err(error) => ConnectError::Protocol(error),
            })?;
    Ok(VerifiedConnection {
        client,
        endpoint: path.to_owned(),
        server_peer_credentials: ServerPeerCredentials {
            pid,
            uid: peer.uid(),
            gid: peer.gid(),
        },
        incoming: tokio::sync::Mutex::new(incoming),
        session_lock: tokio::sync::Mutex::new(()),
        session_context: tokio::sync::OnceCell::new(),
    })
}

fn verified_server_pid(
    pid: Option<i32>,
    expected: Option<u32>,
) -> Result<Option<u32>, ConnectError> {
    let pid = pid
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid != 0);
    if expected.is_some() && expected != pid {
        return Err(ConnectError::ServerPidMismatch);
    }
    Ok(pid)
}

/// Check every component, rejecting symlinks and unprotected replacement
/// paths. A root-owned sticky ancestor (e.g. /tmp) is allowed only above a
/// protected broker/root-owned child. Sticky rules protect that child from
/// other UIDs. The immediate socket directory must never be world-writable.
type PathSnapshot = Vec<(u64, u64, u32, u32, u32)>;

fn verify_path(path: &Path, account: BrokerAccount) -> Result<PathSnapshot, ConnectError> {
    if !path.is_absolute()
        || path
            .as_os_str()
            .as_encoded_bytes()
            .split(|b| *b == b'/')
            .any(|part| part == b"." || part == b"..")
    {
        return Err(ConnectError::InvalidEndpoint);
    }
    let parent = path.parent().ok_or(ConnectError::InvalidEndpoint)?;
    let mut current = PathBuf::new();
    let mut identities = Vec::new();
    for component in path.components() {
        if !matches!(component, Component::RootDir | Component::Normal(_)) {
            return Err(ConnectError::InvalidEndpoint);
        }
        current.push(component);
        let meta = std::fs::symlink_metadata(&current).map_err(ConnectError::Io)?;
        if current == path {
            if !meta.file_type().is_socket() || meta.uid() != account.uid {
                return Err(ConnectError::EndpointOwnership);
            }
        } else {
            let sticky_ancestor = current != parent && meta.uid() == 0 && meta.mode() & 0o1000 != 0;
            if !meta.is_dir()
                || (meta.uid() != 0 && meta.uid() != account.uid)
                || (meta.mode() & 0o022 != 0 && !sticky_ancestor)
            {
                return Err(ConnectError::EndpointOwnership);
            }
        }
        identities.push((meta.dev(), meta.ino(), meta.uid(), meta.gid(), meta.mode()));
    }
    Ok(identities)
}

// Scan before the compatibility parser can overwrite identical header keys.
// BUS-014 allows only one canonical spelling. JSON duplicates and known-field
// types are then checked by lib-bus read_principal.
pub(crate) fn principal_header_is_unique(wire: &str) -> bool {
    let Some(content) = wire.strip_prefix("---\n") else {
        return false;
    };
    // Match the compatibility parser's boundary, including unterminated
    // headers. The opening delimiter must not terminate the scan.
    let headers = content
        .split_once("\n---\n")
        .map_or(content, |(headers, _)| headers);
    let mut seen = false;
    for line in headers.lines() {
        if let Some((key, _)) = line.split_once(':')
            && key.trim().eq_ignore_ascii_case("broker_principal")
        {
            if seen || key != "broker_principal" {
                return false;
            }
            seen = true;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_helpers::tests::{TestDir, options};
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;

    #[tokio::test]
    async fn missing_environment_socket_never_uses_the_system_runtime_socket() {
        let dir = TestDir::new();
        let run = dir.0.join("session-run");
        let system_run = dir.0.join("system-run");
        std::fs::create_dir_all(system_run.join("noded")).unwrap();
        let system_socket = system_run.join("noded/bus.sock");
        let listener = UnixListener::bind(&system_socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut options = options(&[]);
        assert!(verify_path(&system_socket, options.broker_account).is_ok());
        let environment = config::Environment {
            run: Some(run.clone()),
            ..Default::default()
        };
        let directories = config::Dirs::from_source(&environment, "").unwrap();
        options.default_endpoints =
            crate::client_helpers::default_unix_endpoints(&environment, &directories, &system_run)
                .unwrap();
        options.discover_by_ping = false;
        assert!(!options.endpoint_is_pinned());
        let candidates = vec![run.join("noded/bus.sock")];
        assert_eq!(options.endpoint_candidates().unwrap(), candidates);
        let error = NodedClient::connect_unix("", "ws://127.0.0.1:1/ws", &options, None)
            .await
            .err()
            .unwrap();
        assert!(matches!(error, ConnectError::CandidatesMissing(paths) if paths == candidates));
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }

    #[tokio::test]
    async fn missing_handoff_endpoint_never_uses_a_verified_later_candidate() {
        let dir = TestDir::new();
        let missing = dir.0.join("issuing.sock");
        let later = dir.0.join("other.sock");
        let listener = UnixListener::bind(&later).unwrap();
        listener.set_nonblocking(true).unwrap();
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        tcp.set_nonblocking(true).unwrap();
        let url = format!("ws://{}/ws", tcp.local_addr().unwrap());
        for configured in [false, true] {
            let mut options = options(&[]);
            assert!(verify_path(&later, options.broker_account).is_ok());
            if configured {
                options.configured_endpoint = Some(missing.clone());
            } else {
                options.endpoint = Some(missing.clone());
                options.configured_endpoint = Some(later.clone());
            }
            options.default_endpoints = vec![later.clone()];
            options.endpoint_only = true;
            options.require_native_session = false;
            options.allow_unverified_tcp_fallback = true;
            assert!(options.discover_by_ping);
            if configured {
                assert_eq!(
                    options.endpoint_candidates().unwrap_err().to_string(),
                    "a launch handoff requires the parent's broker endpoint"
                );
                continue;
            }
            assert_eq!(
                options.endpoint_candidates().unwrap(),
                vec![missing.clone()]
            );
            let error = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                NodedClient::connect_unix("", &url, &options, None),
            )
            .await
            .expect("a missing handoff endpoint must refuse immediately")
            .err()
            .unwrap();
            assert!(matches!(error.endpoint_cause(), ConnectError::Io(error)
                if error.raw_os_error() == Some(libc::ENOENT)));
            assert!(error.to_string().contains(&missing.display().to_string()));
        }
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        assert!(
            matches!(tcp.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        let mut options = options(&[]);
        options.endpoint_only = true;
        options.default_endpoints = vec![later];
        assert!(options.endpoint_candidates().is_err());
    }

    #[tokio::test]
    async fn existing_wrong_owner_socket_is_fatal_without_fallback() {
        let dir = TestDir::new();
        let path = dir.0.join("wrong-owner.sock");
        let _socket = UnixListener::bind(&path).unwrap();
        let mut options = options(std::slice::from_ref(&path));
        options.broker_account.uid = options.broker_account.uid.wrapping_add(1);
        options.require_native_session = false;
        options.allow_unverified_tcp_fallback = true;
        let error = NodedClient::connect_unix("", "ws://127.0.0.1:1/ws", &options, None)
            .await
            .err()
            .unwrap();
        assert!(matches!(
            error.endpoint_cause(),
            ConnectError::EndpointOwnership
        ));
        assert!(error.to_string().contains(&path.display().to_string()));
    }

    #[tokio::test]
    async fn symlinks_and_existing_non_sockets_are_fatal() {
        let dir = TestDir::new();
        let file = dir.0.join("file");
        std::fs::write(&file, "not a socket").unwrap();
        let alias = dir.0.join("alias");
        symlink(&file, &alias).unwrap();
        let broken = dir.0.join("broken");
        symlink(dir.0.join("missing"), &broken).unwrap();
        let later = dir.0.join("later.sock");
        let _socket = UnixListener::bind(&later).unwrap();
        for (path, symlink) in [(file, false), (alias, true), (broken.join("socket"), true)] {
            let mut options = options(&[path.clone(), later.clone()]);
            options.require_native_session = false;
            options.allow_unverified_tcp_fallback = true;
            let error = NodedClient::connect_unix("", "ws://127.0.0.1:1/ws", &options, None)
                .await
                .err()
                .unwrap();
            if symlink {
                assert!(matches!(
                    error.endpoint_cause(),
                    ConnectError::InvalidEndpoint
                ));
            } else {
                assert!(matches!(
                    error.endpoint_cause(),
                    ConnectError::EndpointOwnership
                ));
            }
            assert!(error.to_string().contains(&path.display().to_string()));
        }
    }

    #[tokio::test]
    async fn refused_existing_socket_does_not_skip_or_fall_back() {
        let dir = TestDir::new();
        let path = dir.0.join("stale.sock");
        let later = dir.0.join("later.sock");
        drop(UnixListener::bind(&path).unwrap());
        let _socket = UnixListener::bind(&later).unwrap();
        let mut options = options(&[path.clone(), later]);
        options.require_native_session = false;
        options.allow_unverified_tcp_fallback = true;
        let error = NodedClient::connect_unix("", "ws://127.0.0.1:1/ws", &options, None)
            .await
            .err()
            .unwrap();
        assert!(
            matches!(error.endpoint_cause(), ConnectError::Io(error) if error.raw_os_error() == Some(libc::ECONNREFUSED))
        );
        assert!(error.to_string().contains(&path.display().to_string()));
    }
    #[test]
    fn pinned_pid_requires_usable_matching_kernel_credential() {
        assert_eq!(verified_server_pid(Some(42), Some(42)).unwrap(), Some(42));
        for pid in [None, Some(-1), Some(0), Some(43)] {
            assert!(matches!(
                verified_server_pid(pid, Some(42)),
                Err(ConnectError::ServerPidMismatch)
            ));
        }
        // The opt-in does not tighten the previous UID/GID-only path.
        for pid in [None, Some(-1), Some(0)] {
            assert_eq!(verified_server_pid(pid, None).unwrap(), None);
        }
    }
    #[test]
    fn principal_duplicates_are_detected_before_compatibility_parse() {
        assert!(principal_header_is_unique(
            "---\nbus: 1\nbroker_principal: {}\n---\nbroker_principal: body"
        ));
        for key in ["broker_principal", "BROKER_PRINCIPAL", "Broker_Principal"] {
            assert!(!principal_header_is_unique(&format!(
                "---\nbroker_principal: {{}}\n{key}: {{}}\n---\n"
            )));
        }
        assert!(!principal_header_is_unique(
            "---\nBROKER_PRINCIPAL: {}\n---\n"
        ));
        assert!(!principal_header_is_unique(
            "---\n broker_principal: {}\n---\n"
        ));
        assert!(!principal_header_is_unique(
            "---\nbroker_principal: {}\nbroker_principal: {}"
        ));
        assert!(principal_header_is_unique(
            "---\nbus: 1\n---\nBROKER_PRINCIPAL: body"
        ));
    }
}
