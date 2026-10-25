// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Web service configuration, listeners and ACME issuance settings.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebdConfig {
    /// Unknown settings survive typed node configuration round trips.
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
    pub enabled: bool,
    pub port: u16,
    pub www_dir: String,
    /// Opt-in shared static asset installation, served under versioned
    /// `/_mixos/assets/` URLs. Never resolved from a user's XDG overrides.
    pub shared_assets_dir: Option<String>,
    /// Permit anonymous cross-origin font reads from the explicitly published sets.
    pub shared_assets_cross_origin: bool,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    /// Expected web FQDN(s) for the legacy top-level
    /// `tls_cert`/`tls_key` pair. Used as the SAN-coverage list when
    /// `validate_le_chain` runs on the legacy collapse, and as the
    /// host-routing key-set for the legacy `Arc<VhostState>`. Every
    /// entry must be covered by the leaf certificate's SANs or
    /// startup hard-fails. Non-empty is required whenever the legacy
    /// pair is configured; the resolve step rejects an empty value
    /// with a clear error message. **This is the *web* identity** —
    /// independent of `served_mail_domains` (the autoconfig
    /// admission gate). Phase 3 deprecates the legacy top-level pair
    /// under the SPEC 12 namespace and this field deprecates with
    /// it.
    pub tls_server_name: Vec<String>,
    /// Optional plain-HTTP listener address (e.g. `"0.0.0.0:80"`).
    ///
    /// When set, webd binds a *second* listener serving the NS 3.0
    /// `00-default-http` shape: `301 → https://$host$request_uri` for
    /// everything (mail-client autoconfig / ACME paths are carved out
    /// in a follow-up — see `_doc/planned/maild-autoconfig.md`).
    ///
    /// Deliberately a full `addr:port` string, **not** a `wg_ip`-derived
    /// port like `web_listen()`. Mail-client autoconfig is fetched from
    /// the public internet, so this listener must bind a publicly
    /// reachable address — deriving it from the WireGuard IP would make
    /// it unreachable and defeat the purpose. `None` (the default) means
    /// **no plain-HTTP listener at all**, preserving the WG-only bind
    /// posture (`feedback_wg_only_binding`) on every node that has not
    /// explicitly opted in. Opening :80 is an edge-node deployment
    /// decision, expressed here as an explicit per-node config value
    /// rather than a webd code default.
    pub http_listen: Option<String>,
    /// LE Subscriber-Agreement URL the operator has accepted for this
    /// node's `letsencrypt_prod` vhosts. Per the umbrella's Open Q6
    /// gate and `_doc/planned/webd-vhosts-phase2.md`, a node with **any**
    /// `provider = "letsencrypt_prod"` row must set this field to the
    /// URL of the Subscriber Agreement the operator actually read. The
    /// gate strips leading/trailing whitespace at mint time and stores
    /// the **canonical post-trim string** in
    /// `account.letsencrypt_prod.meta.json` for audit; whitespace-only
    /// edits to the operator's raw input are tolerated, visible URL
    /// edits (host, path, scheme) trip the subsequent-boot mismatch
    /// detector. Staging-only nodes skip the gate (LE staging has no
    /// Subscriber Agreement to record).
    ///
    /// `None` (the default) is the safe default: a node with no prod
    /// vhosts is unaffected, while a node that gains a prod vhost
    /// without setting this field hard-fails at resolve time with a
    /// pointer at <https://letsencrypt.org/repository/>. Validation
    /// (LE host prefix, non-empty, trim-tolerance) is enforced by
    /// the private ToS gate inside the [`crate::acme_policy`] module
    /// and surfaces only through [`crate::acme_policy::resolve_webd_acme`]
    /// — see that module for the structural-unforgeability rationale.
    pub acme_tos_accepted: Option<String>,
    /// Mail domains this node serves client autoconfig/autodiscover
    /// for. **This is the security gate**, not a convenience list: a
    /// `Host`-derived domain that is not a member is `404`ed *before*
    /// any DNS lookup, closing the SSRF / DNS-amplification vector
    /// inherent in resolving an attacker-controlled Host
    /// (`feedback_bus_wire_trust_boundary`, maild-autoconfig.md
    /// §Security). Empty (the default) means autoconfig is effectively
    /// disabled — every request 404s and no resolver is constructed.
    pub served_mail_domains: Vec<String>,
    /// Internal mail host to advertise (and probe) in autoconfig instead of
    /// the domain's public MX. This node exposes the mail client protocols
    /// (IMAPS:993 / SMTPS:465) only on the WireGuard interface — the public
    /// FQDN serves :25 + :443 only — so autoconfig points clients at this
    /// WG-reachable host (e.g. `epsilon.example.com`). `None` (default) ⇒ legacy
    /// behaviour: resolve + probe the domain's public MX.
    #[serde(default)]
    pub autoconfig_mail_host: Option<String>,
    /// Per-vhost configuration. Each row picks one of two issuance
    /// modes: **manual-PEM** (`tls_cert` + `tls_key`, validated by
    /// `daemon::tls::le_validator::validate_le_chain` at
    /// startup) or **ACME** (`acme = {...}`, provisioned by webd
    /// against Let's Encrypt). The two modes are mutually exclusive
    /// per row; `crate::acme_policy::resolve_webd_acme` rejects a row
    /// that sets both.
    ///
    /// Empty (the default) ⇒ webd serves the legacy top-level
    /// `tls_cert` + `tls_key` pair under every FQDN listed in
    /// `tls_server_name`. Non-empty ⇒ webd serves each listed
    /// vhost; the legacy fields, if also present, collapse to one
    /// resolver identity per `tls_server_name` entry, with only the
    /// first carrying `default = true, no_sni_fallback = true`.
    /// Duplicate FQDNs across the legacy collapse and any vhost row's
    /// primary/alias are caught by `SniCertResolver::from_config`.
    pub vhost: Vec<WebdVhostConfig>,
    /// Per-interface listeners (one `[[webd.listener]]` block each).
    ///
    /// **Empty (the default) ⇒ back-compat:** webd synthesizes one
    /// implicit WG listener at `web_listen()` (`wg_ip:port`) serving
    /// every vhost — byte-identical to the pre-listener single-bind
    /// behaviour, so existing nodes are untouched.
    ///
    /// Non-empty ⇒ webd binds exactly these listeners, each to an
    /// explicit interface IP with its own vhost allowlist (the
    /// "what is exposed where" block). This is how a node multi-homes
    /// a WG-only internal listener and a public external listener on
    /// the same daemon — the kernel never completes a handshake for
    /// an internal vhost off the mesh. See
    /// [`crate::node::NodeConfig::synthesize_listeners`] for the resolution +
    /// validation rules.
    #[serde(default)]
    pub listener: Vec<WebdListenerConfig>,
    /// Listener-control (SPEC-12 `webd.listeners` kill-switch + guard
    /// namespace) settings. Currently just the operator allowlist for
    /// the privileged `props.write:webd.listeners` capability (cutting
    /// external traffic / retuning guards is operator-tier, unlike the
    /// permissive every-WG-peer posture of `vhosts`/`accounts`).
    #[serde(default)]
    pub listeners: WebdListenersConfig,
    /// Operator-owned account roots. Invalid paths are skipped by webd at startup.
    #[serde(default)]
    pub shares: WebdSharesConfig,
}

