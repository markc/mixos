// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! One serialized authority, bounded native input, blocking work off Tokio.
//!
//! The same loop serves both roles (F1, design §5.4). The authority
//! commits, publishes its topics and pushes each revision to its attached
//! mirrors. A mirror serves reads from its cache, publishes the same topics
//! on its own node, forwards writes to the authority and relays its
//! receipts. Everything is driven by events: a commit, a push, a broker
//! (re)connect, a forwarded write's outcome. Nothing polls.
use crate::authority::{Authority, Role};
use crate::mirroring::{self, AttachRequest, Attached, RevisionPush};
use crate::store::Store;
use bus::native_client::{BoundedIncomingEvent, ConnState, IncomingCommand, SupervisedClient};
use serde::Deserialize;
use serde_json::{Value, json};
use settings::*;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio::time::Instant;

pub const COMMANDS: &[&str] = &[
    "settings.describe",
    "settings.get",
    "settings.validate",
    "settings.apply",
    "settings.reset",
    "settings.status",
    "settings.source",
    "settings.palette",
    MIRROR_ATTACH,
    MIRROR_REVISION,
    // S7-TRANSITION: portald's read until it moves to settings.palette.
    "settings.appearance.get",
];
pub const MIRROR_ATTACH: &str = "settings.mirror.attach";
pub const MIRROR_REVISION: &str = "settings.mirror.revision";
/// The writes a mirror forwards to its authority.
const WRITES: [&str; 3] = ["settings.validate", "settings.apply", "settings.reset"];

