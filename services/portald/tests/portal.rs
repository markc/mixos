// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contract tests against a private dbus-daemon: the Settings interface on the
//! wire, changed-only signals, the startup budget and single ownership.
use futures_util::{Stream, StreamExt};
use portald::appearance::{ACCENT_COLOR, COLOR_SCHEME, CONTRAST, NAMESPACE};
use portald::live::{self, Config};
use portald::portal::{BUS_NAME, INTERFACE, OBJECT_PATH};
use portald::status::{self, Shared};
use settings::Binding;
use settings::Revision;
use settings::appearance::{APPEARANCE_SCHEMA, AppearanceProjection};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use zbus::Message;
use zbus::zvariant::{OwnedValue, Value};

const ACCENT: [f64; 3] = [0.1, 0.2, 0.3];
const NOT_FOUND: &str = "org.freedesktop.portal.Error.NotFound";

/// A dbus-daemon owned by the test; killed and reaped on drop.
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

impl PrivateBus {
    /// Kills the dbus-daemon now, as a crash or a broker restart would.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        self.kill();
    }
}

fn projection(incarnation: &str, revision: u64, mode: &str) -> AppearanceProjection {
    AppearanceProjection {
        schema: APPEARANCE_SCHEMA,
        binding: Binding {
            instance: "host".into(),
            profile: "default".into(),
        },
        incarnation: incarnation.into(),
        revision: Revision(revision),
        design_revision: Revision(1),
        mode: mode.into(),
        contrast: "normal".into(),
        accent: ACCENT,
    }
}

fn spawn_portal(
    address: &str,
    state_dir: Option<PathBuf>,
) -> (
    JoinHandle<anyhow::Result<()>>,
    mpsc::Sender<AppearanceProjection>,
    Shared,
) {
    let (projections, received) = mpsc::channel(8);
    let shared = status::shared();
    let task = tokio::spawn(live::serve(
        Config {
            address: address.to_owned(),
            state_dir,
            binding: projection("a", 1, "dark").binding,
        },
        received,
        shared.clone(),
    ));
    (task, projections, shared)
}

/// Waits until the serving task reports the name owned. If the task ends
/// first, its own error is returned so the cause is not lost.
async fn wait_for_name(
    task: &mut JoinHandle<anyhow::Result<()>>,
    shared: &Shared,
) -> anyhow::Result<()> {
    for _ in 0..200 {
        let owned = shared
            .read()
            .map_err(|_| anyhow::anyhow!("poisoned"))?
            .name_owned;
        if owned {
            return Ok(());
        }
        if task.is_finished() {
            return match task.await? {
                Err(error) => Err(anyhow::anyhow!(
                    "portal task ended before owning {BUS_NAME}: {error:#}"
                )),
                Ok(()) => anyhow::bail!("portal task ended before owning {BUS_NAME}"),
            };
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    anyhow::bail!("portal did not report {BUS_NAME} owned within 5 s")
}

async fn client(bus: &PrivateBus) -> anyhow::Result<zbus::Connection> {
    Ok(zbus::connection::Builder::address(bus.address.as_str())?
        .build()
        .await?)
}

async fn portal_proxy(connection: &zbus::Connection) -> anyhow::Result<zbus::Proxy<'static>> {
    Ok(zbus::Proxy::new(connection, BUS_NAME, OBJECT_PATH, INTERFACE).await?)
}

/// Number of variant layers around a value.
fn depth(value: &Value<'_>) -> usize {
    match value {
        Value::Value(inner) => 1 + depth(inner),
        _ => 0,
    }
}

fn innermost<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
    match value {
        Value::Value(inner) => innermost(inner),
        other => other,
    }
}

async fn read_one_color(proxy: &zbus::Proxy<'_>) -> anyhow::Result<u32> {
    let value: OwnedValue = proxy
        .call_method("ReadOne", &(NAMESPACE, COLOR_SCHEME))
        .await?
        .body()
        .deserialize()?;
    match innermost(&value) {
        Value::U32(colour) => Ok(*colour),
        other => anyhow::bail!("color-scheme is not a u32: {other:?}"),
    }
}

async fn next_signal<S>(stream: &mut S) -> anyhow::Result<(String, String, OwnedValue)>
where
    S: Stream<Item = Message> + Unpin,
{
    let message = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("signal stream ended"))?;
    Ok(message.body().deserialize()?)
}

/// True when nothing arrives within a short window.
async fn silent<S>(stream: &mut S) -> bool
where
    S: Stream<Item = Message> + Unpin,
{
    tokio::time::timeout(Duration::from_millis(400), stream.next())
        .await
        .is_err()
}

