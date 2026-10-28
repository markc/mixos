// SPDX-License-Identifier: MIT OR Apache-2.0
//! FileChooser v4: the session portal forwards to the native files service.
pub mod lifecycle;
pub mod mapping;

use crate::portal::{OBJECT_PATH, PortalError};
use crate::screencast::{handle, request};
use bus::PortReply;
use bus::native_client::{BoundedIncomingEvent, SupervisedClient};
use lifecycle::{Event, Phase, State};
use mapping::Options;
use serde_json::{Value as Json, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};
use zbus::Connection;
use zbus::message::Header;
use zbus::object_server::ResponseDispatchNotifier;
use zbus::zvariant::OwnedObjectPath;

pub const INTERFACE: &str = "org.freedesktop.portal.FileChooser";
pub const VERSION: u32 = 4;
const IO_TIMEOUT: Duration = Duration::from_secs(1);
const CHOOSE_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_secs(5);

struct Entry {
    path: OwnedObjectPath,
    owner: String,
    choices: bool,
    state: State,
    reply_sent: Option<oneshot::Receiver<()>>,
}

#[derive(Default)]
struct Registry {
    entries: HashMap<u64, Entry>,
    sequence: u64,
    generation: Option<u64>,
    incarnation: Option<String>,
}

#[derive(Clone)]
pub struct Bridge {
    client: Arc<SupervisedClient>,
    registry: Arc<Mutex<Registry>>,
    counter: Arc<AtomicU64>,
}

/// Splits the one native incoming lane without introducing another broker
/// connection or another portald identity. Overflow is recovered by list/result.
pub struct Source<I> {
    pub incoming: I,
    pub outcomes: mpsc::Sender<(u64, Json)>,
}

impl<I> Source<I> {
    fn forward(&self, event: BoundedIncomingEvent) -> Option<BoundedIncomingEvent> {
        if let BoundedIncomingEvent::Command(command) = &event
            && command.headers.get("topic").map(String::as_str) == Some("files.chosen")
        {
            if command.headers.get("broker_service").map(String::as_str) == Some("files") {
                if let Ok(value) = serde_json::from_str(&command.body) {
                    let _ = self.outcomes.try_send((command.generation, value));
                }
            } else {
                tracing::warn!("files.chosen without files owner stamp ignored");
            }
            return None;
        }
        Some(event)
    }
}

impl<I: crate::service::CommandSource> crate::service::CommandSource for Source<I> {
    async fn next_event(&mut self) -> Option<BoundedIncomingEvent> {
        loop {
            let event = self.incoming.next_event().await?;
            if let Some(event) = self.forward(event) {
                return Some(event);
            }
        }
    }
    fn try_next_event(&mut self) -> Option<BoundedIncomingEvent> {
        loop {
            let event = self.incoming.try_next_event()?;
            if let Some(event) = self.forward(event) {
                return Some(event);
            }
        }
    }
}

impl Bridge {
    pub fn new(client: Arc<SupervisedClient>) -> Self {
        Self {
            client,
            registry: Arc::default(),
            counter: handle::counter(),
        }
    }

    async fn call(&self, command: &str, body: Json) -> Option<Json> {
        match tokio::time::timeout(IO_TIMEOUT, self.client.call_typed("files", command, body)).await
        {
            Ok(Ok(PortReply::Ok { rc: 0, value })) => Some(value),
            other => {
                tracing::warn!(?other, %command, "files call failed");
                None
            }
        }
    }

    fn cancel(&self, request: String, generation: Option<u64>) {
        let this = self.clone();
        tokio::spawn(async move {
            if generation.is_some() && settings::native::live_generation(&this.client) == generation
            {
                this.call("files.choose.cancel", json!({"request":request}))
                    .await;
            }
        });
    }

    // The lock covers transitions, signal enqueueing and object removal. Thus
    // a Close cannot return before a winning Response has been enqueued, and
    // no old operation can remove a new Request at a reused path.
    async fn apply(&self, connection: &Connection, registry: &mut Registry, id: u64, event: Event) {
        let Some(entry) = registry.entries.get_mut(&id) else {
            return;
        };
        let effects = entry.state.apply(event);
        if let Some(cancel) = effects.cancel {
            self.cancel(cancel, registry.generation);
        }
        if let Some(response) = effects.response {
            if let Some(reply_sent) = entry.reply_sent.take() {
                let _ = reply_sent.await;
            }
            let (code, results) = mapping::outcome(&response, entry.choices);
            request::emit(connection, &entry.path, &entry.owner, code, results).await;
        }
        if effects.unexport {
            let _ = connection
                .object_server()
                .remove::<request::Request, _>(entry.path.clone())
                .await;
        }
        if entry.state.phase == Phase::Ended {
            registry.entries.remove(&id);
        }
    }