pub fn manifest() -> Vec<bus::CommandDescriptor> {
    std::iter::once(bus::CommandDescriptor::new(
        "HELP",
        &[],
        "List served commands",
        true,
    ))
    .chain(COMMANDS.iter().map(|command_name| {
        bus::CommandDescriptor::new(
            command_name,
            &["body"],
            &format!("Desktop settings contract {}", settings::CONTRACT_VERSION),
            !matches!(
                *command_name,
                "settings.apply" | "settings.reset" | MIRROR_ATTACH | MIRROR_REVISION
            ),
        )
    }))
    .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusRequest {
    binding: Binding,
    #[serde(default)]
    operation_id: Option<String>,
}
fn decode<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, Value> {
    if body.len() > 384 * 1024 {
        return Err(json!({"status":"validation_failed","message":"Request byte limit exceeded"}));
    }
    serde_json::from_str(body)
        .map_err(|e| json!({"status":"validation_failed","message":e.to_string()}))
}
/// The registry's catalogue, or the keys under `{prefix}`, with the mirror
/// commands under `mirror`.
fn describe(body: &str) -> Result<Value, Value> {
    #[derive(Deserialize, Default)]
    #[serde(deny_unknown_fields)]
    struct DescribeRequest {
        #[serde(default)]
        prefix: Option<String>,
    }
    let request: DescribeRequest = if body.trim().is_empty() {
        DescribeRequest::default()
    } else {
        decode(body)?
    };
    let mirror = json!({
        MIRROR_ATTACH: {"from": "mirror", "sender": "the broker-stamped peer node equals body.node",
            "body": {"binding": "the authority's", "node": "the mirror's node", "revision": "the revision it holds"},
            "answer": "the push, so the mirror catches up in the same round trip"},
        MIRROR_REVISION: {"from": "authority", "sender": "the broker-stamped settingsd on the mirror's --mirror-of node",
            "body": {"binding": "", "authority": "its node", "cache": "the sealed record", "changed": "prefixes (informational)", "changed_at": "per topic", "stopping": "bool"}},
    });
    // The mirror commands are no registry prefix: they come with the whole
    // catalogue, or alone for `{prefix: "mirror"}`.
    if request.prefix.as_deref() == Some("mirror") {
        return Ok(json!({"version": settings::CONTRACT_VERSION, "mirror": mirror}));
    }
    let mut described = settings::registry::describe(request.prefix.as_deref());
    described["version"] = settings::CONTRACT_VERSION.into();
    if request.prefix.is_none() {
        described["mirror"] = mirror;
    }
    Ok(described)
}
pub fn dispatch(authority: &mut Authority, command_name: &str, body: &str) -> Result<Value, Value> {
    match command_name {
        "HELP" => Ok(json!(manifest())),
        "settings.describe" => describe(body),
        "settings.get" => authority.read(decode(body)?),
        "settings.appearance.get" => authority.appearance(decode(body)?),
        "settings.source" => authority.source(&decode(body)?),
        "settings.palette" => authority.palette(&decode(body)?),
        "settings.status" => {
            let req: StatusRequest = decode(body)?;
            authority.status(&req.binding, req.operation_id.as_deref())
        }
        "settings.validate" => authority.validate(&decode(body)?),
        "settings.apply" => authority.apply(decode(body)?),
        "settings.reset" => {
            let req: ApplyRequest = decode(body)?;
            if !req.changes.is_empty() {
                return Err(
                    json!({"status":"validation_failed","message":"Reset accepts only reset paths"}),
                );
            }
            authority.apply(req)
        }
        _ => Err(json!({
            "error_code": "UNKNOWN_COMMAND",
            "message": format!("unknown command {command_name}"),
            "command": command_name,
        })),
    }
}

/// S7-TRANSITION: what contract 1.0.0 keeps of 0.8.0 so every follower not
/// yet on the 1.0.0 topics and views keeps working while the stack lands
/// daily. Deleted in the landing that brings S8a-c, before S9; `grep -rn
/// S7-TRANSITION` finds every piece. [`tests::the_transition_keeps_exactly`]
/// lists it.
pub const TRANSITION_TOPIC_PREFIXES: [&str; 3] = [
    settings::TOPIC_PREFIX,
    settings::COMPACT_TOPIC_PREFIX,
    settings::SUMMARY_TOPIC_PREFIX,
];
/// S7-TRANSITION: the 0.8.0 commands kept beside 1.0.0's.
pub const TRANSITION_COMMANDS: [&str; 1] = ["settings.appearance.get"];
/// S7-TRANSITION: the 0.8.0 `settings.get` views kept; `full` is still the
/// default when `view` is omitted.
pub const TRANSITION_VIEWS: [View; 2] = [View::Full, View::Summary];

/// S7-TRANSITION: the format-4 snapshot in schema 1 and 2 and the summary,
/// on [`TRANSITION_TOPIC_PREFIXES`].
async fn publish_legacy(
    authority: &mut Authority,
    client: &SupervisedClient,
) -> anyhow::Result<()> {
    let profile = &authority.accepted.binding.profile;
    let [full, compact, summary] = TRANSITION_TOPIC_PREFIXES.map(|p| format!("{p}{profile}"));
    let snapshot = &authority.snapshot;
    publish_retained(client, full, serde_json::to_string(snapshot)?).await?;
    let encoded = serde_json::to_string(&settings::compact::encode(snapshot)?)?;
    publish_retained(client, compact, encoded).await?;
    let summarised = serde_json::to_string(&settings::Summary::of(snapshot))?;
    publish_retained(client, summary, summarised).await?;
    authority.published = Some(authority.accepted.revision);
    Ok(())
}

/// Publish every pending 1.0.0 topic, then the transitional ones. Each topic
/// is marked published as it goes; a failure, including a body that cannot
/// be built, leaves it and the rest pending for the bounded retry. The
/// transitional ones set `authority.published` ([`publish_legacy`]), which
/// with an empty board is what `publication_pending: false` reports.
async fn publish(authority: &mut Authority, client: &SupervisedClient) -> anyhow::Result<()> {
    for key in authority.board.pending() {
        let (topic, body) = authority
            .topic(&key)
            .ok_or_else(|| anyhow::anyhow!("topic {key} has no body"))?;
        publish_retained(client, topic, body).await?;
        authority.board.published(&key);
    }
    if authority.published != Some(authority.accepted.revision) {
        publish_legacy(authority, client).await?;
    }
    Ok(())
}

async fn publish_retained(
    client: &SupervisedClient,
    topic: String,
    body: String,
) -> anyhow::Result<()> {
    let mut inner = bus::wire::BusMessage::new();
    inner.set("command", &topic);
    inner.body = body;
    let headers = BTreeMap::from([("name".into(), topic), ("retain".into(), "true".into())]);
    tokio::time::timeout(
        Duration::from_secs(3),
        client.call_with_headers("noded", "topic.publish", &headers, &inner.to_wire()),
    )
    .await??;
    Ok(())
}

/// One bounded retry job exists only while publication is pending. No idle
/// interval or heartbeat. A new revision/connection starts a fresh attempt set.
#[derive(Default)]
struct PublicationRetry {
    failures: u8,
    deadline: Option<Instant>,
}
impl PublicationRetry {
    fn complete(&mut self, success: bool) {
        if success {
            *self = Self::default();
            return;
        }
        self.failures = self.failures.saturating_add(1);
        self.deadline = Some(Instant::now() + mirroring::backoff(self.failures));
    }
    async fn wait(&self) {
        match self.deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending::<()>().await,
        }
    }
}

/// What a spawned exchange reports back to the loop.
enum Event {
    /// A push to a mirror finished.
    Pushed {
        node: String,
        revision: Revision,
        ok: bool,
    },
    /// A mirror's attach answered (the push it carries) or failed, on
    /// the broker connection `generation` it was sent on.
    Attached {
        generation: u64,
        outcome: Result<Box<RevisionPush>, String>,
    },
    /// A forwarded write reached the authority, or could not.
    Forwarded { reached: bool },
}

/// The loop's state beside the authority itself.
struct Ctx {
    client: Arc<SupervisedClient>,
    root: PathBuf,
    events: mpsc::UnboundedSender<Event>,
    /// This settingsd's node, from the broker on each connect.
    node: Option<String>,
    /// The authority's attached mirrors (empty on a mirror).
    mirrors: Attached,
    retry: PublicationRetry,
    /// A mirror's attach is in flight.
    attaching: bool,
}

