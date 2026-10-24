// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Consent for screen casts. Every `Start` is decided here before anything
//! is captured:
//!
//! 1. policy `allow` grants it (headless and test sessions);
//! 2. else a standing grant that matches the caller's app is used up
//!    (`portald.screencast.grant`, how automation shares without a person);
//! 3. else the person is asked: a notifyd question with Allow and Deny, and
//!    the request waits as a pending prompt. It is decided by the button, by
//!    `portald.screencast.answer` (an agent standing in for the person), or
//!    by the timeout, which denies.
//!
//! The state is plain data behind one lock, so the Bus commands (synchronous)
//! and the D-Bus side (async) share it without awaiting under the lock.
use bus::native_client::{IncomingCommand, SupervisedClient};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;

/// How long a person has to answer before the request is denied.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);
/// Grants a caller may hold at once; more is refused.
pub const MAX_GRANTS: usize = 16;
/// The longest a grant may live, and the default when none is given.
pub const MAX_GRANT_TTL: Duration = Duration::from_secs(3600);
pub const DEFAULT_GRANT_TTL: Duration = Duration::from_secs(300);
/// Requests waiting for a person at once, in all and per caller. More are
/// denied straight away: each holds a notifyd question, and notifyd limits
/// the questions open on the desk.
pub const MAX_PENDING: usize = 4;
pub const MAX_PENDING_PER_OWNER: usize = 1;
/// The longest process name the kernel keeps (`TASK_COMM_LEN` - 1).
pub const MAX_APP_NAME: usize = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Ask the person unless a grant covers the request.
    Prompt,
    /// Grant every request (headless or test sessions only).
    Allow,
}

impl std::str::FromStr for Policy {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "prompt" => Ok(Self::Prompt),
            "allow" => Ok(Self::Allow),
            other => Err(format!("unknown consent policy {other:?} (prompt, allow)")),
        }
    }
}

/// How a request was decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Granted by the `allow` policy.
    Policy,
    /// Granted by a standing grant.
    Grant,
    /// The person pressed Allow.
    Allowed,
    /// Answered over the Bus with `portald.screencast.answer`.
    AnsweredAllow,
    AnsweredDeny,
    /// The person pressed Deny or closed the question.
    Denied,
    /// Nobody answered in time.
    Expired,
    /// The app closed the request or its session, or left, while it waited.
    Abandoned,
}

impl Decision {
    pub fn granted(self) -> bool {
        matches!(
            self,
            Self::Policy | Self::Grant | Self::Allowed | Self::AnsweredAllow
        )
    }
}

/// A standing permission to start casts without asking.
#[derive(Clone, Debug, Serialize)]
pub struct Grant {
    pub id: u64,
    /// The caller's process name (`/proc/<pid>/comm`); `None` matches any.
    pub app: Option<String>,
    /// Starts it still covers.
    pub remaining: u32,
    /// Unix milliseconds after which it no longer applies.
    pub expires_ms: u64,
    #[serde(skip)]
    expires: Instant,
}

/// One request waiting for a person.
#[derive(Debug, Serialize)]
pub struct Pending {
    /// The Start request's object path: what `answer` names.
    pub request: String,
    pub app: String,
    pub owner: String,
    pub output: String,
    /// The notifyd notification id, once posted.
    pub notification: Option<u64>,
    pub asked_ms: u64,
    #[serde(skip)]
    answer: Option<oneshot::Sender<Decision>>,
}

#[derive(Default)]
struct State {
    grants: Vec<Grant>,
    pending: BTreeMap<String, Pending>,
    next_grant: u64,
}

#[derive(Clone)]
pub struct Consent {
    policy: Policy,
    state: Arc<Mutex<State>>,
}

fn unix_ms(at: Instant) -> u64 {
    let now = Instant::now();
    let wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let offset = if at >= now {
        wall + (at - now)
    } else {
        wall.saturating_sub(now - at)
    };
    offset.as_millis() as u64
}

