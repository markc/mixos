// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The command table and dispatch. Every command is closed and typed: no command runs
//! a caller-supplied command, script or shell string.
use crate::{
    caller::{Origin, Peer},
    desk, freeze, info, link, net,
    rate::RateLimiter,
    receipt::{Outcome, Receipt},
    txn, unit, wg, wifi,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

/// How a command is gated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Any caller that can reach hostd, on either transport.
    Read,
    /// The kill switch: local callers only, with rules of its own.
    Control,
    /// Changes the host: any local caller, while not frozen.
    Write,
}

pub struct Command {
    pub name: &'static str,
    pub summary: &'static str,
    pub tier: Tier,
}

pub const COMMANDS: &[Command] = &[
    Command {
        name: "host.info",
        summary: "What this host is: hostname, kernel, OS, MixOS mode, CPU, memory",
        tier: Tier::Read,
    },
    Command {
        name: "net.state",
        summary: "Who manages the network: networkd links, resolved DNS, iwd stations, hostd-owned files",
        tier: Tier::Read,
    },
    Command {
        name: "hostd.status",
        summary: "Frozen or not, and the caller's origin",
        tier: Tier::Read,
    },
    Command {
        name: "hostd.receipt",
        summary: "One write's receipt: what it changed, its origin, and how it ended",
        tier: Tier::Read,
    },
    Command {
        name: "hostd.freeze",
        summary: "Refuse every write until a local caller thaws hostd",
        tier: Tier::Control,
    },
    Command {
        name: "hostd.thaw",
        summary: "Accept writes again (any local caller)",
        tier: Tier::Control,
    },
    Command {
        name: "net.link.apply",
        summary: "Configure one link (DHCP or static) through networkd; rolled back unless it comes up",
        tier: Tier::Write,
    },
    Command {
        name: "hostd.undo",
        summary: "Put back what an applied write replaced",
        tier: Tier::Write,
    },
    Command {
        name: "net.wifi.scan",
        summary: "Scan for Wi-Fi networks on one device (iwd); drives the radio",
        tier: Tier::Write,
    },
    Command {
        name: "net.wifi.connect",
        summary: "Connect a Wi-Fi device to a network (iwd); put back unless connected in time",
        tier: Tier::Write,
    },
    Command {
        name: "net.wg.apply",
        summary: "Configure a WireGuard link through networkd; rolled back unless it comes up and its peers complete a handshake",
        tier: Tier::Write,
    },
    Command {
        name: "desk.restart",
        summary: "Restart the named desk machine; the host stays up",
        tier: Tier::Write,
    },
    Command {
        name: "desk.stop",
        summary: "Power off the named desk machine; the host stays up",
        tier: Tier::Write,
    },
    Command {
        name: "unit.control",
        summary: "Start, stop or restart one named system unit; hostd never controls its own units",
        tier: Tier::Write,
    },
];

pub fn command_name(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|v| v.name == name)
}

pub fn manifest() -> Vec<bus::CommandDescriptor> {
    std::iter::once(bus::CommandDescriptor::new(
        "HELP",
        &[],
        "List served commands",
        true,
    ))
    .chain(
        COMMANDS
            .iter()
            .map(|v| bus::CommandDescriptor::new(v.name, &[], v.summary, v.tier == Tier::Read)),
    )
    .collect()
}

/// Where hostd reads the host and keeps its state: the real system in
/// production, temporary directories in tests.
#[derive(Clone)]
pub struct Host {
    pub root: PathBuf,
    pub state: PathBuf,
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
            state: STATE.into(),
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