impl Ctx {
    async fn publish(&mut self, authority: &mut Authority) {
        if !self.client.is_connected() || authority.recovering() {
            return;
        }
        // A mirror attaching holds its topics until the answer, so a
        // reconnect publishes the authority's revision, not the cache's.
        if self.attaching {
            return;
        }
        if authority.board.is_current() && authority.published == Some(authority.accepted.revision)
        {
            return;
        }
        tracing::debug!(revision = authority.accepted.revision.0, pending = ?authority.board.pending(), "publishing");
        let result = publish(authority, &self.client).await;
        self.retry.complete(result.is_ok());
        if let Err(error) = result {
            tracing::warn!(%error, failures=self.retry.failures, "settings publication pending");
        }
    }

    /// This settingsd's node, asked again of the broker while unknown (the
    /// answer on connect can fail while the connection itself is fine).
    async fn own_node(&mut self) -> Option<String> {
        if self.node.is_none() && self.client.is_connected() {
            self.node = node_name(&self.client).await;
        }
        self.node.clone()
    }

    /// Push the current revision to every mirror that is behind it.
    async fn push_due(&mut self, authority: &Authority) {
        if !self.client.is_connected() || !matches!(authority.role, Role::Authority(_)) {
            return;
        }
        let Some(own) = self.own_node().await else {
            return;
        };
        let revision = authority.accepted.revision;
        for (node, since) in self.mirrors.due(revision, Instant::now()) {
            let body = push_of(authority, &own, since).and_then(|p| Ok(serde_json::to_string(&p)?));
            let body = match body {
                Ok(body) => body,
                Err(error) => {
                    tracing::warn!(%error, node, "mirror push not built");
                    self.mirrors
                        .finished(&node, revision, false, Instant::now());
                    continue;
                }
            };
            let client = self.client.clone();
            let events = self.events.clone();
            tokio::spawn(async move {
                let reply = client
                    .call_with_headers_raw_within(
                        &mirroring::address(&node),
                        MIRROR_REVISION,
                        &BTreeMap::new(),
                        &body,
                        mirroring::PUSH_TIMEOUT,
                    )
                    .await;
                let ok = matches!(reply, Ok((0, _, _)));
                if !ok {
                    tracing::warn!(node, ?reply, "mirror push failed");
                }
                let _ = events.send(Event::Pushed { node, revision, ok });
            });
        }
    }

    /// A mirror attaches to its authority (on each connect, and when a
    /// forwarded write finds a detached authority back).
    async fn attach(&mut self, authority: &Authority) {
        let Role::Mirror {
            node: authority_node,
        } = &authority.role
        else {
            return;
        };
        if self.attaching || !self.client.is_connected() {
            return;
        }
        let Some(own) = self.own_node().await else {
            tracing::warn!("mirror node unknown; attach waits for the next connect or forward");
            return;
        };
        self.attaching = true;
        let request = AttachRequest {
            binding: authority.accepted.binding.clone(),
            node: own,
            revision: authority.accepted.revision,
        };
        let body = serde_json::to_string(&request).unwrap_or_default();
        let to = mirroring::address(authority_node);
        let client = self.client.clone();
        let events = self.events.clone();
        let generation = client.connection_generation();
        tokio::spawn(async move {
            let reply = client
                .call_with_headers_raw_within(
                    &to,
                    MIRROR_ATTACH,
                    &BTreeMap::new(),
                    &body,
                    mirroring::PUSH_TIMEOUT,
                )
                .await;
            let outcome = match reply {
                Ok((0, body, _)) => serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| serde_json::from_value(v.get("push")?.clone()).ok())
                    .ok_or_else(|| format!("attach answer without a push: {body}")),
                Ok((rc, body, error)) => Err(format!("rc {rc}: {body} {error:?}")),
                Err(error) => Err(error.to_string()),
            };
            let outcome = outcome.map(Box::new);
            let _ = events.send(Event::Attached {
                generation,
                outcome,
            });
        });
    }

    /// Forward a write to the authority and relay its answer; refuse it with
    /// `authority_unreachable` when the authority cannot be reached.
    fn forward(&self, authority: &Authority, command: &IncomingCommand) {
        let Role::Mirror { node } = &authority.role else {
            return;
        };
        let (client, events, node) = (self.client.clone(), self.events.clone(), node.clone());
        let (generation, from, name, id, body) = (
            command.generation,
            command.from.clone(),
            command.command.clone(),
            command.id.clone(),
            command.body.clone(),
        );
        tokio::spawn(async move {
            let reply = client
                .call_with_headers_raw_within(
                    &mirroring::address(&node),
                    &name,
                    &BTreeMap::new(),
                    &body,
                    mirroring::FORWARD_TIMEOUT,
                )
                .await;
            let (rc, answer, reached) = match reply {
                Ok((rc, answer, _)) if mirroring::from_authority(rc, &answer) => (rc, answer, true),
                other => {
                    tracing::warn!(node, reply=?other, "write not forwarded; authority unreachable");
                    (10, mirroring::unreachable(&node).to_string(), false)
                }
            };
            if let Err(error) = client
                .respond_parts(generation, &from, &name, id.as_deref(), rc, &answer)
                .await
            {
                tracing::warn!(%error, "forwarded reply unavailable; the authority holds the receipt");
            }
            let _ = events.send(Event::Forwarded { reached });
        });
    }
}