#[tokio::test]
async fn read_keeps_the_extra_layer_and_read_one_does_not() -> anyhow::Result<()> {
    let bus = PrivateBus::start()?;
    let (mut task, _projections, shared) = spawn_portal(&bus.address, None);
    wait_for_name(&mut task, &shared).await?;
    let connection = client(&bus).await?;
    let proxy = portal_proxy(&connection).await?;

    assert_eq!(proxy.get_property::<u32>("version").await?, 2);

    let read: OwnedValue = proxy
        .call_method("Read", &(NAMESPACE, COLOR_SCHEME))
        .await?
        .body()
        .deserialize()?;
    let one: OwnedValue = proxy
        .call_method("ReadOne", &(NAMESPACE, COLOR_SCHEME))
        .await?
        .body()
        .deserialize()?;
    assert_eq!(
        depth(&read),
        depth(&one) + 1,
        "Read carries one more variant"
    );
    assert_eq!(innermost(&read), &Value::U32(1), "defaults serve dark");
    assert_eq!(innermost(&one), &Value::U32(1));

    task.abort();
    Ok(())
}

#[tokio::test]
async fn read_all_matches_globs_and_unknown_keys_are_not_found() -> anyhow::Result<()> {
    let bus = PrivateBus::start()?;
    let (mut task, _projections, shared) = spawn_portal(&bus.address, None);
    wait_for_name(&mut task, &shared).await?;
    let connection = client(&bus).await?;
    let proxy = portal_proxy(&connection).await?;

    let all: BTreeMap<String, BTreeMap<String, OwnedValue>> = proxy
        .call_method("ReadAll", &(vec!["org.freedesktop.*"],))
        .await?
        .body()
        .deserialize()?;
    let keys = all
        .get(NAMESPACE)
        .ok_or_else(|| anyhow::anyhow!("appearance namespace missing"))?;
    assert!(keys.contains_key(COLOR_SCHEME) && keys.contains_key(CONTRAST));
    assert!(
        !keys.contains_key(ACCENT_COLOR),
        "no accent before the source answers"
    );

    let none: BTreeMap<String, BTreeMap<String, OwnedValue>> = proxy
        .call_method("ReadAll", &(vec!["org.gnome.*"],))
        .await?
        .body()
        .deserialize()?;
    assert!(none.is_empty());

    for filters in [vec![], vec![""], vec!["org.gnome.*", ""]] {
        let all: BTreeMap<String, BTreeMap<String, OwnedValue>> = proxy
            .call_method("ReadAll", &(filters,))
            .await?
            .body()
            .deserialize()?;
        assert_eq!(all[NAMESPACE].len(), 2, "empty filters mean all namespaces");
    }

    // Accepted worst-case patterns exercise the actual synchronous D-Bus handler.
    let hostile = format!("{}X", "*".repeat(255));
    let none: BTreeMap<String, BTreeMap<String, OwnedValue>> = tokio::time::timeout(
        Duration::from_millis(250),
        proxy.call_method(
            "ReadAll",
            &(vec![hostile; portald::appearance::MAX_FILTERS],),
        ),
    )
    .await??
    .body()
    .deserialize()?;
    assert!(none.is_empty());
    let stars: BTreeMap<String, BTreeMap<String, OwnedValue>> = proxy
        .call_method("ReadAll", &(vec!["*".repeat(256)],))
        .await?
        .body()
        .deserialize()?;
    assert_eq!(stars[NAMESPACE].len(), 2);
    for filters in [
        vec!["*".repeat(257)],
        vec!["*".into(); 65],
        vec!["".into(), "*".repeat(100_000)],
    ] {
        match tokio::time::timeout(
            Duration::from_millis(250),
            proxy.call_method("ReadAll", &(filters,)),
        )
        .await?
        {
            Err(zbus::Error::MethodError(name, _, _)) => {
                assert_eq!(
                    name.as_str(),
                    "org.freedesktop.portal.Error.InvalidArgument"
                );
            }
            other => panic!("oversized filters were not rejected: {other:?}"),
        }
    }
    assert_eq!(
        read_one_color(&proxy).await?,
        1,
        "service remains responsive"
    );

    match proxy.call_method("Read", &(NAMESPACE, "font-name")).await {
        Err(zbus::Error::MethodError(name, _, _)) => assert_eq!(name.as_str(), NOT_FOUND),
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("an unserved key was answered"),
    }

    task.abort();
    Ok(())
}