    pub async fn close(&self, connection: &Connection, id: u64) {
        let mut registry = self.registry.lock().await;
        self.apply(connection, &mut registry, id, Event::Close)
            .await;
    }

    async fn lost(&self, connection: &Connection, registry: &mut Registry) {
        for id in registry.entries.keys().copied().collect::<Vec<_>>() {
            self.apply(connection, registry, id, Event::Lost).await;
        }
        registry.incarnation = None;
    }

    /// A queued reply or publication can beat the state-watch arm after the
    /// transport has already changed. Fence it at the transition, as the
    /// Settings consumer does, so an old connection cannot finish a Request.
    async fn fence(&self, connection: &Connection, registry: &mut Registry) -> bool {
        if registry.generation.is_some()
            && registry.generation != settings::native::live_generation(&self.client)
        {
            registry.generation = None;
            self.lost(connection, registry).await;
            return false;
        }
        true
    }

    async fn owner_left(&self, connection: &Connection, owner: &str) {
        let mut registry = self.registry.lock().await;
        let ids: Vec<_> = registry
            .entries
            .iter()
            .filter(|(_, e)| e.owner == owner)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.apply(connection, &mut registry, id, Event::Close)
                .await;
        }
    }

    async fn outcome(&self, connection: &Connection, generation: u64, value: Json) {
        let mut registry = self.registry.lock().await;
        if !self.fence(connection, &mut registry).await {
            return;
        }
        if registry.generation != Some(generation) {
            return;
        }
        let ids: Vec<_> = registry
            .entries
            .iter()
            .filter(|(_, entry)| match &entry.state.phase {
                Phase::Sending => value["correlation"].as_str() == Some(entry.path.as_str()),
                Phase::Open(id) => value["request"].as_str() == Some(id),
                _ => false,
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.apply(connection, &mut registry, id, Event::Outcome(value.clone()))
                .await;
        }
    }

    async fn reply(
        &self,
        connection: &Connection,
        generation: Option<u64>,
        id: u64,
        reply: Option<Json>,
    ) {
        let request = reply
            .as_ref()
            .and_then(|v| v["request"].as_str())
            .map(str::to_owned);
        let mut registry = self.registry.lock().await;
        if !self.fence(connection, &mut registry).await {
            return;
        }
        if registry.generation != generation {
            return;
        }
        if !registry.entries.contains_key(&id) {
            if let Some(request) = request {
                self.cancel(request, generation);
            }
            return;
        }
        if registry.incarnation.is_none() {
            registry.incarnation = request
                .as_ref()
                .and_then(|s| s.split_once('-'))
                .map(|(inc, _)| inc.to_owned());
        }
        self.apply(connection, &mut registry, id, Event::Reply(request))
            .await;
    }

    async fn choose(&self, connection: Connection, id: u64, mut body: Json, owner: String) {
        // The caller may disconnect before its method was dispatched. Watchers
        // alone cannot catch a NameOwnerChanged already delivered before insert.
        let present = async {
            let proxy = zbus::fdo::DBusProxy::new(&connection).await?;
            proxy
                .name_has_owner(zbus::names::BusName::try_from(owner.clone())?)
                .await
                .map_err(zbus::Error::from)
        }
        .await;
        if matches!(present, Ok(false)) {
            self.close(&connection, id).await;
        }
        let app = crate::screencast::consent::app_name(
            crate::screencast::caller_pid(&connection, &owner).await,
        )
        .unwrap_or_else(|| "unknown".into());
        body["on_behalf_of"] = json!(mapping::cut(&format!("{owner} {app}"), 128));
        let generation = {
            let registry = self.registry.lock().await;
            if !registry.entries.contains_key(&id) {
                return;
            }
            registry.generation
        };
        if generation.is_none() || generation != settings::native::live_generation(&self.client) {
            self.reply(&connection, generation, id, None).await;
            return;
        }
        // Keep the native reply future alive after our deadline. A late window
        // is cancelled even when a closing sweep has already retired the entry.
        let call = self.client.call_typed("files", "files.choose", body);
        tokio::pin!(call);
        let reply = match tokio::time::timeout(CHOOSE_TIMEOUT, &mut call).await {
            Ok(reply) => reply,
            Err(_) => {
                {
                    let mut registry = self.registry.lock().await;
                    self.apply(&connection, &mut registry, id, Event::Timeout)
                        .await;
                }
                call.await
            }
        };
        let reply = match reply {
            Ok(PortReply::Ok { rc: 0, value }) => Some(value),
            other => {
                tracing::warn!(?other, "files.choose refused");
                None
            }
        };
        self.reply(&connection, generation, id, reply).await;
    }

    async fn poll(&self, connection: &Connection) {
        let (generation, entries) = {
            let registry = self.registry.lock().await;
            (
                registry.generation,
                registry
                    .entries
                    .iter()
                    .map(|(id, e)| {
                        (
                            *id,
                            e.path.to_string(),
                            match &e.state.phase {
                                Phase::Open(request) => Some(request.clone()),
                                _ => None,
                            },
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        };
        if entries.is_empty() {
            return;
        }
        let list = self.call("files.choose.list", json!({})).await;
        let Some((incarnation, open)) = list
            .as_ref()
            .and_then(|v| Some((v["incarnation"].as_str()?, v["requests"].as_array()?)))
        else {
            let mut registry = self.registry.lock().await;
            if registry.generation == generation {
                self.lost(connection, &mut registry).await;
            }
            return;
        };
        {
            let mut registry = self.registry.lock().await;
            if registry.generation != generation {
                return;
            }
            if registry
                .incarnation
                .as_deref()
                .is_some_and(|old| old != incarnation)
            {
                self.lost(connection, &mut registry).await;
                registry.incarnation = Some(incarnation.into());
                return;
            }
            registry.incarnation = Some(incarnation.into());
        }
        let mut missing = Vec::new();
        for (id, path, request) in entries {
            if let Some(request) = request {
                if open.iter().any(|v| v["request"] == request) {
                    continue;
                }
                missing.push(request);
            } else {
                let mut registry = self.registry.lock().await;
                if registry.generation != generation {
                    return;
                }
                if registry
                    .entries
                    .get(&id)
                    .is_some_and(|e| e.state.phase == Phase::Closing)
                {
                    // A token can be reused immediately after Close. Its path
                    // alone cannot distinguish two sends until both replies
                    // arrive. Defer that sweep while a newer send is pending,
                    // and never cancel the id of another live Request.
                    if registry.entries.iter().any(|(other, entry)| {
                        *other != id
                            && entry.path.as_str() == path
                            && entry.state.phase == Phase::Sending
                    }) {
                        continue;
                    }
                    for request in open
                        .iter()
                        .filter(|v| v["correlation"].as_str() == Some(&path))
                    {
                        if let Some(request) = request["request"].as_str()
                            && !registry.entries.values().any(|entry| {
                                matches!(&entry.state.phase, Phase::Open(live) if live == request)
                            })
                        {
                            self.cancel(request.into(), generation);
                        }
                    }
                    self.apply(connection, &mut registry, id, Event::Sweep)
                        .await;
                }
            }
        }
        // A missed burst must not cost one timeout per dialog or delay the
        // next five-second liveness check. Native result reads run together.
        let results = futures_util::future::join_all(
            missing
                .iter()
                .map(|request| self.call("files.choose.result", json!({"request": request}))),
        )
        .await;
        for (request, result) in missing.into_iter().zip(results) {
            let value = result
                .filter(|v| v["state"] == "done")
                .map(|v| v["outcome"].clone())
                .unwrap_or_else(|| json!({"request":request, "response":2}));
            self.outcome(connection, generation.unwrap_or_default(), value)
                .await;
        }
    }

    /// Subscribe before exporting FileChooser. Settings already owns the name
    /// and answers reads while this source initialisation runs independently.
    pub async fn attach(
        &self,
        connection: Connection,
        mut outcomes: mpsc::Receiver<(u64, Json)>,
    ) -> anyhow::Result<()> {
        use futures_util::StreamExt;
        let proxy = zbus::fdo::DBusProxy::new(&connection).await?;
        let mut owners = proxy.receive_name_owner_changed().await?;
        let mut lifecycle = self.client.subscribe_state();
        let mut timer = tokio::time::interval(POLL);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut exported = false;
        loop {
            let generation = settings::native::live_generation(&self.client);
            let held = self.registry.lock().await.generation;
            if generation != held {
                let mut registry = self.registry.lock().await;
                self.lost(&connection, &mut registry).await;
                registry.generation = None;
                drop(registry);
            }
            let needs_subscription = self.registry.lock().await.generation.is_none();
            if let Some(generation) = generation
                && needs_subscription
                && matches!(
                    tokio::time::timeout(IO_TIMEOUT, self.client.subscribe_topic("files.chosen"))
                        .await,
                    Ok(Ok(()))
                )
                && settings::native::live_generation(&self.client) == Some(generation)
            {
                // Startup and every reconnect clear choosers from the previous
                // portald. Do this before admitting any new native requests.
                if let Some(list) = self.call("files.choose.list", json!({})).await {
                    if let Some(open) = list["requests"].as_array() {
                        for entry in open {
                            if entry["correlation"].as_str().is_some_and(|s| {
                                s.starts_with(&format!("{}/", handle::REQUEST_ROOT))
                            }) && let Some(id) = entry["request"].as_str()
                            {
                                self.call("files.choose.cancel", json!({"request":id}))
                                    .await;
                            }
                        }
                    }
                    self.registry.lock().await.incarnation =
                        list["incarnation"].as_str().map(str::to_owned);
                }
                if settings::native::live_generation(&self.client) == Some(generation) {
                    self.registry.lock().await.generation = Some(generation);
                }
            }
            if !exported && self.registry.lock().await.generation.is_some() {
                connection
                    .object_server()
                    .at(
                        OBJECT_PATH,
                        FileChooser {
                            bridge: self.clone(),
                        },
                    )
                    .await?;
                exported = true;
            }
            tokio::select! {
                _ = connection.closed() => { self.owner_shutdown(&connection).await; return Ok(()); }
                change = owners.next() => {
                    let Some(change) = change else { self.owner_shutdown(&connection).await; return Ok(()); };
                    if let Ok(args) = change.args()
                        && args.new_owner().is_none() && args.name().as_str().starts_with(':')
                    { self.owner_left(&connection, args.name().as_str()).await; }
                }
                value = outcomes.recv() => {
                    let Some((generation, value)) = value else {
                        let mut registry = self.registry.lock().await;
                        registry.generation = None;
                        self.lost(&connection, &mut registry).await;
                        return Ok(());
                    };
                    self.outcome(&connection, generation, value).await;
                }
                changed = lifecycle.changed() => {
                    if changed.is_err() {
                        let mut registry = self.registry.lock().await;
                        registry.generation = None;
                        self.lost(&connection, &mut registry).await;
                        return Ok(());
                    }
                    lifecycle.borrow_and_update();
                }
                _ = timer.tick() => { self.poll(&connection).await; }
            }
        }
    }

    async fn owner_shutdown(&self, connection: &Connection) {
        let owners: Vec<_> = self
            .registry
            .lock()
            .await
            .entries
            .values()
            .map(|e| e.owner.clone())
            .collect();
        for owner in owners {
            self.owner_left(connection, &owner).await;
        }
    }
}

pub struct FileChooser {
    bridge: Bridge,
}

impl FileChooser {
    async fn begin(
        &self,
        mode: &str,
        parent: &str,
        title: &str,
        options: Options,
        header: Header<'_>,
        connection: &Connection,
    ) -> Result<ResponseDispatchNotifier<OwnedObjectPath>, PortalError> {
        let owner = header
            .sender()
            .map(|s| s.as_str().to_owned())
            .ok_or_else(|| PortalError::InvalidArgument("no sender".into()))?;
        let path = handle::request_path(
            &owner,
            &handle::token(&options, "handle_token", &self.bridge.counter)?,
        )?;
        let mut body = mapping::map(mode, parent, title, &options)?;
        body["correlation"] = json!(path.as_str());
        let mut registry = self.bridge.registry.lock().await;
        registry.sequence += 1;
        let id = registry.sequence;
        if !connection
            .object_server()
            .at(
                path.clone(),
                request::Request::filechooser(&owner, self.bridge.clone(), id),
            )
            .await?
        {
            return Err(PortalError::InvalidArgument(
                "handle_token already has a live Request".into(),
            ));
        }
        let (sent, reply_sent) = oneshot::channel();
        registry.entries.insert(
            id,
            Entry {
                path: path.clone(),
                owner: owner.clone(),
                choices: body["choices"].as_array().is_some_and(|v| !v.is_empty()),
                state: State::default(),
                reply_sent: Some(reply_sent),
            },
        );
        drop(registry);
        let bridge = self.bridge.clone();
        let connection = connection.clone();
        let (response, dispatched) = ResponseDispatchNotifier::new(path);
        tokio::spawn(async move {
            dispatched.await;
            let _ = sent.send(());
            bridge.choose(connection, id, body, owner).await;
        });
        Ok(response)
    }
}

#[zbus::interface(name = "org.freedesktop.portal.FileChooser")]
impl FileChooser {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }

    #[zbus(name = "OpenFile")]
    async fn open_file(
        &self,
        parent_window: &str,
        title: &str,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<ResponseDispatchNotifier<OwnedObjectPath>, PortalError> {
        self.begin("open", parent_window, title, options, header, connection)
            .await
    }
    #[zbus(name = "SaveFile")]
    async fn save_file(
        &self,
        parent_window: &str,
        title: &str,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<ResponseDispatchNotifier<OwnedObjectPath>, PortalError> {
        self.begin("save", parent_window, title, options, header, connection)
            .await
    }
    #[zbus(name = "SaveFiles")]
    async fn save_files(
        &self,
        parent_window: &str,
        title: &str,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<ResponseDispatchNotifier<OwnedObjectPath>, PortalError> {
        self.begin(
            "save_files",
            parent_window,
            title,
            options,
            header,
            connection,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use zbus::{MatchRule, MessageStream};

    struct Daemon(Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[tokio::test]
    async fn queued_reply_and_outcome_cannot_win_after_native_connection_loss() {
        let dir = tempfile::tempdir().unwrap();
        let mut daemon = Daemon(
            Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
                .arg(format!("--address=unix:path={}/bus", dir.path().display()))
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut address = String::new();
        BufReader::new(daemon.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let server = zbus::connection::Builder::address(address.trim())
            .unwrap().build().await.unwrap();
        let owner = zbus::connection::Builder::address(address.trim())
            .unwrap().build().await.unwrap();
        let broker = test_broker::Broker::start();
        let client = Arc::new(SupervisedClient::connect_options("portal-fence-test", &broker.url)
            .connect().await.unwrap());
        let generation = settings::native::live_generation(&client).unwrap();
        client.close().await;
        let bridge = Bridge::new(client);
        let owner_name = owner.unique_name().unwrap().as_str();
        for reply in [false, true] {
            let path = handle::request_path(owner_name, if reply { "reply" } else { "outcome" }).unwrap();
            let value = json!({"request":"0000000000000001-1", "correlation":path.as_str(), "response":0, "uris":["file:///tmp/chosen"]});
            let mut state = State::default();
            if reply { state.apply(Event::Outcome(value.clone())); }
            else { state.phase = Phase::Open("0000000000000001-1".into()); }
            {
                let mut registry = bridge.registry.lock().await;
                registry.generation = Some(generation);
                registry.entries.insert(1, Entry { path:path.clone(), owner:owner_name.into(), choices:false, state, reply_sent:None });
            }
            server.object_server().at(path.clone(), request::Request::filechooser(owner_name, bridge.clone(), 1)).await.unwrap();
            let rule = MatchRule::builder().msg_type(zbus::message::Type::Signal)
                .interface("org.freedesktop.portal.Request").unwrap().member("Response").unwrap()
                .path(path).unwrap().build();
            let mut signals = MessageStream::for_match_rule(rule, &owner, Some(4)).await.unwrap();
            // Deliberately do not run the state watch: these completions were
            // queued before it processed the already lost native connection.
            if reply {
                bridge.reply(&server, Some(generation), 1, Some(value.clone())).await;
            } else {
                bridge.outcome(&server, generation, value.clone()).await;
            }
            let message = tokio::time::timeout(Duration::from_secs(5), signals.next()).await.unwrap().unwrap().unwrap();
            let (code, results): (u32, HashMap<String, zbus::zvariant::OwnedValue>) = message.body().deserialize().unwrap();
            assert_eq!(code, 2);
            assert!(results.is_empty());
            assert!(bridge.registry.lock().await.entries.is_empty());
            bridge.outcome(&server, generation, value).await;
            assert!(tokio::time::timeout(Duration::from_millis(50), signals.next()).await.is_err());
        }
    }
}