/// The push of `authority`'s current record to a mirror that holds `since`.
fn push_of(
    authority: &Authority,
    own: &str,
    since: Option<Revision>,
) -> anyhow::Result<RevisionPush> {
    let changed_at = authority.board.changed_at();
    let changed = changed_at
        .iter()
        .filter(|(key, _)| settings::topics::PREFIXES.contains(&key.as_str()))
        .filter(|(_, at)| since.is_none_or(|since| **at > since))
        .map(|(key, _)| key.clone())
        .collect();
    Ok(RevisionPush {
        binding: authority.accepted.binding.clone(),
        authority: own.to_owned(),
        cache: Some(authority.cache(own)?),
        changed,
        changed_at,
        stopping: false,
    })
}

/// `settings.mirror.attach` at the authority: attach the mirror and answer
/// with the push, so it catches up in the same round trip. Only the node the
/// request names may attach it (the broker's stamp, [`mirroring::Sender`]),
/// into a capped table. The mirror is recorded at the revision it says it
/// holds, and the caller pushes after answering, so a lost answer heals on
/// that push.
async fn attach(
    authority: &Authority,
    ctx: &mut Ctx,
    sender: mirroring::Sender<'_>,
    body: &str,
) -> Result<Value, Value> {
    if !matches!(authority.role, Role::Authority(_)) {
        return Err(json!({"status":"wrong_role","role":"mirror"}));
    }
    let request: AttachRequest = decode(body)?;
    if request.binding != authority.accepted.binding {
        return Err(json!({"status":"wrong_target","binding":authority.accepted.binding}));
    }
    if !mirroring::valid_node(&request.node) {
        return Err(json!({"status":"validation_failed","path":"node"}));
    }
    if !sender.is_node(&request.node) {
        return Err(mirroring::wrong_sender(&sender, &request.node));
    }
    let own = ctx
        .own_node()
        .await
        .ok_or_else(|| json!({"status":"not_ready","message":"Broker node not known yet"}))?;
    let push = push_of(authority, &own, Some(request.revision))
        .map_err(|e| json!({"status":"storage_failed","message":e.to_string()}))?;
    if !ctx.mirrors.attach(&request.node, request.revision) {
        return Err(json!({"status":"mirror_limit","maximum":mirroring::MAX_MIRRORS}));
    }
    Ok(json!({"status":"attached","revision":authority.accepted.revision,"push":push}))
}

/// A push at a mirror, from its authority (the caller checked the sender):
/// verify and cache the record, then take it. A refused push leaves the
/// cache and marks the mirror stale: it knows it is behind.
async fn adopt(
    authority: Authority,
    ctx: &Ctx,
    push: RevisionPush,
) -> (Authority, Result<Value, Value>) {
    let (mut authority, result) = take(authority, ctx, push).await;
    if result.is_err() && matches!(authority.role, Role::Mirror { .. }) {
        authority.stale = true;
    }
    (authority, result)
}

async fn take(
    authority: Authority,
    ctx: &Ctx,
    push: RevisionPush,
) -> (Authority, Result<Value, Value>) {
    let Role::Mirror { node } = &authority.role else {
        return (
            authority,
            Err(json!({"status":"wrong_role","role":"authority"})),
        );
    };
    if push.binding != authority.accepted.binding || push.authority != *node {
        let binding = authority.accepted.binding.clone();
        return (
            authority,
            Err(json!({"status":"wrong_target","binding":binding})),
        );
    }
    let Some(cache) = push.cache else {
        return (
            authority,
            Err(json!({"status":"validation_failed","path":"cache"})),
        );
    };
    let held = (
        authority.accepted.incarnation.as_str(),
        authority.accepted.revision,
    );
    let next = (cache.incarnation.as_str(), cache.revision);
    if !mirroring::supersedes(held, next) {
        let (incarnation, revision) = (held.0.to_owned(), held.1);
        // An earlier revision of what the mirror holds is a reordered
        // push: acknowledged, ignored. An older incarnation is refused.
        if next.0 == held.0 {
            return (
                authority,
                Ok(json!({"status":"current","revision":revision})),
            );
        }
        return (
            authority,
            Err(
                json!({"status":"older_incarnation","incarnation":incarnation,"revision":revision}),
            ),
        );
    }
    let root = ctx.root.clone();
    let (changed, changed_at) = (push.changed, push.changed_at);
    // Checked against the mirror's own binding and authority, as at open.
    let (binding, node) = (authority.accepted.binding.clone(), push.authority);
    let result = tokio::task::spawn_blocking(move || {
        let mut authority = authority;
        // A panic on a pushed record refuses the push; it never takes the
        // mirror down. `adopt` assigns only once the record has resolved.
        let taken = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::mirror::write(&root, &binding, &node, &cache)
                .and_then(|()| authority.adopt(&cache, &changed_at))
        }))
        .unwrap_or_else(|panic| {
            let what = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(anyhow::anyhow!(
                "adopting the pushed record panicked: {what}"
            ))
        });
        (authority, taken)
    })
    .await
    .expect("the adoption job catches its own panics");
    match result {
        (mut authority, Ok(moved)) => {
            authority.stale = false;
            let revision = authority.accepted.revision;
            tracing::info!(
                revision = revision.0,
                ?moved,
                ?changed,
                "mirror took the authority's revision"
            );
            (
                authority,
                Ok(json!({"status":"current","revision":revision})),
            )
        }
        (authority, Err(error)) => {
            tracing::warn!(%error, "pushed record refused; the cache is kept");
            (
                authority,
                Err(json!({"status":"validation_failed","message":error.to_string()})),
            )
        }
    }
}