impl Consent {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Adds a grant covering `count` Starts by `app` (any app when `None`)
    /// for `ttl`.
    pub fn grant(&self, app: Option<String>, count: u32, ttl: Duration) -> Result<Grant, String> {
        if count == 0 || count > 100 {
            return Err("count must be 1 to 100".into());
        }
        if ttl.is_zero() || ttl > MAX_GRANT_TTL {
            return Err(format!("ttl_ms must be 1 to {}", MAX_GRANT_TTL.as_millis()));
        }
        if let Some(app) = &app
            && (app.is_empty() || app.len() > MAX_APP_NAME)
        {
            return Err(format!(
                "app must be 1 to {MAX_APP_NAME} bytes (a process name)"
            ));
        }
        self.with(|state| {
            let now = Instant::now();
            state
                .grants
                .retain(|grant| grant.expires > now && grant.remaining > 0);
            if state.grants.len() >= MAX_GRANTS {
                return Err(format!("at most {MAX_GRANTS} grants at once"));
            }
            state.next_grant += 1;
            let expires = now + ttl;
            let grant = Grant {
                id: state.next_grant,
                app,
                remaining: count,
                expires_ms: unix_ms(expires),
                expires,
            };
            state.grants.push(grant.clone());
            Ok(grant)
        })
    }

    /// Removes one grant, or every grant when `id` is `None`. Returns how many.
    pub fn revoke(&self, id: Option<u64>) -> usize {
        self.with(|state| {
            let before = state.grants.len();
            state
                .grants
                .retain(|grant| id.is_some_and(|id| grant.id != id));
            before - state.grants.len()
        })
    }

    pub fn grants(&self) -> Vec<Grant> {
        self.with(|state| {
            let now = Instant::now();
            state
                .grants
                .retain(|grant| grant.expires > now && grant.remaining > 0);
            state.grants.clone()
        })
    }

    /// Uses up one Start of the first live grant that covers `app`. A grant
    /// naming the app is preferred over one for any app; an app whose name
    /// is unknown is covered only by a grant for any app.
    pub fn take_grant(&self, app: Option<&str>) -> Option<u64> {
        self.with(|state| {
            let now = Instant::now();
            state
                .grants
                .retain(|grant| grant.expires > now && grant.remaining > 0);
            let index = app
                .and_then(|app| {
                    state
                        .grants
                        .iter()
                        .position(|grant| grant.app.as_deref() == Some(app))
                })
                .or_else(|| state.grants.iter().position(|grant| grant.app.is_none()))?;
            let grant = &mut state.grants[index];
            grant.remaining -= 1;
            let id = grant.id;
            if grant.remaining == 0 {
                state.grants.remove(index);
            }
            Some(id)
        })
    }

    /// Registers a request waiting for a person; the receiver yields the
    /// decision. A request path can wait only once, and the number waiting
    /// is bounded ([`MAX_PENDING`], [`MAX_PENDING_PER_OWNER`]).
    pub fn wait(
        &self,
        request: &str,
        app: &str,
        owner: &str,
        output: &str,
    ) -> Result<oneshot::Receiver<Decision>, String> {
        let (sender, receiver) = oneshot::channel();
        self.with(|state| {
            if state.pending.contains_key(request) {
                return Err(format!("{request} is already waiting"));
            }
            if state.pending.len() >= MAX_PENDING {
                return Err(format!(
                    "{MAX_PENDING} screen-share requests are already waiting"
                ));
            }
            if state.pending.values().filter(|p| p.owner == owner).count() >= MAX_PENDING_PER_OWNER
            {
                return Err(format!(
                    "{owner} already has a screen-share request waiting"
                ));
            }
            state.pending.insert(
                request.to_owned(),
                Pending {
                    request: request.to_owned(),
                    app: app.to_owned(),
                    owner: owner.to_owned(),
                    output: output.to_owned(),
                    notification: None,
                    asked_ms: unix_ms(Instant::now()),
                    answer: Some(sender),
                },
            );
            Ok(())
        })?;
        Ok(receiver)
    }

    /// Records the question posted for `request`. Returns false when the
    /// request was decided meanwhile, so the caller withdraws the question.
    pub fn set_notification(&self, request: &str, id: u64) -> bool {
        self.with(|state| match state.pending.get_mut(request) {
            Some(pending) => {
                pending.notification = Some(id);
                true
            }
            None => false,
        })
    }

    /// Decides a waiting request. Returns its notification id (to withdraw
    /// the question) when it was waiting, `Err` when it was not.
    pub fn decide(&self, request: &str, decision: Decision) -> Result<Option<u64>, String> {
        self.with(|state| {
            let mut pending = state
                .pending
                .remove(request)
                .ok_or_else(|| format!("{request} is not waiting for consent"))?;
            if let Some(answer) = pending.answer.take() {
                let _ = answer.send(decision);
            }
            Ok(pending.notification)
        })
    }