/// Native config: `webd: { shares: { roots: { "user@example.test": "/srv/files/user" } } }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WebdSharesConfig {
    pub roots: std::collections::BTreeMap<String, PathBuf>,
}

/// `[webd.listeners]` — listener-control settings (the SPEC-12
/// `webd.listeners` namespace's L0 seed). The namespace itself holds
/// the live `enabled`/guard state; this block carries only the
/// bootstrap-time policy that can't live in L1 (it gates *who* may
/// write L1).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WebdListenersConfig {
    /// Bus sender names (`cmd.from`) allowed to `props.write` the
    /// `webd.listeners` namespace — i.e. flip the `enabled` kill switch
    /// or retune a listener's guards. Empty (the default) ⇒ no remote
    /// peer may write (the daemon's own backend-origin writes — bootstrap
    /// + the reaction loop's status writeback — still work). read/describe
    ///   stay open to every WG peer. Tightens further when the cross-mesh
    ///   principal model lands; the capability name is stable now.
    pub operators: Vec<String>,
}

impl Default for WebdConfig {
    fn default() -> Self {
        Self {
            extra: Default::default(),
            enabled: false,
            port: 443,
            www_dir: ::paths::root(::paths::Root::State).join("www").to_string_lossy().into_owned(),
            shared_assets_dir: None,
            shared_assets_cross_origin: false,
            tls_cert: None,
            tls_key: None,
            tls_server_name: Vec::new(),
            http_listen: None,
            acme_tos_accepted: None,
            served_mail_domains: Vec::new(),
            autoconfig_mail_host: None,
            vhost: Vec::new(),
            listener: Vec::new(),
            listeners: WebdListenersConfig::default(),
            shares: WebdSharesConfig::default(),
        }
    }
}