/// Serve as the machine's authority, or with `mirror_of` as a mirror of
/// that node's authority (F1).
pub async fn serve(
    root: PathBuf,
    binding: Binding,
    mirror_of: Option<String>,
) -> anyhow::Result<()> {
    let url = bus::client_helpers::resolve_noded_url();
    match mirror_of {
        None => serve_at(root, binding, url).await,
        Some(node) => serve_mirror_at(root, binding, node, url).await,
    }
}

/// The production authority loop with an explicit broker endpoint. This also
/// lets native integration tests isolate the broker without process-global env.
pub async fn serve_at(root: PathBuf, binding: Binding, noded_url: String) -> anyhow::Result<()> {
    let opened = root.clone();
    let authority = tokio::task::spawn_blocking(move || {
        let (store, accepted) = Store::open(&opened, &binding)?;
        Authority::new(store, accepted)
    })
    .await??;
    run(authority, root, noded_url).await
}

/// A mirror of `node`'s authority, serving its sealed cache. With no cache
/// that verifies, the first comes from the authority (`settings.mirror.attach`
/// answers with the record); with one, it is served stale until the attach
/// on connect brings it current.
pub async fn serve_mirror_at(
    root: PathBuf,
    binding: Binding,
    node: String,
    noded_url: String,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        mirroring::valid_node(&node),
        "invalid authority node {node:?}"
    );
    let (opened, url) = (root.clone(), noded_url.clone());
    let runtime = tokio::runtime::Handle::current();
    let authority = tokio::task::spawn_blocking(move || {
        let (cache, origin) = crate::mirror::open(&opened, &binding, &node, || {
            runtime
                .block_on(first_cache(&binding, &node, &url))
                .map_err(|e| anyhow::anyhow!("the authority on {node} cannot be reached: {e}"))
        })?;
        tracing::info!(
            ?origin,
            revision = cache.revision.0,
            "serving the mirror cache"
        );
        Authority::mirror(&cache)
    })
    .await??;
    run(authority, root, noded_url).await
}

/// The authority's record for a mirror starting with no cache: an attach
/// over a short-lived connection, before the cache is opened. A mirror with
/// a cache that verifies never uses it; the attach on connect catches it
/// up. Only called by [`crate::mirror::open`] when it needs a first cache.
async fn first_cache(
    binding: &Binding,
    node: &str,
    noded_url: &str,
) -> anyhow::Result<crate::mirror::Cache> {
    // The connect is inside the bound too: a black-holing broker must not
    // hold the mirror's start past it.
    let client = tokio::time::timeout(
        mirroring::PUSH_TIMEOUT,
        bus::native_client::NodedClient::connect_anonymous(noded_url),
    )
    .await
    .map_err(|_| anyhow::anyhow!("broker connect timed out"))??;
    let fetched = async {
        let info = client.call("noded", "noded.info", json!({})).await?;
        let own = info["node"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("broker node name unavailable"))?;
        let request = AttachRequest {
            binding: binding.clone(),
            node: own.to_owned(),
            revision: Revision(0),
        };
        let (rc, body, error) = client
            .call_with_headers_raw(
                &mirroring::address(node),
                MIRROR_ATTACH,
                &BTreeMap::new(),
                &serde_json::to_string(&request)?,
            )
            .await?;
        anyhow::ensure!(rc == 0, "rc {rc}: {body} {error:?}");
        let answer: Value = serde_json::from_str(&body)?;
        let push: RevisionPush = serde_json::from_value(answer["push"].clone())?;
        push.cache
            .ok_or_else(|| anyhow::anyhow!("the authority sent no record"))
    };
    let fetched = tokio::time::timeout(mirroring::PUSH_TIMEOUT, fetched).await;
    client.close().await;
    fetched?
}