    pub fn is_pending(&self, request: &str) -> bool {
        self.with(|state| state.pending.contains_key(request))
    }

    /// Forgets a request without deciding it (its Start ended another way).
    pub fn forget(&self, request: &str) -> Option<u64> {
        self.with(|state| state.pending.remove(request).and_then(|p| p.notification))
    }

    /// The requests waiting for a person, oldest first.
    pub fn pending(&self) -> Vec<serde_json::Value> {
        self.with(|state| {
            let mut list: Vec<_> = state
                .pending
                .values()
                .map(|pending| serde_json::to_value(pending).unwrap_or_default())
                .collect();
            list.sort_by_key(|value| value["asked_ms"].as_u64().unwrap_or(0));
            list
        })
    }
}

/// The calling process's name, for the prompt and grant matching; `None`
/// when it cannot be read. A process name says which app is asking; it does
/// not prove it (any local process can set its own), so a named grant is a
/// convenience for trusted, unsandboxed sessions, not an access control.
pub fn app_name(pid: Option<u32>) -> Option<String> {
    pid.and_then(|pid| std::fs::read_to_string(format!("/proc/{pid}/comm")).ok())
        .map(|comm| comm.trim().to_owned())
        .filter(|comm| !comm.is_empty())
}

/// How a request whose app could not be named is shown in the prompt.
pub const UNKNOWN_APP: &str = "an application";

// ── the process-wide consent and the Bus side ───────────────────────────
//
// One portald serves one session. The Bus feed (synchronous command handling,
// a client it owns) and the D-Bus side meet here rather than threading the
// state through the settings feed's signatures.

static CONSENT: OnceLock<Consent> = OnceLock::new();
static CLIENT: Mutex<Option<Arc<SupervisedClient>>> = Mutex::new(None);

/// Installs the consent state; the first call wins.
pub fn install(consent: Consent) -> Consent {
    CONSENT.get_or_init(|| consent).clone()
}

pub fn installed() -> Option<&'static Consent> {
    CONSENT.get()
}

/// The Bus client the feed connected, used to post and read questions.
pub fn set_client(client: Arc<SupervisedClient>) {
    *CLIENT.lock().unwrap_or_else(PoisonError::into_inner) = Some(client);
}

fn client() -> Option<Arc<SupervisedClient>> {
    CLIENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

pub const COMMANDS: &[(&str, &str, bool)] = &[
    (
        "portald.screencast.pending",
        "Screen-share requests waiting for consent, and the standing grants",
        true,
    ),
    (
        "portald.screencast.answer",
        "Allow or deny a waiting screen-share request: {request, allow}",
        false,
    ),
    (
        "portald.screencast.grant",
        "Allow screen sharing without asking: {app?, count?, ttl_ms?}",
        false,
    ),
    (
        "portald.screencast.revoke",
        "Withdraw one grant {id} or all grants {}",
        false,
    ),
];

/// rc 13: the caller is not on this node.
const FORBIDDEN: u8 = 13;

/// The request body; `Err` for a body that is not JSON, which is refused
/// rather than read as empty (an empty grant covers any app).
fn args(command: &IncomingCommand) -> Result<serde_json::Value, serde_json::Error> {
    if command.body.trim().is_empty() {
        Ok(command.args.clone())
    } else {
        serde_json::from_str(&command.body)
    }
}

/// A whole-number field: `default` when absent or null, `Err` when present
/// with any other type.
fn optional_u64(body: &serde_json::Value, key: &str, default: u64) -> Result<u64, ()> {
    match body.get(key) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(value) => value.as_u64().ok_or(()),
    }
}

fn usage(message: impl Into<String>) -> (u8, serde_json::Value) {
    (
        10,
        json!({"error_code": "USAGE", "message": message.into()}),
    )
}

