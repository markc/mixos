// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::{BTreeMap, VecDeque};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use bus::{PortReply, native_client::{BoundedIncomingEvent, IncomingCommand, SupervisedClient}};
use chacha20poly1305::{XChaCha20Poly1305, XNonce, KeyInit, aead::{Aead, Payload}};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};

use crate::{model::*, wayland::{Driver, Event}};

pub const COMMANDS: &[(&str, bool)] = &[
    ("clipboard.get", true), ("clipboard.set", false), ("clipboard.clear", false),
    ("clipboard.history", true), ("clipboard.pick", false), ("clipboard.info", true),
    ("clipboard.push", false), ("clipboard.receive", false), ("clipboard.fetch", true),
    ("desktop.clipboard.menu", false), ("desktop.clipboard.rotate", false),
];
pub fn manifest() -> Vec<bus::CommandDescriptor> {
    std::iter::once(bus::CommandDescriptor::new("HELP", &[], "List clipboard commands", true))
        .chain(COMMANDS.iter().map(|(name, read)| {
            let args: &[&str] = match *name {
                "clipboard.get" => &["mime?", "primary?", "offset?"],
                "clipboard.set" => &["mime", "data", "encoding?", "primary?", "sensitive?", "stamp?"],
                "clipboard.clear" => &["primary?"],
                "clipboard.pick" => &["n", "expected_id?"],
                "clipboard.push" => &["to"],
                "clipboard.receive" | "clipboard.fetch" => &["from_session", "to_session", "expires", "nonce", "ciphertext"],
                _ => &[],
            };
            bus::CommandDescriptor::new(name, args, name, *read)
        })).collect()
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    /// Exact Bus target on a broker belonging to this host's configured scope.
    pub target: String,
    #[serde(default)]
    pub images: bool,
    #[serde(default)]
    pub follow: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub schema: u32,
    pub scope_key_file: Option<PathBuf>,
    #[serde(default)]
    pub peers: BTreeMap<String, Peer>,
    #[serde(default = "default_cap")]
    pub eager_text_bytes: usize,
}
fn default_cap() -> usize { TEXT_CAP }
impl Default for Settings {
    fn default() -> Self { Self { schema: 1, scope_key_file: None, peers: BTreeMap::new(), eager_text_bytes: TEXT_CAP } }
}
impl Settings {
    pub fn load(path: Option<&Path>, own_session: &str) -> Result<(Self, Option<[u8; 32]>)> {
        let settings: Self = match path {
            Some(path) => { ensure!(std::fs::metadata(path)?.len() <= 65536, "SETTINGS: file exceeds 64 KiB"); serde_json::from_value(strict::parse_file(path)?)? },
            None => Self::default(),
        };
        ensure!(settings.schema == 1 && settings.eager_text_bytes <= TEXT_CAP, "SETTINGS: unsupported schema or text cap above 1 MiB");
        ensure!(settings.peers.len() <= 2, "SCOPE: only the other two sessions on this host are supported");
        for (name, peer) in &settings.peers {
            ensure!(session(name)? == *name && name != own_session, "SCOPE: peer must name a different canonical session");
            ensure!(peer.target.starts_with("clipd.") && peer.target.ends_with(".bus") && peer.target.len() <= 255 && peer.target.bytes().all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c)), "SCOPE: peer target must be an exact clipd.<node>.bus target");
        }
        let key = if let Some(path) = &settings.scope_key_file {
            // The operator provisions ONE credential for a logical user's
            // sessions on ONE host, including any mapped UID in isolated desks.
            // It is never derived from a hostname, a claimed UID or a wire field.
            let file = std::fs::File::open(path)?;
            let meta = file.metadata()?;
            ensure!(meta.is_file() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0, "SCOPE: key must be a private file owned by this session user");
            use std::io::Read;
            let mut bytes = Vec::new(); file.take(33).read_to_end(&mut bytes)?;
            ensure!(bytes.len() == 32, "SCOPE: key must contain exactly 32 random bytes");
            Some(bytes.try_into().expect("length checked"))
        } else { None };
        ensure!(settings.peers.is_empty() || key.is_some(), "SCOPE: peers require a same-host, same-user scope credential");
        Ok((settings, key))
    }
}