#[tokio::test]
async fn only_changed_keys_are_signalled_and_stale_projections_are_silent() -> anyhow::Result<()> {
    let bus = PrivateBus::start()?;
    let (mut task, projections, shared) = spawn_portal(&bus.address, None);
    wait_for_name(&mut task, &shared).await?;
    let connection = client(&bus).await?;
    let proxy = portal_proxy(&connection).await?;
    let mut signals = proxy.receive_signal("SettingChanged").await?;

    projections.send(projection("a", 2, "light")).await?;
    let (namespace, key, value) = next_signal(&mut signals).await?;
    assert_eq!(
        (namespace.as_str(), key.as_str()),
        (NAMESPACE, COLOR_SCHEME)
    );
    assert_eq!(innermost(&value), &Value::U32(2));
    let (_, key, value) = next_signal(&mut signals).await?;
    assert_eq!(key, ACCENT_COLOR);
    assert!(matches!(innermost(&value), Value::Structure(_)));

    // A repeat changes nothing; an older revision of the same incarnation is refused.
    projections.send(projection("a", 2, "light")).await?;
    projections.send(projection("a", 1, "dark")).await?;
    assert!(
        silent(&mut signals).await,
        "no signal for a repeat or stale projection"
    );
    assert_eq!(read_one_color(&proxy).await?, 2);
    assert_eq!(
        shared
            .read()
            .map_err(|_| anyhow::anyhow!("poisoned"))?
            .stale,
        1
    );

    task.abort();
    Ok(())
}

#[tokio::test]
async fn a_valid_cache_serves_before_the_source_answers() -> anyhow::Result<()> {
    let bus = PrivateBus::start()?;
    let dir = tempfile::tempdir()?;
    portald::cache::save(dir.path(), &projection("a", 3, "light"))?;
    let (mut task, _projections, shared) = spawn_portal(&bus.address, Some(dir.path().to_owned()));
    wait_for_name(&mut task, &shared).await?;
    let connection = client(&bus).await?;
    let proxy = portal_proxy(&connection).await?;

    assert_eq!(read_one_color(&proxy).await?, 2);
    let origin = shared
        .read()
        .map_err(|_| anyhow::anyhow!("poisoned"))?
        .origin;
    assert_eq!(origin, status::Origin::Cache);

    task.abort();
    Ok(())
}

#[tokio::test]
async fn a_silent_source_never_holds_the_name() -> anyhow::Result<()> {
    let bus = PrivateBus::start()?;
    // The sender stays alive, so the source is silent rather than closed.
    let (mut task, _projections, shared) = spawn_portal(&bus.address, None);
    tokio::time::timeout(
        Duration::from_millis(250),
        wait_for_name(&mut task, &shared),
    )
    .await??;
    let connection = client(&bus).await?;
    let proxy = portal_proxy(&connection).await?;
    assert_eq!(read_one_color(&proxy).await?, 1, "defaults serve dark");

    task.abort();
    Ok(())
}