/// Serves one `portald.screencast.*` command. Synchronous: the feed calls it
/// from its event loop. Every command answers callers on this node only, since
/// granting is granting capture of the whole screen.
pub fn handle_command(command: &IncomingCommand) -> (u8, serde_json::Value) {
    let Some(consent) = installed() else {
        return (
            10,
            json!({"status": "UNKNOWN_COMMAND", "command": command.command, "message": "portald runs without --screencast"}),
        );
    };
    if command.header("broker_origin") != Some("local") {
        tracing::warn!(command_name = %command.command, origin = ?command.header("broker_origin"), "screencast consent command refused: not a local caller");
        return (
            FORBIDDEN,
            json!({"error_code": "FORBIDDEN", "message": "screen-share consent is answered on this node only"}),
        );
    }
    let caller = command.from.clone();
    let body = match args(command) {
        Ok(body) => body,
        Err(error) => return usage(format!("body is not JSON: {error}")),
    };
    match command.command.as_str() {
        "portald.screencast.pending" => (
            0,
            json!({
                "policy": match consent.policy() { Policy::Prompt => "prompt", Policy::Allow => "allow" },
                "pending": consent.pending(),
                "grants": consent.grants(),
            }),
        ),
        "portald.screencast.answer" => {
            let Some(request) = body.get("request").and_then(|v| v.as_str()) else {
                return usage("answer needs {request, allow}");
            };
            let Some(allow) = body.get("allow").and_then(|v| v.as_bool()) else {
                return usage("answer needs {request, allow}");
            };
            let decision = if allow {
                Decision::AnsweredAllow
            } else {
                Decision::AnsweredDeny
            };
            match consent.decide(request, decision) {
                Ok(notification) => {
                    tracing::info!(%caller, %request, allow, "screencast consent answered over the Bus");
                    if let Some(id) = notification {
                        withdraw(id);
                    }
                    (0, json!({"request": request, "decision": decision}))
                }
                Err(message) => (14, json!({"error_code": "NOT_FOUND", "message": message})),
            }
        }
        "portald.screencast.grant" => {
            let app = match body.get("app") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(app)) => Some(app.clone()),
                Some(_) => return usage("app must be a string"),
            };
            let Ok(count) = optional_u64(&body, "count", 1) else {
                return usage("count must be a whole number");
            };
            let Ok(ttl_ms) = optional_u64(&body, "ttl_ms", DEFAULT_GRANT_TTL.as_millis() as u64)
            else {
                return usage("ttl_ms must be a whole number");
            };
            match consent.grant(
                app,
                u32::try_from(count).unwrap_or(0),
                Duration::from_millis(ttl_ms),
            ) {
                Ok(grant) => {
                    tracing::info!(%caller, id = grant.id, app = ?grant.app, count, ttl_ms, "screencast grant added");
                    (0, serde_json::to_value(grant).unwrap_or_default())
                }
                Err(message) => usage(message),
            }
        }
        "portald.screencast.revoke" => {
            // A malformed id is refused, never read as "revoke all".
            let id = match body.get("id") {
                None | Some(serde_json::Value::Null) => None,
                Some(value) => match value.as_u64() {
                    Some(id) => Some(id),
                    None => return usage("id must be a grant id (a whole number)"),
                },
            };
            let revoked = consent.revoke(id);
            tracing::info!(%caller, ?id, revoked, "screencast grants revoked");
            (0, json!({"revoked": revoked}))
        }
        other => (10, json!({"status": "UNKNOWN_COMMAND", "command": other})),
    }
}

/// Withdraws a question that was decided some other way.
fn withdraw(id: u64) {
    if let Some(client) = client() {
        tokio::spawn(async move {
            if let Err(error) = client
                .call("notify", "notify.close", json!({"id": id}))
                .await
            {
                tracing::debug!(id, %error, "consent question not withdrawn; it expires on its own");
            }
        });
    }
}

/// How often the outcome of a posted question is read.
const POLL: Duration = Duration::from_millis(500);