async fn run(mut authority: Authority, root: PathBuf, noded_url: String) -> anyhow::Result<()> {
    let build = buildinfo::build_info!();
    let provenance = bus::RegisterProvenance::from_parts(
        build.pkg,
        build.version,
        build.git_sha,
        build.git_dirty,
        build.build_time,
        buildinfo::now_rfc3339(),
    );
    let client = Arc::new(
        SupervisedClient::connect_options("settingsd", &noded_url)
            .bounded_incoming(64)
            .fatal_on_registration_rejection(true)
            .with_commands(manifest())
            .with_provenance(provenance)
            .connect()
            .await?,
    );
    let mut incoming = client
        .incoming_bounded()
        .ok_or_else(|| anyhow::anyhow!("native incoming already taken"))?;
    let mut state = client.subscribe_state();
    let (events, mut reports) = mpsc::unbounded_channel();
    let mirrors = match authority.role {
        Role::Authority(_) => Attached::load(&root),
        Role::Mirror { .. } => Attached::default(),
    };
    let mut ctx = Ctx {
        client: client.clone(),
        root,
        events,
        node: None,
        mirrors,
        retry: PublicationRetry::default(),
        attaching: false,
    };
    connected(&mut authority, &mut ctx).await;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    loop {
        let mirror_retry = ctx.mirrors.next_retry();
        tokio::select! {
            changed = state.changed() => {
                if changed.is_err() { break; }
                let now = *state.borrow_and_update();
                if now == ConnState::Fatal { anyhow::bail!("settings authority registration rejected"); }
                if now == ConnState::Connected {
                    // Re-registration always repopulates retained state, including
                    // ordinary restart when broker remains alive. No heartbeat.
                    connected(&mut authority, &mut ctx).await;
                } else {
                    authority.published = None;
                    ctx.retry = PublicationRetry::default();
                    if matches!(authority.role, Role::Mirror { .. }) { authority.stale = true; }
                }
            }
            _ = ctx.retry.wait() => {
                ctx.retry.deadline = None;
                ctx.publish(&mut authority).await;
            }
            _ = sleep_until(mirror_retry), if mirror_retry.is_some() => {
                ctx.push_due(&authority).await;
            }
            Some(report) = reports.recv() => match report {
                Event::Pushed { node, revision, ok } => {
                    if ctx.mirrors.finished(&node, revision, ok, Instant::now()) {
                        tracing::warn!(node, "mirror dropped after repeated push failures; it re-attaches on its reconnect");
                    }
                    ctx.push_due(&authority).await;
                }
                // An answer from before the latest reconnect: that
                // connection's attach is void, a new one is in flight.
                Event::Attached { generation, .. } if generation != client.connection_generation() => {}
                Event::Attached { outcome: Ok(push), .. } => {
                    ctx.attaching = false;
                    let (taken, result) = adopt(authority, &ctx, *push).await;
                    authority = taken;
                    match result {
                        Ok(_) => authority.stale = false,
                        Err(error) => tracing::warn!(%error, "attach answer refused"),
                    }
                    ctx.publish(&mut authority).await;
                }
                Event::Attached { outcome: Err(error), .. } => {
                    ctx.attaching = false;
                    tracing::warn!(%error, "authority unreachable; serving the cache stale until its push or our reconnect");
                    ctx.publish(&mut authority).await;
                }
                Event::Forwarded { reached } => {
                    if !reached { authority.stale = true; }
                    else if authority.stale { ctx.attach(&authority).await; }
                }
            },
            event = incoming.recv() => {
                match event {
                    None => break,
                    Some(BoundedIncomingEvent::Overflow { dropped }) => tracing::warn!(dropped,"settings request input overflow; callers must resolve receipts"),
                    Some(BoundedIncomingEvent::Command(command)) => {
                        if command.is_topic_delivery() { continue; }
                        authority = handle(authority, &mut ctx, command).await?;
                    }
                }
            }
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
        }
    }
    stopping(&authority, &ctx).await;
    let _ = tokio::time::timeout(Duration::from_secs(3), client.deregister()).await;
    Ok(())
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending::<()>().await,
    }
}

/// A (re)connected broker has none of our retained state: republish every
/// topic. The authority pushes to every mirror again; a mirror re-attaches.
async fn connected(authority: &mut Authority, ctx: &mut Ctx) {
    authority.published = None;
    authority.board.republish_all();
    ctx.retry = PublicationRetry::default();
    ctx.node = node_name(&ctx.client).await;
    match authority.role {
        Role::Authority(_) => {
            ctx.publish(authority).await;
            ctx.mirrors.forget_acks();
            ctx.push_due(authority).await;
        }
        // A mirror publishes once the attach answers, so a reachable
        // authority's revision goes out, not the cache's; with the
        // authority unreachable it publishes the cache, stale.
        Role::Mirror { .. } => {
            authority.stale = true;
            ctx.attaching = false;
            ctx.attach(authority).await;
            tracing::debug!(attaching = ctx.attaching, node = ?ctx.node, generation = ctx.client.connection_generation(), "mirror connected");
            if !ctx.attaching {
                ctx.publish(authority).await;
            }
        }
    }
}