    /// Everything under one directory, for tests and `--root` runs.
    pub fn fixture(dir: &std::path::Path) -> Self {
        Self {
            root: dir.into(),
            state: dir.join("var/lib/hostd"),
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

fn refuse(status: &str, command_name: &str) -> Value {
    json!({"status": status, "command": command_name})
}

/// The one refusal every component gives a command it does not serve.
fn unknown_command(command_name: &str) -> Value {
    json!({
        "error_code": "UNKNOWN_COMMAND",
        "message": format!("unknown command {command_name}"),
        "command": command_name,
    })
}

/// Requests are bounded in size before they reach dispatch, and in time here.
pub async fn dispatch(
    host: &Host,
    command_name: &str,
    body: &Value,
    origin: &Origin,
) -> Result<Value, Value> {
    dispatch_from(host, command_name, body, origin, None).await
}

/// `dispatch` for a local caller whose kernel identity hostd holds: a write
/// reads the caller afresh from `peer` once it holds the writer lock.
pub async fn dispatch_from(
    host: &Host,
    command_name: &str,
    body: &Value,
    origin: &Origin,
    peer: Option<Arc<Peer>>,
) -> Result<Value, Value> {
    let deadline = match self::command_name(command_name).map(|v| v.tier) {
        Some(Tier::Write) => WRITE_DEADLINE,
        _ => DEADLINE,
    };
    let result = tokio::time::timeout(deadline, serve(host, command_name, body, origin, peer))
        .await
        .unwrap_or_else(|_| Err(refuse("timeout", command_name)));
    audit(command_name, origin, &result);
    result
}

async fn serve(
    host: &Host,
    name: &str,
    body: &Value,
    origin: &Origin,
    peer: Option<Arc<Peer>>,
) -> Result<Value, Value> {
    if matches!(origin, Origin::Unstamped) {
        return Err(refuse("unstamped", name));
    }
    if name == "HELP" {
        return Ok(json!(manifest()));
    }
    let Some(command_name) = command_name(name) else {
        return Err(unknown_command(name));
    };
    if command_name.tier != Tier::Read && !origin.is_local() {
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
        "hostd.receipt" => receipt_command(host, body),
        "hostd.freeze" => freeze_command(host, body, origin),
        "hostd.thaw" => thaw_command(host, body),
        desk::RESTART | desk::STOP => {
            let request: desk::Request = decode(body)?;
            if !desk::valid_name(&request.desk) {
                return Err(
                    json!({"status":"bad_request","message":"desk must be a valid machine name"}),
                );
            }
            let desk = request.desk;
            let (command_name, run_desk) = (name.to_owned(), desk.clone());
            write(host, origin, peer, name, move |host, origin| async move {
                desk::act(&host.root, &host.state, &command_name, &run_desk, &origin).await
            })
            .await
        }
        unit::COMMAND => {
            let request: unit::Request = decode(body)?;
            if !unit::valid_unit(&request.unit) {
                return Err(
                    json!({"status":"bad_request","message":"unit must be a .service name of letters, digits and -_.@:"}),
                );
            }
            if !unit::valid_action(&request.action) {
                return Err(
                    json!({"status":"bad_request","message":"action must be start, stop or restart"}),
                );
            }
            let (run_unit, run_action) = (request.unit, request.action);
            write(host, origin, peer, name, move |host, origin| async move {
                unit::act(&host.state, &run_unit, &run_action, &origin).await
            })
            .await
        }
        "net.link.apply" => {
            let plan = link::plan(&host.root, decode(body)?)?;
            write(host, origin, peer, name, move |host, origin| {
                link::apply(host, plan, origin)
            })
            .await
        }
        "net.wg.apply" => {
            let request: wg::Request = decode(body)?;
            if !link::valid_link(&request.name) {
                return Err(
                    json!({"status":"bad_request","message":"name must be 1-15 characters of [A-Za-z0-9._-]"}),
                );
            }
            // The plan reads or makes the key, so it is built under the
            // writer lock: two applies to one link never make two keys.
            write(host, origin, peer, name, move |host, origin| async move {
                let plan = wg::plan(&host.root, request)?;
                wg::apply(host, plan, origin).await
            })
            .await
        }
        "net.wifi.scan" => {
            let request: wifi::ScanRequest = decode(body)?;
            if !link::valid_link(&request.device) {
                return Err(
                    json!({"status":"bad_request","message":"device must be 1-15 characters of [A-Za-z0-9._-]"}),
                );
            }
            // Nothing durable changes, so no receipt; the writer lock keeps a
            // scan from overlapping a connect on the same radio.
            write(host, origin, peer, name, move |_host, _origin| async move {
                wifi::scan(&request.device).await
            })
            .await
        }
        "net.wifi.connect" => {
            let request: wifi::ConnectRequest = decode(body)?;
            let window = request.check()?;
            write(host, origin, peer, name, move |host, origin| {
                wifi::connect(host, request, window, origin)
            })
            .await
        }
        "hostd.undo" => undo_command(host, body, origin, peer).await,
        _ => Err(unknown_command(name)),
    }
}

/// How long a write may wait behind others for the writer lock before it
/// is refused `busy`. A write never starts long after its caller asked.
const QUEUE: std::time::Duration = std::time::Duration::from_secs(15);

/// Runs one write transaction under the host-wide writer lock, in a task of
/// its own: a caller that hangs up, or a request deadline, never interrupts
/// a write or its rollback halfway.
///
/// The gate runs before queueing (counting the rate) and under the lock,
/// so a freeze that lands while a write waits is obeyed. Socket identity is
/// refreshed for the receipt, without restricting who may write.
///
/// Under the lock, a change hostd couldn't finish earlier is finished
/// first; while that fails, every write is refused `recovery_pending`.
async fn write<F, Fut>(
    host: &Host,
    origin: &Origin,
    peer: Option<Arc<Peer>>,
    command_name: &str,
    run: F,
) -> Result<Value, Value>
where
    F: FnOnce(Host, Origin) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Value, Value>> + Send + 'static,
{
    gate(host, command_name, origin, true)?;
    let (host, origin, command_name) = (host.clone(), origin.clone(), command_name.to_owned());
    tokio::spawn(async move {
        let Ok(_writer) = tokio::time::timeout(QUEUE, host.writer.clone().lock_owned()).await
        else {
            return Err(refuse("busy", &command_name));
        };
        if host.blocked().is_some()
            && let Err(e) = host.recover().await
        {
            return Err(
                json!({"status":"recovery_pending","command":command_name,"message":format!("{e:#}")}),
            );
        }
        let origin = match &peer {
            Some(peer) => Origin::Local(peer.caller()),
            None => origin,
        };
        gate(&host, &command_name, &origin, false)?;
        run(host, origin).await
    })
    .await
    .unwrap_or_else(|e| Err(json!({"status":"internal","message":e.to_string()})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptBody {
    receipt: String,
}

fn receipt_command(host: &Host, body: &Value) -> Result<Value, Value> {
    let body: ReceiptBody = decode(body)?;
    Receipt::load(&host.state, &body.receipt)
        .map(|r| {
            // Never hand back file contents: they may hold secrets.
            let files: Vec<_> = r.files.iter().map(|f| &f.path).collect();
            json!({"id": r.id, "command": r.command, "origin": r.origin, "at": r.at,
                   "summary": r.summary, "files": files, "outcome": r.outcome, "reason": r.reason})
        })
        .map_err(|_| refuse("no_such_receipt", "hostd.receipt"))
}

/// Any local caller may undo an applied receipt. An undo that failed part
/// way (`undoing`) can be retried; file ownership is checked by the transaction.
async fn undo_command(
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
    if !matches!(
        receipt.command.as_str(),
        link::COMMAND | wg::COMMAND | wifi::COMMAND
    ) {
        return Err(refuse("not_undoable", &receipt.command));
    }
    if receipt.command == wifi::COMMAND
        && ["link", "ssid"]
            .iter()
            .any(|key| receipt.summary.get(key).and_then(Value::as_str).is_none())
    {
        return Err(refuse("not_undoable", &receipt.command));
    }
    let command_name = receipt.command.clone();
    write(
        host,
        origin,
        peer,
        &command_name,
        move |host, _origin| async move {
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

fn status(host: &Host, origin: &Origin) -> Value {
    let frozen = match freeze::state(&host.state) {
        Ok(record) => json!(record),
        Err(error) => json!({"record": "unreadable", "error": error.to_string()}),
    };
    json!({"frozen": frozen, "caller": origin})
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

fn freeze_command(host: &Host, body: &Value, origin: &Origin) -> Result<Value, Value> {
    let body: FreezeBody = decode(body)?;
    if body.reason.as_ref().is_some_and(|r| r.len() > 200) {
        return Err(json!({"status":"bad_request","message":"reason over 200 bytes"}));
    }
    freeze::freeze(&host.state, origin, body.reason.as_deref())
        .map(|record| json!({"frozen": record}))
        .map_err(|e| json!({"status":"internal","message":e.to_string()}))
}

fn thaw_command(host: &Host, body: &Value) -> Result<Value, Value> {
    let _: Empty = decode(body)?;
    freeze::thaw(&host.state)
        .map(|thawed| json!({"thawed": thawed}))
        .map_err(|e| json!({"status":"internal","message":e.to_string()}))
}

/// Only origin, freeze and rate constrain writes. Identity is for rate
/// accounting and audit, never for permission.
fn gate(host: &Host, command_name: &str, origin: &Origin, count: bool) -> Result<(), Value> {
    if !origin.is_local() {
        return Err(refuse(
            if matches!(origin, Origin::Unstamped) {
                "unstamped"
            } else {
                "local_only"
            },
            command_name,
        ));
    }
    if freeze::blocks_writes(&host.state) {
        return Err(refuse("frozen", command_name));
    }
    if count
        && !host
            .limiter
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .admit(command_name, &origin.rate_key(), Instant::now())
    {
        return Err(refuse("rate_limited", command_name));
    }
    Ok(())
}

/// One journald record per call. Bodies are not logged: write commands will
/// add the fields they declare non-secret.
/// Every request, with its outcome. Refusals log at warn, so a refused
/// write (a power click among them) stands out in the journal.
fn audit(command_name: &str, origin: &Origin, result: &Result<Value, Value>) {
    let origin = serde_json::to_string(origin).unwrap_or_default();
    match result {
        Ok(_) => tracing::info!(target: "hostd::audit", command_name, %origin, outcome = "ok"),
        Err(error) => {
            tracing::warn!(target: "hostd::audit", command_name, %origin, outcome = "refused", %error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::Caller;
    use crate::receipt;

    fn socket_caller() -> Origin {
        Origin::Local(Caller {
            uid: 1000,
            gid: 1000,
            pid: None,
            cgroup: None,
            verified: false,
            host_ns: false,
        })
    }

    fn local_bus() -> Origin {
        Origin::LocalBus {
            from: "arbitrary-agent".into(),
        }
    }

    fn fixture() -> (tempfile::TempDir, Host) {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::fixture(dir.path());
        (dir, host)
    }

    #[tokio::test]
    async fn unknown_commands_are_not_served() {
        let (_dir, host) = fixture();
        let err = dispatch(&host, "net.exec", &json!({}), &Origin::Mesh)
            .await
            .unwrap_err();
        assert_eq!(err["error_code"], "UNKNOWN_COMMAND");
        assert_eq!(err["command"], "net.exec");
    }

    #[test]
    fn manifest_marks_only_reads_read_only() {
        let command_names = manifest();
        let names: Vec<&str> = command_names.iter().map(|v| v.name.as_str()).collect();
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
                "net.wg.apply",
                "desk.restart",
                "desk.stop",
                "unit.control"
            ]
        );
        let read_only: Vec<bool> = command_names.iter().map(|v| v.read_only).collect();
        assert_eq!(
            read_only,
            [
                true, true, true, true, true, false, false, false, false, false, false, false,
                false, false, false
            ]
        );
    }

    #[tokio::test]
    async fn bus_and_socket_writes_need_no_grant_or_verified_process() {
        for origin in [local_bus(), socket_caller()] {
            let (_dir, host) = fixture();
            let result = write(
                &host,
                &origin,
                None,
                "net.link.apply",
                |host, origin| async move {
                    let mut receipt =
                        Receipt::new("net.link.apply", &origin, json!({"link":"hd0"}));
                    receipt.outcome = Outcome::Applied;
                    receipt.save(&host.state).unwrap();
                    Ok(json!({"receipt":receipt.id}))
                },
            )
            .await
            .unwrap();
            let receipt = Receipt::load(&host.state, result["receipt"].as_str().unwrap()).unwrap();
            assert_eq!(receipt.origin, origin);
        }
    }

    #[tokio::test]
    async fn mesh_writes_and_controls_are_refused() {
        let (_dir, host) = fixture();
        for name in COMMANDS
            .iter()
            .filter(|command| command.tier != Tier::Read)
            .map(|command| command.name)
        {
            let err = dispatch(&host, name, &json!({}), &Origin::Mesh)
                .await
                .unwrap_err();
            assert_eq!(err["status"], "local_only", "{name}");
        }
        assert_eq!(
            dispatch(&host, "host.info", &json!({}), &Origin::Unstamped)
                .await
                .unwrap_err()["status"],
            "unstamped"
        );
    }

    #[tokio::test]
    async fn any_local_caller_can_freeze_and_thaw() {
        let (_dir, host) = fixture();
        for origin in [local_bus(), socket_caller()] {
            dispatch(&host, "hostd.freeze", &json!({"reason":"stop"}), &origin)
                .await
                .unwrap();
            assert_eq!(
                gate(&host, "net.link.apply", &origin, true).unwrap_err()["status"],
                "frozen"
            );
            let other = local_bus();
            let thawed = dispatch(&host, "hostd.thaw", &json!({}), &other)
                .await
                .unwrap();
            assert_eq!(thawed["thawed"], true);
            assert!(gate(&host, "net.link.apply", &origin, true).is_ok());
        }
    }

    #[test]
    fn write_rates_still_apply() {
        let (_dir, host) = fixture();
        for _ in 0..crate::rate::COUNT {
            gate(&host, "net.link.apply", &local_bus(), true).unwrap();
        }
        assert_eq!(
            gate(&host, "net.link.apply", &local_bus(), true).unwrap_err()["status"],
            "rate_limited"
        );
        assert!(gate(&host, "net.link.apply", &socket_caller(), true).is_ok());
    }

    #[tokio::test]
    async fn receipts_never_return_file_contents() {
        let (_dir, host) = fixture();
        let mut receipt = Receipt::new("net.link.apply", &local_bus(), json!({"link":"hd0"}));
        receipt.files.push(receipt::FileChange {
            path: "/etc/systemd/network/50-hostd-hd0.network".into(),
            previous: Some("PrivateKey=secret".into()),
            installed: None,
            mode: 0o644,
            group: None,
            owner: None,
        });
        receipt.save(&host.state).unwrap();
        let got = dispatch(
            &host,
            "hostd.receipt",
            &json!({"receipt":receipt.id}),
            &Origin::Mesh,
        )
        .await
        .unwrap();
        assert!(!got.to_string().contains("secret"));
        assert_eq!(got["files"][0], "/etc/systemd/network/50-hostd-hd0.network");
        assert_eq!(
            dispatch(
                &host,
                "hostd.receipt",
                &json!({"receipt":"../breaker"}),
                &Origin::Mesh
            )
            .await
            .unwrap_err()["status"],
            "no_such_receipt"
        );
    }

    #[tokio::test]
    async fn undo_needs_an_applied_receipt_and_preserves_later_edits() {
        let (_dir, host) = fixture();
        let mut receipt = Receipt::new("net.link.apply", &socket_caller(), json!({"link":"hd0"}));
        receipt.outcome = Outcome::RolledBack;
        receipt.save(&host.state).unwrap();
        assert_eq!(
            dispatch(
                &host,
                "hostd.undo",
                &json!({"receipt":receipt.id}),
                &local_bus()
            )
            .await
            .unwrap_err()["status"],
            "not_applied"
        );
        let path = host.root.join("later.network");
        std::fs::write(&path, "later edit").unwrap();
        receipt.files.push(receipt::FileChange {
            path: path.clone(),
            previous: Some("before".into()),
            installed: Some(receipt::digest(b"ours")),
            mode: 0o644,
            group: None,
            owner: None,
        });
        receipt.outcome = Outcome::Applied;
        receipt.save(&host.state).unwrap();
        let err = dispatch(
            &host,
            "hostd.undo",
            &json!({"receipt":receipt.id}),
            &local_bus(),
        )
        .await
        .unwrap_err();
        assert_eq!(err["status"], "superseded");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "later edit");
    }

    #[tokio::test]
    async fn queued_writes_obey_a_freeze_under_the_lock() {
        let (_dir, host) = fixture();
        let held = host.writer.clone().lock_owned().await;
        let queued = {
            let host = host.clone();
            tokio::spawn(async move {
                write(&host, &local_bus(), None, "net.link.apply", |_, _| async {
                    Ok(json!({"ran":true}))
                })
                .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        freeze::freeze(&host.state, &socket_caller(), None).unwrap();
        drop(held);
        assert_eq!(queued.await.unwrap().unwrap_err()["status"], "frozen");
    }

    #[tokio::test]
    async fn status_reports_origin_and_freeze() {
        let (_dir, host) = fixture();
        let result = dispatch(&host, "hostd.status", &json!({}), &local_bus())
            .await
            .unwrap();
        assert_eq!(result["caller"], json!(local_bus()));
        assert!(result["frozen"].is_null());
    }
}
