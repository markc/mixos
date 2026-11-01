// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The pages' one channel to settingsd: the values every row shows, the
//! catalogue that shapes each control, and the writes.
//!
//! Every gesture applies at once, as in COSMIC Settings: there is no draft
//! and no global Apply. A write is one fenced `settings.apply` on the
//! revision last read. Writes made while one is in flight wait and go out
//! together when it settles, latest value per key. Every write is an
//! absolute value, so a write whose answer was lost (or that met another
//! writer's revision) is safe to send again after a fresh read: it lands
//! the same value once, never toggles twice.
//!
//! On a mirror the writes are disabled before any click: `settings.status`
//! says whether the mirror is stale, and a write refused as
//! `authority_unreachable` disables them until the next status says the
//! authority is back. Nothing polls: topics and the Bus connection drive
//! every read.
use crate::catalog::{Catalog, Kind};
use crate::engine::{Effect, Engine};
use citizen::{CallError, Reply};
use serde_json::{Value, json};
use settings::model::{ApplyRequest, Binding, Revision};
use settings::registry::Values;
use std::collections::{BTreeMap, VecDeque};

/// Which settingsd the window talks to, as `settings.status` reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    Unknown,
    Authority,
    Mirror,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Job {
    Describe,
    Read,
    Status,
    Apply,
}

impl Job {
    fn command(self) -> &'static str {
        match self {
            Job::Describe => "settings.describe",
            Job::Read => "settings.get",
            Job::Status => "settings.status",
            Job::Apply => "settings.apply",
        }
    }
}

/// Why the pages cannot write now, as a label key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Blocked {
    /// Not read yet (or settingsd is not on the Bus).
    Unread,
    Disconnected,
    /// A mirror whose authority it cannot confirm.
    Stale,
    /// A write was refused because the authority is down.
    AuthorityUnreachable,
}

