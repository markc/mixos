// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Contract tests for `org.freedesktop.portal.ScreenCast` against a private
//! dbus-daemon, with no compositor: the request/session lifecycle, ownership
//! and the failure answer when there is nothing to capture. Capture itself
//! needs compd and PipeWire and is checked on a desktop session.
use futures_util::StreamExt;
use portald::live::{self, Config};
use portald::portal::{BUS_NAME, OBJECT_PATH};
use portald::screencast::{INTERFACE, VERSION};
use portald::status;
use settings::Binding;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

const TIMEOUT: Duration = Duration::from_secs(5);

struct PrivateBus {
    child: Child,
    address: String,
    _dir: tempfile::TempDir,
}

impl PrivateBus {
    fn start() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("bus");
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .arg(format!("--address=unix:path={}", socket.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("dbus-daemon has no stdout"))?;
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line)?;
        let address = line.trim().to_owned();
        anyhow::ensure!(
            address.starts_with("unix:"),
            "dbus-daemon printed no address"
        );
        Ok(Self {
            child,
            address,
            _dir: dir,
        })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn client(address: &str) -> Connection {
    zbus::connection::Builder::address(address)
        .expect("address")
        .build()
        .await
        .expect("client connection")
}

async fn wait_for_owner(connection: &Connection) {
    let proxy = zbus::fdo::DBusProxy::new(connection)
        .await
        .expect("dbus proxy");
    let deadline = Instant::now() + TIMEOUT;
    while !proxy
        .name_has_owner(BUS_NAME.try_into().expect("name"))
        .await
        .unwrap_or(false)
    {
        assert!(Instant::now() < deadline, "portald never owned {BUS_NAME}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn segment(connection: &Connection) -> String {
    connection
        .unique_name()
        .expect("unique name")
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_")
}

fn request_path(connection: &Connection, token: &str) -> String {
    format!(
        "/org/freedesktop/portal/desktop/request/{}/{token}",
        segment(connection)
    )
}

fn session_path(connection: &Connection, token: &str) -> String {
    format!(
        "/org/freedesktop/portal/desktop/session/{}/{token}",
        segment(connection)
    )
}

/// Subscribes to `Response` on `path` before the call that will answer it.
async fn responses(connection: &Connection, path: &str) -> MessageStream {
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")
        .expect("interface")
        .member("Response")
        .expect("member")
        .path(path.to_owned())
        .expect("path")
        .build();
    MessageStream::for_match_rule(rule, connection, None)
        .await
        .expect("match rule")
}

async fn next_response(stream: &mut MessageStream) -> (u32, HashMap<String, OwnedValue>) {
    let message = tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("Response in time")
        .expect("stream open")
        .expect("message");
    message.body().deserialize().expect("Response body")
}

fn options(pairs: &[(&'static str, Value<'static>)]) -> HashMap<&'static str, Value<'static>> {
    pairs.iter().cloned().collect()
}

async fn call<B>(connection: &Connection, method: &str, body: &B) -> zbus::Result<OwnedObjectPath>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    connection
        .call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, body)
        .await?
        .body()
        .deserialize()
}

fn error_name(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, _, _) => name.as_str().to_owned(),
        other => format!("{other}"),
    }
}

async fn session_alive(connection: &Connection, path: &str) -> bool {
    connection
        .call_method(
            Some(BUS_NAME),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.portal.Session", "version"),
        )
        .await
        .is_ok()
}

