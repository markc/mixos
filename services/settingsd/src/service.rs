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
    MIRROR_ATTACH,
    MIRROR_REVISION,
    // TRANSITION (S8c deletes): portald's read until it moves to
    // settings.palette.
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
    let mut described = settings::registry::describe(request.prefix.as_deref());
    described["version"] = settings::CONTRACT_VERSION.into();
    described["mirror"] = json!({
        MIRROR_ATTACH: {"from": "mirror", "body": {"binding": "the authority's", "node": "the mirror's node", "revision": "the revision it holds"},
            "answer": "the push, so the mirror catches up in the same round trip"},
        MIRROR_REVISION: {"from": "authority", "body": {"binding": "", "authority": "its node", "cache": "the sealed record", "changed": "prefixes", "changed_at": "per topic", "stopping": "bool"}},
    });
    Ok(described)
}
pub fn dispatch(authority: &mut Authority, command_name: &str, body: &str) -> Result<Value, Value> {
    match command_name {
        "HELP" => Ok(json!(manifest())),
        "settings.describe" => describe(body),
        "settings.get" => authority.read(decode(body)?),
        "settings.appearance.get" => authority.appearance(decode(body)?),
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

/// TRANSITION (S8 deletes): the format-4 snapshot in schema 1 and 2 and the
/// summary, for the followers not yet on the 1.0.0 topics.
async fn publish_legacy(authority: &mut Authority, client: &SupervisedClient) -> anyhow::Result<()> {
    let profile = &authority.accepted.binding.profile;
    publish_retained(
        client,
        settings::topic(profile),
        serde_json::to_string(&authority.snapshot)?,
    )
    .await?;
    publish_retained(
        client,
        settings::compact_topic(profile),
        serde_json::to_string(&settings::compact::encode(&authority.snapshot)?)?,
    )
    .await?;
    publish_retained(
        client,
        settings::summary_topic(profile),
        serde_json::to_string(&settings::Summary::of(&authority.snapshot))?,
    )
    .await?;
    authority.published = Some(authority.accepted.revision);
    Ok(())
}

/// Publish every pending 1.0.0 topic, then the transitional ones. Each topic
/// is marked published as it goes; a failure leaves the rest pending for
/// the bounded retry.
async fn publish(authority: &mut Authority, client: &SupervisedClient) -> anyhow::Result<()> {
    for key in authority.board.pending() {
        if let Some((topic, body)) = authority.topic(&key) {
            publish_retained(client, topic, body).await?;
        }
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
    /// A mirror's attach answered (the push it carries) or failed.
    Attached(Result<RevisionPush, String>),
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
        if authority.board.is_current() && authority.published == Some(authority.accepted.revision)
        {
            return;
        }
        let result = publish(authority, &self.client).await;
        self.retry.complete(result.is_ok());
        if let Err(error) = result {
            tracing::warn!(%error, failures=self.retry.failures, "settings publication pending");
        }
    }

    /// Push the current revision to every mirror that is behind it.
    fn push_due(&mut self, authority: &Authority) {
        let Some(own) = self.node.clone() else {
            return;
        };
        if !self.client.is_connected() || !matches!(authority.role, Role::Authority(_)) {
            return;
        }
        let revision = authority.accepted.revision;
        for (node, since) in self.mirrors.due(revision, Instant::now()) {
            let body = push_of(authority, &own, since).and_then(|p| Ok(serde_json::to_string(&p)?));
            let body = match body {
                Ok(body) => body,
                Err(error) => {
                    tracing::warn!(%error, node, "mirror push not built");
                    self.mirrors.finished(&node, revision, false, Instant::now());
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
    fn attach(&mut self, authority: &Authority) {
        let Role::Mirror { node: authority_node } = &authority.role else {
            return;
        };
        if self.attaching || !self.client.is_connected() {
            return;
        }
        let Some(own) = self.node.clone() else {
            tracing::warn!("mirror node unknown; attach waits for the next connect");
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
            let _ = events.send(Event::Attached(outcome));
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
                Ok((rc, answer, _)) if mirroring::from_authority(&answer) => (rc, answer, true),
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
fn push_of(authority: &Authority, own: &str, since: Option<Revision>) -> anyhow::Result<RevisionPush> {
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
/// with the push, so it catches up in the same round trip.
fn attach(authority: &Authority, ctx: &mut Ctx, body: &str) -> Result<Value, Value> {
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
    let own = ctx
        .node
        .clone()
        .ok_or_else(|| json!({"status":"not_ready","message":"Broker node not known yet"}))?;
    let push = push_of(authority, &own, Some(request.revision))
        .map_err(|e| json!({"status":"storage_failed","message":e.to_string()}))?;
    ctx.mirrors.attach(&request.node, authority.accepted.revision);
    Ok(json!({"status":"attached","revision":authority.accepted.revision,"push":push}))
}

/// A push at a mirror: verify and cache the record, then take it. A push
/// older than what the mirror holds is acknowledged and ignored.
async fn adopt(
    authority: Authority,
    ctx: &Ctx,
    push: RevisionPush,
) -> (Authority, Result<Value, Value>) {
    let Role::Mirror { node } = &authority.role else {
        return (authority, Err(json!({"status":"wrong_role","role":"authority"})));
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
    if cache.incarnation == authority.accepted.incarnation
        && cache.revision < authority.accepted.revision
    {
        let revision = authority.accepted.revision;
        return (
            authority,
            Ok(json!({"status":"current","revision":revision})),
        );
    }
    let root = ctx.root.clone();
    let changed_at = push.changed_at;
    let mut authority = authority;
    let result = tokio::task::spawn_blocking(move || {
        let taken = crate::mirror::write(&root, &cache)
            .and_then(|()| authority.adopt(&cache, &changed_at));
        (authority, taken)
    })
    .await;
    match result {
        Ok((mut authority, Ok(moved))) => {
            authority.stale = false;
            let revision = authority.accepted.revision;
            tracing::info!(revision = revision.0, ?moved, "mirror took the authority's revision");
            (
                authority,
                Ok(json!({"status":"current","revision":revision})),
            )
        }
        Ok((authority, Err(error))) => {
            tracing::warn!(%error, "pushed record refused; the cache is kept");
            (
                authority,
                Err(json!({"status":"validation_failed","message":error.to_string()})),
            )
        }
        Err(join) => panic!("mirror adoption panicked: {join}"),
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
    anyhow::ensure!(mirroring::valid_node(&node), "invalid authority node {node:?}");
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
    let client = bus::native_client::NodedClient::connect_anonymous(noded_url).await?;
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
                ctx.push_due(&authority);
            }
            Some(report) = reports.recv() => match report {
                Event::Pushed { node, revision, ok } => {
                    if ctx.mirrors.finished(&node, revision, ok, Instant::now()) {
                        tracing::warn!(node, "mirror dropped after repeated push failures; it re-attaches on its reconnect");
                    }
                    ctx.push_due(&authority);
                }
                Event::Attached(Ok(push)) => {
                    ctx.attaching = false;
                    let (taken, result) = adopt(authority, &ctx, push).await;
                    authority = taken;
                    if let Err(error) = result { tracing::warn!(%error, "attach answer refused"); }
                    ctx.publish(&mut authority).await;
                }
                Event::Attached(Err(error)) => {
                    ctx.attaching = false;
                    tracing::warn!(%error, "authority unreachable; serving the cache stale until its push or our reconnect");
                }
                Event::Forwarded { reached } => {
                    if !reached { authority.stale = true; }
                    else if authority.stale { ctx.attach(&authority); }
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
    ctx.publish(authority).await;
    match authority.role {
        Role::Authority(_) => {
            ctx.mirrors.forget_acks();
            ctx.push_due(authority);
        }
        Role::Mirror { .. } => {
            authority.stale = true;
            ctx.attaching = false;
            ctx.attach(authority);
        }
    }
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
    let Ok(body) = serde_json::to_string(&notice) else { return };
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
    let reply = match name {
        MIRROR_ATTACH => attach(&authority, ctx, &command.body),
        MIRROR_REVISION => match decode::<RevisionPush>(&command.body) {
            Ok(push) if push.stopping && mirror => {
                authority.stale = true;
                tracing::info!("authority stopping; serving the cache stale");
                Ok(json!({"status":"detached"}))
            }
            Ok(push) => {
                let (taken, reply) = adopt(authority, ctx, push).await;
                authority = taken;
                ctx.publish(&mut authority).await;
                reply
            }
            Err(error) => Err(error),
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
                ctx.push_due(&authority);
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
    Ok(authority)
}

#[cfg(test)]
mod tests {
    use super::*;
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