impl Blocked {
    pub fn label(self) -> &'static str {
        match self {
            Blocked::Unread => "store-unread",
            Blocked::Disconnected => "store-disconnected",
            Blocked::Stale => "store-stale",
            Blocked::AuthorityUnreachable => "store-unreachable",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Store {
    pub catalog: Catalog,
    /// settingsd's own catalogue replaced the built-in one.
    pub described: bool,
    /// The authored values settingsd holds (sparse: defaults come from the
    /// catalogue).
    pub values: Values,
    fence: Option<(String, Revision)>,
    pub role: Role,
    pub stale: bool,
    pub unreachable: bool,
    /// The last refusal, for the page's notice; cleared by the next write.
    pub error: Option<String>,
    /// Writes not sent yet, latest per key.
    pending: BTreeMap<String, Value>,
    /// The write in flight: its operation id and changes.
    inflight: Option<(String, BTreeMap<String, Value>)>,
    job: Option<(u64, Job)>,
    queue: VecDeque<Job>,
    applies: u64,
    retargeted: bool,
    /// Status answers lost while unreachable, retried a bounded number of
    /// times before waiting for a topic or the Bus.
    status_tries: u32,
    /// A write met another revision or lost its answer: nothing more goes
    /// out until a read succeeds, so a dead settingsd is not sent the same
    /// write again and again.
    reread: bool,
}

impl Store {
    pub fn new() -> Self {
        Self {
            catalog: Catalog::builtin(),
            ..Self::default()
        }
    }

    /// The revision last read.
    pub fn revision(&self) -> Option<u64> {
        self.fence.as_ref().map(|(_, r)| r.0)
    }

    /// The keys whose writes have not been accepted yet.
    pub fn waiting(&self) -> Vec<&str> {
        self.pending
            .keys()
            .chain(self.inflight.iter().flat_map(|(_, c)| c.keys()))
            .map(String::as_str)
            .collect()
    }

    /// Whether any write is waiting or in flight.
    pub fn writing(&self) -> bool {
        self.inflight.is_some() || !self.pending.is_empty()
    }

    /// The value a control shows: the latest write, else settingsd's,
    /// else the default.
    pub fn value(&self, key: &str) -> Option<Value> {
        self.pending
            .get(key)
            .or_else(|| self.inflight.as_ref().and_then(|(_, c)| c.get(key)))
            .or_else(|| self.values.get(key))
            .cloned()
            .filter(|v| !v.is_null())
            .or_else(|| self.catalog.default_of(key))
    }

    pub fn text(&self, key: &str) -> Option<String> {
        self.value(key).and_then(|v| v.as_str().map(str::to_owned))
    }

    pub fn flag(&self, key: &str) -> bool {
        self.value(key).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        self.value(key).and_then(|v| v.as_f64())
    }

    /// Whether `key` holds something other than its default.
    pub fn changed(&self, key: &str) -> bool {
        self.value(key) != self.catalog.default_of(key)
    }

    fn enqueue(&mut self, job: Job) {
        if !self.queue.contains(&job) {
            self.queue.push_back(job);
        }
    }

    fn operation(&mut self) -> String {
        self.applies += 1;
        format!(
            "prefs-page-{}-{}-{}",
            std::process::id(),
            self.applies,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        )
    }
}

/// The topics the pages read again on: the look and every key prefix
/// settingsd publishes (a page writes `shell.panels`, `window`, … not
/// only `appearance`).
pub fn topics() -> Vec<String> {
    let profile = crate::appearance::PROFILE;
    std::iter::once(settings::topics::look_topic(profile))
        .chain(
            settings::topics::PREFIXES
                .iter()
                .map(|prefix| settings::topics::keys_topic(profile, prefix)),
        )
        .collect()
}

/// Whether a delivery on `topic` is one of [`topics`].
pub fn is_topic(topic: &str) -> bool {
    topics().iter().any(|t| t == topic)
}

/// Status answers retried while the authority is unreachable.
const STATUS_TRIES: u32 = 3;

/// What a settingsd answer was.
enum Answer {
    Ok(Value),
    Refused(Value),
    Lost(String),
}

fn answer(result: Result<Reply, CallError>) -> Answer {
    match result {
        Ok(reply) => match serde_json::from_str::<Value>(&reply.body) {
            Ok(body) if reply.rc == 0 => Answer::Ok(body),
            Ok(body) if body.is_object() => Answer::Refused(body),
            _ => Answer::Lost(reply.error.unwrap_or(reply.body)),
        },
        Err(error) => Answer::Lost(error.to_string()),
    }
}

fn message(value: &Value) -> String {
    if let Some(list) = value["diagnostics"].as_array() {
        let lines: Vec<String> = list
            .iter()
            .filter_map(|d| {
                Some(format!(
                    "{}: {}",
                    d["path"].as_str()?,
                    d["message"].as_str().unwrap_or_default()
                ))
            })
            .collect();
        if !lines.is_empty() {
            return lines.join("; ");
        }
    }
    value["message"]
        .as_str()
        .or_else(|| value["status"].as_str())
        .map_or_else(|| value.to_string(), str::to_owned)
}

impl Engine {
    /// Start the pages: the catalogue, the values and the role.
    pub fn start_store(&mut self) {
        for job in [Job::Describe, Job::Read, Job::Status] {
            self.store.enqueue(job);
        }
        self.pump_store();
    }

    /// settingsd announced a change, or the Bus came back: read again.
    pub fn store_hint(&mut self) {
        self.store.enqueue(Job::Read);
        self.store.enqueue(Job::Status);
        self.pump_store();
    }

    /// Why a write cannot go out now; `None` when it can.
    pub fn blocked(&self) -> Option<Blocked> {
        if !self.connected {
            Some(Blocked::Disconnected)
        } else if self.store.unreachable {
            Some(Blocked::AuthorityUnreachable)
        } else if self.store.role == Role::Mirror && self.store.stale {
            Some(Blocked::Stale)
        } else if self.store.fence.is_none() || self.store.role == Role::Unknown {
            // Not read, or not yet known to be the authority or a current
            // mirror: no write until it is.
            Some(Blocked::Unread)
        } else {
            None
        }
    }

    /// `value` for `key`, checked against the catalogue settingsd
    /// described (this binary's registry only for shapes the catalogue
    /// cannot state), or the localised refusal.
    fn checked(&self, key: &str, value: Value) -> Result<Value, String> {
        let info = self
            .store
            .catalog
            .get(key)
            .ok_or_else(|| self.label("set-unknown", &[("key", key)]))?;
        if info.live() {
            let owner = info.owner.as_deref().unwrap_or("?");
            return Err(self.label("set-live", &[("key", key), ("owner", owner)]));
        }
        if value.is_null() {
            return Ok(value);
        }
        let value = coerce(&info.kind, value);
        match crate::catalog::check(&info.kind, &value) {
            Some(Ok(())) => Ok(value),
            Some(Err(why)) => Err(self.label("set-invalid", &[("key", key), ("why", &why)])),
            None => settings::registry::check_value(key, &value)
                .map_err(|d| self.label("set-invalid", &[("key", key), ("why", &d.message)])),
        }
    }

    /// Set `key` to `value` at once (fenced); refused with a message when
    /// the key is unknown, live, the value invalid, or writes are blocked.
    pub fn set_key(&mut self, key: &str, value: Value) -> Result<(), String> {
        let value = self.checked(key, value)?;
        if let Some(blocked) = self.blocked() {
            return Err(self.label(blocked.label(), &[]));
        }
        self.store.error = None;
        self.store.pending.insert(key.to_owned(), value);
        self.pump_store();
        Ok(())
    }

    /// Several keys in one write, all or nothing (Reset, cross-writes).
    pub fn set_keys(&mut self, changes: BTreeMap<String, Value>) -> Result<(), String> {
        let mut checked = BTreeMap::new();
        for (key, value) in changes {
            let value = self.checked(&key, value)?;
            checked.insert(key, value);
        }

        if let Some(blocked) = self.blocked() {
            return Err(self.label(blocked.label(), &[]));
        }
        self.store.error = None;
        self.store.pending.extend(checked);
        self.pump_store();
        Ok(())
    }

    fn pump_store(&mut self) {
        if self.store.job.is_some() || !self.connected {
            return;
        }
        let job = match self.store.queue.pop_front() {
            Some(job) => job,
            None if !self.store.pending.is_empty()
                && self.store.fence.is_some()
                && !self.store.reread
                && self.blocked().is_none() =>
            {
                Job::Apply
            }
            None => return,
        };
        let binding = self.look.binding.clone();
        let body = match job {
            Job::Describe => json!({}),
            Job::Read => json!({"binding": binding, "view": "values"}),
            Job::Status => json!({"binding": binding}),
            Job::Apply => {
                let (incarnation, revision) = self.store.fence.clone().expect("a fenced apply");
                let changes = std::mem::take(&mut self.store.pending);
                let operation = self.store.operation();
                let request = ApplyRequest {
                    binding: binding.clone(),
                    expected_incarnation: incarnation,
                    expected_revision: revision,
                    operation_id: operation.clone(),
                    changes: changes.clone(),
                    reset: Vec::new(),
                    request_digest: None,
                };
                self.store.inflight = Some((operation, changes));
                serde_json::to_value(request).expect("an apply request serialises")
            }
        };
        let ticket = self.next();
        self.store.job = Some((ticket, job));
        self.effects.push(Effect::Store {
            ticket,
            command: job.command(),
            body,
        });
    }

    /// The settingsd call `ticket` finished.
    pub fn stored(&mut self, ticket: u64, result: Result<Reply, CallError>) {
        let Some((expected, job)) = self.store.job else {
            return;
        };
        if expected != ticket {
            return;
        }
        self.store.job = None;
        match (job, answer(result)) {
            (Job::Describe, Answer::Ok(body)) => {
                if let Ok(catalog) = Catalog::from_describe(&body)
                    && !catalog.is_empty()
                {
                    self.store.catalog = catalog;
                    self.store.described = true;
                }
            }
            (Job::Describe, _) => {}
            (Job::Read, Answer::Ok(body)) => self.store_read(&body),
            (Job::Read | Job::Status, Answer::Refused(body))
                if body["status"] == "wrong_target" && !self.store.retargeted =>
            {
                if let Ok(binding) = serde_json::from_value::<Binding>(body["binding"].clone())
                    && binding.validate().is_ok()
                    && binding.profile == crate::appearance::PROFILE
                {
                    self.look.binding.instance = binding.instance;
                    self.store.retargeted = true;
                    self.store.enqueue(job);
                }
            }
            (Job::Read, Answer::Refused(body)) => self.store.error = Some(message(&body)),
            (Job::Read, Answer::Lost(text)) => self.store.error = Some(text),
            (Job::Status, Answer::Ok(body)) => self.store_status(&body),
            (Job::Status, _) => {
                if self.store.unreachable && self.store.status_tries < STATUS_TRIES {
                    self.store.status_tries += 1;
                    self.store.enqueue(Job::Status);
                }
            }
            (Job::Apply, Answer::Ok(body)) => self.store_applied(&body),
            (Job::Apply, Answer::Refused(body)) => self.store_refused(&body),
            (Job::Apply, Answer::Lost(_)) => {
                // Absolute values: read, then send them again.
                self.requeue_inflight();
                self.store.enqueue(Job::Read);
            }
        }
        self.pump_store();
    }

    /// Take a `settings.status` answer: the role, and whether a mirror is
    /// stale. A status that answers current says the authority is back.
    pub fn store_status(&mut self, body: &Value) {
        self.store.role = match body["role"].as_str() {
            Some("authority") => Role::Authority,
            Some("mirror") => Role::Mirror,
            _ => Role::Unknown,
        };
        self.store.stale = body["stale"].as_bool().unwrap_or(false);
        self.store.status_tries = 0;
        if self.store.unreachable && (self.store.role == Role::Authority || !self.store.stale) {
            // Back: read, then the edits kept while it was away go out.
            self.store.unreachable = false;
            self.store.error = None;
            self.store.enqueue(Job::Read);
        }
    }

    /// Take a `settings.get {view: values}` answer: the values and the
    /// fence the next write goes out on.
    pub fn store_read(&mut self, body: &Value) {
        let incarnation = body["incarnation"].as_str().map(str::to_owned);
        let revision = serde_json::from_value::<Revision>(body["revision"].clone()).ok();
        let values = serde_json::from_value::<Values>(body["values"].clone());
        match (incarnation, revision, values) {
            (Some(incarnation), Some(revision), Ok(values)) => {
                self.store.values = values;
                self.store.fence = Some((incarnation, revision));
                self.store.reread = false;
                self.refresh_hits();
                // Writes in hand stay (they win over what was read), unless
                // settingsd already holds them.
                let held = &self.store.values;
                self.store.pending.retain(|key, value| {
                    if value.is_null() {
                        held.contains_key(key)
                    } else {
                        settings::registry::effective(held, key).as_ref() != Some(value)
                    }
                });
            }
            _ => self.store.error = Some("settings.get: no values view".into()),
        }
    }

    fn store_applied(&mut self, body: &Value) {
        let Some((_, changes)) = self.store.inflight.take() else {
            return;
        };
        let receipt = &body["receipt"];
        let incarnation = receipt["incarnation"].as_str().map(str::to_owned);
        let revision = serde_json::from_value::<Revision>(receipt["revision"].clone()).ok();
        // The fence held, so settingsd's values are ours with the changes;
        // a read confirms them (settingsd may have normalised a value).
        if let Ok(values) = settings::registry::apply(&self.store.values, &changes, &[]) {
            self.store.values = values;
        }
        self.store.enqueue(Job::Read);
        match (incarnation, revision) {
            (Some(incarnation), Some(revision)) => {
                self.store.fence = Some((incarnation, revision));
            }
            _ => self.store.enqueue(Job::Read),
        }
    }

    fn store_refused(&mut self, body: &Value) {
        match body["status"].as_str() {
            Some("conflict" | "outcome_unknown") => {
                self.requeue_inflight();
                self.store.enqueue(Job::Read);
            }
            Some("authority_unreachable") => {
                // The edits stay, shown, and go out once the authority is
                // back (a status says so, then a read re-fences them).
                self.requeue_inflight();
                self.store.unreachable = true;
                self.store.error = Some(self.label(Blocked::AuthorityUnreachable.label(), &[]));
                self.store.enqueue(Job::Status);
            }
            _ => {
                self.store.inflight = None;
                self.store.error = Some(message(body));
                self.store.enqueue(Job::Read);
            }
        }
    }

    /// Return the write in flight to the pending writes, under any newer
    /// value for the same key.
    fn requeue_inflight(&mut self) {
        self.store.reread = true;
        if let Some((_, changes)) = self.store.inflight.take() {
            for (key, value) in changes {
                self.store.pending.entry(key).or_insert(value);
            }
        }
    }

    /// The Bus went away: the call in flight will not be answered. Its
    /// writes wait, and the next connection reads before they go out.
    pub fn store_disconnected(&mut self) {
        if let Some((_, Job::Apply)) = self.store.job {
            self.requeue_inflight();
        }
        self.store.job = None;
        self.store.reread = true;
    }

    /// The pages' state for `prefs.info` and agents.
    pub fn store_view(&self) -> Value {
        json!({"role": self.store.role, "stale": self.store.stale, "unreachable": self.store.unreachable,
            "blocked": self.blocked(), "revision": self.store.revision().map(|r| r.to_string()),
            "described": self.store.described, "writing": self.store.writing(), "error": self.store.error,
            "waiting": self.store.waiting()})
    }
}

/// Bring a Bus or UI value to the key's shape: a slider's float to an
/// integer key, a number written as text by a macro.
fn coerce(kind: &Kind, value: Value) -> Value {
    match (kind, &value) {
        (Kind::Int { .. } | Kind::Set(_), Value::Number(n)) if n.as_i64().is_none() => n
            .as_f64()
            .map_or(value.clone(), |f| json!(f.round() as i64)),
        (Kind::Int { .. } | Kind::Set(_), Value::String(s)) => {
            s.parse::<i64>().map_or(value.clone(), |i| json!(i))
        }
        (Kind::Number { .. }, Value::String(s)) => {
            s.parse::<f64>().map_or(value.clone(), |f| json!(f))
        }
        (Kind::Bool, Value::String(s)) if s == "true" || s == "false" => json!(s == "true"),
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(body: Value) -> Result<Reply, CallError> {
        Ok(Reply {
            rc: 0,
            body: body.to_string(),
            error: None,
        })
    }

    fn refused(body: Value) -> Result<Reply, CallError> {
        Ok(Reply {
            rc: 10,
            body: body.to_string(),
            error: None,
        })
    }

    /// The store's next call, answered by the caller.
    fn next(e: &mut Engine) -> (u64, &'static str, Value) {
        let found = e
            .take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                Effect::Store {
                    ticket,
                    command,
                    body,
                } => Some((ticket, command, body)),
                _ => None,
            });
        found.expect("a store call")
    }

    fn read(revision: &str, values: Value) -> Value {
        json!({"incarnation":"inc-1","revision":revision,"values":values})
    }

    /// An engine whose store has read revision 7 as the authority.
    pub(crate) fn engine() -> Engine {
        let mut e = Engine::new(|key, _| key.to_owned());
        e.take_effects();
        e.start_store();
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.describe");
        e.stored(t, ok(settings::registry::describe(None)));
        let (t, command, body) = next(&mut e);
        assert_eq!(command, "settings.get");
        assert_eq!(body["view"], "values");
        e.stored(t, ok(read("7", json!({"appearance.mode":"light"}))));
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.status");
        e.stored(t, ok(json!({"role":"authority","stale":false})));
        e
    }

    #[test]
    fn a_write_applies_at_once_fenced_and_later_writes_merge() {
        let mut e = engine();
        assert_eq!(e.store.text("appearance.mode").as_deref(), Some("light"));
        assert_eq!(
            e.store.text("appearance.density").as_deref(),
            Some("comfortable")
        );
        e.set_key("appearance.density", json!("compact")).unwrap();
        let (t, command, body) = next(&mut e);
        assert_eq!(command, "settings.apply");
        assert_eq!(body["expected_revision"], "7");
        assert_eq!(body["changes"], json!({"appearance.density":"compact"}));
        // Shown at once, before the answer.
        assert_eq!(
            e.store.text("appearance.density").as_deref(),
            Some("compact")
        );
        e.set_key("appearance.roundness", json!("square")).unwrap();
        e.set_key("appearance.density", json!("spacious")).unwrap();
        e.stored(
            t,
            ok(json!({"status":"changed","receipt":{"incarnation":"inc-1","revision":"8"}})),
        );
        // The receipt is confirmed by a read before the next write.
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.get");
        e.stored(
            t,
            ok(read(
                "8",
                json!({"appearance.mode":"light","appearance.density":"compact"}),
            )),
        );
        let (_, command, body) = next(&mut e);
        assert_eq!(command, "settings.apply");
        assert_eq!(body["expected_revision"], "8");
        assert_eq!(
            body["changes"],
            json!({"appearance.density":"spacious","appearance.roundness":"square"})
        );
    }

    #[test]
    fn a_conflict_reads_again_and_resends_the_same_value() {
        let mut e = engine();
        e.set_key("window.gaps", json!(8.0)).unwrap();
        let (t, _, body) = next(&mut e);
        assert_eq!(
            body["changes"],
            json!({"window.gaps":8}),
            "coerced to the int key"
        );
        e.stored(t, refused(json!({"status":"conflict","revision":"9"})));
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.get");
        e.stored(t, ok(read("9", json!({}))));
        let (_, command, body) = next(&mut e);
        assert_eq!(command, "settings.apply");
        assert_eq!(body["expected_revision"], "9");
        assert_eq!(body["changes"], json!({"window.gaps":8}));
    }

    #[test]
    fn a_lost_write_waits_for_a_read_before_it_goes_again() {
        let mut e = engine();
        let lost = || Err(CallError::from("timed out".to_owned()));
        e.set_key("appearance.mode", json!("dark")).unwrap();
        let (t, _, _) = next(&mut e);
        e.stored(t, lost());
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.get");
        e.stored(t, lost());
        assert!(e.take_effects().is_empty(), "no resend without a read");
        assert_eq!(e.store.text("appearance.mode").as_deref(), Some("dark"));
        e.store_hint();
        let (t, _, _) = next(&mut e);
        e.stored(t, ok(read("8", json!({"appearance.mode":"light"}))));
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.status");
        e.stored(t, ok(json!({"role":"authority","stale":false})));
        let (_, command, body) = next(&mut e);
        assert_eq!(command, "settings.apply");
        assert_eq!(body["expected_revision"], "8");
    }

    #[test]
    fn a_lost_write_that_landed_is_not_sent_again() {
        let mut e = engine();
        // A reset and a value, both lost; settingsd holds both already.
        e.set_keys(
            [
                ("appearance.mode".to_owned(), Value::Null),
                ("appearance.density".to_owned(), json!("compact")),
            ]
            .into(),
        )
        .unwrap();
        let (t, _, _) = next(&mut e);
        e.stored(t, Err(CallError::from("timed out".to_owned())));
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.get");
        e.stored(t, ok(read("8", json!({"appearance.density":"compact"}))));
        assert!(!e.store.writing(), "{:?}", e.store.waiting());
        assert!(e.take_effects().is_empty());
    }

    #[test]
    fn a_stale_mirror_blocks_writes_before_any_click() {
        let mut e = engine();
        e.store_hint();
        let (t, _, _) = next(&mut e);
        e.stored(t, ok(read("7", json!({}))));
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.status");
        e.stored(t, ok(json!({"role":"mirror","stale":true})));
        assert_eq!(e.blocked(), Some(Blocked::Stale));
        assert!(e.set_key("appearance.mode", json!("dark")).is_err());
        assert!(e.take_effects().is_empty());
    }

    #[test]
    fn an_unknown_role_blocks_writes_until_status_answers() {
        let mut e = Engine::new(|key, _| key.to_owned());
        e.take_effects();
        e.store_read(&read("7", json!({})));
        assert_eq!(e.blocked(), Some(Blocked::Unread));
        e.store_status(&json!({"role":"authority"}));
        assert_eq!(e.blocked(), None);
    }

    #[test]
    fn an_unreachable_authority_keeps_the_edits_and_sends_them_when_back() {
        let mut e = engine();
        e.set_key("appearance.mode", json!("dark")).unwrap();
        let (t, _, _) = next(&mut e);
        e.stored(
            t,
            refused(json!({"status":"authority_unreachable","node":"a"})),
        );
        assert_eq!(e.blocked(), Some(Blocked::AuthorityUnreachable));
        assert_eq!(e.store.text("appearance.mode").as_deref(), Some("dark"));
        assert_eq!(e.store.waiting(), ["appearance.mode"]);
        // One lost status answer is retried, not left hanging.
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.status");
        e.stored(t, Err(CallError::from("timed out".to_owned())));
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.status");
        e.stored(t, ok(json!({"role":"mirror","stale":false})));
        assert_eq!(e.blocked(), None);
        let (t, command, _) = next(&mut e);
        assert_eq!(command, "settings.get");
        e.stored(t, ok(read("9", json!({"appearance.mode":"light"}))));
        let (_, command, body) = next(&mut e);
        assert_eq!(command, "settings.apply");
        assert_eq!(body["expected_revision"], "9");
        assert_eq!(body["changes"], json!({"appearance.mode":"dark"}));
    }

    #[test]
    fn a_disconnect_drops_the_call_and_keeps_its_writes() {
        let mut e = engine();
        e.set_key("appearance.mode", json!("dark")).unwrap();
        let (t, _, _) = next(&mut e);
        e.delivery(citizen::Delivery::Disconnected);
        // A late answer for the dropped call lands nowhere.
        e.stored(
            t,
            ok(json!({"status":"changed","receipt":{"incarnation":"inc-1","revision":"8"}})),
        );
        assert_eq!(e.store.waiting(), ["appearance.mode"]);
        e.delivery(citizen::Delivery::Connected);
        let commands: Vec<&str> = e
            .take_effects()
            .into_iter()
            .filter_map(|x| match x {
                Effect::Store { command, .. } => Some(command),
                _ => None,
            })
            .collect();
        assert_eq!(commands, ["settings.get"], "a read before the resend");
    }

    #[test]
    fn every_page_prefix_is_a_topic_the_pages_read_on() {
        for prefix in ["shell.panels", "window", "appearance", "wallpaper"] {
            let topic = settings::topics::keys_topic(crate::appearance::PROFILE, prefix);
            assert!(is_topic(&topic), "{topic}");
        }
    }

    #[test]
    fn writes_are_checked_against_the_described_catalogue() {
        let mut e = engine();
        // settingsd narrowed the hint width to 0..=8.
        let mut described = settings::registry::describe(None);
        for key in described["keys"].as_array_mut().unwrap() {
            if key["path"] == "window.active_hint" {
                key["max"] = json!(8);
            }
        }
        e.store.catalog = Catalog::from_describe(&described).unwrap();
        assert!(e.set_key("window.active_hint", json!(9)).is_err());
        assert!(e.set_key("window.active_hint", json!(8)).is_ok());
    }

    #[test]
    fn invalid_and_live_writes_are_refused_without_a_call() {
        let mut e = engine();
        assert!(e.set_key("appearance.density", json!("huge")).is_err());
        assert!(e.set_key("network.wifi.enabled", json!(true)).is_err());
        assert!(e.set_key("no.such.key", json!(1)).is_err());
        assert!(e.take_effects().is_empty());
    }
}