/// Asks the person through notifyd, and keeps reading the answer until the
/// request is decided by any route. Without notifyd the request still waits,
/// answerable over the Bus, until it expires.
async fn ask(consent: Consent, request: String, app: String, output: String) {
    let Some(client) = client() else {
        tracing::warn!(%request, "no Bus client: screen-share consent can only be answered with portald.screencast.answer");
        return;
    };
    let question = json!({
        "summary": "Share your screen?",
        "body": format!("{app} wants to share {output}. It will see everything on this screen."),
        "app": "portald",
        "source": "portald",
        "actions": [{"id": "allow", "label": "Allow"}, {"id": "deny", "label": "Deny"}],
        "timeout_ms": PROMPT_TIMEOUT.as_millis() as u64,
        "request_id": request,
    });
    let id = match client.call("notify", "notify.send", question).await {
        Ok(reply) => match reply.get("id").and_then(|v| v.as_u64()) {
            Some(id) => id,
            None => {
                tracing::warn!(%request, ?reply, "notifyd gave no id; answer with portald.screencast.answer");
                return;
            }
        },
        Err(error) => {
            tracing::warn!(%request, %error, "notifyd unavailable; answer with portald.screencast.answer");
            return;
        }
    };
    if !consent.set_notification(&request, id) {
        // Decided while the question was being posted: take it down.
        withdraw(id);
        return;
    }
    loop {
        tokio::time::sleep(POLL).await;
        if !consent.is_pending(&request) {
            return;
        }
        let Ok(record) = client.call("notify", "notify.get", json!({"id": id})).await else {
            continue;
        };
        let outcome = &record["outcome"];
        let decision = match (outcome["kind"].as_str(), outcome["action"].as_str()) {
            (None, _) => continue,
            (Some("action"), Some("allow")) => Decision::Allowed,
            (Some("expired"), _) => Decision::Expired,
            _ => Decision::Denied,
        };
        let _ = consent.decide(&request, decision);
        return;
    }
}