/// The authority's own `settings.status`, asked at its address, to confirm
/// a push or a stop notice with the only service that can answer there.
async fn authority_status(ctx: &Ctx, node: &str, binding: &Binding) -> Option<(u8, String)> {
    let body = json!({"binding": binding}).to_string();
    ctx.client
        .call_with_headers_raw_within(
            &mirroring::address(node),
            "settings.status",
            &BTreeMap::new(),
            &body,
            mirroring::PUSH_TIMEOUT,
        )
        .await
        .ok()
        .map(|(rc, body, _)| (rc, body))
}

/// This broker's node name.
async fn node_name(client: &SupervisedClient) -> Option<String> {
    let info = tokio::time::timeout(
        Duration::from_secs(3),
        client.call("noded", "noded.info", json!({})),
    )
    .await;
    match info {
        Ok(Ok(info)) => info["node"].as_str().map(str::to_owned),
        other => {
            tracing::warn!(?other, "broker node name unavailable");
            None
        }
    }
}

/// A clean stop tells each attached mirror, which then reports itself
/// stale until the next push. Best effort, bounded.
async fn stopping(authority: &Authority, ctx: &Ctx) {
    let Some(own) = ctx.node.clone() else { return };
    if !matches!(authority.role, Role::Authority(_)) {
        return;
    }
    let notice = RevisionPush {
        binding: authority.accepted.binding.clone(),
        authority: own,
        cache: None,
        changed: Vec::new(),
        changed_at: BTreeMap::new(),
        stopping: true,
    };
    let Ok(body) = serde_json::to_string(&notice) else {
        return;
    };
    let sends: Vec<_> = ctx
        .mirrors
        .nodes()
        .into_iter()
        .map(|node| {
            let (client, body) = (ctx.client.clone(), body.clone());
            tokio::spawn(async move {
                let _ = client
                    .call_with_headers_raw_within(
                        &mirroring::address(&node),
                        MIRROR_REVISION,
                        &BTreeMap::new(),
                        &body,
                        Duration::from_secs(2),
                    )
                    .await;
            })
        })
        .collect();
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        for send in sends {
            let _ = send.await;
        }
    })
    .await;
}