/// One `[[webd.listener]]` row — a per-interface listener with a vhost
/// allowlist. The bind is an **explicit** interface IP (e.g. the WG
/// address `192.0.2.5:443` or a public `203.0.113.5:443`), never a
/// `wg_ip`-derived port like `web_listen()`: binding a specific
/// interface is the kernel-level isolation boundary
/// (`feedback_wg_only_binding` — public is an explicit opt-in).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WebdListenerConfig {
    /// Stable id — the control-command key, log tag, and connection tag.
    /// Unique within the node's listener set; non-empty.
    pub id: String,
    /// Bind address `"ip:port"`. A specific interface IP, not a
    /// wildcard sharing a port with another listener (rejected by
    /// `synthesize_listeners`).
    pub bind: String,
    /// Public / untrusted-facing? Drives the `external` connection
    /// flag and the daemon's guard defaults. Does not itself change
    /// bind behaviour — the address does.
    pub external: bool,
    /// Bootstrap enable (the config-time on/off). The live kill
    /// switch is the SPEC-12 `webd.listeners` namespace (Phase 3); a
    /// disabled listener is held but not bound. Default `true`.
    pub enabled: bool,
    /// The vhost FQDNs served on this listener — the allowlist. Every
    /// vhost webd serves must be named in at least one **enabled**
    /// listener, and may be served by several listeners. Each host may
    /// appear only once here and must be a known vhost
    /// (`synthesize_listeners` rejects both gaps).
    pub vhosts: Vec<String>,
}

impl Default for WebdListenerConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            bind: String::new(),
            external: false,
            // A listener row that omits `enabled` is on — the
            // serde(default) fallback must not silently disable it.
            enabled: true,
            vhosts: Vec::new(),
        }
    }
}

/// A resolved webd listener — the output of
/// [`crate::node::NodeConfig::synthesize_listeners`]. Carries the bind address and
/// the (validated, complete) set of vhost hosts it serves. webd turns
/// each of these into a `daemon::listen::ListenerSpec` plus a
/// scoped `ListenerTls`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWebdListener {
    pub id: String,
    pub bind: String,
    pub external: bool,
    pub enabled: bool,
    /// Vhost FQDNs served here, in declaration order.
    pub hosts: Vec<String>,
}