/// Decides one `Start`: policy, then a grant, then the person (or an agent
/// answering for them). Never waits longer than [`PROMPT_TIMEOUT`] plus a
/// little for the question to be posted. While it waits, `still_wanted` is
/// checked every [`POLL`]; once the app has closed the request or session,
/// or left, the request is [`Decision::Abandoned`] and its question taken
/// down, so the person can no longer approve a share nobody asked for.
pub async fn decide_start<F, Fut>(
    consent: &Consent,
    request: &str,
    app: Option<&str>,
    owner: &str,
    output: &str,
    still_wanted: F,
) -> Decision
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    if consent.policy() == Policy::Allow {
        return Decision::Policy;
    }
    if let Some(grant) = consent.take_grant(app) {
        tracing::info!(?app, %owner, grant, "screencast covered by a standing grant");
        return Decision::Grant;
    }
    let shown = app.unwrap_or(UNKNOWN_APP);
    let mut receiver = match consent.wait(request, shown, owner, output) {
        Ok(receiver) => receiver,
        Err(message) => {
            tracing::warn!(%request, "{message}");
            return Decision::Denied;
        }
    };
    tokio::spawn(ask(
        consent.clone(),
        request.to_owned(),
        shown.to_owned(),
        output.to_owned(),
    ));
    let deadline = tokio::time::Instant::now() + PROMPT_TIMEOUT + Duration::from_secs(5);
    loop {
        tokio::select! {
            decided = &mut receiver => return decided.unwrap_or(Decision::Expired),
            _ = tokio::time::sleep(POLL) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            if let Some(id) = consent.forget(request) {
                withdraw(id);
            }
            return Decision::Expired;
        }
        if !still_wanted().await {
            // Decided by another route meanwhile: the receiver has it.
            if let Ok(notification) = consent.decide(request, Decision::Abandoned) {
                if let Some(id) = notification {
                    withdraw(id);
                }
                tracing::info!(%request, %owner, "screencast request abandoned while it waited for consent");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_prefer_the_named_app_and_run_out() {
        let consent = Consent::new(Policy::Prompt);
        let any = consent.grant(None, 1, DEFAULT_GRANT_TTL).unwrap();
        let firefox = consent
            .grant(Some("firefox".into()), 2, DEFAULT_GRANT_TTL)
            .unwrap();
        assert_eq!(consent.take_grant(Some("firefox")), Some(firefox.id));
        assert_eq!(consent.take_grant(Some("zoom")), Some(any.id));
        assert_eq!(consent.take_grant(Some("zoom")), None);
        assert_eq!(consent.take_grant(Some("firefox")), Some(firefox.id));
        assert_eq!(consent.take_grant(Some("firefox")), None);
        assert!(consent.grants().is_empty());
    }

    #[test]
    fn an_unnamed_app_needs_a_grant_for_any_app() {
        let consent = Consent::new(Policy::Prompt);
        consent
            .grant(Some("obs".into()), 1, DEFAULT_GRANT_TTL)
            .unwrap();
        assert_eq!(consent.take_grant(None), None);
        let any = consent.grant(None, 1, DEFAULT_GRANT_TTL).unwrap();
        assert_eq!(consent.take_grant(None), Some(any.id));
        // A name the kernel can never report is refused at grant time.
        assert!(
            consent
                .grant(Some("obs-studio-portal".into()), 1, DEFAULT_GRANT_TTL)
                .is_err()
        );
        assert!(
            consent
                .grant(Some("a".repeat(MAX_APP_NAME)), 1, DEFAULT_GRANT_TTL)
                .is_ok()
        );
    }

    #[test]
    fn waiting_requests_are_bounded() {
        let consent = Consent::new(Policy::Prompt);
        let _first = consent.wait("/r/a1", "x", ":1.1", "o").unwrap();
        assert!(
            consent.wait("/r/a2", "x", ":1.1", "o").is_err(),
            "one per caller"
        );
        let _rest: Vec<_> = (2..=MAX_PENDING)
            .map(|n| {
                consent
                    .wait(&format!("/r/{n}"), "x", &format!(":1.{n}"), "o")
                    .unwrap()
            })
            .collect();
        assert!(
            consent.wait("/r/over", "x", ":1.99", "o").is_err(),
            "at most MAX_PENDING"
        );
    }

    #[test]
    fn a_question_posted_after_the_decision_is_reported() {
        let consent = Consent::new(Policy::Prompt);
        let _receiver = consent.wait("/r/late", "x", ":1.1", "o").unwrap();
        consent.decide("/r/late", Decision::AnsweredDeny).unwrap();
        assert!(!consent.set_notification("/r/late", 9));
    }

    #[tokio::test]
    async fn an_abandoned_request_is_not_left_waiting() {
        let consent = Consent::new(Policy::Prompt);
        let wanted = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let waiting = tokio::spawn({
            let consent = consent.clone();
            let wanted = wanted.clone();
            async move {
                decide_start(
                    &consent,
                    "/r/gone",
                    Some("firefox"),
                    ":1.3",
                    "HDMI-A-1",
                    || {
                        let wanted = wanted.clone();
                        async move { wanted.load(std::sync::atomic::Ordering::SeqCst) }
                    },
                )
                .await
            }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !consent.is_pending("/r/gone") {
            assert!(Instant::now() < deadline, "the Start never waited");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        wanted.store(false, std::sync::atomic::Ordering::SeqCst);
        let decision = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(decision, Decision::Abandoned);
        assert!(!decision.granted());
        assert!(!consent.is_pending("/r/gone"));
    }

    #[test]
    fn grants_expire_and_are_bounded() {
        let consent = Consent::new(Policy::Prompt);
        consent.grant(None, 1, Duration::from_millis(1)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(consent.take_grant(Some("x")), None);
        assert!(consent.grant(None, 0, DEFAULT_GRANT_TTL).is_err());
        assert!(consent.grant(None, 1, MAX_GRANT_TTL * 2).is_err());
        for _ in 0..MAX_GRANTS {
            consent.grant(None, 1, DEFAULT_GRANT_TTL).unwrap();
        }
        assert!(consent.grant(None, 1, DEFAULT_GRANT_TTL).is_err());
        assert_eq!(consent.revoke(None), MAX_GRANTS);
    }

    #[test]
    fn a_pending_request_is_decided_once() {
        let consent = Consent::new(Policy::Prompt);
        let mut receiver = consent.wait("/r/1", "firefox", ":1.5", "HDMI-A-1").unwrap();
        assert!(consent.wait("/r/1", "firefox", ":1.5", "HDMI-A-1").is_err());
        consent.set_notification("/r/1", 7);
        assert_eq!(consent.pending().len(), 1);
        assert_eq!(consent.decide("/r/1", Decision::AnsweredAllow), Ok(Some(7)));
        assert_eq!(receiver.try_recv(), Ok(Decision::AnsweredAllow));
        assert!(consent.decide("/r/1", Decision::Denied).is_err());
        assert!(consent.pending().is_empty());
    }

    fn command(command_name: &str, body: serde_json::Value, origin: Option<&str>) -> IncomingCommand {
        let mut headers = std::collections::BTreeMap::new();
        if let Some(origin) = origin {
            headers.insert("broker_origin".to_owned(), origin.to_owned());
        }
        IncomingCommand {
            generation: 0,
            from: "agent".into(),
            command: command_name.into(),
            id: None,
            args: serde_json::Value::Null,
            body: body.to_string(),
            headers,
        }
    }

    /// The commands, the Bus answer and the grant path, against the installed
    /// (process-wide) consent. One test, since the state is shared.
    #[tokio::test]
    async fn bus_commands_answer_and_grant_for_local_callers_only() {
        let consent = install(Consent::new(Policy::Prompt));

        // Mesh and unstamped callers are refused.
        for origin in [Some("mesh"), None] {
            let (rc, _) = handle_command(&command("portald.screencast.grant", json!({}), origin));
            assert_eq!(rc, FORBIDDEN);
        }

        // A grant covers the next Start by that app without asking.
        let (rc, grant) = handle_command(&command(
            "portald.screencast.grant",
            json!({"app": "obs", "count": 1, "ttl_ms": 60000}),
            Some("local"),
        ));
        assert_eq!(rc, 0, "{grant}");
        let (_, listed) = handle_command(&command(
            "portald.screencast.pending",
            json!({}),
            Some("local"),
        ));
        assert_eq!(listed["grants"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            decide_start(
                &consent,
                "/r/obs",
                Some("obs"),
                ":1.9",
                "HDMI-A-1",
                || async { true }
            )
            .await,
            Decision::Grant
        );

        // Without a grant the Start waits; an agent answers it.
        let waiting = tokio::spawn({
            let consent = consent.clone();
            async move {
                decide_start(
                    &consent,
                    "/r/ff",
                    Some("firefox"),
                    ":1.10",
                    "HDMI-A-1",
                    || async { true },
                )
                .await
            }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !consent.is_pending("/r/ff") {
            assert!(Instant::now() < deadline, "the Start never waited");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let (_, listed) = handle_command(&command(
            "portald.screencast.pending",
            json!({}),
            Some("local"),
        ));
        assert_eq!(listed["pending"][0]["app"], "firefox");
        let (rc, _) = handle_command(&command(
            "portald.screencast.answer",
            json!({"request": "/r/ff", "allow": true}),
            Some("local"),
        ));
        assert_eq!(rc, 0);
        assert_eq!(waiting.await.unwrap(), Decision::AnsweredAllow);

        // Answering what is not waiting, and malformed bodies.
        let (rc, _) = handle_command(&command(
            "portald.screencast.answer",
            json!({"request": "/r/none", "allow": false}),
            Some("local"),
        ));
        assert_eq!(rc, 14);
        let (rc, _) = handle_command(&command(
            "portald.screencast.answer",
            json!({"request": "/r/x"}),
            Some("local"),
        ));
        assert_eq!(rc, 10);

        // A body that is not JSON is refused, never read as an any-app grant.
        let mut garbage = command("portald.screencast.grant", json!({}), Some("local"));
        garbage.body = "garbage".into();
        assert_eq!(handle_command(&garbage).0, 10);

        // Wrongly typed fields are refused, never read as their defaults.
        let (rc, _) = handle_command(&command(
            "portald.screencast.grant",
            json!({"count": "5"}),
            Some("local"),
        ));
        assert_eq!(rc, 10);
        let (rc, _) = handle_command(&command(
            "portald.screencast.grant",
            json!({"ttl_ms": -1}),
            Some("local"),
        ));
        assert_eq!(rc, 10);

        // Revoke clears grants; a malformed id revokes nothing.
        handle_command(&command(
            "portald.screencast.grant",
            json!({}),
            Some("local"),
        ));
        let (rc, _) = handle_command(&command(
            "portald.screencast.revoke",
            json!({"id": "5"}),
            Some("local"),
        ));
        assert_eq!(rc, 10);
        let (_, listed) = handle_command(&command(
            "portald.screencast.pending",
            json!({}),
            Some("local"),
        ));
        assert_eq!(listed["grants"].as_array().map(Vec::len), Some(1));
        let (_, revoked) = handle_command(&command(
            "portald.screencast.revoke",
            json!({}),
            Some("local"),
        ));
        assert_eq!(revoked["revoked"], 1);
    }

    #[test]
    fn decisions_say_whether_they_grant() {
        assert!(Decision::Grant.granted());
        assert!(Decision::AnsweredAllow.granted());
        assert!(!Decision::Expired.granted());
        assert!(!Decision::AnsweredDeny.granted());
    }
}