/// One sequential test: the cases share process environment (no compositor
/// socket), which must not be changed while another test thread reads it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn screencast_contract() {
    let runtime = tempfile::tempdir().expect("runtime dir");
    // SAFETY: set once before any portal or client thread exists, and this
    // binary holds this single test.
    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", runtime.path());
        std::env::set_var("WAYLAND_DISPLAY", "portald-test-no-compositor");
    }
    let bus = PrivateBus::start().expect("dbus-daemon");
    let (_projections, received) = mpsc::channel(8);
    let portal = tokio::spawn(live::serve(
        Config {
            address: bus.address.clone(),
            state_dir: None,
            binding: Binding {
                instance: "host".into(),
                profile: "default".into(),
            },
            screencast: Some(portald::screencast::consent::Policy::Prompt),
            screencast_max_framerate: Default::default(),
        },
        received,
        status::shared(),
    ));
    let alice = client(&bus.address).await;
    wait_for_owner(&alice).await;

    // Properties.
    let proxy = zbus::Proxy::new(&alice, BUS_NAME, OBJECT_PATH, INTERFACE)
        .await
        .expect("proxy");
    assert_eq!(
        proxy.get_property::<u32>("version").await.expect("version"),
        VERSION
    );
    assert_eq!(
        proxy
            .get_property::<u32>("AvailableSourceTypes")
            .await
            .expect("types"),
        1
    );
    assert_eq!(
        proxy
            .get_property::<u32>("AvailableCursorModes")
            .await
            .expect("cursors"),
        3
    );

    // CreateSession answers with the session handle at the predicted paths.
    let session = session_path(&alice, "s1");
    let mut created = responses(&alice, &request_path(&alice, "c1")).await;
    let request = call(
        &alice,
        "CreateSession",
        &(options(&[
            ("handle_token", Value::from("c1")),
            ("session_handle_token", Value::from("s1")),
        ]),),
    )
    .await
    .expect("CreateSession");
    assert_eq!(request.as_str(), request_path(&alice, "c1"));
    let (code, results) = next_response(&mut created).await;
    assert_eq!(code, 0);
    assert_eq!(
        results["session_handle"]
            .downcast_ref::<&str>()
            .expect("string"),
        session
    );
    assert!(session_alive(&alice, &session).await);
    let session_object = OwnedObjectPath::try_from(session.clone()).expect("path");

    // Start before SelectSources is refused at the call.
    let early = call(
        &alice,
        "Start",
        &(
            &session_object,
            "",
            options(&[("handle_token", Value::from("e1"))]),
        ),
    )
    .await
    .expect_err("Start before SelectSources");
    assert_eq!(
        error_name(&early),
        "org.freedesktop.portal.Error.InvalidArgument"
    );

    // Unoffered source types and cursor modes are refused.
    let windows = call(
        &alice,
        "SelectSources",
        &(&session_object, options(&[("types", Value::from(2u32))])),
    )
    .await
    .expect_err("window sources");
    assert_eq!(
        error_name(&windows),
        "org.freedesktop.portal.Error.InvalidArgument"
    );
    let metadata = call(
        &alice,
        "SelectSources",
        &(
            &session_object,
            options(&[("cursor_mode", Value::from(4u32))]),
        ),
    )
    .await
    .expect_err("metadata cursor");
    assert_eq!(
        error_name(&metadata),
        "org.freedesktop.portal.Error.InvalidArgument"
    );

    // Another caller cannot use or close Alice's session.
    let mallory = client(&bus.address).await;
    let stolen = call(
        &mallory,
        "SelectSources",
        &(&session_object, options(&[("types", Value::from(1u32))])),
    )
    .await
    .expect_err("foreign session");
    assert_eq!(error_name(&stolen), "org.freedesktop.portal.Error.NotFound");
    let closed = mallory
        .call_method(
            Some(BUS_NAME),
            session.as_str(),
            Some("org.freedesktop.portal.Session"),
            "Close",
            &(),
        )
        .await
        .expect_err("foreign Close");
    assert_eq!(
        error_name(&closed),
        "org.freedesktop.DBus.Error.AccessDenied"
    );

    // SelectSources answers success.
    let mut selected = responses(&alice, &request_path(&alice, "p1")).await;
    call(
        &alice,
        "SelectSources",
        &(
            &session_object,
            options(&[
                ("handle_token", Value::from("p1")),
                ("types", Value::from(1u32)),
                ("cursor_mode", Value::from(2u32)),
                ("multiple", Value::from(false)),
            ]),
        ),
    )
    .await
    .expect("SelectSources");
    assert_eq!(next_response(&mut selected).await.0, 0);

    // With no compositor, Start is answered with failure (2), not an error.
    let mut started = responses(&alice, &request_path(&alice, "g1")).await;
    call(
        &alice,
        "Start",
        &(
            &session_object,
            "",
            options(&[("handle_token", Value::from("g1"))]),
        ),
    )
    .await
    .expect("Start");
    assert_eq!(next_response(&mut started).await.0, 2);
    assert!(
        session_alive(&alice, &session).await,
        "a failed Start keeps the session"
    );

    // A remote is only handed out for a started session.
    let remote = alice
        .call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE),
            "OpenPipeWireRemote",
            &(&session_object, HashMap::<&str, Value>::new()),
        )
        .await
        .expect_err("remote before start");
    assert_eq!(
        error_name(&remote),
        "org.freedesktop.portal.Error.InvalidArgument"
    );

    // The owner closes its session.
    alice
        .call_method(
            Some(BUS_NAME),
            session.as_str(),
            Some("org.freedesktop.portal.Session"),
            "Close",
            &(),
        )
        .await
        .expect("owner Close");
    assert!(!session_alive(&alice, &session).await);

    // A caller that leaves the bus loses its sessions.
    let bob = client(&bus.address).await;
    let bob_session = session_path(&bob, "b1");
    let mut bob_created = responses(&bob, &request_path(&bob, "c2")).await;
    call(
        &bob,
        "CreateSession",
        &(options(&[
            ("handle_token", Value::from("c2")),
            ("session_handle_token", Value::from("b1")),
        ]),),
    )
    .await
    .expect("CreateSession");
    assert_eq!(next_response(&mut bob_created).await.0, 0);
    assert!(session_alive(&alice, &bob_session).await);
    drop(bob_created);
    drop(bob);
    let deadline = Instant::now() + TIMEOUT;
    while session_alive(&alice, &bob_session).await {
        assert!(Instant::now() < deadline, "session outlived its owner");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    portal.abort();
}
