// SPDX-License-Identifier: MIT OR Apache-2.0
//! The verb table and dispatch. Every verb is closed and typed: no verb runs
//! a caller-supplied command, script or shell string.
use crate::{
    caller::{Caller, Origin, Peer},
    freeze, info, link, net,
    policy::{self, Policy, RateLimiter},
    receipt::{self, Outcome, Receipt},
    txn, wg, wifi,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

/// How a verb is gated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Any caller that can reach hostd, on either transport.
    Read,
    /// The kill switch: local callers only, with rules of its own.
    Control,
    /// Changes the host: local callers holding a grant, while not frozen.
    Write,
}

pub struct Verb {
    pub name: &'static str,
    pub summary: &'static str,
    pub tier: Tier,
}

pub const VERBS: &[Verb] = &[
    Verb {
        name: "host.info",
        summary: "What this host is: hostname, kernel, OS, MixOS mode, CPU, memory",
        tier: Tier::Read,
    },
    Verb {
        name: "net.state",
        summary: "Who manages the network: networkd links, resolved DNS, iwd stations, hostd-owned files",
        tier: Tier::Read,
    },
    Verb {
        name: "hostd.status",
        summary: "Frozen or not, the policy's state, and the grants the caller holds",
        tier: Tier::Read,
    },
    Verb {
        name: "hostd.receipt",
        summary: "One write's receipt: what it changed, under which grant, and how it ended",
        tier: Tier::Read,
    },
    Verb {
        name: "hostd.freeze",
        summary: "Refuse every write until local root thaws hostd",
        tier: Tier::Control,
    },
    Verb {
        name: "hostd.thaw",
        summary: "Accept writes again and lift grant suspensions (local root only)",
        tier: Tier::Control,
    },
    Verb {
        name: "net.link.apply",
        summary: "Configure one link (DHCP or static) through networkd; rolled back unless it comes up",
        tier: Tier::Write,
    },
    Verb {
        name: "hostd.undo",
        summary: "Put back what an applied write replaced",
        tier: Tier::Write,
    },
    Verb {
        name: "net.wifi.scan",
        summary: "Scan for Wi-Fi networks on one device (iwd); drives the radio, so it needs a grant",
        tier: Tier::Write,
    },
    Verb {
        name: "net.wifi.connect",
        summary: "Connect a Wi-Fi device to a network (iwd); put back unless connected in time",
        tier: Tier::Write,
    },
    Verb {
        name: "net.wg.apply",
        summary: "Configure a WireGuard link through networkd; rolled back unless it comes up and its peers complete a handshake",
        tier: Tier::Write,
    },
];

pub fn verb(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.name == name)
}

pub fn manifest() -> Vec<bus::VerbDescriptor> {
    std::iter::once(bus::VerbDescriptor::new(
        "HELP",
        &[],
        "List served verbs",
        true,
    ))
    .chain(
        VERBS
            .iter()
            .map(|v| bus::VerbDescriptor::new(v.name, &[], v.summary, v.tier == Tier::Read)),
    )
    .collect()
}

/// Where hostd reads the host and keeps its state: the real system in
/// production, temporary directories in tests.
#[derive(Clone)]
pub struct Host {
    pub root: PathBuf,
    pub policy: PathBuf,
    pub state: PathBuf,
    /// Refuse a policy file that anyone but root could change.
    pub require_root_policy: bool,
    pub limiter: Arc<Mutex<RateLimiter>>,
    /// One write transaction at a time, host-wide.
    pub writer: Arc<tokio::sync::Mutex<()>>,
    /// Why writes are blocked: a change hostd couldn't finish or record.
    /// Every write retries recovery first and clears this when it succeeds.
    pub blocked: Arc<Mutex<Option<String>>>,
}

impl Host {
    pub fn system() -> Self {
        Self {
            root: "/".into(),
            policy: policy::POLICY.into(),
            state: STATE.into(),
            require_root_policy: true,
            limiter: Arc::default(),
            writer: Arc::default(),
            blocked: Arc::default(),
        }
    }