/// Per-vhost configuration row, one per `[[webd.vhost]]` block in
/// `node.conf.mix`. Each row picks one of two issuance modes:
///
/// 1. **Manual-PEM** — `tls_cert` + `tls_key` are operator-supplied
///    paths to a Let's-Encrypt-issued chain that
///    `daemon::tls::le_validator::validate_le_chain` runs
///    against at startup. A non-LE chain (self-signed, internal-CA,
///    DigiCert, etc.) hard-fails the bind.
/// 2. **ACME** — `acme = {...}` declares provider + challenge +
///    contact_email; the webd provisioner obtains a Let's Encrypt
///    cert via the configured ACME directory.
///
/// The two modes are mutually exclusive per row; the lib-config
/// resolver (`crate::acme_policy::resolve_webd_acme`) rejects a row
/// that sets both.
///
/// NOTE: this struct deliberately does NOT use `deny_unknown_fields`
/// (unlike the stricter core structs). node.conf.mix is read by many
/// daemons of DIFFERENT versions, and webd-vhost fields are the
/// fastest-evolving part of it; a strict deny here made the whole
/// config unparseable for any daemon built from a branch older than the
/// newest field, dropping its broker resolution to loopback. Tolerating
/// unknown webd-vhost fields keeps the shared config forward-compatible.
/// Typos in a vhost row still surface at resolve time (a row with a bad
/// cert / missing www_dir / unopenable CMS DB is dropped).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WebdVhostConfig {
    /// Primary FQDN this vhost serves. LDH-validated against
    /// `daemon::http_host::parse_request_host` at startup.
    pub host: String,
    /// Optional alias hostnames that route to this same vhost. Each
    /// alias goes through the same LDH validation as `host`.
    pub aliases: Vec<String>,
    /// Filesystem path to this vhost's document root. Must exist at
    /// startup; webd hard-fails the bind otherwise.
    pub www_dir: String,
    /// Path to operator-supplied PEM cert chain (leaf +
    /// intermediates). Required in Phase 1; mutually exclusive with
    /// `acme` once Phase 2 lands. Validated by
    /// `daemon::tls::le_validator::validate_le_chain` at
    /// startup; a non-LE chain (self-signed, internal-CA, DigiCert,
    /// etc.) hard-fails the bind.
    pub tls_cert: Option<String>,
    /// Path to operator-supplied PEM private key matching
    /// `tls_cert`. Required if `tls_cert` is set.
    pub tls_key: Option<String>,
    /// Real ACME sub-config (Phase 2). Mutually exclusive with
    /// `tls_cert` / `tls_key`; `crate::acme_policy::resolve_webd_acme`
    /// rejects a row that sets both. `None` (the default) means this
    /// vhost runs in manual-PEM mode (Phase 1 path); `Some(_)` means
    /// the webd provisioner is expected to obtain a Let's Encrypt
    /// cert via ACME for this row.
    pub acme: Option<WebdVhostAcmeConfig>,
    /// Per-vhost CMS SQLite path. Absent ⇒ this vhost has no posts
    /// surface; the `/api/posts*` routes 404 for this host.
    pub cms_db_path: Option<String>,
    /// Auxiliary SQLite databases ATTACHed onto this vhost's connection at
    /// open, each under its own schema name. Lets a route family keep its
    /// state in a database with an INDEPENDENT lifecycle from `cms_db_path`
    /// (e.g. an admin panel whose data must survive a cms.db re-clone). Each
    /// entry is `{ name, path }`; `name` is a validated schema identifier
    /// (`[a-z_][a-z0-9_]{0,31}`, never `main`/`temp`) referenced as
    /// `<name>.<table>` in SQL. Access to an attached schema is NOT granted by
    /// the plain `db` capability — a route must additionally hold the matching
    /// `db-schema:<name>` capability (enforced by a per-statement SQLite
    /// authorizer), so co-hosted handlers on the shared connection cannot read
    /// or write another family's schema. Absent/empty ⇒ no aux databases.
    pub aux_dbs: Vec<WebdAuxDb>,
    /// Per-vhost JMAP upstream URL. Absent ⇒ this vhost has no mail
    /// surface; the `/jmap` + `/jmap/{*rest}` routes 404 for this host.
    pub jmap_upstream: Option<String>,
    /// Per-vhost noded WebSocket URL. Absent ⇒ this vhost has no Bus
    /// bridge surface.
    pub noded_ws: Option<String>,
    /// Per-vhost documentation directory. Absent ⇒ this vhost has
    /// no `/docs/` route.
    pub docs_dir: Option<String>,
    /// DEV-ONLY auto-session identity (email + password — set BOTH or neither).
    /// When set, a request to this vhost that carries no valid `mixos_session`
    /// cookie is treated as this maild identity — `$SESSION.authenticated` is
    /// true and the `jmap()` seam authenticates to maild via HTTP Basic.
    /// Honoured ONLY on an internal listener: the hard gate is per-request
    /// (`!scope.external`), and webd additionally refuses at startup to bind a
    /// dev_session vhost unless every serving listener is `external = false` AND
    /// bound to an internal (loopback/RFC1918/ULA/link-local) address. For a
    /// sealed WG dev box; NEVER set on a public vhost.
    pub dev_session_email: Option<String>,
    /// `skip_serializing` so a secret can never round-trip into a serialized
    /// config dump; also redacted by the manual `Debug` below. (Deserialization
    /// from `node.conf.mix` is unaffected.)
    #[serde(skip_serializing)]
    pub dev_session_password: Option<String>,
    /// PUBLIC read-only content credential (email + password — set BOTH or
    /// neither). When set, an ANONYMOUS request to this vhost (no session cookie,
    /// no dev_session) has its `jmap()` seam authenticate to maild via HTTP Basic
    /// as this identity — but it grants NO session (`$SESSION.authenticated`
    /// stays false: no admin, no manage). Intended for a DEDICATED content
    /// account (vtoken segment-1) holding only public-intended content, so the
    /// public read is safe by construction. UNLIKE `dev_session`, this is NOT
    /// internal-gated — it is the public `:443` blog-read path. The credential
    /// is consumed ONLY by routes that opt in via the `public_read` capability
    /// (so it is bounded to audited routes, not vhost-wide across every `jmap`
    /// handler). (A maild-enforced scoped read-only principal is the future
    /// hardening; for now the dedicated-account boundary + the route opt-in +
    /// the handler's redaction allowlist + live predicate are the guarantees.)
    pub public_read_email: Option<String>,
    /// `skip_serializing` + redacted in `Debug` (mirrors `dev_session_password`).
    #[serde(skip_serializing)]
    pub public_read_password: Option<String>,
    /// SYSTEM transactional-sender credential (email + password — set BOTH or
    /// neither). When set, webd can send transactional mail (2FA codes,
    /// registration confirm/approval links) FROM this account with **no user
    /// session** — the pre-auth send path. The account is a real maild account
    /// on this vhost's own mail domain (e.g. `noreply@<domain>`); maild's
    /// RFC 6409 §6.1 envelope-sender check means it can only send AS itself, so
    /// the `From` is pinned to this address. Consumed ONLY by webd's
    /// `send_system_mail` (never a vhost-wide `jmap` credential like
    /// `public_read`). NOT internal-gated — transactional mail is a production
    /// surface — so guard the callers behind rate-limiting (P0c) before exposing
    /// any public trigger.
    pub system_sender_email: Option<String>,
    /// `skip_serializing` + redacted in `Debug` (mirrors `public_read_password`).
    #[serde(skip_serializing)]
    pub system_sender_password: Option<String>,
    /// Email-2FA operator BREAK-GLASS (default `false`). The 2FA
    /// enrollment lookup fails CLOSED: an INDETERMINATE read (broker
    /// down, timeout, transport error) refuses the login outright, so a
    /// broker fault can't be induced to skip a user's second factor —
    /// but that also makes the broker a login-availability SPOF for the
    /// vhost. Setting this to `true` lets an indeterminate lookup
    /// proceed password-only, logged loudly per bypassed login. It is
    /// an EXPLICIT, AUDITED operator action for a confirmed outage
    /// window (root-owned config edit + webd restart — never a silent
    /// fallback); remove it as soon as the broker is healthy.
    /// Determinate reads (2FA on / off / no props row) are never
    /// affected.
    pub mfa_break_glass: bool,
}