/// Authenticated encryption keeps clipboard bytes confidential even if a
/// configured Bus target is accidentally registered by a different principal.
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    from_session: String,
    to_session: String,
    expires: u64,
    nonce: String,
    ciphertext: String,
}
fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
fn aad(from: &str, to: &str, expires: u64, command: &str) -> Vec<u8> {
    serde_json::to_vec(&(from, to, expires, command)).expect("serialisable")
}
pub fn seal(key: &[u8; 32], from: &str, to: &str, command: &str, body: Value) -> Result<Value> {
    let nonce: [u8; 24] = rand::random();
    let expires = now() + 30;
    let cipher = XChaCha20Poly1305::new(key.into());
    let ciphertext = cipher.encrypt(XNonce::from_slice(&nonce), Payload { msg: &serde_json::to_vec(&body)?, aad: &aad(from, to, expires, command) }).map_err(|_| anyhow!("SCOPE: encryption failed"))?;
    Ok(serde_json::to_value(Envelope { from_session: from.into(), to_session: to.into(), expires, nonce: B64.encode(nonce), ciphertext: B64.encode(ciphertext) })?)
}
pub fn open(key: &[u8; 32], own: &str, command: &str, envelope: &Envelope) -> Result<Value> {
    ensure!(envelope.to_session == own && envelope.expires >= now() && envelope.expires <= now() + 30, "SCOPE: wrong session or expired transfer");
    let nonce = B64.decode(&envelope.nonce)?;
    ensure!(nonce.len() == 24, "SCOPE: invalid nonce");
    let bytes = XChaCha20Poly1305::new(key.into()).decrypt(XNonce::from_slice(&nonce), Payload { msg: &B64.decode(&envelope.ciphertext)?, aad: &aad(&envelope.from_session, &envelope.to_session, envelope.expires, command) }).map_err(|_| anyhow!("SCOPE: peer is outside this host/user scope"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receive {
    pub stamp: Stamp,
    pub mime_types: Vec<String>,
    pub size: Option<usize>,
    /// Complete eager offer, or no bytes for a lazy offer.
    pub data: Option<BTreeMap<String, String>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fetch {
    pub stamp: Stamp,
    pub mime: String,
    pub offset: usize,
    #[serde(default)]
    pub visited: Vec<String>,
}

pub struct Service {
    pub state: Mutex<Clipboard>,
    pub driver: Driver,
    pub client: Arc<SupervisedClient>,
    pub settings: Settings,
    key: Option<[u8; 32]>,
    nonces: Mutex<VecDeque<(u64, String)>>,
    /// Serialises Wayland mutations and selection classification. Slow reads
    /// use generation fences, rather than holding this lock across I/O.
    mutation: Mutex<()>,
    reads: Mutex<()>,
    pub url: String,
    menu: std::sync::Mutex<Option<std::process::Child>>,
}

async fn call(client: &SupervisedClient, target: &str, command: &str, body: Value) -> Result<Value> {
    match tokio::time::timeout(Duration::from_secs(6), client.call_typed(target, command, body)).await?? {
        PortReply::Ok { rc: 0, value } => Ok(value),
        PortReply::Ok { rc, .. } | PortReply::AppError { rc, .. } => bail!("PEER_REFUSED: {command} returned rc {rc}"),
    }
}

impl Service {
    pub async fn peer_call(&self, peer: &str, command: &str, body: Value) -> Result<Value> {
        let pair = self.settings.peers.get(peer).ok_or_else(|| anyhow!("SCOPE: session is not configured as a same-host peer"))?;
        let key = self.key.as_ref().ok_or_else(|| anyhow!("SCOPE: cross-session sharing is disabled"))?;
        let own = self.state.lock().await.session.clone();
        let reply = call(&self.client, &pair.target, command, seal(key, &own, peer, command, body)?).await?;
        let envelope: Envelope = serde_json::from_value(reply)?;
        ensure!(envelope.from_session == peer, "SCOPE: unexpected reply session");
        open(key, &own, &format!("{command}.reply"), &envelope)
    }

    async fn bytes(&self, selection: &Selection, mime: &str, primary: bool) -> Result<Arc<Vec<u8>>> {
        self.bytes_with_path(selection, mime, primary, &[]).await
    }

    async fn bytes_with_path(&self, selection: &Selection, mime: &str, primary: bool, visited: &[String]) -> Result<Arc<Vec<u8>>> {
        let own = self.state.lock().await.session.clone();
        ensure!(visited.len() < 3 && !visited.contains(&own), "SOURCE_CYCLE: clipboard fetch returned to a visited session");
        ensure!(selection.mime_types.iter().any(|m| m == mime), "MIME: type is not offered");
        if let Some(bytes) = selection.data.get(mime) { return Ok(bytes.clone()); }
        // One materialisation at a time; recheck the live cache after acquiring.
        let _read = self.reads.lock().await;
        {
            let state = self.state.lock().await;
            let current = state.selections[usize::from(primary)].as_ref().ok_or_else(|| anyhow!("SOURCE_GONE: selection cleared"))?;
            ensure!(current.stamp == selection.stamp, "SOURCE_GONE: selection changed");
            if let Some(bytes) = current.data.get(mime) { return Ok(bytes.clone()); }
        }
        let bytes = if let Some(remote) = &selection.remote {
            let mut bytes = Vec::new();
            let mut path = visited.to_vec();
            path.push(own);
            let started = std::time::Instant::now();
            loop {
                ensure!(started.elapsed() < Duration::from_secs(30), "SOURCE_TIMEOUT: lazy clipboard deadline exceeded");
                let part = self.peer_call(&remote.session, "clipboard.fetch", json!(Fetch { stamp: remote.stamp.clone(), mime: mime.into(), offset: bytes.len(), visited: path.clone() })).await.map_err(|_| anyhow!("SOURCE_GONE: lazy clipboard source is unavailable"))?;
                let chunk = B64.decode(part["data"].as_str().ok_or_else(|| anyhow!("SOURCE_GONE: malformed chunk"))?)?;
                ensure!(chunk.len() <= CHUNK && bytes.len() + chunk.len() <= MAX_BYTES, "SIZE_CAP: remote clipboard exceeds limit");
                let eof = part["eof"].as_bool().ok_or_else(|| anyhow!("SOURCE_GONE: missing end marker"))?;
                ensure!(!chunk.is_empty() || eof, "SOURCE_GONE: empty intermediate chunk");
                bytes.extend(chunk);
                if eof { break bytes; }
            }
        } else {
            self.driver.read(selection.offer.clone().ok_or_else(|| anyhow!("SOURCE_GONE: selection owner is gone"))?, mime.into(), MAX_BYTES).await?
        };
        let mut state = self.state.lock().await;
        let current = state.selections[usize::from(primary)].as_mut().ok_or_else(|| anyhow!("SOURCE_GONE: selection cleared"))?;
        ensure!(current.stamp == selection.stamp, "SOURCE_GONE: selection changed during read");
        current.cache(mime.into(), bytes)
    }

    pub async fn set(&self, selection: Selection, primary: bool, clear: bool) -> Result<bool> {
        self.apply(selection, primary, clear, true).await
    }

    async fn apply(&self, selection: Selection, primary: bool, clear: bool, remember: bool) -> Result<bool> {
        let _mutation = self.mutation.lock().await;
        let mut state = self.state.lock().await;
        if state.is_echo(&selection.stamp) { return Ok(false); }
        state.observed[usize::from(primary)] = self.driver.set(&selection, primary, clear).await?;
        state.adopt(primary, selection);
        if remember { state.remember(primary); }
        drop(state);
        self.changed(primary).await;
        Ok(true)
    }

    async fn changed(&self, primary: bool) {
        let state = self.state.lock().await;
        let body = json!(state.metadata(primary));
        drop(state);
        let mut event = bus::BusMessage::new().with_header("command", "clipboard.changed");
        event.body = serde_json::to_string(&body).unwrap_or_default();
        let headers = BTreeMap::from([("name".into(), "clipboard.changed".into())]);
        if let Err(error) = self.client.send_with_headers("noded", "topic.publish", &headers, &event.to_string()).await {
            tracing::warn!(%error, "clipboard metadata publication unavailable");
        }
    }

    async fn begin_observe(&self, primary: bool, offer: Option<crate::wayland::Offer>, mut types: Vec<String>, sequence: u64) -> Option<Selection> {
        let _mutation = self.mutation.lock().await;
        let mut state = self.state.lock().await;
        if sequence <= state.observed[usize::from(primary)] { return None; }
        state.observed[usize::from(primary)] = sequence;
        if state.selections[usize::from(primary)].as_ref().is_some_and(|s| types.contains(&s.stamp.marker())) { return None; }
        types.retain(|m| !m.starts_with(MARKER));
        // Invalid or excessive lists are represented as a closed, sensitive
        // selection, rather than leaving the previous clipboard accessible.
        let invalid = !types.is_empty() && validate_mimes(&types).is_err();
        if invalid { types.clear(); }
        let stamp = Stamp::new(&state.session);
        let size = offer.is_none().then_some(0);
        let selection = Selection { stamp: stamp.clone(), sensitive: invalid || types.iter().any(|m| m == SECRET_HINT), mime_types: types, size, data: BTreeMap::new(), offer, remote: None };
        state.adopt(primary, selection.clone());
        drop(state);
        self.changed(primary).await;
        Some(selection)
    }

    async fn classify(self: &Arc<Self>, primary: bool, selection: Selection) {
        let stamp = selection.stamp.clone();
        // Unknown hints fail closed. No bytes enter history until classification.
        let sensitive = if selection.mime_types.iter().any(|m| m == SECRET_HINT) {
            match selection.offer.clone() {
                Some(offer) => self.driver.read(offer, SECRET_HINT.into(), 256).await.map_or(true, |v| v == b"secret"),
                None => true,
            }
        } else { selection.sensitive };
        let text = if !sensitive && let (Some(mime), Some(offer)) = (selection.text_mime(), selection.offer.clone()) {
            self.driver.read(offer, mime.clone(), TEXT_CAP).await.ok().map(|data| (mime, data))
        } else { None };
        let mut state = self.state.lock().await;
        if let Some(current) = state.selections[usize::from(primary)].as_mut().filter(|s| s.stamp == stamp) {
            current.sensitive = sensitive;
            if let Some((mime, data)) = text { current.size = Some(data.len()); let _ = current.cache(mime, data); }
            state.remember(primary);
        } else { return; }
        drop(state);
        self.changed(primary).await;
        if !primary && !sensitive {
            for (peer, pair) in &self.settings.peers { if pair.follow && let Err(error) = self.push(peer).await { tracing::warn!(%error, "clipboard follow unavailable"); } }
        }
    }

    async fn push(&self, to: &str) -> Result<Value> {
        let to = session(to)?;
        let pair = self.settings.peers.get(&to).ok_or_else(|| anyhow!("SCOPE: destination is not configured on this host"))?;
        let s = self.state.lock().await.selections[0].clone().ok_or_else(|| anyhow!("EMPTY: clipboard has no selection"))?;
        s.can_export(pair.images)?;
        // Only complete text-only offers travel eagerly. Mixed offers preserve
        // their complete list and fetch every type lazily on paste.
        let mut budget = self.settings.eager_text_bytes;
        let eager: BTreeMap<_, _> = s.data.iter().filter_map(|(mime, bytes)| {
            if (mime.starts_with("text/") || ["UTF8_STRING", "STRING", "TEXT"].contains(&mime.as_str())) && bytes.len() <= budget && std::str::from_utf8(bytes).is_ok() {
                budget -= bytes.len();
                Some((mime.clone(), B64.encode(bytes.as_slice())))
            } else { None }
        }).collect();
        let data = (!eager.is_empty()).then_some(eager);
        self.peer_call(&to, "clipboard.receive", json!(Receive { stamp: s.stamp, mime_types: s.mime_types, size: s.size, data })).await
    }

    async fn remote(&self, command: &IncomingCommand) -> Result<Value> {
        let envelope: Envelope = serde_json::from_value(command.args.clone())?;
        let own = self.state.lock().await.session.clone();
        let peer = self.settings.peers.get(&envelope.from_session).ok_or_else(|| anyhow!("SCOPE: sender is not a same-host peer"))?;
        let key = self.key.as_ref().ok_or_else(|| anyhow!("SCOPE: sharing disabled"))?;
        let body = open(key, &own, &command.command, &envelope)?;
        {
            let mut nonces = self.nonces.lock().await;
            nonces.retain(|(until, _)| *until >= now());
            ensure!(!nonces.iter().any(|(_, n)| n == &envelope.nonce), "SCOPE: replayed transfer");
            ensure!(nonces.len() < 4096, "SCOPE: transfer rate limit");
            nonces.push_back((envelope.expires, envelope.nonce.clone()));
        }
        let result = match command.command.as_str() {
            "clipboard.receive" => {
                let r: Receive = serde_json::from_value(body)?;
                if r.mime_types.is_empty() { ensure!(r.size == Some(0), "MIME: empty offer must describe a clear"); } else { validate_mimes(&r.mime_types)?; }
                ensure!(session(&r.stamp.origin_session)? == r.stamp.origin_session && uuid::Uuid::parse_str(&r.stamp.generation).is_ok(), "SCOPE: invalid origin generation");
                // The authenticated sender classified any KDE hint before
                // exporting. A non-secret hint preserves its MIME; every lazy
                // fetch repeats the source's sensitive-selection guard.
                let mut data = BTreeMap::new();
                if let Some(items) = r.data {
                    ensure!(items.len() <= r.mime_types.len(), "MIME: excessive eager types");
                    let mut size = 0usize;
                    for (mime, encoded) in items {
                        ensure!(r.mime_types.contains(&mime), "MIME: unadvertised eager type");
                        ensure!(mime.starts_with("text/") || ["UTF8_STRING", "TEXT", "STRING"].contains(&mime.as_str()), "MIME: only text is eager");
                        let bytes = B64.decode(encoded)?;
                        std::str::from_utf8(&bytes)?;
                        size = size.saturating_add(bytes.len());
                        ensure!(size <= self.settings.eager_text_bytes, "SIZE_CAP: eager transfer exceeds text cap");
                        data.insert(mime, Arc::new(bytes));
                    }
                }
                let lazy = data.len() < r.mime_types.len();
                let s = Selection { stamp: r.stamp.clone(), mime_types: r.mime_types, sensitive: false, size: r.size, data, offer: None, remote: lazy.then(|| Remote { session: envelope.from_session.clone(), stamp: r.stamp }) };
                ensure!(peer.images || !s.mime_types.iter().any(|m| m.starts_with("image/")), "IMAGES_DISABLED: receiving pair forbids images");
                ensure!(s.size.is_none_or(|n| n <= MAX_BYTES), "SIZE_CAP: advertised size exceeds 64 MiB");
                let clear = s.mime_types.is_empty();
                json!({"applied": self.set(s, false, clear).await?, "lazy": lazy})
            }
            "clipboard.fetch" => {
                let r: Fetch = serde_json::from_value(body)?;
                let s = self.state.lock().await.selections[0].clone().ok_or_else(|| anyhow!("SOURCE_GONE: selection cleared"))?;
                ensure!(s.stamp == r.stamp, "SOURCE_GONE: source generation replaced");
                s.can_export(peer.images)?;
                let bytes = self.bytes_with_path(&s, &r.mime, false, &r.visited).await?;
                ensure!(r.offset <= bytes.len(), "OFFSET: beyond selection");
                let end = r.offset.saturating_add(CHUNK).min(bytes.len());
                json!({"data": B64.encode(&bytes[r.offset..end]), "eof": end == bytes.len()})
            }
            _ => bail!("UNKNOWN_COMMAND"),
        };
        seal(key, &own, &envelope.from_session, &format!("{}.reply", command.command), result)
    }

    pub async fn dispatch(&self, command: &IncomingCommand) -> Result<Value> {
        if ["clipboard.receive", "clipboard.fetch"].contains(&command.command.as_str()) { return self.remote(command).await; }
        if ["HELP", "app.describe", "clipboard.info"].contains(&command.command.as_str()) {
            return if command.command == "clipboard.info" {
                let state = self.state.lock().await;
                Ok(json!({"session": state.session, "protocol": self.driver.protocol, "service": "clipd", "clipboard":state.metadata(false), "primary":state.metadata(true), "history_entries":state.history.len(), "history_limit": HISTORY_LEN, "eager_text_bytes": self.settings.eager_text_bytes, "max_bytes": MAX_BYTES, "peers": self.settings.peers}))
            } else { Ok(json!(manifest())) };
        }
        ensure!(command.header("broker_origin") == Some("local"), "SESSION_ONLY: use clipboard.push to share with another local session");
        let result = match command.command.as_str() {
            "clipboard.get" => {
                let p: Get = if command.args.is_null() && command.body.is_empty() { Get::default() } else { serde_json::from_value(command.args.clone())? };
                let s = self.state.lock().await.selections[usize::from(p.primary)].clone().ok_or_else(|| anyhow!("EMPTY: no selection"))?;
                let mime = p.mime.or_else(|| s.text_mime()).or_else(|| s.mime_types.first().cloned()).ok_or_else(|| anyhow!("EMPTY: no offered types"))?;
                let bytes = self.bytes(&s, &mime, p.primary).await?;
                ensure!(p.offset <= bytes.len(), "OFFSET: beyond selection");
                let end = p.offset.saturating_add(TEXT_CAP).min(bytes.len());
                let (encoding, data) = match std::str::from_utf8(&bytes[p.offset..end]) { Ok(text) => ("utf8", text.into()), Err(_) => ("base64", B64.encode(&bytes[p.offset..end])) };
                Ok(json!({"mime": mime, "data": data, "encoding": encoding, "size": bytes.len(), "offset": p.offset, "next_offset": end, "eof": end == bytes.len(), "sensitive": s.sensitive, "stamp": s.stamp}))
            }
            "clipboard.set" => {
                let p: Set = serde_json::from_value(command.args.clone())?;
                validate_mimes(std::slice::from_ref(&p.mime))?;
                let data = match p.encoding.as_str() { "utf8" => p.data.into_bytes(), "base64" => B64.decode(p.data)?, _ => bail!("ENCODING: expected utf8 or base64") };
                ensure!(data.len() <= TEXT_CAP, "SIZE_CAP: Bus sets are limited to 1 MiB");
                let own = self.state.lock().await.session.clone();
                let stamp = p.stamp.unwrap_or_else(|| Stamp::new(&own));
                ensure!(stamp.origin_session == own && uuid::Uuid::parse_str(&stamp.generation).is_ok(), "ORIGIN: local sets must originate in this session");
                let sensitive = p.sensitive || (p.mime == SECRET_HINT && data == b"secret");
                Ok(json!({"applied": self.set(Selection::owned(stamp, p.mime, data, sensitive), p.primary, false).await?}))
            }
            "clipboard.clear" => {
                let p: Clear = if command.args.is_null() && command.body.is_empty() { Clear::default() } else { serde_json::from_value(command.args.clone())? };
                let own = self.state.lock().await.session.clone();
                let s = Selection { stamp: Stamp::new(&own), mime_types: Vec::new(), sensitive: false, size: Some(0), data: BTreeMap::new(), offer: None, remote: None };
                Ok(json!({"applied": self.set(s, p.primary, true).await?}))
            }
            "clipboard.history" => Ok(json!(self.state.lock().await.history_view())),
            "clipboard.pick" | "desktop.clipboard.rotate" => {
                let mut state = self.state.lock().await;
                let rotate = command.command == "desktop.clipboard.rotate";
                let requested = if rotate { None } else { Some(serde_json::from_value::<Pick>(command.args.clone())?) };
                let n = requested.as_ref().map_or_else(|| (state.rotate + 1) % state.history.len().max(1), |p| p.n);
                let entry = state.history.get(n).cloned().ok_or_else(|| anyhow!("HISTORY: index out of range"))?;
                ensure!(requested.as_ref().and_then(|p| p.expected_id.as_ref()).is_none_or(|id| id == &entry.id), "HISTORY_CHANGED: refresh the picker before choosing");
                let selection = Selection::owned(Stamp::new(&state.session), entry.mime, entry.data.into_bytes(), false);
                drop(state);
                // Preserve ring order while rotating; choosing a row is explicit.
                self.apply(selection, false, false, false).await?;
                state = self.state.lock().await;
                state.rotate = if rotate { n } else { 0 };
                Ok(json!({"picked": n}))
            }
            "clipboard.push" => self.push(&serde_json::from_value::<Push>(command.args.clone())?.to).await,
            "desktop.clipboard.menu" => {
                if self.state.lock().await.session == "vt1" {
                    // Phase 2 keeps Plasma's existing history UI. Only this
                    // presentation request uses Klipper's existing session API;
                    // clipboard ownership and cross-VT data remain native Bus.
                    tokio::time::timeout(Duration::from_secs(3), async {
                        let connection = zbus::Connection::session().await?;
                        let proxy = zbus::Proxy::new(&connection, "org.kde.klipper", "/klipper", "org.kde.klipper.klipper").await?;
                        proxy.call::<_, _, ()>("showKlipperPopupMenu", &()).await
                    }).await??;
                    return Ok(json!({"opened": true, "ui": "klipper"}));
                }
                let mut menu = self.menu.lock().unwrap();
                if let Some(child) = menu.as_mut() && child.try_wait()?.is_none() { return Ok(json!({"opened": false, "already_open": true})); }
                let executable = paths::resolve("mixos:prefix/bin/clipd").map_err(anyhow::Error::msg)?;
                *menu = Some(std::process::Command::new(executable).arg("--menu").arg("--bus").arg(&self.url).spawn()?);
                Ok(json!({"opened": true}))
            }
            _ => bail!("UNKNOWN_COMMAND: unsupported clipboard command"),
        };
        if result.is_ok()
            && command.args.get("primary").and_then(Value::as_bool) != Some(true)
            && self.state.lock().await.selections[0].as_ref().is_some_and(|s| !s.sensitive)
            && matches!(command.command.as_str(), "clipboard.set" | "clipboard.clear" | "clipboard.pick" | "desktop.clipboard.rotate") {
            for (peer, pair) in &self.settings.peers {
                if pair.follow && let Err(error) = self.push(peer).await { tracing::warn!(%error, "clipboard follow unavailable"); }
            }
        }
        result
    }
}

pub async fn run(session_name: String, url: String, settings: Settings, key: Option<[u8; 32]>, seat: Option<String>) -> Result<()> {
    let (driver, mut events) = Driver::start(seat)?;
    let build = buildinfo::build_info!();
    let client = Arc::new(SupervisedClient::connect_options("clipd", &url).bounded_incoming(64).fatal_on_registration_rejection(true).with_commands(manifest()).with_provenance(bus::RegisterProvenance::from_parts(build.pkg, build.version, build.git_sha, build.git_dirty, build.build_time, buildinfo::now_rfc3339())).start());
    let mut incoming = client.incoming_bounded().ok_or_else(|| anyhow!("Bus receiver already taken"))?;
    let service = Arc::new(Service { state: Mutex::new(Clipboard::new(session_name)), driver, client: client.clone(), settings, key, nonces: Mutex::new(VecDeque::new()), mutation: Mutex::new(()), reads: Mutex::new(()), url, menu: std::sync::Mutex::new(None) });
    let permits = Arc::new(Semaphore::new(16));
    let mut tasks = tokio::task::JoinSet::new();
    let mut observations: [Option<tokio::task::AbortHandle>; 2] = [None, None];
    let mut connection = client.subscribe_state();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let result = loop {
        tokio::select! {
            _ = sigterm.recv() => break Ok(()),
            _ = tokio::signal::ctrl_c() => break Ok(()),
            changed = connection.changed() => {
                if changed.is_err() { break Err(anyhow!("Bus supervisor closed")); }
                let state = *connection.borrow_and_update();
                match state {
                    bus::native_client::ConnState::Connected => {
                        service.changed(false).await;
                        service.changed(true).await;
                    }
                    bus::native_client::ConnState::Fatal => break Err(anyhow!("Bus registration rejected")),
                    _ => {}
                }
            },
            done = tasks.join_next(), if !tasks.is_empty() => { if let Some(Err(error)) = done && !error.is_cancelled() { tracing::warn!(%error, "clipboard task failed"); } },
            event = incoming.recv() => match event {
                Some(BoundedIncomingEvent::Command(command)) if !command.is_topic_delivery() => {
                    if let Ok(permit) = permits.clone().try_acquire_owned() {
                        let service = service.clone();
                        tasks.spawn(async move {
                            let _permit = permit;
                            let (rc, body) = match service.dispatch(&command).await { Ok(value) => (0, value), Err(error) => (10, json!({"error": error.to_string()})) };
                            let _ = service.client.respond(&command, rc, &body.to_string()).await;
                        });
                    } else { let _ = client.respond(&command, 10, "{\"error\":\"BUSY\"}").await; }
                }
                Some(BoundedIncomingEvent::Overflow { .. }) => tracing::warn!("clipboard Bus queue overflow; requesters must retry queries"),
                Some(_) => {},
                None => break Err(anyhow!("Bus command receiver closed")),
            },
            event = events.recv() => match event {
                Some(Event::Selection { primary, offer, types, sequence }) => {
                    // Adoption is ordered by the event loop. At most one
                    // classification per selection remains in flight.
                    if let Some(selection) = service.begin_observe(primary, offer, types, sequence).await {
                        if let Some(old) = observations[usize::from(primary)].take() { old.abort(); }
                        let service = service.clone();
                        observations[usize::from(primary)] = Some(tasks.spawn(async move { service.classify(primary, selection).await; }));
                    }
                }
                Some(Event::Send { stamp, primary, mime, fd }) => {
                    if let Ok(permit) = permits.clone().try_acquire_owned() {
                        let service = service.clone();
                        tasks.spawn(async move {
                            let _permit = permit;
                            let selection = service.state.lock().await.selections[usize::from(primary)].clone().filter(|s| s.stamp == stamp);
                            let result = async {
                                let selection = selection.ok_or_else(|| anyhow!("SOURCE_GONE: replaced clipboard"))?;
                                let bytes = service.bytes(&selection, &mime, primary).await?;
                                crate::wayland::write_fd(fd, &bytes).await
                            }.await;
                            if let Err(error) = result { tracing::warn!(%error, "clipboard paste failed; transfer fd closed"); }
                        });
                    }
                }
                Some(Event::Failed(error)) => break Err(anyhow!(error)),
                None => break Err(anyhow!("Wayland connection closed")),
            }
        }
    };
    service.driver.stop();
    tasks.abort_all();
    let _ = tokio::time::timeout(Duration::from_secs(2), client.deregister()).await;
    client.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Desk {
        service: Arc<Service>,
        reader: Driver,
        events: tokio::sync::mpsc::Receiver<Event>,
        changes: tokio::sync::mpsc::Receiver<(bool, Vec<String>)>,
        tasks: Vec<tokio::task::JoinHandle<()>>,
        stop: std::sync::mpsc::Sender<()>,
        server: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for Desk {
        fn drop(&mut self) {
            self.service.driver.stop(); self.reader.stop();
            for task in &self.tasks { task.abort(); }
            let _ = self.stop.send(());
            if let Some(server) = self.server.take() { let _ = server.join(); }
        }
    }
    impl Desk {
        async fn start(session: &str, peer: &str, url: &str) -> Self {
            let (ready, connections) = std::sync::mpsc::sync_channel(1);
            let (stop, stopped) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                let mut harness = testkit::Harness::new();
                ready.send((harness.connect_external(), harness.connect_external())).unwrap();
                while matches!(stopped.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
                    harness.pump(); std::thread::sleep(Duration::from_millis(1));
                }
            });
            let (owner, reader) = connections.recv_timeout(Duration::from_secs(5)).unwrap();
            let (driver, mut events) = Driver::start_connection(wayland_client::Connection::from_socket(owner).unwrap(), None).unwrap();
            let (reader, observed) = Driver::start_connection(wayland_client::Connection::from_socket(reader).unwrap(), None).unwrap();
            let client = Arc::new(SupervisedClient::connect_options(&format!("clipd-{session}"), url).bounded_incoming(64).with_commands(manifest()).connect().await.unwrap());
            let mut incoming = client.incoming_bounded().unwrap();
            // Two real registered services on the test broker stand in for the
            // two exact mesh routes; requests still traverse native noded.
            let settings = Settings { peers: BTreeMap::from([(peer.into(), Peer { target: format!("clipd-{peer}"), images: true, follow: false })]), ..Default::default() };
            let service = Arc::new(Service { state: Mutex::new(Clipboard::new(session.into())), driver, client, settings, key: Some([7; 32]), nonces: Mutex::new(VecDeque::new()), mutation: Mutex::new(()), reads: Mutex::new(()), url: url.into(), menu: std::sync::Mutex::new(None) });
            let rpc = service.clone();
            let calls = tokio::spawn(async move {
                while let Some(BoundedIncomingEvent::Command(command)) = incoming.recv().await {
                    let (rc, value) = match rpc.dispatch(&command).await { Ok(value) => (0, value), Err(error) => (10, json!({"error":error.to_string()})) };
                    let _ = rpc.client.respond(&command, rc, &value.to_string()).await;
                }
            });
            let owner = service.clone();
            let (changed, changes) = tokio::sync::mpsc::channel(16);
            let sends = tokio::spawn(async move {
                let mut classifiers = tokio::task::JoinSet::new();
                while let Some(event) = events.recv().await {
                    match event {
                        Event::Selection { primary, offer, types, sequence } => {
                            let observed_types = types.clone();
                            if let Some(selection) = owner.begin_observe(primary, offer, types, sequence).await {
                                let owner = owner.clone();
                                let changed = changed.clone();
                                classifiers.spawn(async move {
                                    owner.classify(primary, selection).await;
                                    let _ = changed.send((primary, observed_types)).await;
                                });
                            }
                        }
                        Event::Send { stamp, primary, mime, fd } => {
                            let selection = owner.state.lock().await.selections[usize::from(primary)].clone().filter(|s| s.stamp == stamp);
                            if let Some(s) = selection && let Ok(bytes) = owner.bytes(&s, &mime, primary).await { let _ = crate::wayland::write_fd(fd, &bytes).await; }
                        }
                        Event::Failed(error) => panic!("{error}"),
                    }
                }
            });
            Self { service, reader, events: observed, changes, tasks: vec![calls, sends], stop, server: Some(server) }
        }
        async fn copy_external(&mut self, selection: Selection, primary: bool) {
            self.reader.set(&selection, primary, false).await.unwrap();
            let marker = selection.stamp.marker();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    tokio::select! {
                        event = self.events.recv() => match event.expect("external clipboard connection closed") {
                            Event::Send { stamp, mime, fd, .. } if stamp == selection.stamp => {
                                if let Some(bytes) = selection.data.get(&mime) { crate::wayland::write_fd(fd, bytes).await.unwrap(); }
                            }
                            Event::Failed(error) => panic!("{error}"),
                            _ => {}
                        },
                        update = self.changes.recv() => {
                            let (got_primary, types) = update.expect("selection observer closed");
                            if got_primary == primary && types.contains(&marker) { break; }
                        }
                    }
                }
            }).await.unwrap();
        }
        async fn paste(&mut self, marker: &str, mime: &str) -> Vec<u8> {
            let offer = tokio::time::timeout(Duration::from_secs(5), async {
                loop { match self.events.recv().await {
                    Some(Event::Selection { offer: Some(offer), types, primary: false, .. }) if types.iter().any(|t| t == marker) => break offer,
                    Some(Event::Failed(error)) => panic!("{error}"),
                    None => panic!("Wayland reader closed"),
                    _ => {}
                } }
            }).await.unwrap();
            self.reader.read(offer, mime.into(), MAX_BYTES).await.unwrap()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn push_is_eager_for_text_lazy_for_images_and_closes_when_source_disappears() {
        let broker = test_broker::Broker::start();
        let source = Desk::start("vt1", "vt4", &broker.url).await;
        let mut target = Desk::start("vt4", "vt1", &broker.url).await;
        let text = Selection::owned(Stamp::new("vt1"), "text/plain".into(), b"eager text".to_vec(), false);
        source.service.set(text.clone(), false, false).await.unwrap();
        assert_eq!(source.service.push("vt4").await.unwrap()["lazy"], false);
        assert_eq!(target.paste(&text.stamp.marker(), "text/plain").await, b"eager text");

        let image_bytes: Vec<u8> = (0u8..=255).cycle().take(CHUNK * 2 + 13).collect();
        let image = Selection::owned(Stamp::new("vt1"), "image/png".into(), image_bytes.clone(), false);
        source.service.set(image.clone(), false, false).await.unwrap();
        assert_eq!(source.service.push("vt4").await.unwrap()["lazy"], true);
        assert!(target.service.state.lock().await.selections[0].as_ref().unwrap().data.is_empty());
        let current = target.service.state.lock().await.selections[0].clone().unwrap();
        assert!(target.service.bytes_with_path(&current, "image/png", false, &["vt4".into()]).await.unwrap_err().to_string().contains("SOURCE_CYCLE"));
        assert_eq!(target.paste(&image.stamp.marker(), "image/png").await, image_bytes);
        let get = call(&source.service.client, "clipd-vt4", "clipboard.get", json!({"mime":"image/png"})).await.unwrap();
        assert_eq!(get["encoding"], "base64");
        assert_eq!(B64.decode(get["data"].as_str().unwrap()).unwrap(), image_bytes);

        let mut command = IncomingCommand { generation:0, from:"fixture".into(), command:"clipboard.receive".into(), id:None, args:seal(&[7;32], "vt1", "vt4", "clipboard.receive", json!(Receive { stamp:image.stamp.clone(), mime_types:image.mime_types.clone(), size:image.size, data:None })).unwrap(), body:String::new(), headers:BTreeMap::new() };
        assert!(target.service.dispatch(&command).await.is_ok());
        assert!(target.service.dispatch(&command).await.unwrap_err().to_string().contains("replayed"));
        command.command = "clipboard.set".into();
        command.args = json!({"mime":"text/plain", "data":"x".repeat(TEXT_CAP + 1)});
        command.headers.insert("broker_origin".into(), "local".into());
        assert!(source.service.dispatch(&command).await.unwrap_err().to_string().contains("SIZE_CAP"));
        command.headers.insert("broker_origin".into(), "mesh".into());
        command.args = json!({"mime":"text/plain", "data":"remote"});
        assert!(source.service.dispatch(&command).await.unwrap_err().to_string().contains("SESSION_ONLY"));

        let secret = Selection::owned(Stamp::new("vt1"), "text/plain".into(), b"secret".to_vec(), true);
        source.service.set(secret, false, false).await.unwrap();
        assert!(source.service.push("vt4").await.unwrap_err().to_string().contains("SENSITIVE"));
        assert!(source.service.push("vt3").await.is_err());

        let gone = Selection::owned(Stamp::new("vt1"), "image/png".into(), vec![7; CHUNK + 1], false);
        source.service.set(gone.clone(), false, false).await.unwrap();
        source.service.push("vt4").await.unwrap();
        source.service.client.close().await;
        let selection = target.service.state.lock().await.selections[0].clone().unwrap();
        assert!(target.service.bytes(&selection, "image/png", false).await.unwrap_err().to_string().contains("SOURCE_GONE"));
        assert!(target.paste(&gone.stamp.marker(), "image/png").await.is_empty());
        target.service.client.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn wayland_watcher_classifies_kde_hints_for_clipboard_and_primary() {
        let broker = test_broker::Broker::start();
        let mut source = Desk::start("vt1", "vt4", &broker.url).await;
        let mut target = Desk::start("vt4", "vt1", &broker.url).await;
        let mut public = Selection::owned(Stamp::new("vt1"), "text/plain".into(), b"public text".to_vec(), false);
        public.mime_types.push(SECRET_HINT.into());
        public.data.insert(SECRET_HINT.into(), Arc::new(b"public".to_vec()));
        source.copy_external(public, false).await;
        assert!(!source.service.state.lock().await.selections[0].as_ref().unwrap().sensitive);
        assert_eq!(source.service.state.lock().await.history[0].data, "public text");
        let stamp = source.service.state.lock().await.selections[0].as_ref().unwrap().stamp.clone();
        source.service.push("vt4").await.unwrap();
        assert_eq!(target.paste(&stamp.marker(), "text/plain").await, b"public text");
        for primary in [false, true] {
            source.copy_external(Selection::owned(Stamp::new("vt1"), "text/plain".into(), b"private text".to_vec(), true), primary).await;
            assert!(source.service.state.lock().await.selections[usize::from(primary)].as_ref().unwrap().sensitive);
            assert!(source.service.state.lock().await.history.iter().all(|e| e.data != "private text"));
        }
        assert!(source.service.push("vt4").await.unwrap_err().to_string().contains("SENSITIVE"));
        source.service.client.close().await;
        target.service.client.close().await;
    }
    #[test]
    fn scope_is_cryptographic_and_bound_to_destination_and_command() {
        let value = seal(&[7; 32], "vt1", "vt4", "clipboard.receive", json!({"data":"private"})).unwrap();
        let envelope: Envelope = serde_json::from_value(value).unwrap();
        assert_eq!(open(&[7; 32], "vt4", "clipboard.receive", &envelope).unwrap()["data"], "private");
        assert!(open(&[8; 32], "vt4", "clipboard.receive", &envelope).is_err());
        assert!(open(&[7; 32], "vt3", "clipboard.receive", &envelope).is_err());
        assert!(open(&[7; 32], "vt4", "clipboard.fetch", &envelope).is_err());
    }
    #[test]
    fn pair_settings_require_a_private_scope_key_and_bounded_caps() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard.conf.mix");
        let key = dir.path().join("scope.key");
        let mut settings = Settings { scope_key_file: Some(key.clone()), peers: BTreeMap::from([("vt4".into(), Peer { target:"clipd.alpha.example.bus".into(), images:false, follow:false })]), ..Default::default() };
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        assert!(Settings::load(Some(&path), "vt1").is_err());
        std::fs::write(&key, [7;32]).unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Settings::load(Some(&path), "vt1").is_err());
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Settings::load(Some(&path), "vt1").is_ok());
        settings.eager_text_bytes = TEXT_CAP + 1;
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        assert!(Settings::load(Some(&path), "vt1").is_err());
        settings.eager_text_bytes = TEXT_CAP;
        settings.peers.insert("remote-host".into(), Peer { target:"clipd.beta.example.bus".into(), images:true, follow:true });
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        assert!(Settings::load(Some(&path), "vt1").is_err());
    }
    #[tokio::test]
    async fn lazy_fetch_fails_cleanly_when_source_is_gone() {
        let broker = test_broker::Broker::start();
        let client = SupervisedClient::connect_options("clipd-test", &broker.url).connect().await.unwrap();
        let result = call(&client, "missing-clipd", "clipboard.fetch", json!({})).await;
        assert!(result.is_err());
        client.close().await;
    }
}