/// One command: the mirror protocol, a forwarded write, or the serialized
/// blocking dispatch.
async fn handle(
    mut authority: Authority,
    ctx: &mut Ctx,
    command: IncomingCommand,
) -> anyhow::Result<Authority> {
    let name = command.command.as_str();
    let mirror = matches!(authority.role, Role::Mirror { .. });
    if mirror && WRITES.contains(&name) {
        ctx.forward(&authority, &command);
        return Ok(authority);
    }
    let sender = mirroring::Sender::of(&command);
    let mirror_of = match &authority.role {
        Role::Mirror { node } => Some(node.clone()),
        Role::Authority(_) => None,
    };
    let reply = match name {
        MIRROR_ATTACH => attach(&authority, ctx, sender, &command.body).await,
        MIRROR_REVISION => match (mirror_of.as_deref(), decode::<RevisionPush>(&command.body)) {
            (None, _) => Err(json!({"status":"wrong_role","role":"authority"})),
            // Only the configured authority's settingsd, as the broker
            // stamps it, may push or stop: anything else changes nothing,
            // not even `stale`.
            (Some(node), _) if !sender.is_authority(node) => {
                let refused = mirroring::wrong_sender(&sender, &mirroring::address(node));
                tracing::warn!(%refused, "mirror command from the wrong sender refused");
                Err(refused)
            }
            (_, Ok(push)) if push.binding != authority.accepted.binding => {
                Err(json!({"status":"wrong_target","binding":authority.accepted.binding}))
            }
            // A stop notice from an authority that still answers is not
            // one: ignored.
            (Some(node), Ok(push)) if push.stopping => {
                let binding = authority.accepted.binding.clone();
                if matches!(authority_status(ctx, node, &binding).await, Some((0, _))) {
                    tracing::warn!("stop notice while the authority answers; ignored");
                    Ok(json!({"status":"current","revision":authority.accepted.revision}))
                } else {
                    authority.stale = true;
                    tracing::info!("authority stopping; serving the cache stale");
                    Ok(json!({"status":"detached"}))
                }
            }
            (Some(node), Ok(push)) => {
                let binding = authority.accepted.binding.clone();
                let confirmed = match &push.cache {
                    Some(cache) => {
                        let status = authority_status(ctx, node, &binding).await;
                        mirroring::confirm(status.as_ref().map(|(rc, b)| (*rc, b.as_str())), cache)
                    }
                    None => mirroring::Confirmation::Refused("no record".into()),
                };
                match confirmed {
                    mirroring::Confirmation::Holds => {
                        let (taken, reply) = adopt(authority, ctx, push).await;
                        authority = taken;
                        ctx.publish(&mut authority).await;
                        reply
                    }
                    // A newer push is on its way: nothing to take, nothing
                    // stale about it.
                    mirroring::Confirmation::Behind => {
                        Ok(json!({"status":"current","revision":authority.accepted.revision}))
                    }
                    // Not the authority's record: refused, the cache and
                    // `stale` untouched.
                    mirroring::Confirmation::Refused(why) => {
                        tracing::warn!(%why, "pushed record is not the authority's; refused");
                        Err(json!({"status":"not_confirmed","reason":why}))
                    }
                    mirroring::Confirmation::Unreachable => {
                        authority.stale = true;
                        Err(mirroring::unreachable(node))
                    }
                }
            }
            (_, Err(error)) => {
                authority.stale = true;
                Err(error)
            }
        },
        _ => {
            let command_name = command.command.clone();
            let body = command.body.clone();
            let previous_revision = authority.accepted.revision;
            // At most one blocking job. Durable work completes even
            // when its reply cannot be sent; no mutation replay.
            let (taken, reply) = tokio::task::spawn_blocking(move || {
                let reply = dispatch(&mut authority, &command_name, &body);
                (authority, reply)
            })
            .await?;
            authority = taken;
            if authority.accepted.revision != previous_revision {
                ctx.retry = PublicationRetry::default();
            }
            ctx.publish(&mut authority).await;
            if authority.accepted.revision != previous_revision {
                ctx.push_due(&authority).await;
            }
            reply
        }
    };
    let mut reply = reply;
    if let Ok(ref mut value) = reply
        && let Some(object) = value.as_object_mut()
    {
        if object.contains_key("publication_pending") {
            let pending = authority.published != Some(authority.accepted.revision)
                || !authority.board.is_current();
            object.insert("publication_pending".into(), json!(pending));
        }
        if name == "settings.status" && matches!(authority.role, Role::Authority(_)) {
            object.insert("mirrors".into(), ctx.mirrors.status());
            // The digest of the record a mirror would cache, which a mirror
            // checks a push against ([`mirroring::confirm`]).
            if let Some(own) = ctx.node.as_deref()
                && let Ok(cache) = authority.cache(own)
            {
                object.insert("mirror_digest".into(), json!(cache.content_digest));
            }
        }
    }
    let (rc, value) = match reply {
        Ok(value) => (0, value),
        Err(error) => (10, error),
    };
    let body = serde_json::to_string(&value)?;
    if let Err(error) = ctx.client.respond(&command, rc, &body).await {
        tracing::warn!(%error, "settings reply unavailable; operation receipt retained");
    }
    // A mirror attached at the revision it holds is pushed now: harmless
    // when the answer arrives, the catch-up when it was lost.
    if name == MIRROR_ATTACH && rc == 0 {
        ctx.push_due(&authority).await;
    }
    Ok(authority)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S7-TRANSITION: exactly what 1.0.0 keeps of 0.8.0, so the landing
    /// that brings S8a-c deletes a known list and this test with it.
    #[test]
    fn the_transition_keeps_exactly() {
        assert_eq!(
            TRANSITION_TOPIC_PREFIXES,
            [
                "settingsd.desktop.changed.",
                "settingsd.desktop.compact.",
                "settingsd.desktop.summary.",
            ]
        );
        assert_eq!(TRANSITION_COMMANDS, ["settings.appearance.get"]);
        assert_eq!(TRANSITION_VIEWS, [View::Full, View::Summary]);
        assert_eq!(View::default(), View::Full, "an omitted view is still full");
        // Every served command is 1.0.0 or listed above.
        let current = [
            "settings.describe",
            "settings.get",
            "settings.validate",
            "settings.apply",
            "settings.reset",
            "settings.status",
            "settings.source",
            "settings.palette",
            MIRROR_ATTACH,
            MIRROR_REVISION,
        ];
        let kept: Vec<&str> = COMMANDS
            .iter()
            .copied()
            .filter(|c| !current.contains(c))
            .collect();
        assert_eq!(kept, TRANSITION_COMMANDS);
        // No 1.0.0 topic shares a prefix with a kept one.
        for topic in [
            settings::topics::keys_topic("default", "appearance"),
            settings::topics::look_topic("default"),
            settings::topics::snapshot_topic("default"),
        ] {
            assert!(
                TRANSITION_TOPIC_PREFIXES
                    .iter()
                    .all(|p| !topic.starts_with(p))
            );
        }
    }

    #[test]
    fn pending_publication_has_bounded_backoff_and_success_removes_all_idle_work() {
        let mut retry = PublicationRetry::default();
        assert!(retry.deadline.is_none());
        for _ in 0..20 {
            retry.complete(false);
            assert!(retry.deadline.is_some());
        }
        assert!(retry.deadline.unwrap() <= Instant::now() + Duration::from_secs(30));
        retry.complete(true);
        assert_eq!(retry.failures, 0);
        assert!(retry.deadline.is_none());
    }
}