impl std::fmt::Debug for WebdVhostConfig {
    // Hand-rolled (Debug dropped from the derive above) so `dev_session_password`
    // can never render in cleartext via a `?`-format of a parsed config.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebdVhostConfig")
            .field("host", &self.host)
            .field("aliases", &self.aliases)
            .field("www_dir", &self.www_dir)
            .field("tls_cert", &self.tls_cert)
            .field("tls_key", &self.tls_key)
            .field("acme", &self.acme)
            .field("cms_db_path", &self.cms_db_path)
            .field("aux_dbs", &self.aux_dbs)
            .field("jmap_upstream", &self.jmap_upstream)
            .field("noded_ws", &self.noded_ws)
            .field("docs_dir", &self.docs_dir)
            .field("dev_session_email", &self.dev_session_email)
            .field(
                "dev_session_password",
                &self.dev_session_password.as_ref().map(|_| "<redacted>"),
            )
            .field("public_read_email", &self.public_read_email)
            .field(
                "public_read_password",
                &self.public_read_password.as_ref().map(|_| "<redacted>"),
            )
            .field("system_sender_email", &self.system_sender_email)
            .field(
                "system_sender_password",
                &self.system_sender_password.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// One auxiliary SQLite database ATTACHed to a vhost connection
/// (`[[webd.vhost.aux_dbs]]` in `node.conf.mix`).
///
/// `deny_unknown_fields` so a typo'd key is a loud config error rather than a
/// silently-ignored aux database. Validation of `name` (schema identifier) and
/// `path` (absolute, distinct from cms/legacy paths) happens at open time in
/// webd, not here — a bad row fails that one vhost soft, never the whole node.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebdAuxDb {
    /// Schema name the database is ATTACHed as; referenced `<name>.<table>` in
    /// SQL and gated by the `db-schema:<name>` route capability.
    pub name: String,
    /// Filesystem path to the SQLite file. Created (with parent dirs) at open
    /// if absent, opened WAL + `synchronous=NORMAL`.
    pub path: String,
}

/// Per-vhost ACME sub-config (`[webd.vhost.acme]` in `node.conf.mix`).
///
/// All three fields are required — the resolve step rejects a row
/// with `provider`/`contact_email` omitted at serde time
/// (`deny_unknown_fields` does the reverse direction; the
/// `serde(default)` shape would silently parse a half-filled block).
/// `challenge` defaults to `Http01` so the common case is one line of
/// config; an explicit `dns01` value parses successfully but is
/// rejected by the resolver with a Phase 4 pointer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebdVhostAcmeConfig {
    /// Which ACME directory to talk to. Closed enum — the umbrella's
    /// "Let's Encrypt only" directive treats adding a non-LE provider
    /// as a normative-doc change, not an API-compat extension.
    pub provider: WebdAcmeProvider,
    /// Which challenge type to satisfy. Defaults to HTTP-01 (the only
    /// Phase 2 implemented variant); a configured `dns01` parses but
    /// is rejected by `crate::acme_policy::resolve_webd_acme` with a
    /// pointer at Phase 4.
    #[serde(default = "default_challenge")]
    pub challenge: WebdAcmeChallenge,
    /// Contact email registered with the ACME directory. Used by LE
    /// for expiration warnings; not exposed to challenge responders.
    /// Sanity-validated (must contain `@`, no CR/LF, ≤ 253 bytes) by
    /// the resolver — RFC-5322 conformance is the operator's job.
    pub contact_email: String,
}