#[tokio::test]
async fn a_second_owner_is_refused_rather_than_queued() -> anyhow::Result<()> {
    let bus = PrivateBus::start()?;
    let (mut first, _first_projections, first_shared) = spawn_portal(&bus.address, None);
    wait_for_name(&mut first, &first_shared).await?;

    let (second, _second_projections, _second_shared) = spawn_portal(&bus.address, None);
    let outcome = tokio::time::timeout(Duration::from_secs(5), second).await??;
    assert!(outcome.is_err(), "a second owner must not be queued");

    first.abort();
    Ok(())
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs the actual binary, CLI, runtime, configuration and feed. The single
/// deadline starts BEFORE spawn and ends after a complete deprecated Read reply.
#[tokio::test]
async fn production_process_answers_read_within_250_ms_with_a_silent_authority()
-> anyhow::Result<()> {
    use bus::native_client::NodedClient;
    let broker = test_broker::Broker::start();
    let authority = NodedClient::connect("settingsd", &broker.url).await?;
    let mut requests = authority
        .incoming_async()
        .await
        .ok_or_else(|| anyhow::anyhow!("authority incoming already taken"))?;
    let dir = tempfile::tempdir()?;
    portald::cache::save(dir.path(), &projection("cached", 3, "light"))?;
    let alternate = Binding {
        instance: "host".into(),
        profile: "alternate".into(),
    };
    // A misfiled projection must be rejected even though its structure is valid.
    let wrong = portald::cache::path(dir.path(), &alternate)?;
    std::fs::create_dir_all(wrong.parent().unwrap())?;
    std::fs::write(
        wrong,
        serde_json::to_vec(&projection("foreign", 3, "light"))?,
    )?;

    for (profile, cache, expected) in [
        ("default", false, 1),
        ("default", true, 2),
        ("switched", true, 1),
        ("alternate", true, 1),
    ] {
        let bus = PrivateBus::start()?;
        let connection = client(&bus).await?;
        let proxy = portal_proxy(&connection).await?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_portald"));
        command
            .args(["serve", "--instance", "host", "--profile", profile])
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("MIXOS_NODED_URL", &broker.url)
            .env_remove("STATE_DIRECTORY")
            .stdin(Stdio::null());
        if cache {
            command.arg("--state-dir").arg(dir.path());
        }
        let started = Instant::now();
        let mut process = Process(command.spawn()?);
        let value = tokio::time::timeout_at(
            tokio::time::Instant::from_std(started + Duration::from_millis(250)),
            async {
                loop {
                    if let Ok(reply) = proxy.call_method("Read", &(NAMESPACE, COLOR_SCHEME)).await {
                        let value: OwnedValue = reply.body().deserialize()?;
                        return Ok::<_, anyhow::Error>(value);
                    }
                    anyhow::ensure!(process.0.try_wait()?.is_none(), "production portald exited");
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            },
        )
        .await??;
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(innermost(&value), &Value::U32(expected));
        assert_eq!(depth(&value), 1, "Read remains double-boxed on the wire");
        // Prove this is a hung registered authority, not a missing route.
        let request = tokio::time::timeout(Duration::from_secs(5), requests.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("authority receiver ended"))?;
        assert_eq!(request.command, "settings.appearance.get");
        // The authority deliberately never responds.
        let report = tokio::time::timeout(
            Duration::from_millis(250),
            authority.call("portald", "portald.status", serde_json::json!({})),
        )
        .await??;
        assert_eq!(report["settings_generation"], 1);
        assert_eq!(
            report["portal"]["name_owned"], true,
            "status remains responsive during the hung read"
        );
        drop(process);
        tokio::time::timeout(Duration::from_secs(5), async {
            while authority
                .list_services()
                .await?
                .iter()
                .any(|name| name == "portald")
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
    }
    authority.close().await;
    Ok(())
}

/// Waits up to `within` for the process to exit. Returns `None` if it is still
/// running at the deadline.
fn exit_within(process: &mut Process, within: Duration) -> anyhow::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = process.0.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Losing the session bus must end the process non-zero, so the unit's
/// `Restart=on-failure` brings the portal back. A process that survives with no
/// connection holds no name and is never restarted.
#[tokio::test]
async fn losing_the_session_bus_exits_non_zero_for_the_restart() -> anyhow::Result<()> {
    let broker = test_broker::Broker::start();
    let mut bus = PrivateBus::start()?;
    let connection = client(&bus).await?;
    let proxy = portal_proxy(&connection).await?;
    let mut process = Process(
        Command::new(env!("CARGO_BIN_EXE_portald"))
            .args(["serve", "--instance", "host", "--profile", "default"])
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("MIXOS_NODED_URL", &broker.url)
            .env_remove("STATE_DIRECTORY")
            .stdin(Stdio::null())
            .spawn()?,
    );
    let value = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(reply) = proxy.call_method("Read", &(NAMESPACE, COLOR_SCHEME)).await {
                return Ok::<_, anyhow::Error>(reply.body().deserialize::<OwnedValue>()?);
            }
            anyhow::ensure!(process.0.try_wait()?.is_none(), "production portald exited");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    assert_eq!(innermost(&value), &Value::U32(1), "defaults serve dark");

    bus.kill();
    let status = exit_within(&mut process, Duration::from_secs(5))?.ok_or_else(|| {
        anyhow::anyhow!("portald still running 5 s after its session bus was killed")
    })?;
    assert!(
        !status.success(),
        "a lost bus must exit non-zero, got {status}"
    );
    Ok(())
}

#[test]
fn cache_is_partitioned_by_instance_and_profile_and_checks_the_stored_binding() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let first = projection("a", 3, "light");
    let mut second = projection("b", 1, "dark");
    second.binding.profile = "alternate".into();
    let mut third = second.clone();
    third.binding.instance = "elsewhere".into();
    portald::cache::save(dir.path(), &first)?;
    portald::cache::save(dir.path(), &second)?;
    portald::cache::save(dir.path(), &third)?;
    for item in [&first, &second, &third] {
        assert_eq!(
            portald::cache::load(dir.path(), &item.binding).as_ref(),
            Some(item)
        );
    }
    std::fs::copy(
        portald::cache::path(dir.path(), &first.binding)?,
        portald::cache::path(dir.path(), &second.binding)?,
    )?;
    assert!(portald::cache::load(dir.path(), &second.binding).is_none());
    Ok(())
}

/// The actual production authority and portal loops against an embedded real
/// noded and private D-Bus. No injected projection or replacement authority.
struct NativePortal {
    feed: JoinHandle<anyhow::Result<()>>,
    live: JoinHandle<anyhow::Result<()>>,
    shared: Shared,
}
impl NativePortal {
    fn start(bus: &PrivateBus, url: &str) -> Self {
        let (projections, received) = mpsc::channel(8);
        let shared = status::shared();
        let binding = projection("a", 1, "dark").binding;
        let feed = tokio::spawn(portald::service::feed(
            binding.clone(),
            url.into(),
            projections,
            shared.clone(),
        ));
        let live = tokio::spawn(live::serve(
            Config {
                address: bus.address.clone(),
                state_dir: None,
                binding,
            },
            received,
            shared.clone(),
        ));
        Self { feed, live, shared }
    }
}
impl Drop for NativePortal {
    fn drop(&mut self) {
        self.live.abort();
        self.feed.abort();
    }
}

struct AuthorityTask(JoinHandle<anyhow::Result<()>>);
impl AuthorityTask {
    fn start(root: &std::path::Path, url: &str) -> Self {
        Self(tokio::spawn(settingsd::service::serve_at(
            root.to_owned(),
            projection("a", 1, "dark").binding,
            url.into(),
        )))
    }
    async fn stop(&mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}
impl Drop for AuthorityTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn authority_store(root: &std::path::Path, mode: &str) -> anyhow::Result<()> {
    let mut desktop = settings::Desktop::default();
    desktop.appearance.mode = mode.into();
    let (store, _) =
        settingsd::store::Store::create(root, projection("a", 1, "dark").binding, desktop)?;
    drop(store);
    Ok(())
}

async fn wait_source(portal: &NativePortal, revision: u64, after: u64) -> anyhow::Result<()> {
    let expected = revision.to_string();
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let report = status::report(&portal.shared);
            if report["portal"]["revision"] == expected
                && report["portal"]["accepted"].as_u64().unwrap_or(0) > after
            {
                return Ok::<_, anyhow::Error>(());
            }
            anyhow::ensure!(!portal.feed.is_finished(), "native feed exited: {report}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
}

async fn wait_absent(
    client: &bus::native_client::NodedClient,
    service: &str,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while client
            .list_services()
            .await?
            .iter()
            .any(|name| name == service)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await?
}

#[tokio::test]
async fn native_startup_live_change_and_settingsd_restart_without_broker_reconnect()
-> anyhow::Result<()> {
    use bus::native_client::NodedClient;
    use serde_json::json;
    let broker = test_broker::Broker::start();
    let root = tempfile::tempdir()?;
    authority_store(root.path(), "light")?;
    let mut authority = AuthorityTask::start(root.path(), &broker.url);
    let bus = PrivateBus::start()?;
    let mut portal = NativePortal::start(&bus, &broker.url);
    wait_for_name(&mut portal.live, &portal.shared).await?;
    wait_source(&portal, 1, 0).await?;
    let operator = NodedClient::connect("portal-test", &broker.url).await?;
    let connection = client(&bus).await?;
    let proxy = portal_proxy(&connection).await?;
    assert_eq!(
        read_one_color(&proxy).await?,
        2,
        "initial authority fetched"
    );
    let owner: zbus::names::OwnedUniqueName = zbus::fdo::DBusProxy::new(&connection)
        .await?
        .get_name_owner(zbus::names::BusName::try_from(BUS_NAME)?)
        .await?;
    let mut signals = proxy.receive_signal("SettingChanged").await?;
    // Finish any startup announcements before applying the live edit.
    while tokio::time::timeout(Duration::from_millis(50), signals.next())
        .await
        .is_ok()
    {}
    let initial_appearance = operator
        .call(
            "settingsd",
            "settings.appearance.get",
            json!({"binding": projection("a", 1, "dark").binding}),
        )
        .await?;
    let initial = operator
        .call(
            "settingsd",
            "settings.get",
            json!({"binding": projection("a", 1, "dark").binding}),
        )
        .await?;
    let changed = operator
        .call(
            "settingsd",
            "settings.apply",
            json!({
                "binding": projection("a", 1, "dark").binding,
                "expected_incarnation": initial["snapshot"]["incarnation"],
                "expected_revision": "1", "operation_id": "portal-live-dark",
                "changes": {"appearance.mode": "dark"}
            }),
        )
        .await?;
    assert_eq!(changed["status"], "changed");
    wait_source(&portal, 2, 0).await?;
    // The live change arrived as a broker delivery stamped by THIS tree's noded.
    // A refusal here means the broker stopped stamping settingsd as the owner.
    assert_eq!(
        status::report(&portal.shared)["portal"]["delivery_refusals"],
        0
    );
    // Check the broker transport never changed generation while startup/live
    // update/restart were handled, and the signal is from the D-Bus name owner.
    let current_appearance = operator
        .call(
            "settingsd",
            "settings.appearance.get",
            json!({"binding": projection("a", 1, "dark").binding}),
        )
        .await?;
    let before_projection: AppearanceProjection =
        serde_json::from_value(initial_appearance["appearance"].clone())?;
    let after_projection: AppearanceProjection =
        serde_json::from_value(current_appearance["appearance"].clone())?;
    let changes = portald::appearance::Values::from_projection(&before_projection).changed(
        &portald::appearance::Values::from_projection(&after_projection),
    );
    assert_eq!(changes[0], (COLOR_SCHEME, Value::U32(1)));
    for (expected_key, expected_value) in changes {
        let message = tokio::time::timeout(Duration::from_secs(5), signals.next())
            .await?
            .ok_or_else(|| anyhow::anyhow!("signal stream ended"))?;
        assert_eq!(
            message.header().sender().map(|s| s.as_str()),
            Some(owner.as_str())
        );
        let (namespace, key, value): (String, String, OwnedValue) = message.body().deserialize()?;
        assert_eq!(
            (namespace.as_str(), key.as_str()),
            (NAMESPACE, expected_key)
        );
        assert_eq!(innermost(&value), &expected_value);
    }
    assert_eq!(read_one_color(&proxy).await?, 1);

    let before = status::report(&portal.shared)["portal"]["accepted"]
        .as_u64()
        .unwrap();
    authority.stop().await;
    wait_absent(&operator, "settingsd").await?;
    authority = AuthorityTask::start(root.path(), &broker.url);
    // Unchanged revision on restart must trigger a fresh appearance read.
    wait_source(&portal, 2, before).await?;
    assert_eq!(read_one_color(&proxy).await?, 1);
    assert!(silent(&mut signals).await, "unchanged restart is silent");
    let report = operator
        .call("portald", "portald.status", json!({}))
        .await?;
    assert_eq!(report["settings_generation"], 1, "portal did not reconnect");
    operator.close().await;
    authority.stop().await;
    Ok(())
}

#[tokio::test]
async fn native_recovers_when_broker_returns_after_the_initial_attempt_budget() -> anyhow::Result<()>
{
    use bus::native_client::{NodedClient, SupervisedClient};
    use serde_json::json;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let mut broker = test_broker::Broker::with_tcp_port(port);
    let url = broker.url.clone();
    broker.stop();
    let bus = PrivateBus::start()?;
    let mut portal = NativePortal::start(&bus, &url);
    wait_for_name(&mut portal.live, &portal.shared).await?;
    let connection = client(&bus).await?;
    let proxy = portal_proxy(&connection).await?;
    assert_eq!(read_one_color(&proxy).await?, 1);
    // An eager client exhausts all five attempts against this exact endpoint.
    let initial = tokio::time::timeout(
        Duration::from_secs(10),
        SupervisedClient::connect_options("initial-budget-probe", &url).connect(),
    )
    .await?;
    assert!(initial.is_err());
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !portal.feed.is_finished(),
        "feed must survive initial broker failure"
    );
    broker.bounce();
    let root = tempfile::tempdir()?;
    authority_store(root.path(), "light")?;
    let mut authority = AuthorityTask::start(root.path(), &url);
    wait_source(&portal, 1, 0).await?;
    assert_eq!(read_one_color(&proxy).await?, 2);
    let operator = NodedClient::connect("recovery-test", &url).await?;
    let report = operator
        .call("portald", "portald.status", json!({}))
        .await?;
    assert_eq!(report["portal"]["origin"], "settingsd");
    assert_eq!(report["settings_generation"], 1);
    operator.close().await;
    authority.stop().await;
    Ok(())
}