    /// Blocks writes until recovery succeeds.
    pub fn block(&self, why: String) {
        tracing::error!(target: "hostd::audit", %why, "writes blocked");
        *self.blocked.lock().unwrap_or_else(|p| p.into_inner()) = Some(why);
    }

    pub fn blocked(&self) -> Option<String> {
        self.blocked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Finishes every in-flight receipt; on success writes are unblocked.
    /// Call with the writer lock held.
    pub async fn recover(&self) -> anyhow::Result<Vec<String>> {
        match txn::recover(self).await {
            Ok(done) => {
                *self.blocked.lock().unwrap_or_else(|p| p.into_inner()) = None;
                Ok(done)
            }
            Err(e) => {
                self.block(format!("recovery pending: {e:#}"));
                Err(e)
            }
        }
    }

    /// Everything under one directory, the policy not required to be
    /// root-owned: for tests and `--root` runs.
    pub fn fixture(dir: &std::path::Path) -> Self {
        Self {
            root: dir.into(),
            policy: dir.join("etc/hostd/policy.mx"),
            state: dir.join("var/lib/hostd"),
            require_root_policy: false,
            limiter: Arc::default(),
            writer: Arc::default(),
            blocked: Arc::default(),
        }
    }
}

/// systemd's `StateDirectory=hostd`.
pub const STATE: &str = "/var/lib/hostd";

/// No read takes longer than this, on either transport; every source also
/// has its own shorter deadline.
pub const DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
/// A write may wait for the writer lock and then its health window (at most
/// 60 s). Past this the caller gets `timeout`, but the transaction runs on
/// in its own task and its receipt records how it ended.
pub const WRITE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(90);

fn refuse(status: &str, verb: &str) -> Value {
    json!({"status": status, "verb": verb})
}

/// Requests are bounded in size before they reach dispatch, and in time here.
pub async fn dispatch(
    host: &Host,
    verb: &str,
    body: &Value,
    origin: &Origin,
) -> Result<Value, Value> {
    dispatch_from(host, verb, body, origin, None).await
}

/// `dispatch` for a local caller whose kernel identity hostd holds: a write
/// reads the caller afresh from `peer` once it holds the writer lock.
pub async fn dispatch_from(
    host: &Host,
    verb: &str,
    body: &Value,
    origin: &Origin,
    peer: Option<Arc<Peer>>,
) -> Result<Value, Value> {
    let deadline = match self::verb(verb).map(|v| v.tier) {
        Some(Tier::Write) => WRITE_DEADLINE,
        _ => DEADLINE,
    };
    let result = tokio::time::timeout(deadline, serve(host, verb, body, origin, peer))
        .await
        .unwrap_or_else(|_| Err(refuse("timeout", verb)));
    audit(verb, origin, &result);
    result
}

async fn serve(
    host: &Host,
    name: &str,
    body: &Value,
    origin: &Origin,
    peer: Option<Arc<Peer>>,
) -> Result<Value, Value> {
    if name == "HELP" {
        return Ok(json!(manifest()));
    }
    let Some(verb) = verb(name) else {
        return Err(refuse("not_served", name));
    };
    if verb.tier != Tier::Read && *origin == Origin::Bus {
        // Mesh writes arrive later, signed by a listed controller.
        return Err(refuse("local_only", name));
    }
    match name {
        "host.info" => {
            let root = host.root.clone();
            tokio::task::spawn_blocking(move || info::collect(&root))
                .await
                .map_err(|e| json!({"status":"internal","message":e.to_string()}))
        }
        "net.state" => Ok(net::collect(&net::Sources {
            root: host.root.clone(),
        })
        .await),
        "hostd.status" => Ok(status(host, origin)),
        "hostd.receipt" => receipt_verb(host, body),
        "hostd.freeze" => freeze_verb(host, body, local(origin)),
        "hostd.thaw" => thaw_verb(host, body, local(origin)),
        "net.link.apply" => {
            let plan = link::plan(&host.root, decode(body)?)?;
            let limits = vec![("links".to_owned(), plan.link.clone())];
            write(
                host,
                origin,
                peer,
                name,
                limits,
                move |host, origin, grant| link::apply(host, plan, grant, origin),
            )
            .await
        }
        "net.wg.apply" => {
            let request: wg::Request = decode(body)?;
            if !link::valid_link(&request.name) {
                return Err(
                    json!({"status":"bad_request","message":"name must be 1-15 characters of [A-Za-z0-9._-]"}),
                );
            }
            let limits = vec![("links".to_owned(), request.name.clone())];
            // The plan reads or makes the key, so it is built under the
            // writer lock: two applies to one link never make two keys.
            write(
                host,
                origin,
                peer,
                name,
                limits,
                move |host, origin, grant| async move {
                    let plan = wg::plan(&host.root, request)?;
                    wg::apply(host, plan, grant, origin).await
                },
            )
            .await
        }
        "net.wifi.scan" => {
            let request: wifi::ScanRequest = decode(body)?;
            if !link::valid_link(&request.device) {
                return Err(json!({"status":"bad_request","message":"device must be 1-15 characters of [A-Za-z0-9._-]"}));
            }
            let limits = vec![("links".to_owned(), request.device.clone())];
            // Nothing durable changes, so no receipt; the writer lock keeps a
            // scan from overlapping a connect on the same radio.
            write(host, origin, peer, name, limits, move |_host, _origin, _grant| async move {
                wifi::scan(&request.device).await
            })
            .await
        }
        "net.wifi.connect" => {
            let request: wifi::ConnectRequest = decode(body)?;
            let window = request.check()?;
            let limits = vec![
                ("links".to_owned(), request.device.clone()),
                ("ssids".to_owned(), request.ssid.clone()),
            ];
            write(host, origin, peer, name, limits, move |host, origin, grant| {
                wifi::connect(host, request, window, grant, origin)
            })
            .await
        }
        "hostd.undo" => undo_verb(host, body, origin, peer).await,
        _ => Err(refuse("not_served", name)),
    }
}

/// How long a write may wait behind others for the writer lock before it
/// is refused `busy`. A write never starts long after its caller asked.
const QUEUE: std::time::Duration = std::time::Duration::from_secs(15);

/// Runs one write transaction under the host-wide writer lock, in a task of
/// its own: a caller that hangs up, or a request deadline, never interrupts
/// a write or its rollback halfway.
///
/// The gate runs twice: before queueing (counting the call against the
/// grant's rate), and again once the lock is held, so a freeze, a breaker
/// trip, an expiry or a policy edit that lands while the write waits is
/// obeyed. The second gate reads the caller afresh from its kernel handle
/// (`peer`): a caller that left its granted cgroup, or exited, while it
/// waited, is refused.
///
/// Under the lock, a change hostd couldn't finish earlier is finished
/// first; while that fails, every write is refused `recovery_pending`.
async fn write<F, Fut>(
    host: &Host,
    origin: &Origin,
    peer: Option<Arc<Peer>>,
    verb: &str,
    limits: Vec<(String, String)>,
    run: F,
) -> Result<Value, Value>
where
    F: FnOnce(Host, Origin, String) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Value, Value>> + Send + 'static,
{
    let borrowed: Vec<(&str, &str)> = limits
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    authorise(host, verb, origin, &borrowed)?;
    let (host, origin, verb) = (host.clone(), origin.clone(), verb.to_owned());
    tokio::spawn(async move {
        let Ok(_writer) = tokio::time::timeout(QUEUE, host.writer.clone().lock_owned()).await
        else {
            return Err(refuse("busy", &verb));
        };
        if host.blocked().is_some()
            && let Err(e) = host.recover().await
        {
            return Err(
                json!({"status":"recovery_pending","verb":verb,"message":format!("{e:#}")}),
            );
        }
        let origin = match &peer {
            Some(peer) => Origin::Local(peer.caller()),
            None => origin,
        };
        let borrowed: Vec<(&str, &str)> = limits
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let grant = gate(&host, &verb, &origin, &borrowed, false)?;
        run(host, origin, grant).await
    })
    .await
    .unwrap_or_else(|e| Err(json!({"status":"internal","message":e.to_string()})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptBody {
    receipt: String,
}

fn receipt_verb(host: &Host, body: &Value) -> Result<Value, Value> {
    let body: ReceiptBody = decode(body)?;
    Receipt::load(&host.state, &body.receipt)
        .map(|r| {
            // Never hand back file contents: they may hold secrets.
            let files: Vec<_> = r.files.iter().map(|f| &f.path).collect();
            json!({"id": r.id, "verb": r.verb, "grant": r.grant, "origin": r.origin, "at": r.at,
                   "summary": r.summary, "files": files, "outcome": r.outcome, "reason": r.reason})
        })
        .map_err(|_| refuse("no_such_receipt", "hostd.receipt"))
}

/// Undo needs the same authority as the write it reverses: a grant that
/// covers the original verb and its limits. An undo that failed part way
/// (`undoing`) can be retried.
async fn undo_verb(
    host: &Host,
    body: &Value,
    origin: &Origin,
    peer: Option<Arc<Peer>>,
) -> Result<Value, Value> {
    let undoable = |o: &Outcome| matches!(o, Outcome::Applied | Outcome::Undoing);
    let body: ReceiptBody = decode(body)?;
    let receipt = Receipt::load(&host.state, &body.receipt)
        .map_err(|_| refuse("no_such_receipt", "hostd.undo"))?;
    if !undoable(&receipt.outcome) {
        return Err(json!({"status":"not_applied","outcome":receipt.outcome}));
    }
    let limits: Vec<(String, String)> = match receipt.verb.as_str() {
        // Both verbs record their link under `link`, and grants limit both
        // by `links`.
        link::VERB | wg::VERB => receipt
            .summary
            .get("link")
            .and_then(Value::as_str)
            .map(|l| vec![("links".to_owned(), l.to_owned())])
            .unwrap_or_default(),
        wifi::VERB => {
            let get = |k: &str| receipt.summary.get(k).and_then(Value::as_str).map(str::to_owned);
            match (get("link"), get("ssid")) {
                (Some(device), Some(ssid)) => {
                    vec![("links".to_owned(), device), ("ssids".to_owned(), ssid)]
                }
                _ => return Err(refuse("not_undoable", &receipt.verb)),
            }
        }
        _ => return Err(refuse("not_undoable", &receipt.verb)),
    };
    let verb = receipt.verb.clone();
    write(
        host,
        origin,
        peer,
        &verb,
        limits,
        move |host, _origin, _grant| async move {
            // Re-read under the lock: two undos of one receipt run one after
            // the other, and the second must see the first's result.
            let mut receipt = Receipt::load(&host.state, &receipt.id)
                .map_err(|e| json!({"status":"internal","message":e.to_string()}))?;
            if !undoable(&receipt.outcome) {
                return Err(json!({"status":"not_applied","outcome":receipt.outcome}));
            }
            txn::undo(&host, &mut receipt).await?;
            Ok(json!({"receipt": receipt.id, "outcome": receipt.outcome}))
        },
    )
    .await
}

fn local(origin: &Origin) -> &Caller {
    match origin {
        Origin::Local(caller) => caller,
        Origin::Bus => unreachable!("non-read verbs are refused on the Bus above"),
    }
}

fn load_policy(host: &Host) -> anyhow::Result<Policy> {
    Policy::load(&host.policy, host.require_root_policy)
}

fn status(host: &Host, origin: &Origin) -> Value {
    // One load, so every part describes the same policy. `held` names the
    // caller's grants; `held_grants` gives each in full (verbs, limits,
    // expiry, rate), so a UI offers only what the caller may do.
    let (policy, held, held_grants) = match load_policy(host) {
        Ok(policy) => {
            let grants = match origin {
                Origin::Local(caller) => policy.held_grants(caller, chrono::Utc::now()),
                Origin::Bus => vec![],
            };
            let names: Vec<&str> = grants.iter().map(|g| g.name.as_str()).collect();
            (
                json!({"grants": policy.grants.len()}),
                json!(names),
                json!(grants),
            )
        }
        Err(error) => (json!({"error": error.to_string()}), json!([]), json!([])),
    };
    let frozen = match freeze::state(&host.state) {
        Ok(record) => json!(record),
        // Unreadable counts as frozen; say so.
        Err(error) => json!({"record": "unreadable", "error": error.to_string()}),
    };
    json!({
        "frozen": frozen,
        "policy": policy,
        "caller": origin,
        "held": held,
        "held_grants": held_grants,
    })
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FreezeBody {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

fn decode<T: serde::de::DeserializeOwned>(body: &Value) -> Result<T, Value> {
    serde_json::from_value(body.clone())
        .map_err(|e| json!({"status":"bad_request","message":e.to_string()}))
}

/// Root on the host may always freeze; anyone else, container root
/// included, needs a grant naming `hostd.freeze`, so an agent can stop
/// itself and others.
fn freeze_verb(host: &Host, body: &Value, caller: &Caller) -> Result<Value, Value> {
    let body: FreezeBody = decode(body)?;
    if body.reason.as_ref().is_some_and(|r| r.len() > 200) {
        return Err(json!({"status":"bad_request","message":"reason over 200 bytes"}));
    }
    if !caller.host_root() {
        let policy = load_policy(host).map_err(|e| policy_invalid(&e))?;
        policy
            .grant_for(caller, "hostd.freeze", chrono::Utc::now())
            .map_err(|r| refuse(r.status(), "hostd.freeze"))?;
    }
    freeze::freeze(&host.state, caller, body.reason.as_deref())
        .map(|record| json!({"frozen": record}))
        .map_err(|e| json!({"status":"internal","message":e.to_string()}))
}

/// Only root on the host itself thaws: never container root, never a
/// grant. Thawing also lifts every grant suspension.
fn thaw_verb(host: &Host, body: &Value, caller: &Caller) -> Result<Value, Value> {
    let _: Empty = decode(body)?;
    if !caller.host_root() {
        return Err(refuse("root_only", "hostd.thaw"));
    }
    let internal = |e: anyhow::Error| json!({"status":"internal","message":e.to_string()});
    let thawed = freeze::thaw(&host.state).map_err(internal)?;
    let lifted = receipt::clear(&host.state).map_err(internal)?;
    Ok(json!({"thawed": thawed, "suspensions_lifted": lifted}))
}

fn policy_invalid(error: &anyhow::Error) -> Value {
    json!({"status":"policy_invalid","message":error.to_string()})
}

/// The gate every write verb passes before it touches the host: not frozen,
/// a local verified caller, a matching unexpired grant whose allow-lists
/// cover `limits`, not suspended by the circuit breaker, and within its
/// rate. Returns the name of the grant that allowed the call.
pub fn authorise(
    host: &Host,
    verb: &str,
    origin: &Origin,
    limits: &[(&str, &str)],
) -> Result<String, Value> {
    gate(host, verb, origin, limits, true)
}

/// The gate itself. `count` records the call against the grant's rate; the
/// re-check under the writer lock doesn't count it twice.
fn gate(
    host: &Host,
    verb: &str,
    origin: &Origin,
    limits: &[(&str, &str)],
    count: bool,
) -> Result<String, Value> {
    if freeze::blocks_writes(&host.state) {
        return Err(refuse("frozen", verb));
    }
    let Origin::Local(caller) = origin else {
        return Err(refuse("local_only", verb));
    };
    let policy = load_policy(host).map_err(|e| policy_invalid(&e))?;
    let candidates = policy
        .grants_for(caller, verb, chrono::Utc::now())
        .map_err(|r| refuse(r.status(), verb))?;
    let Some(grant) = candidates
        .into_iter()
        .find(|g| limits.iter().all(|(key, value)| g.allows(key, value)))
    else {
        return Err(json!({"status":"outside_grant","verb":verb,"limits":limits}));
    };
    if let Some(why) = receipt::suspended(&host.state, &grant.name) {
        return Err(json!({"status":"grant_suspended","grant":grant.name,"reason":why}));
    }
    if count {
        let who = caller.cgroup.as_deref().unwrap_or_default();
        host.limiter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .admit(grant, verb, who, Instant::now())
            .map_err(|r| refuse(r.status(), verb))?;
    }
    Ok(grant.name.clone())
}

/// One journald record per call. Bodies are not logged: write verbs will
/// add the fields they declare non-secret.
fn audit(verb: &str, origin: &Origin, result: &Result<Value, Value>) {
    let origin = serde_json::to_string(origin).unwrap_or_default();
    match result {
        Ok(_) => tracing::info!(target: "hostd::audit", verb, %origin, outcome = "ok"),
        Err(error) => {
            tracing::info!(target: "hostd::audit", verb, %origin, outcome = "refused", %error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A verified caller in a container (not in hostd's namespaces).
    fn caller(uid: u32, cgroup: &str) -> Origin {
        Origin::Local(Caller {
            uid,
            gid: uid,
            pid: Some(1),
            cgroup: Some(cgroup.into()),
            verified: true,
            host_ns: false,
        })
    }

    /// Root on the host itself.
    fn host_root() -> Origin {
        Origin::Local(Caller {
            uid: 0,
            gid: 0,
            pid: Some(1),
            cgroup: Some("/user.slice/root".into()),
            verified: true,
            host_ns: true,
        })
    }

    fn fixture(policy: &str) -> (tempfile::TempDir, Host) {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::fixture(dir.path());
        std::fs::create_dir_all(host.policy.parent().unwrap()).unwrap();
        std::fs::write(&host.policy, policy).unwrap();
        (dir, host)
    }

    const POLICY: &str = r#"{schema: 1, grants: [
        {name: "agent", caller: {cgroup: "/agent.slice/*"}, verbs: ["net.*", "hostd.freeze"],
         limits: {links: ["hd0"]}, rate: {count: 1, per_s: 600}},
        {name: "agent-wide", caller: {cgroup: "/agent.slice/*"}, verbs: ["net.link.apply"],
         limits: {links: ["hd1"]}}
    ]}"#;

    #[tokio::test]
    async fn unknown_verbs_are_not_served() {
        let (_dir, host) = fixture(POLICY);
        let err = dispatch(&host, "net.exec", &json!({}), &Origin::Bus)
            .await
            .unwrap_err();
        assert_eq!(err["status"], "not_served");
    }

    #[test]
    fn manifest_marks_only_reads_read_only() {
        let verbs = manifest();
        let names: Vec<&str> = verbs.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "HELP",
                "host.info",
                "net.state",
                "hostd.status",
                "hostd.receipt",
                "hostd.freeze",
                "hostd.thaw",
                "net.link.apply",
                "hostd.undo",
                "net.wifi.scan",
                "net.wifi.connect",
                "net.wg.apply"
            ]
        );
        let read_only: Vec<bool> = verbs.iter().map(|v| v.read_only).collect();
        assert_eq!(
            read_only,
            [
                true, true, true, true, true, false, false, false, false, false, false, false
            ]
        );
    }

    #[tokio::test]
    async fn writes_are_local_only() {
        let (_dir, host) = fixture(POLICY);
        let body = json!({"link":"hd0","config":{"mode":"dhcp"}});
        let err = dispatch(&host, "net.link.apply", &body, &Origin::Bus)
            .await
            .unwrap_err();
        assert_eq!(err["status"], "local_only");
    }

    #[tokio::test]
    async fn control_verbs_are_local_only() {
        let (_dir, host) = fixture(POLICY);
        for name in ["hostd.freeze", "hostd.thaw"] {
            let err = dispatch(&host, name, &json!({}), &Origin::Bus)
                .await
                .unwrap_err();
            assert_eq!(err["status"], "local_only");
        }
    }

    #[tokio::test]
    async fn freeze_needs_root_or_a_grant_and_only_root_thaws() {
        let (_dir, host) = fixture(POLICY);
        let stranger = caller(1000, "/user.slice/x");
        let agent = caller(1000, "/agent.slice/run");
        // Root inside a container, without uid remapping: uid 0, but not
        // in hostd's namespaces.
        let container_root = caller(0, "/machine.slice/systemd-nspawn@desk.service/payload");

        let err = dispatch(&host, "hostd.freeze", &json!({}), &stranger)
            .await
            .unwrap_err();
        assert_eq!(err["status"], "no_grant");
        let err = dispatch(&host, "hostd.freeze", &json!({}), &container_root)
            .await
            .unwrap_err();
        assert_eq!(err["status"], "no_grant");
        dispatch(&host, "hostd.freeze", &json!({"reason":"stop"}), &agent)
            .await
            .unwrap();
        assert!(freeze::blocks_writes(&host.state));

        for not_root in [&agent, &container_root] {
            let err = dispatch(&host, "hostd.thaw", &json!({}), not_root)
                .await
                .unwrap_err();
            assert_eq!(err["status"], "root_only");
        }
        let ok = dispatch(&host, "hostd.thaw", &json!({}), &host_root())
            .await
            .unwrap();
        assert_eq!(ok["thawed"], true);
    }

    #[test]
    fn authorise_gates_writes() {
        let (_dir, host) = fixture(POLICY);
        let agent = caller(1000, "/agent.slice/run");
        let hd0 = [("links", "hd0")];
        assert_eq!(
            authorise(&host, "net.link.apply", &Origin::Bus, &hd0).unwrap_err()["status"],
            "local_only"
        );
        assert_eq!(
            authorise(&host, "power.reboot", &agent, &[]).unwrap_err()["status"],
            "no_grant"
        );
        assert_eq!(
            authorise(&host, "net.link.apply", &agent, &[("links", "eth0")]).unwrap_err()["status"],
            "outside_grant"
        );
        assert_eq!(
            authorise(&host, "net.link.apply", &agent, &hd0).unwrap(),
            "agent"
        );
        // The first grant allows one call per ten minutes per caller.
        assert_eq!(
            authorise(&host, "net.link.apply", &agent, &hd0).unwrap_err()["status"],
            "rate_limited"
        );
        // The second grant covers another link, with no rate.
        assert_eq!(
            authorise(&host, "net.link.apply", &agent, &[("links", "hd1")]).unwrap(),
            "agent-wide"
        );
        freeze::freeze(&host.state, local(&agent), None).unwrap();
        assert_eq!(
            authorise(&host, "net.link.apply", &agent, &[("links", "hd1")]).unwrap_err()["status"],
            "frozen"
        );
    }

    #[test]
    fn a_suspended_grant_refuses_until_thawed() {
        let (_dir, host) = fixture(POLICY);
        let agent = caller(1000, "/agent.slice/run");
        let hd1 = [("links", "hd1")];
        receipt::record(&host.state, "agent-wide", true).unwrap();
        assert!(receipt::record(&host.state, "agent-wide", true).unwrap());
        assert_eq!(
            authorise(&host, "net.link.apply", &agent, &hd1).unwrap_err()["status"],
            "grant_suspended"
        );
        receipt::clear(&host.state).unwrap();
        assert!(authorise(&host, "net.link.apply", &agent, &hd1).is_ok());
    }

    #[test]
    fn a_broken_policy_refuses_writes() {
        let (_dir, host) = fixture("{schema: 9}");
        let agent = caller(1000, "/agent.slice/run");
        assert_eq!(
            authorise(&host, "net.link.apply", &agent, &[]).unwrap_err()["status"],
            "policy_invalid"
        );
    }

    #[tokio::test]
    async fn receipts_never_return_file_contents() {
        let (_dir, host) = fixture(POLICY);
        let mut r = Receipt::new(
            "net.link.apply",
            "agent",
            &Origin::Bus,
            json!({"link":"hd0"}),
        );
        r.files.push(receipt::FileChange {
            path: "/etc/systemd/network/50-hostd-hd0.network".into(),
            previous: Some("PrivateKey=secret".into()),
            installed: None,
            mode: 0o644,
            group: None,
            owner: None,
        });
        r.save(&host.state).unwrap();
        let got = dispatch(
            &host,
            "hostd.receipt",
            &json!({"receipt": r.id}),
            &Origin::Bus,
        )
        .await
        .unwrap();
        assert!(!got.to_string().contains("secret"));
        assert_eq!(got["files"][0], "/etc/systemd/network/50-hostd-hd0.network");
        let err = dispatch(
            &host,
            "hostd.receipt",
            &json!({"receipt": "../breaker"}),
            &Origin::Bus,
        )
        .await
        .unwrap_err();
        assert_eq!(err["status"], "no_such_receipt");
    }

    #[tokio::test]
    async fn undo_needs_an_applied_receipt_and_the_same_authority() {
        let (_dir, host) = fixture(POLICY);
        let stranger = caller(1000, "/user.slice/x");
        let mut r = Receipt::new(
            "net.link.apply",
            "agent",
            &Origin::Bus,
            json!({"link":"hd0"}),
        );
        r.outcome = Outcome::RolledBack;
        r.save(&host.state).unwrap();
        let err = dispatch(&host, "hostd.undo", &json!({"receipt": r.id}), &stranger)
            .await
            .unwrap_err();
        assert_eq!(err["status"], "not_applied");
        r.outcome = Outcome::Applied;
        r.save(&host.state).unwrap();
        let err = dispatch(&host, "hostd.undo", &json!({"receipt": r.id}), &stranger)
            .await
            .unwrap_err();
        assert_eq!(err["status"], "no_grant");
    }

    /// A write that passed the gate and then waited behind another must
    /// obey a freeze or a breaker trip that landed while it waited.
    #[tokio::test]
    async fn queued_writes_are_gated_again_under_the_lock() {
        let (_dir, host) = fixture(POLICY);
        let agent = caller(1000, "/agent.slice/run");
        for trip in ["freeze", "breaker"] {
            let held = host.writer.clone().lock_owned().await;
            let queued = {
                let (host, agent) = (host.clone(), agent.clone());
                tokio::spawn(async move {
                    write(
                        &host,
                        &agent,
                        None,
                        "net.link.apply",
                        vec![("links".into(), "hd1".into())],
                        |_, _, _| async { Ok(json!({"ran": true})) },
                    )
                    .await
                })
            };
            // Let the write pass the first gate and queue for the lock.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            match trip {
                "freeze" => {
                    freeze::freeze(&host.state, local(&agent), None).unwrap();
                }
                _ => {
                    receipt::record(&host.state, "agent-wide", true).unwrap();
                    receipt::record(&host.state, "agent-wide", true).unwrap();
                }
            }
            drop(held);
            let err = queued.await.unwrap().unwrap_err();
            let want = if trip == "freeze" {
                "frozen"
            } else {
                "grant_suspended"
            };
            assert_eq!(err["status"], want, "{trip}");
            freeze::thaw(&host.state).unwrap();
            receipt::clear(&host.state).unwrap();
        }
    }

    #[tokio::test]
    async fn status_reports_held_grants() {
        let (_dir, host) = fixture(POLICY);
        let agent = caller(1000, "/agent.slice/run");
        let s = dispatch(&host, "hostd.status", &json!({}), &agent)
            .await
            .unwrap();
        assert_eq!(s["held"], json!(["agent", "agent-wide"]));
        assert_eq!(s["policy"]["grants"], 2);
        assert_eq!(s["frozen"], Value::Null);
        // Each held grant comes back in full, limits verbatim.
        assert_eq!(s["held_grants"][0]["name"], "agent");
        assert_eq!(s["held_grants"][0]["limits"]["links"], json!(["hd0"]));
        assert_eq!(s["held_grants"][1]["verbs"], json!(["net.link.apply"]));
        assert_eq!(
            s["held_grants"][0]["rate"],
            json!({"count": 1, "per_s": 600})
        );
        // A caller with no grant gets empty lists, never someone else's.
        let stranger = caller(1000, "/user.slice/x");
        let s = dispatch(&host, "hostd.status", &json!({}), &stranger)
            .await
            .unwrap();
        assert_eq!(s["held_grants"], json!([]));
    }
}