fn default_challenge() -> WebdAcmeChallenge {
    WebdAcmeChallenge::Http01
}

/// ACME provider selection. Closed (`#[non_exhaustive]` deliberately
/// omitted) so a hypothetical fork adding a non-LE provider is forced
/// to confront every match site that today exhaustively enumerates
/// `LetsEncryptProd` and `LetsEncryptStaging`.
///
/// The wire spelling is pinned per-variant rather than via
/// `rename_all = "snake_case"` because the operator-facing form is
/// `"letsencrypt_prod"` / `"letsencrypt_staging"` (no underscore
/// between *lets* and *encrypt*) — it matches Let's Encrypt's own
/// branding, every reference in `_doc/planned/webd-vhosts*.md`, every
/// in-source comment in `mixos-lib-daemon::tls`, and the meta-file
/// names (`account.letsencrypt_prod.meta.json`) the Phase 2 plan
/// reserves on disk. `rename_all = "snake_case"` would derive
/// `"lets_encrypt_*"`, splitting the wire form away from the docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebdAcmeProvider {
    /// Let's Encrypt **production** directory. Subject to the LE
    /// Subscriber-Agreement gate (umbrella Open Q6) — a node with
    /// any `LetsEncryptProd` row must also set `[webd]
    /// acme_tos_accepted` or the resolver hard-fails at startup.
    #[serde(rename = "letsencrypt_prod")]
    LetsEncryptProd,
    /// Let's Encrypt **staging** directory. Untrusted certs intended
    /// for end-to-end issuance testing without burning prod
    /// rate-limit slots. No ToS gate (LE staging accepts any
    /// account by design).
    #[serde(rename = "letsencrypt_staging")]
    LetsEncryptStaging,
}

/// ACME challenge type selection. `Dns01` is reserved (parses
/// successfully) but the resolver rejects it with a Phase 4 pointer —
/// DNS-01 needs mixos-dnsd's RFC-2136-style update surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebdAcmeChallenge {
    /// HTTP-01: webd publishes the keyauth at
    /// `http://<fqdn>/.well-known/acme-challenge/<token>` from a
    /// shared in-memory map populated by the provisioner.
    Http01,
    /// DNS-01: reserved for Phase 4. Parses successfully so the
    /// resolver can produce a clean Phase 4 pointer rather than a
    /// serde "unknown variant" error.
    Dns01,
}
